//! A single window of independent comparison editors, using the existing component kit.

mod decision_dialog;
mod disk_dialog;
mod files;
mod persistence;
mod tabs;
#[cfg(test)]
mod tests;

use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Root, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    notification::Notification,
    tab::{Tab, TabBar, TabVariant},
    tooltip::Tooltip,
};
use gpui_kit::{
    App, AppContext, AsyncWindowContext, Context, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyBinding, MouseButton, ParentElement, PathPromptOptions,
    Render, ScrollHandle, StatefulInteractiveElement, Styled, Subscription, TestSupportExt, Window,
    div, px,
};
#[cfg(test)]
use yori_document::Document;

use crate::comparison::{ComparisonPaths, MergePaths};
use crate::editor::{AlignedEditor, DirtyChanged, PaneDocument};
use decision_dialog::{Decision, DecisionDialog, DecisionShortcut};
use tabs::Tabs;

const KEY_CONTEXT: &str = "ComparisonWorkspace";

gpui_kit::actions!(
    workspace,
    [
        OpenComparison,
        OpenMerge,
        Save,
        CloseComparison,
        Quit,
        NextTab,
        PreviousTab
    ]
);

struct OpenTab {
    editor: Entity<AlignedEditor>,
    _subscription: Subscription,
    files: files::Files,
    message: Option<String>,
}

pub(super) struct Workspace {
    tabs: Tabs<OpenTab>,
    focus: FocusHandle,
    tab_scroll: ScrollHandle,
    picking_files: bool,
    saving: bool,
    notice_scheduled: bool,
    disk_notice: std::rc::Weak<std::cell::RefCell<disk_dialog::DiskNotice>>,
    scan: persistence::ScanState,
    disk_epoch: u64,
    disk_watch: Option<crate::storage::FileWatch>,
    watch_error: Option<String>,
    monitor: Option<gpui_kit::Task<()>>,
}

