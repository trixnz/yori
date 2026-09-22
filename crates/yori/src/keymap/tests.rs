use super::*;
use gpui_kit::{Action, App, TestAppContext};
use std::{collections::HashMap, fs};

gpui_kit::actions!(keymap_tests, [BaselineAction]);

fn raw(entries: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
    entries
        .iter()
        .map(|(name, bindings)| {
            (
                (*name).to_owned(),
                bindings
                    .iter()
                    .map(|binding| (*binding).to_owned())
                    .collect(),
            )
        })
        .collect()
}

fn validated(entries: &[(&str, &[&str])]) -> KeybindingOverrides {
    KeybindingOverrides::from_raw(raw(entries), Vec::new()).unwrap()
}

fn action(name: &str) -> &'static ActionDefinition {
    ACTIONS.iter().find(|action| action.name == name).unwrap()
}

fn has_binding(cx: &App, action: &dyn Action, source: &str) -> bool {
    let input = parse_sequence(&expand_primary(source)).unwrap();

    cx.key_bindings()
        .borrow()
        .bindings_for_action(action)
        .any(|binding| binding.match_keystrokes(&input) == Some(false))
}

fn binding_count(cx: &App, action: &dyn Action) -> usize {
    cx.key_bindings()
        .borrow()
        .bindings_for_action(action)
        .count()
}

#[test]
fn missing_actions_use_defaults_and_overrides_replace_them() {
    let defaults = KeybindingOverrides::default();
    assert_eq!(defaults.bindings_for(action("save")), vec!["primary-s"]);
    assert_eq!(
        defaults.bindings_for(action("redo")),
        vec!["primary-shift-z", "primary-y"]
    );

    let overrides = validated(&[("save", &["primary-k", "ctrl-alt-s"])]);
    assert_eq!(
        overrides.bindings_for(action("save")),
        vec!["primary-k", "ctrl-alt-s"]
    );
    assert_eq!(
        overrides.bindings_for(action("redo")),
        vec!["primary-shift-z", "primary-y"]
    );
}

#[test]
fn empty_array_disables_an_action() {
    let overrides = validated(&[("redo", &[])]);

    assert!(overrides.bindings_for(action("redo")).is_empty());
}

#[test]
fn primary_expands_to_the_platform_command_modifier() {
    let expected = if cfg!(target_os = "macos") {
        "cmd-k cmd-shift-p"
    } else {
        "ctrl-k ctrl-shift-p"
    };

    assert_eq!(expand_primary("primary-k primary-shift-p"), expected);
}

#[test]
fn validation_reports_every_unknown_malformed_and_duplicate_binding() {
    let result = KeybindingOverrides::from_raw(
        raw(&[
            ("future_action", &["primary-f"]),
            ("save", &["ctrl-tab"]),
            ("next_change", &["not-a-real-keystroke"]),
        ]),
        vec!["action `undo` must be an array of keystroke strings".into()],
    );
    let errors = result.unwrap_err().join("\n");

    assert!(errors.contains("unknown action `future_action`"));
    assert!(errors.contains("action `next_change` has invalid keystroke"));
    assert!(errors.contains("duplicates action"));
    assert!(errors.contains("`save`"));
    assert!(errors.contains("`next_tab`"));
    assert!(errors.contains("action `undo` must be an array"));
}

#[test]
fn validation_rejects_duplicates_with_fixed_same_context_bindings() {
    let errors = KeybindingOverrides::from_raw(raw(&[("next_change", &["left"])]), Vec::new())
        .unwrap_err()
        .join("\n");

    assert!(errors.contains("action `next_change` duplicates fixed action"));
    assert!(errors.contains("context `AlignedEditor`"));
}

#[test]
fn same_keystroke_is_allowed_in_different_contexts() {
    KeybindingOverrides::from_raw(
        raw(&[("save", &["alt-down"]), ("next_change", &["alt-down"])]),
        Vec::new(),
    )
    .unwrap();
}

#[test]
fn test_event_key_char_notation_is_rejected() {
    let errors = KeybindingOverrides::from_raw(raw(&[("save", &["ctrl-k->x"])]), Vec::new())
        .unwrap_err()
        .join("\n");

    assert!(errors.contains("action `save` has invalid keystroke `ctrl-k->x`"));
    assert!(errors.contains("test-event key_char notation is not supported"));
}

