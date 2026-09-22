use super::*;
use crate::storage::FileWatch;
use std::time::Duration;

fn path(directory: &tempfile::TempDir) -> PathBuf {
    directory.path().join("yori").join(FILE_NAME)
}

fn wait_for_change(events: &async_channel::Receiver<()>) {
    let events = events.clone();
    let (done, completion) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        done.send(events.recv_blocking()).unwrap();
    });

    completion
        .recv_timeout(Duration::from_secs(3))
        .expect("configuration watcher did not report the file change")
        .unwrap();
    worker.join().unwrap();
}

fn drain(events: &async_channel::Receiver<()>) {
    while events.try_recv().is_ok() {}
}

#[test]
fn missing_configuration_uses_defaults() {
    let directory = tempfile::tempdir().unwrap();
    let configuration = Configuration::persistent(path(&directory));

    assert_eq!(configuration.editor, EditorConfig::default());
    assert!(configuration.diagnostic.is_none());
}

#[test]
fn in_memory_updates_preserve_the_unavailable_configuration_diagnostic() {
    let diagnostic = "cannot locate the platform configuration directory";
    let editor = EditorConfig {
        vim_keybindings: true,
        show_whitespace: true,
        show_change_connections: false,
    };
    let mut configuration = Configuration {
        diagnostic: Some(diagnostic.into()),
        ..Configuration::default()
    };

    assert!(configuration.update_editor(editor).unwrap().is_none());

    assert_eq!(configuration.editor, editor);
    assert_eq!(configuration.diagnostic.as_deref(), Some(diagnostic));
}

#[test]
fn valid_configuration_survives_a_new_store() {
    let directory = tempfile::tempdir().unwrap();
    let path = path(&directory);
    let expected = EditorConfig {
        vim_keybindings: true,
        show_whitespace: true,
        show_change_connections: false,
    };
    let mut first = Configuration::persistent(path.clone());

    assert!(first.update_editor(expected).unwrap().is_none());
    let restarted = Configuration::persistent(path);

    assert_eq!(restarted.editor, expected);
    assert!(restarted.diagnostic.is_none());
}

#[test]
fn invalid_individual_settings_only_fall_back_for_their_keys() {
    let document = r#"
[editor]
vim_keybindings = true
show_whitespace = "sometimes"
show_change_connections = true
"#
    .parse::<DocumentMut>()
    .unwrap();

    let (editor, diagnostic) = parse_editor(&document);

    assert_eq!(
        editor,
        EditorConfig {
            vim_keybindings: true,
            show_whitespace: false,
            show_change_connections: true,
        }
    );
    assert!(diagnostic.unwrap().contains("show_whitespace"));
}

#[test]
fn invalid_keybindings_do_not_block_valid_editor_settings() {
    let directory = tempfile::tempdir().unwrap();
    let path = path(&directory);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "[editor]\nshow_whitespace = true\n\n[keybindings]\nsave = \"primary-k\"\nundo = [1]\nfuture_action = [\"primary-f\"]\n",
    )
    .unwrap();

    let configuration = Configuration::persistent(path);
    let diagnostic = configuration.diagnostic.unwrap();

    assert!(configuration.editor.show_whitespace);
    assert_eq!(configuration.keybindings, KeybindingOverrides::default());
    assert!(diagnostic.contains("action `save` must be an array"));
    assert!(diagnostic.contains("action `undo` binding 1 must be a string"));
    assert!(diagnostic.contains("unknown action `future_action`"));
}

#[test]
fn keybinding_validation_aggregates_unknown_shape_element_and_keystroke_errors() {
    let document = r#"
[keybindings]
future_shape = "ctrl-k"
future_values = [1, "not-a-real-keystroke"]
save = [false, "also-broken"]
"#
    .parse::<DocumentMut>()
    .unwrap();

    let errors = parse_keybindings(&document).unwrap_err().join("\n");

    assert!(errors.contains("unknown action `future_shape`"));
    assert!(errors.contains("action `future_shape` must be an array"));
    assert!(errors.contains("unknown action `future_values`"));
    assert!(errors.contains("action `future_values` binding 1 must be a string"));
    assert!(errors.contains("action `future_values` has invalid keystroke"));
    assert!(errors.contains("action `save` binding 1 must be a string"));
    assert!(errors.contains("action `save` has invalid keystroke"));
}

