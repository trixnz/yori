use super::*;
use std::{path::PathBuf, time::Duration};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[test]
fn save_writes_requested_bytes_and_preserves_permissions() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.rs");
    std::fs::write(&path, "old\r\n").unwrap();

    #[cfg(unix)]
    std::fs::set_permissions(&path, Permissions::from_mode(0o754)).unwrap();

    let expected = Snapshot::read(&path).unwrap();
    let bytes = "\t新\r\nlast line".as_bytes();
    let saved = save(&path, &expected, bytes).unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(saved, Snapshot::read(&path).unwrap());

    #[cfg(unix)]
    assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o7777, 0o754);
}

#[test]
fn external_edits_deletions_and_new_destinations_need_exact_approval() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("result.rs");
    let missing = Snapshot::read(&path).unwrap();
    std::fs::write(&path, "external").unwrap();

    assert!(matches!(
        save(&path, &missing, b"ours"),
        Err(SaveError::Changed(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"external");

    let approved = Snapshot::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        save(&path, &approved, b"ours"),
        Err(SaveError::Changed(Snapshot::Missing))
    ));

    save(&path, &Snapshot::Missing, b"ours").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"ours");
}

#[test]
fn edit_during_staging_does_not_overwrite_external_contents() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("local.rs");
    std::fs::write(&path, "before").unwrap();
    let expected = Snapshot::read(&path).unwrap();

    let result = save_with_before_replace(&path, &expected, b"ours", || {
        std::fs::write(&path, "external").unwrap();
    });

    assert!(matches!(result, Err(SaveError::Changed(_))));
    assert_eq!(std::fs::read(&path).unwrap(), b"external");
}

#[test]
fn read_only_files_are_not_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.rs");
    std::fs::write(&path, "before").unwrap();
    let writable_permissions = std::fs::metadata(&path).unwrap().permissions();
    let mut read_only_permissions = writable_permissions.clone();
    read_only_permissions.set_readonly(true);
    std::fs::set_permissions(&path, read_only_permissions).unwrap();
    let snapshot = Snapshot::read(&path).unwrap();

    assert!(matches!(
        save(&path, &snapshot, b"ours"),
        Err(SaveError::Failed(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"before");

    std::fs::set_permissions(&path, writable_permissions).unwrap();
}

#[test]
fn file_becoming_read_only_during_staging_is_not_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.rs");
    std::fs::write(&path, "before").unwrap();
    let expected = Snapshot::read(&path).unwrap();
    let writable_permissions = std::fs::metadata(&path).unwrap().permissions();
    let mut read_only_permissions = writable_permissions.clone();
    read_only_permissions.set_readonly(true);

    let result = save_with_before_replace(&path, &expected, b"ours", || {
        std::fs::set_permissions(&path, read_only_permissions).unwrap();
    });

    assert!(matches!(result, Err(SaveError::Failed(_))));
    assert_eq!(std::fs::read(&path).unwrap(), b"before");

    std::fs::set_permissions(&path, writable_permissions).unwrap();
}

#[cfg(unix)]
#[test]
fn symbolic_link_destinations_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target.rs");
    let path = directory.path().join("source.rs");
    std::fs::write(&target, "before").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();

    assert!(Snapshot::read(&path).is_err());
    assert!(save(&path, &Snapshot::Missing, b"ours").is_err());
    assert!(path.is_symlink());
    assert_eq!(std::fs::read(&target).unwrap(), b"before");
}

#[test]
fn directory_watcher_survives_replacement_deletion_and_recreation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("local.rs");
    std::fs::write(&path, "initial").unwrap();
    let (mut watch, events) = FileWatch::new().unwrap();
    watch.set_paths([path.clone()].into()).unwrap();

    for value in [
        Some(b"replaced".as_slice()),
        None,
        Some(b"recreated".as_slice()),
    ] {
        while events.try_recv().is_ok() {}
        let (done, completion) = std::sync::mpsc::channel();
        let events = events.clone();
        let worker = std::thread::spawn(move || {
            done.send(events.recv_blocking()).unwrap();
        });

        if let Some(value) = value {
            let expected = Snapshot::read(&path).unwrap();
            save(&path, &expected, value).unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }

        completion
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        assert_eq!(Snapshot::read(&path).unwrap().is_missing(), value.is_none());
    }

    watch
        .set_paths(std::collections::HashSet::<PathBuf>::new())
        .unwrap();
}
