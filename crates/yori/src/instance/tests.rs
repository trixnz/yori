use super::*;
use crate::comparison::{Comparison, ComparisonDocument};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
        mpsc,
    },
};

static NEXT_NAME: AtomicUsize = AtomicUsize::new(0);

fn instance_name() -> String {
    format!(
        "yori-test-{}-{}",
        std::process::id(),
        NEXT_NAME.fetch_add(1, AtomicOrdering::Relaxed)
    )
}

fn connect(name: &str) -> Stream {
    let name = name.to_ns_name::<GenericNamespaced>().unwrap();
    Stream::connect(name).unwrap()
}

fn invocation(comparisons: Vec<Comparison>) -> InvocationRequest {
    InvocationRequest::new(std::env::current_dir().unwrap(), comparisons)
}

fn wait_for_connection_count(instance: &Instance, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while instance.active_connections.load(AtomicOrdering::Acquire) != expected {
        assert!(Instant::now() < deadline, "connection count did not settle");
        thread::sleep(Duration::from_millis(10));
    }
}

fn receive_request(instance: &Instance) -> OpenRequest {
    let deadline = Instant::now() + Duration::from_secs(5);

    loop {
        match instance.requests.try_recv() {
            Ok(request) => return request,
            Err(async_channel::TryRecvError::Closed) => panic!("request channel closed"),
            Err(async_channel::TryRecvError::Empty) => {}
        }

        assert!(Instant::now() < deadline, "request was not dispatched");
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn unusual_path(directory: &Path) -> PathBuf {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    directory.join(OsString::from_vec(b"baseline with spaces\xff.rs".to_vec()))
}

#[cfg(windows)]
fn unusual_path(directory: &Path) -> PathBuf {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};

    let units = "baseline with spaces".encode_utf16().chain([
        0xd800,
        u16::from(b'.'),
        u16::from(b'r'),
        u16::from(b's'),
    ]);
    directory.join(OsString::from_wide(&units.collect::<Vec<_>>()))
}

#[test]
fn secondary_waits_for_workspace_acknowledgment_and_preserves_native_paths() {
    let name = instance_name();
    let directory = tempfile::tempdir().unwrap();
    let primary = Instance::establish(&name, &invocation(Vec::new()))
        .unwrap()
        .unwrap();
    let forwarded = InvocationRequest::new(
        unusual_path(directory.path()),
        vec![Comparison::diff(
            unusual_path(directory.path()),
            directory.path().join("local\nfile.rs"),
        )],
    );
    let expected = forwarded.clone();
    let (finished, completion) = mpsc::channel();
    let client = thread::spawn(move || {
        finished
            .send(Instance::establish(&name, &forwarded).map(|instance| instance.is_none()))
            .unwrap();
    });

    let request = receive_request(&primary);
    assert_eq!(request.invocation, expected);
    assert!(
        completion.recv_timeout(Duration::from_millis(30)).is_err(),
        "delivery alone must not release temporary inputs"
    );

    request.complete(Ok(()));
    assert!(
        completion
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap()
    );
    client.join().unwrap();
}

#[test]
fn merge_handoff_preserves_all_four_roles() {
    let name = instance_name();
    let directory = tempfile::tempdir().unwrap();
    let primary = Instance::establish(&name, &invocation(Vec::new()))
        .unwrap()
        .unwrap();
    let paths = [
        directory.path().join("base.rs"),
        directory.path().join("local.rs"),
        directory.path().join("incoming.rs"),
        unusual_path(directory.path()),
    ];
    let expected = vec![Comparison::from_paths(&paths).unwrap()];
    let forwarded = invocation(expected.clone());
    let client = thread::spawn(move || Instance::establish(&name, &forwarded).unwrap().is_none());

    let request = receive_request(&primary);
    assert_eq!(request.invocation.comparisons, expected);
    assert!(!client.is_finished());

    request.complete(Ok(()));
    assert!(client.join().unwrap());
}

#[test]
fn workspace_errors_are_returned_without_starting_a_second_instance() {
    let name = instance_name();
    let primary = Instance::establish(&name, &invocation(Vec::new()))
        .unwrap()
        .unwrap();
    let client = thread::spawn(move || {
        Instance::establish(&name, &invocation(Vec::new()))
            .err()
            .expect("handoff should fail")
    });

    let request = receive_request(&primary);
    assert!(
        request.invocation.comparisons.is_empty(),
        "an empty request activates the window"
    );
    request.complete(Err("cannot read temporary baseline".into()));

    assert!(
        client
            .join()
            .unwrap()
            .contains("cannot read temporary baseline")
    );
}

#[test]
fn launcher_becomes_primary_when_the_previous_owner_exits_before_handoff() {
    let name = instance_name();
    let request = invocation(Vec::new());
    let mut primary = Some(Instance::establish(&name, &request).unwrap().unwrap());

    let replacement = Instance::establish_with_before_handoff(&name, &request, || {
        drop(primary.take());
    })
    .unwrap();

    assert!(replacement.is_some());
}

#[test]
fn concurrent_launches_elect_exactly_one_owner() {
    let name = instance_name();
    let barrier = Arc::new(Barrier::new(4));
    let clients = (0..4)
        .map(|_| {
            let name = name.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let instance = Instance::establish(&name, &invocation(Vec::new())).unwrap();
                if let Some(primary) = &instance {
                    for _ in 0..3 {
                        receive_request(primary).complete(Ok(()));
                    }
                }

                instance
            })
        })
        .collect::<Vec<_>>();
    let outcomes = clients
        .into_iter()
        .map(|client| client.join().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(
        outcomes
            .iter()
            .filter(|instance| instance.is_some())
            .count(),
        1
    );
}

