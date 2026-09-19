use std::{
    num::NonZeroU32,
    path::Path,
    sync::{Arc, Mutex, atomic::Ordering, mpsc},
    thread,
};

use crate::{
    ChangelistDescription, ChangelistId, ChangelistSummary, ClientInfo, DepotRevision, Error,
    HaveRevision, OpenedFile, PendingChangelists, RawResult, Result, WorkspaceMapping,
    cancellation_requested, ffi, parse,
};

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

enum WorkerRequest {
    Run {
        command: String,
        arguments: Vec<String>,
        cancellation: Arc<crate::CancellationState>,
        response: async_channel::Sender<RawResult>,
    },
    Shutdown,
}

impl P4Client {
    /// Connect using P4 environment, config, ticket, and trust state visible from `working_directory`.
    pub async fn connect(working_directory: impl AsRef<Path>) -> Result<Self> {
        let working_directory = working_directory.as_ref().to_path_buf();
        let cwd = working_directory
            .to_str()
            .ok_or_else(|| Error::invalid_working_directory(&working_directory))?
            .to_owned();
        let (requests, incoming) = mpsc::channel();
        let (initialized, initialization) = async_channel::bounded(1);
        let shutdown = Arc::new(ShutdownCoordinator::new());
        let worker_shutdown = Arc::clone(&shutdown);

        thread::Builder::new()
            .name("yori-p4".to_owned())
            .spawn(move || worker_main(&cwd, &incoming, initialized, &worker_shutdown))
            .map_err(|error| Error::worker_start_failed(&error))?;

        initialization
            .recv()
            .await
            .map_err(|_| Error::worker_stopped())??;

        Ok(Self {
            inner: Arc::new(Inner { requests, shutdown }),
        })
    }

    pub async fn client_info(&self, cancellation: &CancellationToken) -> Result<ClientInfo> {
        let result = self.execute("info", Vec::new(), cancellation).await?;
        parse::client_info(&result)
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
        let result = self.execute("changes", arguments, cancellation).await?;

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

        let result = self.execute("changes", arguments, cancellation).await?;
        parse::submitted_changelists(&result)
    }

    pub async fn opened_files(
        &self,
        changelist: ChangelistId,
        cancellation: &CancellationToken,
    ) -> Result<Vec<OpenedFile>> {
        let arguments = vec!["-c".to_owned(), changelist.to_string()];
        let result = self.execute("opened", arguments, cancellation).await?;

        parse::opened_files(&result)
    }

    pub async fn changelist_description(
        &self,
        changelist: NonZeroU32,
        cancellation: &CancellationToken,
    ) -> Result<ChangelistDescription> {
        let arguments = vec!["-s".to_owned(), changelist.to_string()];
        let result = self.execute("describe", arguments, cancellation).await?;

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
            .execute("have", file_specifications.to_vec(), cancellation)
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
            .execute("where", file_specifications.to_vec(), cancellation)
            .await?;
        parse::workspace_mappings(&result)
    }

