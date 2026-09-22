use std::{
    collections::HashMap,
    io::{Read, Write},
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex, atomic::Ordering, mpsc},
    thread,
    time::Duration,
};

use serde_json::{Map, Value};

use crate::{
    ChangelistDescription, ChangelistId, ChangelistSummary, ClientInfo, DepotRevision, Error,
    HaveRevision, OpenedFile, PendingChangelists, RawField, RawMessage, RawPrintedFile, RawRecord,
    RawResult, Result, WorkspaceMapping, cancellation_requested, parse,
};

const P4_EXECUTABLE: &str = "p4";
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    state: Arc<crate::CancellationState>,
}

impl CancellationToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
pub struct P4Client {
    inner: Arc<Inner>,
}

struct Inner {
    requests: mpsc::Sender<WorkerRequest>,
    shutdown: Arc<ShutdownCoordinator>,
    info: ClientInfo,
}

struct ShutdownCoordinator {
    state: Mutex<ShutdownState>,
}

enum ShutdownState {
    Running,
    InProgress(Vec<async_channel::Sender<Result<()>>>),
    Completed(Result<()>),
}

struct CancelOnDrop(Arc<crate::CancellationState>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::Release);
    }
}

impl Inner {
    fn send(&self, request: WorkerRequest) -> Result<()> {
        let state = self
            .shutdown
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(*state, ShutdownState::Running) {
            return Err(Error::worker_stopped());
        }

        self.requests
            .send(request)
            .map_err(|_| Error::worker_stopped())
    }

    fn request_shutdown(&self) -> async_channel::Receiver<Result<()>> {
        self.shutdown.subscribe(&self.requests)
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.begin_without_waiter(&self.requests);
    }
}

impl ShutdownCoordinator {
    fn new() -> Self {
        Self {
            state: Mutex::new(ShutdownState::Running),
        }
    }

    fn subscribe(
        &self,
        requests: &mpsc::Sender<WorkerRequest>,
    ) -> async_channel::Receiver<Result<()>> {
        let (completion, completed) = async_channel::bounded(1);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &mut *state {
            ShutdownState::Running => {
                *state = ShutdownState::InProgress(vec![completion]);
                if requests.send(WorkerRequest::Shutdown).is_err() {
                    let result = Err(Error::worker_stopped());
                    let waiters = match std::mem::replace(
                        &mut *state,
                        ShutdownState::Completed(result.clone()),
                    ) {
                        ShutdownState::InProgress(waiters) => waiters,
                        ShutdownState::Running | ShutdownState::Completed(_) => Vec::new(),
                    };

                    for waiter in waiters {
                        let _ = waiter.try_send(result.clone());
                    }
                }
            }
            ShutdownState::InProgress(waiters) => waiters.push(completion),
            ShutdownState::Completed(result) => {
                let _ = completion.try_send(result.clone());
            }
        }

        completed
    }

    fn begin_without_waiter(&self, requests: &mpsc::Sender<WorkerRequest>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*state, ShutdownState::Running) {
            *state = if requests.send(WorkerRequest::Shutdown).is_ok() {
                ShutdownState::InProgress(Vec::new())
            } else {
                ShutdownState::Completed(Err(Error::worker_stopped()))
            };
        }
    }

    fn complete(&self, result: &Result<()>) {
        let waiters = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if matches!(*state, ShutdownState::Completed(_)) {
                return;
            }

            match std::mem::replace(&mut *state, ShutdownState::Completed(result.clone())) {
                ShutdownState::Running | ShutdownState::Completed(_) => Vec::new(),
                ShutdownState::InProgress(waiters) => waiters,
            }
        };

        for waiter in waiters {
            let _ = waiter.try_send(result.clone());
        }
    }
}

struct CommandRequest {
    command: String,
    arguments: Vec<String>,
    input_arguments: Vec<String>,
    json: bool,
}