#[test]
fn empty_keystrokes_are_rejected() {
    let errors = KeybindingOverrides::from_raw(raw(&[("save", &[""])]), Vec::new())
        .unwrap_err()
        .join("\n");

    assert!(errors.contains("keystroke cannot be empty"));
}

#[gpui_kit::test]
fn reload_replaces_disables_and_preserves_the_last_valid_keymap(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("yori").join("config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();

    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::config::init_for_path(path.clone(), cx);
        init(cx);
    });

    cx.update(|cx| {
        let save = (action("save").binding)("ctrl-s");
        let redo = (action("redo").binding)("ctrl-y");

        assert!(has_binding(cx, save.action(), "primary-s"));
        assert_eq!(binding_count(cx, redo.action()), 2);
    });

    fs::write(
        &path,
        "[keybindings]\nsave = [\"primary-k\", \"ctrl-alt-s\"]\nredo = []\n",
    )
    .unwrap();
    cx.update(|cx| {
        assert!(crate::config::reload(cx).is_none());

        let save = (action("save").binding)("ctrl-s");
        let redo = (action("redo").binding)("ctrl-y");
        assert!(!has_binding(cx, save.action(), "primary-s"));
        assert!(has_binding(cx, save.action(), "primary-k"));
        assert!(has_binding(cx, save.action(), "ctrl-alt-s"));
        assert_eq!(binding_count(cx, redo.action()), 0);
    });

    let version = cx.update(|cx| cx.key_bindings().borrow().version());
    fs::write(
        &path,
        "[editor]\nshow_whitespace = true\n\n[keybindings]\nsave = [\"ctrl-tab\"]\nunknown = [\"primary-u\"]\nnext_change = [\"broken-key\"]\n",
    )
    .unwrap();
    cx.update(|cx| {
        let diagnostic = crate::config::reload(cx).unwrap();
        assert!(diagnostic.contains("unknown action `unknown`"));
        assert!(diagnostic.contains("duplicates action"));
        assert!(diagnostic.contains("`save`"));
        assert!(diagnostic.contains("`next_tab`"));
        assert!(diagnostic.contains("action `next_change` has invalid keystroke"));
        assert!(crate::config::editor(cx).show_whitespace);
        assert!(cx.key_bindings().borrow().version() == version);

        let save = (action("save").binding)("ctrl-s");
        assert!(has_binding(cx, save.action(), "primary-k"));
        assert!(has_binding(cx, save.action(), "ctrl-alt-s"));
    });

    fs::write(
        &path,
        "[editor]\nshow_whitespace = false\n\n[keybindings]\nsave = [\"ctrl-k->x\"]\nnext_tab = [\"ctrl-k\"]\n",
    )
    .unwrap();
    cx.update(|cx| {
        let diagnostic = crate::config::reload(cx).unwrap();
        assert!(diagnostic.contains("test-event key_char notation is not supported"));
        assert!(!crate::config::editor(cx).show_whitespace);
        assert!(cx.key_bindings().borrow().version() == version);

        let save = (action("save").binding)("ctrl-s");
        assert!(has_binding(cx, save.action(), "primary-k"));
        assert!(has_binding(cx, save.action(), "ctrl-alt-s"));
    });
}

#[gpui_kit::test]
fn atomic_rebuild_preserves_preexisting_and_fixed_bindings(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("yori").join("config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "[keybindings]\nsave = [\"primary-k\"]\n").unwrap();

    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys([KeyBinding::new("f12", BaselineAction, Some("Baseline"))]);
        crate::config::init_for_path(path.clone(), cx);
        init(cx);

        let baseline = BaselineAction;
        let move_left = crate::editor::MoveLeft;
        assert!(has_binding(cx, &baseline, "f12"));
        assert!(has_binding(cx, &move_left, "left"));
    });

    fs::write(&path, "[keybindings]\nsave = [\"primary-l\"]\n").unwrap();
    cx.update(|cx| {
        crate::config::reload(cx);

        let baseline = BaselineAction;
        let move_left = crate::editor::MoveLeft;
        assert!(has_binding(cx, &baseline, "f12"));
        assert!(has_binding(cx, &move_left, "left"));
    });
}