impl Workspace {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            view.update(cx, |this, cx| {
                if this.saving || window.has_active_dialog(cx) {
                    return false;
                }
                if !this.has_modified_tabs(cx) {
                    return true;
                }

                this.request_close(None, window, cx);
                false
            })
            .unwrap_or(true)
        });

        let focus = cx.focus_handle();
        focus.focus(window, cx);

        let (disk_watch, monitor) = Self::start_monitor(window, cx);
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.watch_paths();
                this.scan_disk(cx);
            }
        })
        .detach();

        Self {
            tabs: Tabs::default(),
            focus,
            tab_scroll: ScrollHandle::new(),
            picking_files: false,
            saving: false,
            notice_scheduled: false,
            disk_notice: std::rc::Weak::new(),
            scan: persistence::ScanState::default(),
            disk_epoch: 0,
            watch_error: disk_watch.is_none().then(|| "Live file watching is unavailable. Disk is still checked on activation and before saving.".into()),
            disk_watch,
            monitor,
        }
    }

    #[cfg(test)]
    pub(super) fn open_paths(
        &mut self,
        left: &std::path::Path,
        right: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self.open_comparison(
            &ComparisonPaths::diff(left.into(), right.into()),
            window,
            cx,
        );
        if let Err(error) = result {
            window.push_notification(Notification::error(error), cx);
        }
    }

    /// Process one CLI handoff on the UI thread. Completion means every comparison was
    /// loaded or rejected, not just queued; temporary files can then be released.
    pub(super) fn open_comparisons(
        &mut self,
        comparisons: &[ComparisonPaths],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        window.activate_window();
        if comparisons.is_empty() {
            if !self.picking_files && !window.has_active_dialog(cx) {
                self.focus_active(window, cx);
            }
            return Ok(());
        }
        if self.picking_files || window.has_active_dialog(cx) {
            return Err("yori has a dialog open; finish or cancel it, then retry".into());
        }

        let mut errors = Vec::new();
        for paths in comparisons {
            let result = self.open_comparison(paths, window, cx);
            if let Err(error) = result {
                window.push_notification(Notification::error(error.clone()), cx);
                errors.push(error);
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }

    fn open_comparison(
        &mut self,
        paths: &ComparisonPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let paths = paths.resolve()?;
        if let Some(id) = self.tabs.find(&paths) {
            self.activate(id, window, cx);
            return Ok(());
        }

        // Source contents and overwrite protection come from the same read.
        let files = files::Files::load(&paths)?;
        let editor = match &paths {
            ComparisonPaths::Diff { baseline, local } => {
                let left = PaneDocument::new(
                    baseline.clone(),
                    files
                        .file(files::Role::Baseline)
                        .accepted
                        .document(baseline)?,
                );
                let right = PaneDocument::new(
                    local.clone(),
                    files.file(files::Role::Local).accepted.document(local)?,
                );
                self.deactivate(cx);

                cx.new(|cx| AlignedEditor::new(left, right, window, cx))
            }
            ComparisonPaths::Merge(paths) => {
                let session = yori_diff::merge::MergeSession::new(
                    files
                        .file(files::Role::Base)
                        .accepted
                        .document(&paths.base)?,
                    files
                        .file(files::Role::Local)
                        .accepted
                        .document(&paths.local)?,
                    files
                        .file(files::Role::Incoming)
                        .accepted
                        .document(&paths.incoming)?,
                )
                .map_err(|error| error.to_string())?;
                self.deactivate(cx);

                cx.new(|cx| AlignedEditor::new_merge(paths, session, window, cx))
            }
        };
        let subscription = cx.subscribe(&editor, |_, _, _: &DirtyChanged, cx| cx.notify());
        self.tabs.insert(
            paths,
            OpenTab {
                editor,
                _subscription: subscription,
                files,
                message: None,
            },
        );

        self.disk_epoch += 1;
        self.watch_paths();
        self.scan_disk(cx);
        cx.notify();
        Ok(())
    }

    fn choose_merge(&mut self, _: &OpenMerge, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_files(true, window, cx);
    }

    fn deactivate(&self, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.active.and_then(|id| self.tabs.get(id)) {
            tab.content.editor.update(cx, AlignedEditor::deactivate);
        }
    }

    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.active.and_then(|id| self.tabs.get(id)) {
            tab.content.editor.focus_handle(cx).focus(window, cx);
        } else {
            self.focus.focus(window, cx);
        }
    }

    fn activate(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking_files || window.has_active_dialog(cx) {
            return;
        }

        self.deactivate(cx);
        self.tabs.activate(id);

        self.focus_active(window, cx);
        cx.notify();
    }

    fn cycle(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self
            .tabs
            .entries
            .iter()
            .position(|tab| Some(tab.id) == self.tabs.active)
        else {
            return;
        };

        let count = self.tabs.entries.len();
        let next = if backwards {
            (index + count - 1) % count
        } else {
            (index + 1) % count
        };
        self.activate(self.tabs.entries[next].id, window, cx);
    }

    fn has_modified_tabs(&self, cx: &App) -> bool {
        self.tabs
            .requires_discard_confirmation(None, |tab| tab.editor.read(cx).needs_save())
    }

    fn close(&mut self, target: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = target else {
            window.defer(cx, |window, _| window.remove_window());
            return;
        };

        if self.tabs.active == Some(id) {
            self.deactivate(cx);
        }
        self.tabs.remove(id);
        self.disk_epoch += 1;
        self.watch_paths();

        self.focus_active(window, cx);
        cx.notify();
    }

    fn request_close(
        &mut self,
        target: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.saving || window.has_active_dialog(cx) {
            return;
        }

        let modified = self
            .tabs
            .requires_discard_confirmation(target, |tab| tab.editor.read(cx).needs_save());
        if !modified {
            self.close(target, window, cx);
            return;
        }

        self.deactivate(cx);
        self.focus_active(window, cx);

        let title = target.map_or_else(
            || "Save changes before closing yori?".to_owned(),
            |id| format!("Save changes to {}?", self.tabs.label(id)),
        );
        let unresolved = self.tabs.entries.iter().any(|tab| {
            target.is_none_or(|id| tab.id == id)
                && tab.content.editor.read(cx).unresolved_count() != 0
        });
        let detail = if unresolved {
            "There are unresolved conflicts. Resolve them before saving, or discard this session."
        } else {
            "Your changes have not been saved. Save them, discard them, or keep the workspace open."
        };
        let view = cx.weak_entity();
        let discard_view = view.clone();
        let save_view = view.clone();
        let cancel = Decision::new("cancel", "Cancel", DecisionShortcut::Escape);
        let discard = Decision::new("ok", "Discard", DecisionShortcut::Mnemonic('d')).on_activate(
            move |window, cx| {
                // Restore modal focus before disposing the editor it belonged to.
                let _ = discard_view.update(cx, |this, cx| this.close(target, window, cx));
            },
        );
        let save = Decision::new(
            "save-and-close",
            if target.is_some() { "Save" } else { "Save all" },
            DecisionShortcut::Enter,
        )
        .primary()
        .disabled(unresolved)
        .on_activate(move |window, cx| {
            let _ = save_view.update(cx, |this, cx| {
                this.save_before_close(target, window, cx);
            });
        });

        DecisionDialog::new(title, detail, cancel)
            .alternate(discard)
            .primary(save)
            .open(window, cx);
    }

    fn choose_pair(&mut self, _: &OpenComparison, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_files(false, window, cx);
    }

    fn choose_files(&mut self, merging: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking_files || window.has_active_dialog(cx) {
            return;
        }

        self.deactivate(cx);
        self.picking_files = true;
        cx.notify();

        cx.spawn_in(window, async move |view, cx| {
            let result = choose_paths(cx, merging).await;
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.picking_files = false;
                    match result {
                        Ok(Some(paths)) => {
                            let _ = this.open_comparisons(&[paths], window, cx);
                        }
                        Ok(None) => this.focus_active(window, cx),
                        Err(error) => window.push_notification(Notification::error(error), cx),
                    }

                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.0))
            .child("Compare or merge files")
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child("Open a two-way diff or a three-way merge."),
            )
            .child(
                Button::new("open-first-comparison")
                    .label("Open comparison")
                    .icon(IconName::Plus)
                    .ghost()
                    .disabled(self.picking_files)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.choose_pair(&OpenComparison, window, cx);
                    })),
            )
    }

    fn render_open_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(5.0))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(2.0))
            .child(
                Button::new("open-merge")
                    .icon(gpui_kit::assets::IconName::GitMerge)
                    .ghost()
                    .with_size(px(20.0))
                    .size(px(28.0))
                    .accessibility_label("Open merge")
                    .tooltip("Open three-way merge (Ctrl+Shift+M)")
                    .disabled(self.picking_files)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.choose_merge(&OpenMerge, window, cx);
                    })),
            )
            .child(
                div()
                    .mx(px(1.0))
                    .h(px(16.0))
                    .w(px(1.0))
                    .bg(cx.theme().border),
            )
            .child(
                Button::new("open-comparison")
                    .icon(IconName::Plus)
                    .ghost()
                    .with_size(px(28.0))
                    .accessibility_label("Open comparison")
                    .tooltip("Open comparison (Ctrl+O)")
                    .disabled(self.picking_files)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.choose_pair(&OpenComparison, window, cx);
                    })),
            )
    }

    fn render_tab(&self, tab: &tabs::Tab<OpenTab>, cx: &mut Context<Self>) -> Tab {
        let id = tab.id;
        let label = self.tabs.label(id);
        let description = tab.paths.description();
        let modified = tab.content.editor.read(cx).needs_save();
        let accessible = format!(
            "{label}{}; {description}",
            if modified { "; modified" } else { "" }
        );

        Tab::new()
            .label(label)
            .aria_label(accessible)
            .prefix(
                div()
                    .id(("comparison-paths", id))
                    .pl(px(10.0))
                    .tooltip(move |window, cx| Tooltip::new(description.clone()).build(window, cx))
                    .child(Icon::new(IconName::FileText).with_size(px(14.0))),
            )
            .suffix(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().size(px(6.0)).rounded_full().bg(if modified {
                        cx.theme().foreground
                    } else {
                        cx.theme().transparent
                    }))
                    .child(
                        div()
                            .id(("tab-close-target", id))
                            .test_support()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                Button::new(("close-comparison", id))
                                    .icon(IconName::Close)
                                    .ghost()
                                    .with_size(px(22.0))
                                    .accessibility_label("Close comparison")
                                    .tooltip("Close comparison (Ctrl+W)")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.request_close(Some(id), window, cx);
                                    })),
                            ),
                    ),
            )
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.schedule_disk_notices(window, cx);

        let selected = self
            .tabs
            .entries
            .iter()
            .position(|tab| Some(tab.id) == self.tabs.active);
        let tabs = self
            .tabs
            .entries
            .iter()
            .map(|tab| self.render_tab(tab, cx))
            .collect::<Vec<_>>();
        let ids = self
            .tabs
            .entries
            .iter()
            .map(|tab| tab.id)
            .collect::<Vec<_>>();
        let mut bar = TabBar::new("comparisons")
            .with_variant(TabVariant::Tab)
            .with_size(px(38.0))
            .track_scroll(&self.tab_scroll)
            .max_width(px(260.0))
            .suffix(self.render_open_controls(cx))
            .children(tabs)
            .on_click(cx.listener(move |this, index: &usize, window, cx| {
                if let Some(id) = ids.get(*index) {
                    this.activate(*id, window, cx);
                }
            }));
        if let Some(index) = selected {
            bar = bar.selected_index(index);
        }

        let body = if let Some(tab) = self.tabs.active.and_then(|id| self.tabs.get(id)) {
            div().size_full().child(tab.content.editor.clone())
        } else {
            div().size_full().child(self.render_empty(cx))
        };

        let dialogs = Root::render_dialog_layer(window, cx);
        let notifications = Root::render_notification_layer(window, cx);

        div()
            .id("workspace")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .font_family(cx.theme().font_family.clone())
            .text_size(px(13.0))
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::choose_pair))
            .on_action(cx.listener(Self::choose_merge))
            .on_action(cx.listener(Self::save_active))
            .on_action(cx.listener(|this, _: &CloseComparison, window, cx| {
                if let Some(id) = this.tabs.active {
                    this.request_close(Some(id), window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &Quit, window, cx| this.request_close(None, window, cx)),
            )
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle(false, window, cx)))
            .on_action(
                cx.listener(|this, _: &PreviousTab, window, cx| this.cycle(true, window, cx)),
            )
            .child(
                div()
                    .h(px(38.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .bg(cx.theme().tab_bar)
                    .child(div().flex_1().min_w_0().overflow_hidden().child(bar)),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(body),
            )
            .children(dialogs)
            .children(notifications)
    }
}

async fn choose_file(
    cx: &mut AsyncWindowContext,
    prompt: &'static str,
) -> Result<Option<std::path::PathBuf>, String> {
    let request = cx
        .update(|_, cx| {
            cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(prompt.into()),
            })
        })
        .map_err(|error| error.to_string())?;
    let result = request
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;

    Ok(result.and_then(|paths| paths.into_iter().next()))
}