struct ProcessOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

enum WorkerRequest {
    Run {
        request: CommandRequest,
        cancellation: Arc<crate::CancellationState>,
        response: async_channel::Sender<Result<ProcessOutput>>,
    },
    Shutdown,
}

impl P4Client {
    /// Connect using P4 environment, config, ticket, and trust state visible from `working_directory`.
    pub async fn connect(working_directory: impl AsRef<Path>) -> Result<Self> {
        let working_directory = working_directory.as_ref().to_path_buf();
        let (requests, incoming) = mpsc::channel();
        let (initialized, initialization) = async_channel::bounded(1);
        let shutdown = Arc::new(ShutdownCoordinator::new());
        let worker_shutdown = Arc::clone(&shutdown);

        thread::Builder::new()
            .name("yori-p4".to_owned())
            .spawn(move || {
                worker_main(working_directory, &incoming, initialized, &worker_shutdown);
            })
            .map_err(|error| Error::worker_start_failed(&error))?;

        let info = initialization
            .recv()
            .await
            .map_err(|_| Error::worker_stopped())??;

        Ok(Self {
            inner: Arc::new(Inner {
                requests,
                shutdown,
                info,
            }),
        })
    }

    pub fn client_info(
        &self,
        cancellation: &CancellationToken,
    ) -> std::future::Ready<Result<ClientInfo>> {
        let result = if cancellation.is_cancelled() {
            Err(Error::cancelled())
        } else {
            Ok(self.inner.info.clone())
        };

        std::future::ready(result)
    }

