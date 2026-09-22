//! Central application action catalog and atomic keymap replacement.

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use gpui_kit::{App, AsKeystroke, Global, KeyBinding, Keystroke};

use crate::{editor, review, workspace};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct KeybindingOverrides(HashMap<String, Vec<String>>);

impl KeybindingOverrides {
    pub(crate) fn from_raw(
        raw: HashMap<String, Vec<String>>,
        mut errors: Vec<String>,
    ) -> Result<Self, Vec<String>> {
        validate(&raw, &mut errors);

        if errors.is_empty() {
            Ok(Self(raw))
        } else {
            Err(errors)
        }
    }

    fn bindings_for<'a>(&'a self, action: &'a ActionDefinition) -> Vec<&'a str> {
        self.0.get(action.name).map_or_else(
            || action.defaults.to_vec(),
            |bindings| bindings.iter().map(String::as_str).collect(),
        )
    }
}

struct KeymapState {
    baseline: Vec<KeyBinding>,
}

impl Global for KeymapState {}

struct ActionDefinition {
    name: &'static str,
    defaults: &'static [&'static str],
    context: Option<&'static str>,
    binding: fn(&str) -> KeyBinding,
}

macro_rules! action_catalog {
    ($(($builder:ident, $name:literal, [$($default:literal),* $(,)?], $context:expr, $action:path)),* $(,)?) => {
        $(
            fn $builder(keystroke: &str) -> KeyBinding {
                KeyBinding::new(keystroke, $action, $context)
            }
        )*

        const ACTIONS: &[ActionDefinition] = &[
            $(
                ActionDefinition {
                    name: $name,
                    defaults: &[$($default),*],
                    context: $context,
                    binding: $builder,
                },
            )*
        ];
    };
}

action_catalog!(
    (
        open_comparison,
        "open_comparison",
        ["primary-o"],
        Some(workspace::KEY_CONTEXT),
        workspace::OpenComparison
    ),
    (
        open_merge,
        "open_merge",
        ["primary-shift-m"],
        Some(workspace::KEY_CONTEXT),
        workspace::OpenMerge
    ),
    (
        open_git_review,
        "open_git_review",
        ["primary-shift-g"],
        Some(workspace::KEY_CONTEXT),
        workspace::OpenGitReview
    ),
    (
        save,
        "save",
        ["primary-s"],
        Some(workspace::KEY_CONTEXT),
        workspace::Save
    ),
    (
        close_tab,
        "close_tab",
        ["primary-w"],
        Some(workspace::KEY_CONTEXT),
        workspace::CloseComparison
    ),
    (
        quit,
        "quit",
        ["primary-q"],
        Some(workspace::KEY_CONTEXT),
        workspace::Quit
    ),
    (
        preferences,
        "preferences",
        ["primary-,"],
        None,
        workspace::Preferences
    ),
    (
        next_tab,
        "next_tab",
        ["ctrl-tab"],
        Some(workspace::KEY_CONTEXT),
        workspace::NextTab
    ),
    (
        previous_tab,
        "previous_tab",
        ["ctrl-shift-tab"],
        Some(workspace::KEY_CONTEXT),
        workspace::PreviousTab
    ),
    (
        show_home,
        "show_home",
        ["primary-shift-h"],
        Some(workspace::KEY_CONTEXT),
        workspace::ShowHome
    ),
    (
        previous_change,
        "previous_change",
        ["alt-up"],
        Some(editor::KEY_CONTEXT),
        editor::PreviousChange
    ),
    (
        next_change,
        "next_change",
        ["alt-down"],
        Some(editor::KEY_CONTEXT),
        editor::NextChange
    ),
    (
        restore_selected_lines,
        "restore_selected_lines",
        ["alt-enter"],
        Some(editor::KEY_CONTEXT),
        editor::RestoreSelectedLines
    ),
    (
        focus_previous_pane,
        "focus_previous_pane",
        ["ctrl-h"],
        Some(editor::KEY_CONTEXT),
        editor::FocusPreviousPane
    ),
    (
        focus_next_pane,
        "focus_next_pane",
        ["ctrl-l"],
        Some(editor::KEY_CONTEXT),
        editor::FocusNextPane
    ),
    (
        toggle_word_wrap,
        "toggle_word_wrap",
        [],
        Some(editor::KEY_CONTEXT),
        editor::ToggleWordWrap
    ),
    (
        copy,
        "copy",
        ["primary-c"],
        Some(editor::KEY_CONTEXT),
        editor::CopySelected
    ),
    (
        paste,
        "paste",
        ["primary-v"],
        Some(editor::KEY_CONTEXT),
        editor::Paste
    ),
    (
        cut,
        "cut",
        ["primary-x"],
        Some(editor::KEY_CONTEXT),
        editor::CutSelected
    ),
    (
        select_all,
        "select_all",
        ["primary-a"],
        Some(editor::KEY_CONTEXT),
        editor::SelectAll
    ),
    (
        undo,
        "undo",
        ["primary-z"],
        Some(editor::KEY_CONTEXT),
        editor::Undo
    ),
    (
        redo,
        "redo",
        ["primary-shift-z", "primary-y"],
        Some(editor::KEY_CONTEXT),
        editor::Redo
    ),
);

