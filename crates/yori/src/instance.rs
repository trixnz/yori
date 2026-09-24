//! Cross-platform single-instance ownership and acknowledged file handoff.
//!
//! The first process owns a local socket name. Later invocations connect to it
//! and exit only after the workspace acknowledges the request. A waiting
//! invocation also stays connected until its tab closes.

mod protocol;

#[cfg(test)]
mod tests;

use std::{
    io,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use async_channel::{Receiver, Sender};
use interprocess::{
    ConnectWaitMode,
    local_socket::{
        GenericNamespaced, Listener, ListenerNonblockingMode, ListenerOptions, Stream, prelude::*,
    },
};

use crate::invocation::InvocationRequest;

const INSTANCE_NAME: &str = "io.github.trixnz.yori.instance.v3";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const ELECTION_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const MAX_PENDING_CONNECTIONS: usize = 16;

fn socket_name_is_occupied(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::AddrInUse {
        return true;
    }

    #[cfg(windows)]
    return error.kind() == io::ErrorKind::PermissionDenied;

    #[cfg(not(windows))]
    false
}

#[derive(Debug)]
enum HandoffError {
    OwnerUnavailable(io::Error),
    Failed(String),
}

/// Reports to a waiting invocation when the tab it opened goes away. Dropping
/// it is the report, so every path that disposes of a tab also answers.
pub(crate) struct Completion {
    reply: mpsc::SyncSender<bool>,
    saved: bool,
}

impl Completion {
    pub fn mark_saved(&mut self) {
        self.saved = true;
    }

    #[cfg(test)]
    pub fn channel() -> (Self, mpsc::Receiver<bool>) {
        let (reply, finished) = mpsc::sync_channel(1);

        (
            Self {
                reply,
                saved: false,
            },
            finished,
        )
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        let _ = self.reply.try_send(self.saved);
    }
}

pub(super) struct OpenRequest {
    pub invocation: InvocationRequest,
    received: Instant,
    reply: mpsc::SyncSender<Result<(), String>>,
    completion: Option<Completion>,
}

impl OpenRequest {
    /// Don't apply a request which sat in the UI queue beyond the client timeout.
    pub fn expired(&self) -> bool {
        self.received.elapsed() >= REQUEST_TIMEOUT
    }

    /// The report for a waiting request; attach it to the opened tab.
    pub fn take_completion(&mut self) -> Option<Completion> {
        self.completion.take()
    }

    pub fn complete(self, result: Result<(), String>) {
        let _ = self.reply.try_send(result);
    }
}

pub(super) struct Instance {
    requests: Receiver<OpenRequest>,
    shutdown: Arc<AtomicBool>,
    listener: Option<JoinHandle<()>>,
    #[cfg(test)]
    active_connections: Arc<AtomicUsize>,
}

struct ConnectionPermit(Arc<AtomicUsize>);

impl ConnectionPermit {
    fn acquire(active: Arc<AtomicUsize>) -> Option<Self> {
        active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_PENDING_CONNECTIONS).then_some(count + 1)
            })
            .ok()?;

        Some(Self(active))
    }
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

impl Instance {
    /// Return the primary instance, or `None` after a successful handoff. Failure
    /// never falls back to a second window: a timed-out request may have started.
    pub fn start(invocation: &InvocationRequest) -> Result<Option<Self>, String> {
        Self::establish(&instance_name(), invocation)
    }

