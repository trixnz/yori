//! Exercise the real CLI as a secondary process, without a display server.

#[expect(
    dead_code,
    reason = "the CLI fixture shares production wire types without using their UI methods"
)]
#[path = "../src/comparison.rs"]
mod comparison;
#[path = "../src/invocation.rs"]
mod invocation;
#[expect(
    dead_code,
    reason = "the fake primary uses only the server half of the production protocol"
)]
#[path = "../src/instance/protocol.rs"]
mod protocol;

use comparison::Comparison;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, prelude::*};
use invocation::InvocationRequest;
use std::{
    ffi::OsString,
    io::Read,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

static NEXT_NAME: AtomicUsize = AtomicUsize::new(0);
type Pending = (InvocationRequest, mpsc::SyncSender<Result<(), String>>);

struct RunningCli(Child);

impl Drop for RunningCli {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn instance_name() -> String {
    format!(
        "yori-cli-test-{}-{}",
        std::process::id(),
        NEXT_NAME.fetch_add(1, Ordering::Relaxed)
    )
}

fn workspace_stub(
    name: &str,
    request_count: usize,
) -> (mpsc::Receiver<Pending>, thread::JoinHandle<()>) {
    let socket_name = name.to_ns_name::<GenericNamespaced>().unwrap();
    let listener = ListenerOptions::new()
        .name(socket_name)
        .create_sync()
        .unwrap();
    let (requests, incoming) = mpsc::channel();
    let worker = thread::spawn(move || {
        for _ in 0..request_count {
            let mut stream = listener.accept().unwrap();
            let invocation = protocol::read_request(&mut stream).unwrap();
            let (reply, response) = mpsc::sync_channel(1);
            requests.send((invocation, reply)).unwrap();
            protocol::write_response(&mut stream, response.recv().unwrap()).unwrap();
        }
    });

    (incoming, worker)
}

fn cli(name: &str, directory: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_yori"));
    command
        .current_dir(directory)
        .env("YORI_INSTANCE_NAME", name)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    command
}

#[cfg(unix)]
fn paths() -> [OsString; 4] {
    use std::os::unix::ffi::OsStringExt;

    [
        OsString::from("base with spaces.rs"),
        OsString::from_vec(b"local\xff.rs".to_vec()),
        OsString::from("incoming\nfile.go"),
        OsString::from_vec(b"result\xfe.cpp".to_vec()),
    ]
}

#[cfg(windows)]
fn paths() -> [OsString; 4] {
    use std::os::windows::ffi::OsStringExt;

    let unusual = |stem: &str, surrogate| {
        OsString::from_wide(
            &stem
                .encode_utf16()
                .chain([surrogate, u16::from(b'.'), u16::from(b'r'), u16::from(b's')])
                .collect::<Vec<_>>(),
        )
    };
    [
        OsString::from("base with spaces.rs"),
        unusual("local", 0xd800),
        OsString::from("incoming\nfile.go"),
        unusual("result", 0xd801),
    ]
}

#[test]
fn cli_forwards_invocation_directories_and_file_roles_before_exiting() {
    let name = instance_name();
    let repository_a = tempfile::tempdir().unwrap();
    let repository_b = tempfile::tempdir().unwrap();
    let repository_a_path = repository_a.path().canonicalize().unwrap();
    let repository_b_path = repository_b.path().canonicalize().unwrap();
    let (incoming, worker) = workspace_stub(&name, 5);
    let paths = paths();

    for (index, (count, fail)) in [(2, false), (0, false), (2, true), (4, false), (4, true)]
        .into_iter()
        .enumerate()
    {
        let directory = if index == 1 {
            &repository_b_path
        } else {
            &repository_a_path
        };
        let mut command = cli(&name, directory);
        command.args(&paths[..count]);
        let mut child = RunningCli(command.spawn().unwrap());
        let (invocation, reply) = incoming.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            invocation.directory.canonicalize().unwrap(),
            directory.as_path()
        );
        let expected = if count == 0 {
            Vec::new()
        } else {
            vec![
                Comparison::from_paths(
                    &paths[..count]
                        .iter()
                        .map(|path| invocation.directory.join(path))
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            ]
        };
        assert_eq!(invocation.comparisons, expected);
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "the CLI must wait until all inputs have been consumed"
        );

        reply
            .send(if fail {
                Err("temporary input unreadable".into())
            } else {
                Ok(())
            })
            .unwrap();
        let status = child.0.wait().unwrap();
        let mut stderr = String::new();
        child
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert_eq!(status.success(), !fail, "{stderr}");
        if fail {
            assert!(stderr.contains("temporary input unreadable"));
        }
    }

    worker.join().unwrap();
}

#[test]
fn invalid_arguments_fail_before_instance_startup() {
    let name = instance_name();
    let directory = tempfile::tempdir().unwrap();
    for count in [1, 3, 5, 6] {
        let result = cli(&name, directory.path())
            .args(vec!["file.rs"; count])
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&result.stderr).contains("usage:"));
    }
}