    pub async fn pending_changelists(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<PendingChangelists> {
        let info = self.client_info(cancellation).await?;
        let arguments = vec![
            "-s".to_owned(),
            "pending".to_owned(),
            "-u".to_owned(),
            info.user_name.clone(),
            "-c".to_owned(),
            info.client_name.clone(),
        ];
        let result = self
            .execute_json("changes", arguments, Vec::new(), cancellation)
            .await?;

        parse::pending_changelists(&result, &info)
    }

    pub async fn submitted_changelists(
        &self,
        maximum: NonZeroU32,
        file_specification: Option<&str>,
        cancellation: &CancellationToken,
    ) -> Result<Vec<ChangelistSummary>> {
        let mut arguments = vec![
            "-s".to_owned(),
            "submitted".to_owned(),
            "-m".to_owned(),
            maximum.to_string(),
        ];

        if let Some(file_specification) = file_specification {
            arguments.push(file_specification.to_owned());
        }

        let result = self
            .execute_json("changes", arguments, Vec::new(), cancellation)
            .await?;
        parse::submitted_changelists(&result)
    }

    pub async fn opened_files(
        &self,
        changelist: ChangelistId,
        cancellation: &CancellationToken,
    ) -> Result<Vec<OpenedFile>> {
        let arguments = vec!["-c".to_owned(), changelist.to_string()];
        let result = self
            .execute_json("opened", arguments, Vec::new(), cancellation)
            .await?;

        parse::opened_files(&result)
    }

    pub async fn changelist_description(
        &self,
        changelist: NonZeroU32,
        cancellation: &CancellationToken,
    ) -> Result<ChangelistDescription> {
        let arguments = vec!["-s".to_owned(), changelist.to_string()];
        let result = self
            .execute_json("describe", arguments, Vec::new(), cancellation)
            .await?;

        parse::changelist_description(&result)
    }

    pub async fn have_revisions(
        &self,
        file_specifications: &[String],
        cancellation: &CancellationToken,
    ) -> Result<Vec<HaveRevision>> {
        if file_specifications.is_empty() {
            return Ok(Vec::new());
        }

        let result = self
            .execute_json(
                "have",
                Vec::new(),
                file_specifications.to_vec(),
                cancellation,
            )
            .await?;
        parse::have_revisions(&result)
    }

    pub async fn workspace_mappings(
        &self,
        file_specifications: &[String],
        cancellation: &CancellationToken,
    ) -> Result<Vec<WorkspaceMapping>> {
        if file_specifications.is_empty() {
            return Ok(Vec::new());
        }

        let result = self
            .execute_json(
                "where",
                Vec::new(),
                file_specifications.to_vec(),
                cancellation,
            )
            .await?;
        parse::workspace_mappings(&result)
    }

    pub async fn depot_content(
        &self,
        revision: &DepotRevision,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>> {
        let output = self
            .execute_process(
                CommandRequest {
                    command: "print".to_owned(),
                    arguments: vec!["-q".to_owned(), revision.to_string()],
                    input_arguments: Vec::new(),
                    json: false,
                },
                cancellation,
            )
            .await?;

        raw_output(output)
    }

    pub async fn depot_contents(
        &self,
        revisions: &[DepotRevision],
        cancellation: &CancellationToken,
    ) -> Result<HashMap<DepotRevision, Vec<u8>>> {
        if revisions.is_empty() {
            return Ok(HashMap::new());
        }

        let specifications = revisions.iter().map(ToString::to_string).collect();
        let result = self
            .execute_json("print", Vec::new(), specifications, cancellation)
            .await?;
        let binary_revisions = result
            .printed_files
            .iter()
            .filter(|file| is_binary(file.file_type.as_deref()))
            .map(|file| DepotRevision::new(file.depot_path.clone(), file.revision))
            .collect::<Result<Vec<_>>>()?;
        let mut contents = parse::depot_contents(&result, revisions)?;

        for revision in binary_revisions {
            let content = self.depot_content(&revision, cancellation).await?;
            contents.insert(revision, content);
        }

        Ok(contents)
    }

    /// Stops the owning worker after all requests already queued ahead of shutdown.
    pub async fn shutdown(&self) -> Result<()> {
        self.inner
            .request_shutdown()
            .recv()
            .await
            .map_err(|_| Error::worker_stopped())?
    }

    async fn execute_json(
        &self,
        command: &str,
        arguments: Vec<String>,
        input_arguments: Vec<String>,
        cancellation: &CancellationToken,
    ) -> Result<RawResult> {
        let output = self
            .execute_process(
                CommandRequest {
                    command: command.to_owned(),
                    arguments,
                    input_arguments,
                    json: true,
                },
                cancellation,
            )
            .await?;

        decode_json_output(command, output)
    }

    async fn execute_process(
        &self,
        request: CommandRequest,
        cancellation: &CancellationToken,
    ) -> Result<ProcessOutput> {
        if cancellation.is_cancelled() {
            return Err(Error::cancelled());
        }

        let request_cancellation = Arc::new(crate::CancellationState::following(Arc::clone(
            &cancellation.state,
        )));
        let _cancel_on_drop = CancelOnDrop(Arc::clone(&request_cancellation));
        let (response, result) = async_channel::bounded(1);
        self.inner.send(WorkerRequest::Run {
            request,
            cancellation: request_cancellation,
            response,
        })?;
        let result = result.recv().await.map_err(|_| Error::worker_stopped())??;

        if cancellation.is_cancelled() {
            return Err(Error::cancelled());
        }

        Ok(result)
    }
}

struct CommandRunner {
    working_directory: PathBuf,
}

impl CommandRunner {
    fn run(
        &self,
        request: &CommandRequest,
        cancellation: &crate::CancellationState,
    ) -> Result<ProcessOutput> {
        if cancellation_requested(cancellation) {
            return Err(Error::cancelled());
        }

        validate_input_arguments(&request.input_arguments)?;

        let mut command = Command::new(P4_EXECUTABLE);
        command
            .current_dir(&self.working_directory)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if request.json {
            command.args(["-ztag", "-Mj"]);
        }
        if !request.input_arguments.is_empty() {
            command.args(["-x", "-"]).stdin(Stdio::piped());
        }
        command.arg(&request.command).args(&request.arguments);

        let mut child = command
            .spawn()
            .map_err(|error| Error::process_start_failed(P4_EXECUTABLE, &error))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::process_io_failed("capture p4 stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::process_io_failed("capture p4 stderr"))?;
        let stdout_reader = thread::spawn(move || read_all(stdout));
        let stderr_reader = thread::spawn(move || read_all(stderr));

        if !request.input_arguments.is_empty() {
            write_input_arguments(&mut child, &request.input_arguments)?;
        }

        let status = wait_for_child(&mut child, cancellation)?;
        let stdout = join_reader(stdout_reader, "read p4 stdout")?;
        let stderr = join_reader(stderr_reader, "read p4 stderr")?;

        if cancellation_requested(cancellation) {
            return Err(Error::cancelled());
        }

        Ok(ProcessOutput {
            status,
            stdout,
            stderr,
        })
    }
}

fn worker_main(
    working_directory: PathBuf,
    requests: &mpsc::Receiver<WorkerRequest>,
    initialized: async_channel::Sender<Result<ClientInfo>>,
    shutdown: &ShutdownCoordinator,
) {
    let runner = CommandRunner { working_directory };
    let initialization_cancellation = crate::CancellationState::default();
    let initialization = runner
        .run(
            &CommandRequest {
                command: "info".to_owned(),
                arguments: Vec::new(),
                input_arguments: Vec::new(),
                json: true,
            },
            &initialization_cancellation,
        )
        .and_then(|output| decode_json_output("info", output))
        .and_then(|result| parse::client_info(&result));

    let info = match initialization {
        Ok(info) => info,
        Err(error) => {
            let result = Err(error.clone());
            let _ = initialized.try_send(Err(error));
            shutdown.complete(&result);
            return;
        }
    };

    let _ = initialized.try_send(Ok(info));
    drop(initialized);

    loop {
        let Ok(request) = requests.recv() else {
            break;
        };

        match request {
            WorkerRequest::Run {
                request,
                cancellation,
                response,
            } => {
                let result = runner.run(&request, &cancellation);
                let _ = response.try_send(result);
            }
            WorkerRequest::Shutdown => break,
        }
    }

    shutdown.complete(&Ok(()));
}

fn validate_input_arguments(arguments: &[String]) -> Result<()> {
    if arguments
        .iter()
        .any(|argument| argument.contains(['\r', '\n']))
    {
        return Err(Error::invalid_response(
            "Perforce file specification contained a line break",
        ));
    }

    Ok(())
}

fn write_input_arguments(child: &mut Child, arguments: &[String]) -> Result<()> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| Error::process_io_failed("open p4 stdin"))?;

    for argument in arguments {
        stdin
            .write_all(argument.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .map_err(|_| Error::process_io_failed("write p4 arguments"))?;
    }

    Ok(())
}

fn wait_for_child(
    child: &mut Child,
    cancellation: &crate::CancellationState,
) -> Result<ExitStatus> {
    loop {
        if cancellation_requested(cancellation) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::cancelled());
        }

        if let Some(status) = child
            .try_wait()
            .map_err(|_| Error::process_io_failed("wait for p4"))?
        {
            return Ok(status);
        }

        thread::sleep(CANCELLATION_POLL_INTERVAL);
    }
}