    pub async fn depot_content(
        &self,
        revision: &DepotRevision,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>> {
        let arguments = vec!["-q".to_owned(), revision.to_string()];
        let result = self.execute("print", arguments, cancellation).await?;

        parse::depot_content(&result)
    }

    /// Stops the owning worker after all requests already queued ahead of shutdown.
    pub async fn shutdown(&self) -> Result<()> {
        self.inner
            .request_shutdown()
            .recv()
            .await
            .map_err(|_| Error::worker_stopped())?
    }

    async fn execute(
        &self,
        command: &str,
        arguments: Vec<String>,
        cancellation: &CancellationToken,
    ) -> Result<RawResult> {
        if cancellation.is_cancelled() {
            return Err(Error::cancelled());
        }

        let request_cancellation = Arc::new(crate::CancellationState::following(Arc::clone(
            &cancellation.state,
        )));
        let _cancel_on_drop = CancelOnDrop(Arc::clone(&request_cancellation));
        let (response, result) = async_channel::bounded(1);
        self.inner.send(WorkerRequest::Run {
            command: command.to_owned(),
            arguments,
            cancellation: request_cancellation,
            response,
        })?;
        let result = result.recv().await.map_err(|_| Error::worker_stopped())?;

        if cancellation.is_cancelled() {
            return Err(Error::cancelled());
        }

        Ok(result)
    }
}

fn worker_main(
    cwd: &str,
    requests: &mpsc::Receiver<WorkerRequest>,
    initialized: async_channel::Sender<Result<()>>,
    shutdown: &ShutdownCoordinator,
) {
    let mut thread_initialization = RawResult::default();
    let native_thread = ffi::start_thread(&mut thread_initialization);
    let thread_ready = native_thread.as_ref().is_some_and(ffi::NativeThread::ready);
    let thread_result = parse::lifecycle_result(&thread_initialization, "thread initialization")
        .and_then(|()| {
            if thread_ready {
                Ok(())
            } else {
                Err(Error::lifecycle(
                    "thread initialization",
                    &thread_initialization.messages,
                ))
            }
        });

    if let Err(error) = thread_result {
        let result = Err(error);
        let _ = initialized.try_send(result.clone());
        shutdown.complete(&result);
        return;
    }

    let mut initialization = RawResult::default();
    let client = ffi::connect(cwd, "", &mut initialization);
    let connected = client.as_ref().is_some_and(ffi::NativeClient::connected);
    let initialization_result = parse::check_result(&initialization).and_then(|()| {
        if connected {
            Ok(())
        } else {
            Err(Error::lifecycle(
                "client initialization",
                &initialization.messages,
            ))
        }
    });

    if let Err(mut error) = initialization_result {
        if let Err(cleanup) = shutdown_native(client, native_thread) {
            error = error.with_cleanup_failure(&cleanup);
        }

        let result = Err(error);
        let _ = initialized.try_send(result.clone());
        shutdown.complete(&result);
        return;
    }

    let mut client = client;
    let native_thread = native_thread;
    let _ = initialized.try_send(Ok(()));
    drop(initialized);

    loop {
        let Ok(request) = requests.recv() else {
            break;
        };

        match request {
            WorkerRequest::Run {
                command,
                arguments,
                cancellation,
                response,
            } => {
                let mut result = RawResult::default();

                if !cancellation_requested(&cancellation) {
                    client
                        .pin_mut()
                        .run(&command, &arguments, &cancellation, &mut result);
                }

                let _ = response.try_send(result);
            }
            WorkerRequest::Shutdown => break,
        }
    }

    let cleanup_result = shutdown_native(client, native_thread);
    shutdown.complete(&cleanup_result);
}

fn shutdown_native(
    mut client: cxx::UniquePtr<ffi::NativeClient>,
    mut native_thread: cxx::UniquePtr<ffi::NativeThread>,
) -> Result<()> {
    let mut client_shutdown = RawResult::default();
    if let Some(client) = client.as_mut() {
        client.close(&mut client_shutdown);
    }
    let client_result = parse::lifecycle_result(&client_shutdown, "client shutdown");

    drop(client);

    let mut thread_shutdown = RawResult::default();
    if let Some(native_thread) = native_thread.as_mut() {
        native_thread.shutdown(&mut thread_shutdown);
    }
    let thread_result = parse::lifecycle_result(&thread_shutdown, "thread shutdown");

    drop(native_thread);
    client_result.and(thread_result)
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
    fn concurrent_and_repeated_shutdowns_share_the_cleanup_failure() {
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
        assert!(matches!(
            first.try_recv(),
            Err(async_channel::TryRecvError::Empty)
        ));
        assert!(matches!(
            second.try_recv(),
            Err(async_channel::TryRecvError::Empty)
        ));

        let failure = Error::lifecycle("thread shutdown", &[]);
        client.inner.shutdown.complete(&Err(failure.clone()));

        assert_eq!(first.recv_blocking().unwrap(), Err(failure.clone()));
        assert_eq!(second.recv_blocking().unwrap(), Err(failure.clone()));

        let repeated = client.inner.request_shutdown();
        assert_eq!(repeated.recv_blocking().unwrap(), Err(failure));
        assert!(matches!(
            requests.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn shutdown_completion_waits_for_queued_work_and_worker_cleanup() {
        let (client, requests) = shutdown_test_client();
        let (response, _result) = async_channel::bounded(1);
        client
            .inner
            .send(WorkerRequest::Run {
                command: "info".to_owned(),
                arguments: Vec::new(),
                cancellation: Arc::new(crate::CancellationState::default()),
                response,
            })
            .unwrap();

        let completion = client.inner.request_shutdown();
        let (late_response, _late_result) = async_channel::bounded(1);
        assert_eq!(
            client.inner.send(WorkerRequest::Run {
                command: "late-info".to_owned(),
                arguments: Vec::new(),
                cancellation: Arc::new(crate::CancellationState::default()),
                response: late_response,
            }),
            Err(Error::worker_stopped())
        );

        assert!(matches!(
            completion.try_recv(),
            Err(async_channel::TryRecvError::Empty)
        ));
        assert!(matches!(
            requests.recv().unwrap(),
            WorkerRequest::Run { .. }
        ));
        assert!(matches!(
            completion.try_recv(),
            Err(async_channel::TryRecvError::Empty)
        ));
        assert!(matches!(requests.recv().unwrap(), WorkerRequest::Shutdown));
        assert!(matches!(
            requests.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(matches!(
            completion.try_recv(),
            Err(async_channel::TryRecvError::Empty)
        ));

        client.inner.shutdown.complete(&Ok(()));
        assert_eq!(completion.recv_blocking().unwrap(), Ok(()));
    }

    #[test]
    fn native_bridge_converts_connection_failures_without_a_server() {
        let native_thread = start_native_thread();
        let mut result = RawResult::default();
        let cancellation = crate::CancellationState::default();
        let mut client = ffi::connect(".", "127.0.0.1:1", &mut result);

        if parse::check_result(&result).is_ok() && client.as_ref().is_some() {
            client
                .pin_mut()
                .run("info", &[], &cancellation, &mut result);
        }

        let error = parse::check_result(&result).unwrap_err();
        assert_eq!(error.kind(), crate::ErrorKind::Connectivity);
        shutdown_native(client, native_thread).unwrap();
    }

    #[test]
    fn native_bridge_rejects_thread_cleanup_until_clients_are_destroyed() {
        let mut native_thread = start_native_thread();
        let mut connection = RawResult::default();
        let mut client = ffi::connect(".", "127.0.0.1:1", &mut connection);
        assert!(client.as_ref().is_some());

        let mut premature = RawResult::default();
        native_thread.pin_mut().shutdown(&mut premature);
        let error = parse::lifecycle_result(&premature, "thread shutdown").unwrap_err();
        assert_eq!(error.kind(), crate::ErrorKind::Lifecycle);

        let mut client_shutdown = RawResult::default();
        client.pin_mut().close(&mut client_shutdown);
        parse::lifecycle_result(&client_shutdown, "client shutdown").unwrap();
        drop(client);

        let mut thread_shutdown = RawResult::default();
        native_thread.pin_mut().shutdown(&mut thread_shutdown);
        parse::lifecycle_result(&thread_shutdown, "thread shutdown").unwrap();
    }

    #[test]
    fn native_diagnostics_replace_invalid_utf8_without_losing_bytes() {
        let diagnostic = b"invalid byte: \xff";
        let mut result = RawResult::default();

        crate::capture_diagnostic_for_test(diagnostic, &mut result);

        assert_eq!(result.messages[0].text, diagnostic);
        let error = Error::from_messages(&result.messages).unwrap();
        assert_eq!(error.kind(), crate::ErrorKind::Command);
        assert_eq!(
            error.to_string(),
            "invalid byte: �; check the Perforce command details and retry"
        );
    }

    fn shutdown_test_client() -> (P4Client, mpsc::Receiver<WorkerRequest>) {
        let (requests, incoming) = mpsc::channel();
        let client = P4Client {
            inner: Arc::new(Inner {
                requests,
                shutdown: Arc::new(ShutdownCoordinator::new()),
            }),
        };

        (client, incoming)
    }

    fn start_native_thread() -> cxx::UniquePtr<ffi::NativeThread> {
        let mut result = RawResult::default();
        let native_thread = ffi::start_thread(&mut result);

        parse::lifecycle_result(&result, "thread initialization").unwrap();
        assert!(native_thread.as_ref().is_some_and(ffi::NativeThread::ready));
        native_thread
    }
}