    /// Hand a waiting invocation to the running workspace, starting a detached
    /// one when none runs, and return whether the opened tab saved before it
    /// closed. The waiting process never owns the workspace: closing a tab must
    /// not end a window that later tabs share.
    pub fn wait(
        invocation: &InvocationRequest
    ) -> Result<bool, String> {
        let mut owner: Option<Child> = None;
        
        Self::wait_with_owner(&instance_name(), invocation, || {
            if let Some(child) = &mut owner {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(format!("yori exited during startup: {status}"));
                }
                return Ok(());
            }
            
            owner = Some(spawn_owner(&invocation.directory)?);
        
            Ok(())
        })
    }

    fn wait_with_owner(
        name: &str,
        invocation: &InvocationRequest,
        mut start_owner: impl FnMut() -> Result<(), String>,
    ) -> Result<bool, String> {
        let started = Instant::now();
        let mut owner_started = false;

        let mut stream = loop {
            match Self::handoff(name, invocation) {
                Ok(stream) => break stream,
                Err(HandoffError::OwnerUnavailable(_)) if started.elapsed() < REQUEST_TIMEOUT => {
                    if !owner_started {
                        start_owner()?;
                        owner_started = true;
                    }

                    thread::sleep(ELECTION_RETRY_INTERVAL);
                }
                Err(HandoffError::OwnerUnavailable(error)) => {
                    return Err(format!("cannot connect to yori: {error}"));
                }
                Err(HandoffError::Failed(error)) => return Err(error),
            }
        };

        // Editing takes as long as it takes; only the handoff itself is timed.
        #[cfg(not(windows))]
        stream
            .set_recv_timeout(None)
            .map_err(|error| format!("cannot configure local socket timeout: {error}"))?;

        protocol::read_completion(&mut stream)
            .map_err(|error| format!("yori closed before the tab finished: {error}"))
    }

    fn establish(name: &str, invocation: &InvocationRequest) -> Result<Option<Self>, String> {
        Self::establish_with_before_handoff(name, invocation, || {})
    }

    fn establish_with_before_handoff(
        name: &str,
        invocation: &InvocationRequest,
        mut before_handoff: impl FnMut(),
    ) -> Result<Option<Self>, String> {
        let mut validation = Vec::new();
        protocol::write_request(&mut validation, invocation)
            .map_err(|error| format!("invalid invocation request: {error}"))?;
        let election_started = Instant::now();

        loop {
            let socket_name = name
                .to_ns_name::<GenericNamespaced>()
                .map_err(|error| format!("invalid yori instance name: {error}"))?;
            match ListenerOptions::new()
                .name(socket_name)
                .nonblocking(ListenerNonblockingMode::Accept)
                .create_sync()
            {
                Ok(listener) => return Self::serve(listener),
                Err(error) if socket_name_is_occupied(&error) => {
                    before_handoff();
                    match Self::handoff(name, invocation) {
                        Ok(_) => return Ok(None),
                        Err(HandoffError::OwnerUnavailable(_))
                            if election_started.elapsed() < REQUEST_TIMEOUT =>
                        {
                            thread::sleep(ELECTION_RETRY_INTERVAL);
                        }
                        Err(HandoffError::OwnerUnavailable(error)) => {
                            return Err(format!(
                                "cannot connect to running yori: {error}; no second window was started"
                            ));
                        }
                        Err(HandoffError::Failed(error)) => return Err(error),
                    }
                }
                Err(error) => {
                    return Err(format!("cannot claim yori's local socket: {error}"));
                }
            }
        }
    }

    fn serve(listener: Listener) -> Result<Option<Self>, String> {
        let (requests, incoming) = async_channel::bounded(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let listener_shutdown = Arc::clone(&shutdown);
        let active_connections = Arc::new(AtomicUsize::new(0));
        let listener_connections = Arc::clone(&active_connections);
        let worker = thread::Builder::new()
            .name("yori-instance-listener".into())
            .spawn(move || {
                accept_requests(
                    &listener,
                    &requests,
                    &listener_shutdown,
                    &listener_connections,
                );
            })
            .map_err(|error| format!("cannot start yori's local socket listener: {error}"))?;

        Ok(Some(Self {
            requests: incoming,
            shutdown,
            listener: Some(worker),
            #[cfg(test)]
            active_connections,
        }))
    }

    /// Send `invocation` and return the stream after the workspace accepts it.
    fn handoff(name: &str, invocation: &InvocationRequest) -> Result<Stream, HandoffError> {
        let socket_name = name.to_ns_name::<GenericNamespaced>().map_err(|error| {
            HandoffError::Failed(format!("invalid yori instance name: {error}"))
        })?;
        let mut stream = interprocess::local_socket::ConnectOptions::new()
            .name(socket_name)
            .wait_mode(ConnectWaitMode::Timeout(REQUEST_TIMEOUT))
            .connect_sync()
            .map_err(HandoffError::OwnerUnavailable)?;
        #[cfg(not(windows))]
        configure_timeouts(&stream).map_err(HandoffError::Failed)?;

        protocol::write_request(&mut stream, invocation).map_err(|error| {
            HandoffError::Failed(format!("cannot send request to running yori: {error}"))
        })?;
        protocol::read_response(&mut stream).map_err(|error| {
            HandoffError::Failed(format!("handoff to running yori failed: {error}"))
        })?;

        Ok(stream)
    }

    pub async fn next(&self) -> Result<OpenRequest, async_channel::RecvError> {
        self.requests.recv().await
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

fn accept_requests(
    listener: &Listener,
    requests: &Sender<OpenRequest>,
    shutdown: &AtomicBool,
    active_connections: &Arc<AtomicUsize>,
) {
    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok(stream) => {
                let Some(permit) = ConnectionPermit::acquire(Arc::clone(active_connections)) else {
                    continue;
                };
                let requests = requests.clone();
                let result = thread::Builder::new()
                    .name("yori-instance-request".into())
                    .spawn(move || {
                        let mut stream = stream;
                        handle_request(&mut stream, &requests, permit);
                    });
                if let Err(error) = result {
                    eprintln!("yori: cannot handle local request: {error}");
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(error) => {
                eprintln!("yori: local socket listener failed: {error}");
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
        }
    }
}

fn handle_request(stream: &mut Stream, requests: &Sender<OpenRequest>, permit: ConnectionPermit) {
    #[cfg(not(windows))]
    if let Err(error) = configure_timeouts(stream) {
        let _ = protocol::write_response(stream, Err(error));
        return;
    }

    let invocation = match protocol::read_request(stream) {
        Ok(invocation) => invocation,
        Err(error) => {
            let _ = protocol::write_response(stream, Err(format!("invalid request: {error}")));
            return;
        }
    };
    if invocation.wait && invocation.comparisons.len() != 1 {
        let _ = protocol::write_response(
            stream,
            Err("a waiting request must open exactly one comparison".into()),
        );
        return;
    }

    let (reply, response) = mpsc::sync_channel(1);
    let (completion, finished) = if invocation.wait {
        let (reply, finished) = mpsc::sync_channel(1);
        (
            Some(Completion {
                reply,
                saved: false,
            }),
            Some(finished),
        )
    } else {
        (None, None)
    };
    if requests
        .try_send(OpenRequest {
            invocation,
            received: Instant::now(),
            reply,
            completion,
        })
        .is_err()
    {
        let _ = protocol::write_response(
            stream,
            Err("yori is busy or shutting down; retry the request".into()),
        );
        return;
    }

    // This is a workspace acknowledgment, not merely a transport receipt.
    // Perforce may delete its temporary baseline as soon as the sender exits.
    let result = match response.recv_timeout(REQUEST_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err("yori did not open the comparison before the request timed out".into())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("yori closed before opening the comparison".into())
        }
    };
    let accepted = result.is_ok();
    if protocol::write_response(stream, result).is_err() || !accepted {
        return;
    }

    let Some(finished) = finished else {
        return;
    };

    // A waiting connection lasts as long as its tab. It no longer counts as a
    // pending request, so open tabs cannot exhaust the connection limit.
    drop(permit);

    #[cfg(not(windows))]
    if stream.set_send_timeout(None).is_err() {
        return;
    }

    // A closed workspace drops its completions; report that as unsaved.
    let saved = finished.recv().unwrap_or(false);
    let _ = protocol::write_completion(stream, saved);
}

/// Start a detached workspace owner without the waiting invocation's own
/// console or standard streams.
fn spawn_owner(directory: &Path) -> Result<Child, String> {
    let program =
        std::env::current_exe().map_err(|error| format!("cannot locate yori: {error}"))?;
    let mut command = Command::new(program);
    command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    // Interrupting the waiting tool must not end the shared workspace.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        command.process_group(0);
    }

    command
        .spawn()
        .map_err(|error| format!("cannot start yori: {error}"))
}

fn instance_name() -> String {
    std::env::var("YORI_INSTANCE_NAME").unwrap_or_else(|_| INSTANCE_NAME.into())
}

#[cfg(not(windows))]
fn configure_timeouts(stream: &Stream) -> Result<(), String> {
    stream
        .set_recv_timeout(Some(REQUEST_TIMEOUT))
        .and_then(|()| stream.set_send_timeout(Some(REQUEST_TIMEOUT)))
        .map_err(|error| format!("cannot configure local socket timeout: {error}"))
}