fn read_all(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_reader(
    reader: thread::JoinHandle<std::io::Result<Vec<u8>>>,
    operation: &'static str,
) -> Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| Error::process_io_failed(operation))?
        .map_err(|_| Error::process_io_failed(operation))
}

fn decode_json_output(command: &str, output: ProcessOutput) -> Result<RawResult> {
    let mut result = RawResult::default();
    let mut current_print: Option<RawPrintedFile> = None;

    for line in output.stdout.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }

        let value: Value = serde_json::from_slice(line)
            .map_err(|error| Error::invalid_response(format!("invalid p4 JSON output: {error}")))?;
        let object = value
            .as_object()
            .ok_or_else(|| Error::invalid_response("p4 JSON output was not an object"))?;

        if let Some(message) = raw_message(object) {
            result.messages.push(message);
            continue;
        }

        if let Some(data) = object.get("data").and_then(Value::as_str) {
            if let Some(file) = &mut current_print {
                file.contents.extend_from_slice(data.as_bytes());
            } else {
                result.output.extend_from_slice(data.as_bytes());
            }
            continue;
        }

        let record = raw_record(object);
        if command == "print" {
            if let Some(file) = raw_printed_file(object)? {
                if let Some(previous) = current_print.replace(file) {
                    result.printed_files.push(previous);
                }
            }
        }
        result.records.push(record);
    }

    if let Some(file) = current_print {
        result.printed_files.push(file);
    }

    if !output.stderr.is_empty() {
        result.messages.push(RawMessage {
            severity: 3,
            generic: 0,
            text: output.stderr,
        });
    }

    if !output.status.success() && result.messages.is_empty() {
        result.messages.push(RawMessage {
            severity: 3,
            generic: 0,
            text: format!("p4 {command} exited with {}", output.status).into_bytes(),
        });
    }

    Ok(result)
}