async fn choose_paths(
    cx: &mut AsyncWindowContext,
    merging: bool,
) -> Result<Option<ComparisonPaths>, String> {
    let Some(base) = choose_file(
        cx,
        if merging {
            "Select common ancestor (BASE)"
        } else {
            "Select baseline file"
        },
    )
    .await?
    else {
        return Ok(None);
    };
    let Some(local) = choose_file(cx, "Select local file").await? else {
        return Ok(None);
    };
    if !merging {
        return Ok(Some(ComparisonPaths::diff(base, local)));
    }

    let Some(incoming) = choose_file(cx, "Select incoming file").await? else {
        return Ok(None);
    };
    let directory = local.parent().unwrap_or_else(|| std::path::Path::new("."));
    let name = local
        .file_name()
        .unwrap_or(local.as_os_str())
        .to_string_lossy();
    let request = cx
        .update(|_, cx| cx.prompt_for_new_path(directory, Some(&name)))
        .map_err(|error| error.to_string())?;
    let result = request
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;

    Ok(result.map(|result| {
        ComparisonPaths::Merge(MergePaths {
            base,
            local,
            incoming,
            result,
        })
    }))
}

pub(super) fn init(cx: &mut App) {
    let command = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };

    cx.bind_keys([
        KeyBinding::new(&format!("{command}-o"), OpenComparison, Some(KEY_CONTEXT)),
        KeyBinding::new(&format!("{command}-s"), Save, Some(KEY_CONTEXT)),
        KeyBinding::new(&format!("{command}-shift-m"), OpenMerge, Some(KEY_CONTEXT)),
        KeyBinding::new(&format!("{command}-w"), CloseComparison, Some(KEY_CONTEXT)),
        KeyBinding::new(&format!("{command}-q"), Quit, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-tab", NextTab, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some(KEY_CONTEXT)),
    ]);
}