#[test]
fn invalid_file_retains_the_last_valid_configuration_until_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let path = path(&directory);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "[editor]\nvim_keybindings = true\nshow_whitespace = true\n",
    )
    .unwrap();
    let mut configuration = Configuration::persistent(path.clone());
    let valid = configuration.editor;

    fs::write(&path, "[editor\nvim_keybindings = false").unwrap();
    configuration.reload();

    assert_eq!(configuration.editor, valid);
    assert!(
        configuration
            .diagnostic
            .as_deref()
            .is_some_and(|message| { message.contains("invalid configuration") })
    );

    fs::write(
        &path,
        "[editor]\nvim_keybindings = false\nshow_change_connections = true\n",
    )
    .unwrap();
    configuration.reload();

    assert_eq!(
        configuration.editor,
        EditorConfig {
            vim_keybindings: false,
            show_whitespace: false,
            show_change_connections: true,
        }
    );
    assert!(configuration.diagnostic.is_none());
}

#[test]
fn watcher_handles_direct_edits_atomic_replacement_deletion_and_recreation() {
    let directory = tempfile::tempdir().unwrap();
    let path = path(&directory);
    let mut configuration = Configuration::persistent(path.clone());
    let (mut watch, events) = FileWatch::new().unwrap();
    watch.set_paths([path.clone()].into()).unwrap();

    drain(&events);
    fs::write(&path, "[editor]\nshow_whitespace = true\n").unwrap();
    wait_for_change(&events);
    configuration.reload();

    assert!(configuration.editor.show_whitespace);

    drain(&events);
    update_document(
        &path,
        EditorConfig {
            vim_keybindings: true,
            show_whitespace: false,
            show_change_connections: true,
        },
    )
    .unwrap();
    wait_for_change(&events);
    configuration.reload();

    assert!(configuration.editor.vim_keybindings);
    assert!(configuration.editor.show_change_connections);

    drain(&events);
    fs::remove_file(&path).unwrap();
    wait_for_change(&events);
    configuration.reload();

    assert_eq!(configuration.editor, EditorConfig::default());

    drain(&events);
    fs::write(&path, "[editor]\nshow_whitespace = true\n").unwrap();
    wait_for_change(&events);
    configuration.reload();

    assert!(configuration.editor.show_whitespace);
}

#[test]
fn updates_preserve_comments_unknown_keys_and_file_permissions() {
    let directory = tempfile::tempdir().unwrap();
    let path = path(&directory);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "# user notes\n[editor]\n# key-specific note\nvim_keybindings = false # keep this inline note\nfuture_option = \"future\"\n\n# shortcut notes\n[keybindings]\nsave = [\"primary-k\"] # keep shortcut note\n\n[plugin]\nenabled = true\n",
    )
    .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    }

    let mut configuration = Configuration::persistent(path.clone());
    assert!(
        configuration
            .update_editor(EditorConfig {
                vim_keybindings: true,
                show_whitespace: true,
                show_change_connections: false,
            })
            .unwrap()
            .is_none()
    );
    let updated = fs::read_to_string(&path).unwrap();

    assert!(updated.contains("# user notes\n[editor]"));
    assert!(
        updated.contains("# key-specific note\nvim_keybindings = true # keep this inline note")
    );
    assert!(updated.contains("future_option = \"future\""));
    assert!(updated.contains("# shortcut notes\n[keybindings]"));
    assert!(updated.contains("save = [\"primary-k\"] # keep shortcut note"));
    assert!(updated.contains("[plugin]\nenabled = true"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}