fn raw_message(object: &Map<String, Value>) -> Option<RawMessage> {
    let severity = object.get("severity").and_then(json_i32)?;
    let text = object
        .get("data")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .as_bytes()
        .to_vec();
    let generic = object.get("generic").and_then(json_i32).unwrap_or_default();

    Some(RawMessage {
        severity,
        generic,
        text,
    })
}

fn raw_record(object: &Map<String, Value>) -> RawRecord {
    RawRecord {
        fields: object
            .iter()
            .map(|(name, value)| RawField {
                name: name.clone(),
                value: json_bytes(value),
            })
            .collect(),
    }
}

fn raw_printed_file(object: &Map<String, Value>) -> Result<Option<RawPrintedFile>> {
    let Some(depot_path) = object.get("depotFile").and_then(Value::as_str) else {
        return Ok(None);
    };
    let revision = object
        .get("rev")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid_response("p4 print output omitted rev"))?
        .parse()
        .map_err(|_| Error::invalid_response("p4 print returned an invalid rev"))?;

    Ok(Some(RawPrintedFile {
        depot_path: depot_path.to_owned(),
        revision,
        file_type: object
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned),
        contents: Vec::new(),
    }))
}

fn json_i32(value: &Value) -> Option<i32> {
    value
        .as_i64()
        .and_then(|number| i32::try_from(number).ok())
        .or_else(|| value.as_str()?.parse().ok())
}

fn json_bytes(value: &Value) -> Vec<u8> {
    match value {
        Value::String(value) => value.as_bytes().to_vec(),
        Value::Null => Vec::new(),
        _ => value.to_string().into_bytes(),
    }
}

fn raw_output(output: ProcessOutput) -> Result<Vec<u8>> {
    if !output.stderr.is_empty() {
        return Err(Error::from_command_output(&output.stderr));
    }
    if !output.status.success() {
        return Err(Error::process_failed("print", output.status));
    }

    Ok(output.stdout)
}

