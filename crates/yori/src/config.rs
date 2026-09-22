//! Persistent, edit-preserving application configuration.

#[cfg(test)]
mod tests;

use atomicwrites::{AllowOverwrite, AtomicFile};
use gpui_kit::{App, Global};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use crate::keymap::KeybindingOverrides;
use toml_edit::{DocumentMut, Item, Table, TableLike, Value, value};

const APPLICATION_NAME: &str = "yori";
const FILE_NAME: &str = "config.toml";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the persisted editor schema intentionally exposes independent boolean preferences"
)]
pub(crate) struct EditorConfig {
    pub vim_keybindings: bool,
    pub show_whitespace: bool,
    pub show_change_connections: bool,
    pub word_wrap: bool,
}

#[derive(Default)]
pub(crate) struct Configuration {
    path: Option<PathBuf>,
    editor: EditorConfig,
    keybindings: KeybindingOverrides,
    diagnostic: Option<String>,
}

impl Global for Configuration {}

impl Configuration {
    fn persistent(path: PathBuf) -> Self {
        let directory_error = path.parent().and_then(|parent| {
            fs::create_dir_all(parent)
                .err()
                .map(|error| format!("cannot create {}: {error}", parent.display()))
        });
        let mut configuration = Self {
            path: Some(path),
            ..Self::default()
        };
        configuration.reload();

        if directory_error.is_some() {
            configuration.diagnostic = directory_error;
        }

        configuration
    }

    fn refreshed(&self) -> (EditorConfig, KeybindingOverrides, Option<String>) {
        let Some(path) = &self.path else {
            return (
                self.editor,
                self.keybindings.clone(),
                self.diagnostic.clone(),
            );
        };

        match load(path) {
            Ok(loaded) => match loaded.keybindings {
                Ok(keybindings) => (loaded.editor, keybindings, loaded.editor_diagnostic),
                Err(errors) => (
                    loaded.editor,
                    self.keybindings.clone(),
                    combine_diagnostics(
                        loaded.editor_diagnostic,
                        Some(keybinding_diagnostic(&errors)),
                    ),
                ),
            },
            Err(error) => (self.editor, self.keybindings.clone(), Some(error)),
        }
    }

    fn reload(&mut self) -> Option<String> {
        let previous_diagnostic = self.diagnostic.clone();
        let (editor, keybindings, diagnostic) = self.refreshed();
        self.editor = editor;
        self.keybindings = keybindings;
        self.diagnostic = diagnostic;

        (self.diagnostic != previous_diagnostic)
            .then(|| self.diagnostic.clone())
            .flatten()
    }

    fn update_editor(&mut self, editor: EditorConfig) -> Result<Option<String>, String> {
        let Some(path) = &self.path else {
            self.editor = editor;
            return Ok(None);
        };

        let result = update_document(path, editor);
        match result {
            Ok(()) => Ok(self.reload()),
            Err(error) => {
                self.diagnostic = Some(error.clone());
                Err(error)
            }
        }
    }
}

pub(crate) fn init(cx: &mut App) {
    let configuration = dirs::config_dir().map_or_else(
        || Configuration {
            diagnostic: Some("cannot locate the platform configuration directory".into()),
            ..Configuration::default()
        },
        |directory| Configuration::persistent(directory.join(APPLICATION_NAME).join(FILE_NAME)),
    );

    cx.set_global(configuration);
}

#[cfg(test)]
pub(crate) fn init_for_path(path: PathBuf, cx: &mut App) {
    cx.set_global(Configuration::persistent(path));
}

pub(crate) fn init_transient(cx: &mut App) {
    if cx.try_global::<Configuration>().is_none() {
        cx.set_global(Configuration::default());
    }
}

pub(crate) fn editor(cx: &App) -> EditorConfig {
    cx.global::<Configuration>().editor
}

pub(crate) fn path(cx: &App) -> Option<PathBuf> {
    cx.global::<Configuration>().path.clone()
}

pub(crate) fn diagnostic(cx: &App) -> Option<String> {
    cx.global::<Configuration>().diagnostic.clone()
}

pub(crate) fn keybindings(cx: &App) -> KeybindingOverrides {
    cx.global::<Configuration>().keybindings.clone()
}

/// Reload the current file and return a newly changed diagnostic to report.
pub(crate) fn reload(cx: &mut App) -> Option<String> {
    let previous_keybindings = keybindings(cx);
    let report = cx.global_mut::<Configuration>().reload();
    let current_keybindings = keybindings(cx);

    if current_keybindings != previous_keybindings {
        crate::keymap::apply(&current_keybindings, cx);
    }

    report
}

pub(crate) fn update_editor(editor: EditorConfig, cx: &mut App) -> Result<Option<String>, String> {
    let previous_keybindings = keybindings(cx);
    let result = cx.global_mut::<Configuration>().update_editor(editor);
    let current_keybindings = keybindings(cx);

    if result.is_ok() && current_keybindings != previous_keybindings {
        crate::keymap::apply(&current_keybindings, cx);
    }

    result
}

struct LoadedConfiguration {
    editor: EditorConfig,
    keybindings: Result<KeybindingOverrides, Vec<String>>,
    editor_diagnostic: Option<String>,
}