pub(crate) fn init(cx: &mut App) {
    if cx.try_global::<KeymapState>().is_some() {
        return;
    }

    let baseline = cx.key_bindings().borrow().bindings().cloned().collect();
    cx.set_global(KeymapState { baseline });

    apply(&crate::config::keybindings(cx), cx);
}

pub(crate) fn apply(overrides: &KeybindingOverrides, cx: &mut App) {
    let mut bindings = cx.global::<KeymapState>().baseline.clone();
    bindings.extend(fixed_bindings());
    bindings.extend(catalog_bindings(overrides));

    cx.clear_key_bindings();
    cx.bind_keys(bindings);
}

fn catalog_bindings(overrides: &KeybindingOverrides) -> Vec<KeyBinding> {
    ACTIONS
        .iter()
        .flat_map(|action| {
            overrides
                .bindings_for(action)
                .into_iter()
                .map(|keystroke| (action.binding)(&expand_primary(keystroke)))
        })
        .collect()
}

fn fixed_bindings() -> Vec<KeyBinding> {
    let mut bindings = editor::fixed_key_bindings();
    bindings.extend(review::fixed_key_bindings());
    bindings.extend(workspace::fixed_key_bindings());
    bindings
}

fn validate(raw: &HashMap<String, Vec<String>>, errors: &mut Vec<String>) {
    let known_names = ACTIONS
        .iter()
        .map(|action| action.name)
        .collect::<HashSet<_>>();

    let mut names = raw.keys().collect::<Vec<_>>();
    names.sort();

    let mut parsed_overrides = HashMap::new();
    for name in names {
        if !known_names.contains(name.as_str()) {
            errors.push(format!("unknown action `{name}`"));
        }

        let bindings = raw
            .get(name)
            .expect("key came from the keybinding override map");
        let parsed: Vec<Option<Vec<Keystroke>>> = bindings
            .iter()
            .map(|source| match parse_sequence(&expand_primary(source)) {
                Ok(sequence) => Some(sequence),
                Err(error) => {
                    errors.push(format!(
                        "action `{name}` has invalid keystroke `{source}`: {error}"
                    ));
                    None
                }
            })
            .collect();
        parsed_overrides.insert(name.as_str(), parsed);
    }

    let fixed = fixed_bindings();
    let mut occupied = fixed
        .iter()
        .map(|binding| {
            (
                binding_key(binding),
                format!("fixed action `{}`", binding.action().name()),
            )
        })
        .collect::<HashMap<_, _>>();

    for action in ACTIONS {
        let mut record = |source: &str, sequence: Vec<Keystroke>| {
            let key = (action.context.map(str::to_owned), sequence);

            if let Some(previous) = occupied.insert(key, format!("action `{}`", action.name)) {
                errors.push(format!(
                    "action `{}` duplicates {previous} with keystroke `{source}` in context `{}`",
                    action.name,
                    action.context.unwrap_or("global")
                ));
            }
        };

        if let Some(bindings) = raw.get(action.name) {
            let parsed = parsed_overrides
                .get(action.name)
                .expect("configured action was parsed");

            for (source, sequence) in bindings.iter().zip(parsed) {
                if let Some(sequence) = sequence {
                    record(source, sequence.clone());
                }
            }
        } else {
            for source in action.defaults {
                let sequence = parse_sequence(&expand_primary(source))
                    .expect("catalog defaults must be valid keystrokes");
                record(source, sequence);
            }
        }
    }
}

fn binding_key(binding: &KeyBinding) -> (Option<String>, Vec<Keystroke>) {
    let context = binding.predicate().map(|predicate| predicate.to_string());
    let keystrokes = binding
        .keystrokes()
        .iter()
        .map(|keystroke| keystroke.as_keystroke().clone())
        .collect();

    (context, keystrokes)
}

fn parse_sequence(source: &str) -> Result<Vec<Keystroke>, String> {
    if source.trim().is_empty() {
        return Err("keystroke cannot be empty".into());
    }

    source
        .split_whitespace()
        .map(|source| {
            let keystroke = Keystroke::parse(source).map_err(|error| error.to_string())?;
            if keystroke.key_char.is_some() {
                return Err("test-event key_char notation is not supported".into());
            }

            Ok(keystroke)
        })
        .collect()
}

fn expand_primary(source: &str) -> String {
    let primary = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };

    source
        .split_whitespace()
        .map(|keystroke| {
            keystroke
                .split('-')
                .map(|part| if part == "primary" { primary } else { part })
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}