fn is_binary(file_type: Option<&str>) -> bool {
    let Some(base) = file_type.and_then(|file_type| file_type.split('+').next()) else {
        return false;
    };

    !matches!(base, "text" | "unicode" | "utf8" | "utf16" | "symlink")
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;

    use super::*;

    #[test]
    fn dropped_requests_and_explicit_tokens_share_cancellation() {
        let parent = Arc::new(crate::CancellationState::default());
        let child = Arc::new(crate::CancellationState::following(Arc::clone(&parent)));

        {
            let _cancel_on_drop = CancelOnDrop(Arc::clone(&child));
            assert!(!cancellation_requested(&child));
        }

        assert!(cancellation_requested(&child));

        let child = crate::CancellationState::following(Arc::clone(&parent));
        parent.cancelled.store(true, Ordering::Release);
        assert!(cancellation_requested(&child));
    }

    #[test]
    fn concurrent_and_repeated_shutdowns_share_completion() {
        let (client, requests) = shutdown_test_client();
        let barrier = Arc::new(Barrier::new(3));
        let waiters = [client.clone(), client.clone()].map(|handle| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                handle.inner.request_shutdown()
            })
        });

        barrier.wait();
        let [first, second] = waiters.map(|waiter| waiter.join().unwrap());

        assert!(matches!(requests.recv().unwrap(), WorkerRequest::Shutdown));
        assert!(matches!(
            requests.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        client.inner.shutdown.complete(&Ok(()));

        assert_eq!(first.recv_blocking().unwrap(), Ok(()));
        assert_eq!(second.recv_blocking().unwrap(), Ok(()));
        assert_eq!(
            client.inner.request_shutdown().recv_blocking().unwrap(),
            Ok(())
        );
    }

    #[test]
    fn decodes_real_tagged_json_and_batched_print_records() {
        let output = ProcessOutput {
            status: successful_status(),
            stdout: concat!(
                "{\"action\":\"edit\",\"depotFile\":\"//depot/a.txt\",\"rev\":\"3\",\"type\":\"text\"}\n",
                "{\"data\":\"first\\n\"}\n",
                "{\"data\":\"second\\n\"}\n",
                "{\"action\":\"add\",\"depotFile\":\"//depot/b.txt\",\"rev\":\"1\",\"type\":\"unicode\"}\n",
                "{\"data\":\"other\\n\"}\n",
                "{\"data\":\"\"}\n"
            )
            .as_bytes()
            .to_vec(),
            stderr: Vec::new(),
        };

        let result = decode_json_output("print", output).unwrap();

        assert_eq!(result.printed_files.len(), 2);
        assert_eq!(result.printed_files[0].depot_path, "//depot/a.txt");
        assert_eq!(result.printed_files[0].contents, b"first\nsecond\n");
        assert_eq!(result.printed_files[1].depot_path, "//depot/b.txt");
        assert_eq!(result.printed_files[1].contents, b"other\n");
    }

    #[test]
    fn decodes_tagged_messages_and_plain_stderr() {
        let output = ProcessOutput {
            status: successful_status(),
            stdout:
                b"{\"data\":\"file(s) not in client view.\\n\",\"generic\":36,\"severity\":3}\n"
                    .to_vec(),
            stderr: b"additional diagnostic\n".to_vec(),
        };

        let result = decode_json_output("where", output).unwrap();

        assert_eq!(result.messages.len(), 2);
        assert_eq!(result.messages[0].generic, 36);
        assert_eq!(result.messages[0].severity, 3);
        assert_eq!(result.messages[1].text, b"additional diagnostic\n");
    }

    fn shutdown_test_client() -> (P4Client, mpsc::Receiver<WorkerRequest>) {
        let (requests, incoming) = mpsc::channel();
        let client = P4Client {
            inner: Arc::new(Inner {
                requests,
                shutdown: Arc::new(ShutdownCoordinator::new()),
                info: ClientInfo {
                    server_address: String::new(),
                    server_version: String::new(),
                    user_name: String::new(),
                    client_name: String::new(),
                    client_root: None,
                    current_directory: PathBuf::new(),
                    case_handling: None,
                    unicode_enabled: false,
                },
            }),
        };

        (client, incoming)
    }

    #[cfg(unix)]
    fn successful_status() -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;

        ExitStatus::from_raw(0)
    }

    #[cfg(windows)]
    fn successful_status() -> ExitStatus {
        use std::os::windows::process::ExitStatusExt;

        ExitStatus::from_raw(0)
    }
}