fn load(path: &Path) -> Result<LoadedConfiguration, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LoadedConfiguration {
                editor: EditorConfig::default(),
                keybindings: Ok(KeybindingOverrides::default()),
                editor_diagnostic: None,
            });
        }
        Err(error) => {
            return Err(format!("cannot read {}: {error}", path.display()));
        }
    };

    let document = text
        .parse::<DocumentMut>()
        .map_err(|error| format!("invalid configuration in {}: {error}", path.display()))?;

    let (editor, editor_diagnostic) = parse_editor(&document);

    Ok(LoadedConfiguration {
        editor,
        keybindings: parse_keybindings(&document),
        editor_diagnostic,
    })
}

fn parse_keybindings(document: &DocumentMut) -> Result<KeybindingOverrides, Vec<String>> {
    let Some(item) = document.get("keybindings") else {
        return Ok(KeybindingOverrides::default());
    };
    let Some(table) = item.as_table_like() else {
        return Err(vec!["`keybindings` must be a table".into()]);
    };

    let mut raw = HashMap::new();
    let mut errors = Vec::new();

    for (name, item) in table.iter() {
        let Some(array) = item.as_array() else {
            errors.push(format!(
                "action `{name}` must be an array of keystroke strings"
            ));
            raw.insert(name.to_owned(), Vec::new());
            continue;
        };

        let mut bindings = Vec::new();
        for (index, value) in array.iter().enumerate() {
            if let Some(binding) = value.as_str() {
                bindings.push(binding.to_owned());
            } else {
                errors.push(format!(
                    "action `{name}` binding {} must be a string",
                    index + 1
                ));
            }
        }
        raw.insert(name.to_owned(), bindings);
    }

    KeybindingOverrides::from_raw(raw, errors)
}

fn keybinding_diagnostic(errors: &[String]) -> String {
    format!(
        "invalid keybinding configuration; retained the previous valid keymap:\n- {}",
        errors.join("\n- ")
    )
}

fn combine_diagnostics(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}\n{second}")),
        (Some(diagnostic), None) | (None, Some(diagnostic)) => Some(diagnostic),
        (None, None) => None,
    }
}

fn parse_editor(document: &DocumentMut) -> (EditorConfig, Option<String>) {
    let Some(item) = document.get("editor") else {
        return (EditorConfig::default(), None);
    };
    let Some(editor) = item.as_table_like() else {
        return (
            EditorConfig::default(),
            Some("invalid configuration: editor must be a table; using defaults".into()),
        );
    };

    let mut invalid = Vec::new();
    let config = EditorConfig {
        vim_keybindings: boolean(editor, "vim_keybindings", &mut invalid),
        show_whitespace: boolean(editor, "show_whitespace", &mut invalid),
        show_change_connections: boolean(editor, "show_change_connections", &mut invalid),
        word_wrap: boolean(editor, "word_wrap", &mut invalid),
    };
    let diagnostic = (!invalid.is_empty()).then(|| {
        format!(
            "invalid editor setting{} {}; using defaults for those settings",
            if invalid.len() == 1 { "" } else { "s" },
            invalid.join(", ")
        )
    });

    (config, diagnostic)
}

fn boolean(editor: &dyn TableLike, key: &'static str, invalid: &mut Vec<&'static str>) -> bool {
    match editor.get(key) {
        None => false,
        Some(item) => item.as_bool().unwrap_or_else(|| {
            invalid.push(key);
            false
        }),
    }
}

fn update_document(path: &Path, editor: EditorConfig) -> Result<(), String> {
    let (mut document, permissions) = match fs::read_to_string(path) {
        Ok(text) => {
            let metadata = fs::metadata(path)
                .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
            if metadata.permissions().readonly() {
                return Err(format!(
                    "cannot update {}: file is read-only",
                    path.display()
                ));
            }

            let document = text.parse::<DocumentMut>().map_err(|error| {
                format!(
                    "cannot update {} while it contains invalid TOML: {error}",
                    path.display()
                )
            })?;

            (document, Some(metadata.permissions()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (DocumentMut::new(), None),
        Err(error) => {
            return Err(format!("cannot read {}: {error}", path.display()));
        }
    };

    if document
        .get("editor")
        .is_none_or(|item| item.as_table_like().is_none())
    {
        document["editor"] = Item::Table(Table::new());
    }

    let table = document["editor"]
        .as_table_like_mut()
        .expect("editor was normalized to a table");
    set_boolean(table, "vim_keybindings", editor.vim_keybindings);
    set_boolean(table, "show_whitespace", editor.show_whitespace);
    set_boolean(
        table,
        "show_change_connections",
        editor.show_change_connections,
    );
    set_boolean(table, "word_wrap", editor.word_wrap);

    let parent = path
        .parent()
        .ok_or_else(|| format!("configuration path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;

    let bytes = document.to_string();
    let atomic = AtomicFile::new(path, AllowOverwrite);
    atomic
        .write(|temporary| {
            temporary.write_all(bytes.as_bytes())?;
            if let Some(permissions) = permissions {
                temporary.set_permissions(permissions)?;
            }
            Ok::<_, std::io::Error>(())
        })
        .map_err(|error| format!("cannot update {}: {error}", path.display()))
}

fn set_boolean(table: &mut dyn TableLike, key: &str, enabled: bool) {
    let Some(existing) = table.get_mut(key) else {
        table.insert(key, value(enabled));
        return;
    };

    let decor = existing.as_value().map(Value::decor).cloned();
    let mut replacement = value(enabled);
    if let (Some(decor), Some(value)) = (decor, replacement.as_value_mut()) {
        *value.decor_mut() = decor;
    }

    *existing = replacement;
}
