//! Saving, external-change notices, and explicit reload policy for workspace tabs.

use super::decision_dialog::{Decision, DecisionDialog, DecisionShortcut};
use super::files::{Files, Role};
use super::{OpenTab, Save, Workspace};
use crate::comparison::Comparison;
use crate::editor::{AlignedEditor, DirtyChanged};
use crate::storage::{FileWatch, SaveError, Snapshot};
use gpui_kit::component::WindowExt;
use gpui_kit::{App, AppContext, Context, Task, Window};
use std::{
    collections::{HashSet, VecDeque},
    rc::Rc,
    time::Duration,
};

const WATCH_SETTLE_TIME: Duration = Duration::from_millis(150);
use yori_document::Document;

#[derive(Default)]
pub(super) struct ScanState {
    running: bool,
    requested: bool,
}

#[derive(Clone, Copy)]
pub(super) enum CloseAfter {
    Tab(usize),
    Window,
}

#[derive(Clone)]
struct SaveBatch {
    pending: VecDeque<usize>,
    close: Option<CloseAfter>,
}

impl Workspace {
    pub(super) fn start_monitor(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Option<FileWatch>, Option<Task<()>>) {
        let (watch, events) = match FileWatch::new() {
            Ok(watch) => watch,
            Err(error) => {
                eprintln!(
                    "file watching unavailable: {error}; disk is still checked on activation and save"
                );
                return (None, None);
            }
        };
        let monitor = Self::monitor_disk(events, window, cx);

        (Some(watch), Some(monitor))
    }