#[test]
fn excess_connections_are_rejected_before_ui_dispatch() {
    let name = instance_name();
    let primary = Instance::establish(&name, &invocation(Vec::new()))
        .unwrap()
        .unwrap();
    let held = (0..MAX_PENDING_CONNECTIONS)
        .map(|_| connect(&name))
        .collect::<Vec<_>>();
    wait_for_connection_count(&primary, MAX_PENDING_CONNECTIONS);

    let mut excess = connect(&name);
    let (finished, completion) = mpsc::channel();
    let client = thread::spawn(move || {
        let result = protocol::write_request(&mut excess, &invocation(Vec::new()))
            .and_then(|()| protocol::read_response(&mut excess));
        finished.send(result).unwrap();
    });
    let result = completion
        .recv_timeout(Duration::from_secs(1))
        .expect("excess connection was not rejected promptly");

    assert!(result.is_err());
    client.join().unwrap();
    assert!(primary.requests.try_recv().is_err());

    drop(held);
    wait_for_connection_count(&primary, 0);
}

#[test]
fn protocol_rejects_read_only_local_file_instead_of_losing_its_capability() {
    let directory = tempfile::tempdir().unwrap();
    let comparison = Comparison::two_way(
        ComparisonDocument::read_only_file(directory.path().join("baseline.rs")),
        ComparisonDocument::read_only_file(directory.path().join("local.rs")),
    );

    let error =
        protocol::write_request(&mut Vec::new(), &invocation(vec![comparison])).unwrap_err();

    assert!(error.contains("capabilities cannot be represented"));
}

#[test]
fn protocol_rejects_invalid_requests_before_dispatch() {
    let directory = tempfile::tempdir().unwrap();
    let relative_directory = InvocationRequest::new(PathBuf::from("relative"), Vec::new());
    let error = protocol::write_request(&mut Vec::new(), &relative_directory).unwrap_err();
    assert!(error.contains("invocation directory must be absolute"));

    let relative = Comparison::diff(
        PathBuf::from("relative.rs"),
        directory.path().join("local.rs"),
    );
    assert!(protocol::write_request(&mut Vec::new(), &invocation(vec![relative])).is_err());

    let malformed = [3, 0, 0, 0, 0xff, 0xff, 0xff];
    assert!(protocol::read_request(&mut malformed.as_slice()).is_err());

    let oversized = u32::MAX.to_le_bytes();
    assert!(protocol::read_request(&mut oversized.as_slice()).is_err());
}

fn waiting_merge(directory: &Path) -> InvocationRequest {
    let paths =
        ["base.rs", "local.rs", "incoming.rs", "result.rs"].map(|name| directory.join(name));

    invocation(vec![Comparison::from_paths(&paths).unwrap()]).waiting()
}

#[test]
fn waiting_invocation_returns_when_its_tab_reports_the_save_outcome() {
    let directory = tempfile::tempdir().unwrap();

    for saved in [true, false] {
        let name = instance_name();
        let primary = Instance::establish(&name, &invocation(Vec::new()))
            .unwrap()
            .unwrap();
        let forwarded = waiting_merge(directory.path());
        let client = thread::spawn(move || {
            Instance::wait_with_owner(&name, &forwarded, || {
                panic!("a running owner needs no replacement")
            })
        });

        let mut request = receive_request(&primary);
        assert!(request.invocation.wait);
        let mut completion = request.take_completion().expect("waiting request");
        request.complete(Ok(()));

        // The acknowledged connection waits for the tab, outside the pending limit.
        wait_for_connection_count(&primary, 0);
        thread::sleep(Duration::from_millis(30));
        assert!(!client.is_finished(), "the tab is still open");

        if saved {
            completion.mark_saved();
        }
        drop(completion);

        assert_eq!(client.join().unwrap(), Ok(saved));
    }
}

#[test]
fn waiting_invocation_starts_a_detached_owner_when_none_runs() {
    let name = instance_name();
    let directory = tempfile::tempdir().unwrap();
    let forwarded = waiting_merge(directory.path());
    let owner_name = name.clone();
    let (owners, started) = mpsc::channel();
    let client = thread::spawn(move || {
        let mut starts = 0;
        let result = Instance::wait_with_owner(&name, &forwarded, || {
            starts += 1;
            let owner = Instance::establish(&owner_name, &invocation(Vec::new()))?
                .ok_or("the test owner must claim the name")?;
            owners.send(owner).unwrap();

            Ok(())
        });

        (starts, result)
    });

    let owner = started.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut request = receive_request(&owner);
    let completion = request.take_completion().expect("waiting request");
    request.complete(Ok(()));
    drop(completion);

    assert_eq!(client.join().unwrap(), (1, Ok(false)));
}

#[test]
fn waiting_requests_must_open_exactly_one_comparison() {
    let name = instance_name();
    let _primary = Instance::establish(&name, &invocation(Vec::new()))
        .unwrap()
        .unwrap();

    let error = Instance::wait_with_owner(&name, &invocation(Vec::new()).waiting(), || {
        panic!("a running owner needs no replacement")
    })
    .unwrap_err();

    assert!(error.contains("exactly one comparison"));
}