    pub(super) fn monitor_disk(
        events: async_channel::Receiver<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn_in(window, async move |view, cx| {
            while events.recv().await.is_ok() {
                // Save-by-rename and truncate/write produce intermediate states.
                // Wait for a quiet interval, then inspect the file, not the events.
                loop {
                    cx.background_executor().timer(WATCH_SETTLE_TIME).await;
                    match events.try_recv() {
                        Ok(()) => {}
                        Err(async_channel::TryRecvError::Empty) => break,
                        Err(async_channel::TryRecvError::Closed) => return,
                    }
                }

                if cx
                    .update(|window, cx| {
                        view.update(cx, |this, cx| {
                            Self::reload_config(window, cx);
                            this.scan_disk(cx);
                        })
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    pub(super) fn watch_paths(&mut self, cx: &App) {
        if self.monitor.is_none() {
            return;
        }

        let mut paths: HashSet<_> = self
            .tabs
            .entries
            .iter()
            .flat_map(|tab| {
                tab.content
                    .comparison()
                    .into_iter()
                    .flat_map(|comparison| comparison.files.entries.iter())
                    .map(|file| file.path.clone())
            })
            .collect();
        if let Some(path) = crate::config::path(cx) {
            paths.insert(path);
        }
        if let Some(watch) = &mut self.disk_watch {
            self.watch_error = watch.set_paths(paths).err().map(|error| {
                format!("Live file watching is unavailable: {error}. Disk is still checked on activation and save.")
            });
        }
    }

    pub(super) fn scan_disk(&mut self, cx: &mut Context<Self>) {
        if self.scan.running || self.saving {
            self.scan.requested = true;
            return;
        }

        self.scan.running = true;
        self.scan.requested = false;
        let epoch = self.disk_epoch;
        let paths: Vec<_> = self
            .tabs
            .entries
            .iter()
            .filter_map(|tab| {
                let comparison = tab.content.comparison()?;
                Some((
                    tab.id,
                    comparison
                        .files
                        .entries
                        .iter()
                        .map(|file| file.path.clone())
                        .collect::<Vec<_>>(),
                ))
            })
            .collect();
        let scan = cx.background_executor().spawn(async move {
            paths
                .into_iter()
                .map(|(id, paths)| {
                    (
                        id,
                        paths
                            .into_iter()
                            .map(|path| Snapshot::read(&path))
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |view, cx| {
            let results = scan.await;
            let _ = view.update(cx, |this, cx| {
                this.scan.running = false;
                if epoch != this.disk_epoch || this.saving {
                    this.scan_disk(cx);
                    return;
                }

                for (id, snapshots) in results {
                    if let Some(tab) = this.tabs.entries.iter_mut().find(|tab| tab.id == id)
                        && let Some(comparison) = tab.content.comparison_mut()
                    {
                        for (file, snapshot) in comparison.files.entries.iter_mut().zip(snapshots) {
                            file.current = snapshot;
                        }
                    }
                }
                if this.scan.requested {
                    this.scan_disk(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn save_active(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection != super::WorkspaceSelection::Work
            || self.saving
            || self.picking_files
            || window.has_active_dialog(cx)
        {
            return;
        }
        let Some(id) = self.tabs.active else {
            return;
        };
        if !self
            .tabs
            .get(id)
            .is_some_and(|tab| tab.content.can_save(cx))
        {
            return;
        }

        self.save_next(
            SaveBatch {
                pending: [id].into(),
                close: None,
            },
            None,
            window,
            cx,
        );
    }

    pub(super) fn save_before_close(
        &mut self,
        target: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending = self
            .tabs
            .entries
            .iter()
            .filter(|tab| {
                target.is_none_or(|id| tab.id == id)
                    && tab.content.needs_save(cx)
                    && tab.content.can_save(cx)
            })
            .map(|tab| tab.id)
            .collect();
        let close = Some(target.map_or(CloseAfter::Window, CloseAfter::Tab));

        self.save_next(SaveBatch { pending, close }, None, window, cx);
    }

    fn finish_save_batch(
        &mut self,
        close: Option<CloseAfter>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(close) = close else {
            return;
        };
        let target = match close {
            CloseAfter::Tab(id) => Some(id),
            CloseAfter::Window => None,
        };

        // Editing may continue while filesystem work is in flight. Never close
        // over edits made after the snapshot that was just written.
        if self
            .tabs
            .requires_discard_confirmation(target, |tab| tab.needs_save(cx))
        {
            self.request_close(target, window, cx);
        } else {
            self.close(target, window, cx);
        }
    }

    fn start_review_save(
        &mut self,
        id: usize,
        batch: SaveBatch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(super::OpenTab::Review { session, .. }) =
            self.tabs.get(id).map(|tab| &tab.content)
        else {
            return false;
        };

        let session = session.clone();
        let view = cx.weak_entity();
        self.saving = true;
        session.update(cx, |session, cx| {
            session.save_all(
                Rc::new(move |success, window, cx| {
                    let next = batch.clone();
                    let _ = view.update(cx, |this, cx| {
                        this.saving = false;
                        if success {
                            this.save_next(next, None, window, cx);
                        } else {
                            this.activate(id, window, cx);
                        }
                    });
                }),
                window,
                cx,
            );
        });

        true
    }

    fn save_next(
        &mut self,
        mut batch: SaveBatch,
        approved: Option<Snapshot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = batch.pending.pop_front() else {
            self.finish_save_batch(batch.close, window, cx);
            return;
        };
        if self.start_review_save(id, batch.clone(), window, cx) {
            return;
        }
        let Some(tab) = self.tabs.get(id) else {
            self.save_next(batch, None, window, cx);
            return;
        };
        let comparison = tab
            .content
            .comparison()
            .expect("non-review tabs are comparisons");
        let editor = comparison.editor.clone();
        let checkpoint = match editor.update(cx, AlignedEditor::prepare_save) {
            Ok(checkpoint) => checkpoint,
            Err(error) => {
                self.set_message(id, error, cx);
                return;
            }
        };
        let Some(target) = comparison.files.destination() else {
            self.set_message(id, "This document has no save destination.".into(), cx);
            return;
        };
        let path = target.path.clone();
        let expected = approved.unwrap_or_else(|| target.accepted.clone());
        let text = checkpoint.text.clone();
        self.saving = true;
        self.disk_epoch += 1;
        editor.update(cx, |editor, cx| editor.set_saving(true, cx));

        let save = cx
            .background_executor()
            .spawn(async move { crate::storage::save(&path, &expected, text.as_bytes()) });
        cx.spawn_in(window, async move |view, cx| {
            let result = save.await;
            let _ = view.update_in(cx, |this, window, cx| {
                this.saving = false;
                this.disk_epoch += 1;
                editor.update(cx, |editor, cx| editor.set_saving(false, cx));

                match result {
                    Ok(snapshot) => {
                        if let Some(tab) = this.tabs.entries.iter_mut().find(|tab| tab.id == id)
                            && let Some(comparison) = tab.content.comparison_mut()
                        {
                            comparison.files.saved(&snapshot);
                            comparison.message = None;
                        }
                        editor.update(cx, |editor, cx| editor.mark_saved(checkpoint, cx));
                        this.save_next(batch, None, window, cx);
                    }
                    Err(SaveError::Changed(current)) => {
                        this.set_message(
                            id,
                            "Destination changed on disk. Saving requires explicit overwrite approval.".into(),
                            cx,
                        );
                        if !window.has_active_dialog(cx) && !this.picking_files {
                            this.activate(id, window, cx);
                            batch.pending.push_front(id);
                            let detail = format!(
                                "{} changed outside this tab. Replace that version with this tab's contents?",
                                this.tabs
                                    .get(id)
                                    .and_then(|tab| tab.identity.comparison())
                                    .expect("saving comparison tab remains open")
                                    .target()
                                    .display(),
                            );

                            Self::confirm(
                                "Overwrite the changed file?",
                                detail,
                                "Overwrite",
                                window,
                                cx,
                                move |this, window, cx| {
                                    this.save_next(batch.clone(), Some(current.clone()), window, cx);
                                },
                            );
                        }
                    }
                    Err(SaveError::Failed(error)) => {
                        if batch.close.is_some() {
                            this.activate(id, window, cx);
                        }
                        this.set_message(id, format!("Save failed: {error}"), cx);
                    }
                }

                this.scan_disk(cx);
                cx.notify();
            });
        }).detach();
    }

    fn set_message(&mut self, id: usize, message: String, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.entries.iter_mut().find(|tab| tab.id == id)
            && let Some(comparison) = tab.content.comparison_mut()
        {
            comparison.message = Some(message);
        }
        cx.notify();
    }

    fn confirm(
        title: &'static str,
        detail: String,
        accept: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let action = Rc::new(action);
        let view = cx.weak_entity();
        let accept = Decision::new("ok", accept, DecisionShortcut::Enter)
            .primary()
            .on_activate(move |window, cx| {
                let action = Rc::clone(&action);
                let _ = view.update(cx, |this, cx| action(this, window, cx));
            });
        let cancel = Decision::new("cancel", "Cancel", DecisionShortcut::Escape);

        DecisionDialog::new(title, detail, cancel)
            .primary(accept)
            .open(window, cx);
    }

    pub(super) fn request_reload(
        &mut self,
        id: usize,
        role: Role,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.saving || self.picking_files || window.has_active_dialog(cx) {
            return;
        }
        let Some(tab) = self.tabs.get(id) else {
            return;
        };
        let Some(paths) = tab.identity.comparison() else {
            return;
        };
        let comparison = tab
            .content
            .comparison()
            .expect("comparison identity has comparison content");
        let merging = matches!(paths, Comparison::Merge(_));
        if merging && role == Role::Result {
            return;
        }

        let discards = (merging || role == Role::Local) && comparison.editor.read(cx).needs_save();
        if discards {
            let title = if merging {
                "Restart merge from disk?"
            } else {
                "Discard edits and reload?"
            };
            let accept = if merging { "Restart merge" } else { "Reload" };

            Self::confirm(
                title,
                "This discards the current edits and undo history. A merge restart also discards resolution decisions.".into(),
                accept,
                window,
                cx,
                move |this, window, cx| this.reload(id, role, window, cx),
            );
        } else {
            self.reload(id, role, window, cx);
        }
    }

    fn reload(&mut self, id: usize, role: Role, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(id) else {
            return;
        };
        let Some(paths) = tab.identity.comparison().cloned() else {
            return;
        };
        let comparison = tab
            .content
            .comparison()
            .expect("comparison identity has comparison content");
        let path = comparison.files.file(role).path.clone();
        let merging = matches!(paths, Comparison::Merge(_));
        let editor = comparison.editor.clone();
        editor.update(cx, AlignedEditor::deactivate);
        let checkpoint = editor.read(cx).current_checkpoint();
        self.saving = true;
        self.disk_epoch += 1;

        let read = cx.background_executor().spawn(async move {
            if merging {
                Files::load(&paths).map(Reloaded::Merge)
            } else {
                let snapshot = Snapshot::read(&path)?;
                let document = snapshot.document(&path)?;
                Ok(Reloaded::Diff(snapshot, document))
            }
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = read.await;
            let _ = view.update_in(cx, |this, window, cx| {
                this.saving = false;
                this.disk_epoch += 1;
                if (merging || role == Role::Local)
                    && editor.read(cx).current_checkpoint() != checkpoint
                {
                    this.set_message(
                        id,
                        "Edits changed while reloading; nothing was discarded. Retry when ready."
                            .into(),
                        cx,
                    );
                    this.scan_disk(cx);
                    return;
                }

                match result {
                    Err(error) => this.set_message(id, format!("Reload failed: {error}"), cx),
                    Ok(Reloaded::Diff(snapshot, document)) => {
                        editor.update(cx, |editor, cx| {
                            editor.reload_diff(role == Role::Baseline, document, window, cx);
                        });
                        if let Some(tab) = this.tabs.entries.iter_mut().find(|tab| tab.id == id)
                            && let Some(comparison) = tab.content.comparison_mut()
                        {
                            comparison.files.accept(role, snapshot);
                            comparison.message = None;
                        }
                    }
                    Ok(Reloaded::Merge(files)) => {
                        if let Err(error) = this.restart_merge(id, files, window, cx) {
                            this.set_message(id, format!("Reload failed: {error}"), cx);
                        }
                    }
                }
                this.focus_active(window, cx);
                this.scan_disk(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn restart_merge(
        &mut self,
        id: usize,
        files: Files,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(tab) = self.tabs.get(id) else {
            return Ok(());
        };
        let Some(Comparison::Merge(paths)) = tab.identity.comparison() else {
            return Ok(());
        };
        let session = yori_diff::merge::MergeSession::new(
            files.document(Role::Base).clone(),
            files.document(Role::Local).clone(),
            files.document(Role::Incoming).clone(),
        )
        .map_err(|error| error.to_string())?;
        let editor = cx.new(|cx| AlignedEditor::new_merge(paths, session, window, cx));
        let subscription = cx.subscribe(&editor, |_, _, _: &DirtyChanged, cx| cx.notify());
        if let Some(tab) = self.tabs.entries.iter_mut().find(|tab| tab.id == id) {
            tab.content = OpenTab::Comparison(super::ComparisonTab {
                editor,
                _subscription: subscription,
                files,
                message: None,
            });
        }

        Ok(())
    }
}

enum Reloaded {
    Diff(Snapshot, Document),
    Merge(Files),
}
