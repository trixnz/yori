//! External changes are acknowledgement fences, never rows in the editor layout.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use gpui_kit::component::{
    WindowExt,
    button::{Button, ButtonVariants},
    dialog::DialogFooter,
    notification::Notification,
};
use gpui_kit::{Context, ParentElement, Window};

use super::{Workspace, files::Role};
use crate::{comparison::Comparison, storage::Snapshot};

#[derive(Clone)]
pub(super) struct DiskNotice {
    tab: usize,
    role: Role,
    path: PathBuf,
    observed: Result<Snapshot, String>,
    reloadable: bool,
    merging: bool,
}

impl Workspace {
    pub(super) fn schedule_disk_notices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(notice) = self.disk_notice.upgrade() {
            let mut notice = notice.borrow_mut();
            if let Some(comparison) = self
                .tabs
                .get(notice.tab)
                .and_then(|tab| tab.content.comparison())
            {
                let current = &comparison.files.tracked(notice.role, &notice.path).current;
                notice.observed.clone_from(current);
            }
        }

        if self.notice_scheduled || self.saving || self.picking_files {
            return;
        }

        let pending = self.watch_error.is_some()
            || self.tabs.entries.iter().any(|tab| {
                tab.content.comparison().is_some_and(|comparison| {
                    comparison.message.is_some() || comparison.files.notice().is_some()
                })
            });
        if !pending {
            return;
        }

        // Rendering can run inside Root's update. Open dialogs only after that
        // borrow has ended; never replace another modal or a native file picker.
        self.notice_scheduled = true;
        let view = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            let _ = view.update(cx, |this, cx| {
                this.notice_scheduled = false;
                this.present_disk_notice(window, cx);
            });
        });
    }

    fn next_disk_notice(&self) -> Option<DiskNotice> {
        let active = self.tabs.active.and_then(|id| self.tabs.get(id));
        active
            .into_iter()
            .chain(
                self.tabs
                    .entries
                    .iter()
                    .filter(|tab| Some(tab.id) != self.tabs.active),
            )
            .find_map(|tab| {
                let comparison = tab.content.comparison()?;
                let file = comparison.files.notice()?;
                Some(DiskNotice {
                    tab: tab.id,
                    role: file.role,
                    path: file.path.clone(),
                    observed: file.current.clone(),
                    reloadable: file.reloadable(),
                    merging: matches!(tab.identity.comparison(), Some(Comparison::Merge(_))),
                })
            })
    }

    fn present_nonblocking_disk_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Failures are overlay notifications: no status row may move source text.
        for tab in &mut self.tabs.entries {
            if let Some(message) = tab
                .content
                .comparison_mut()
                .and_then(|comparison| comparison.message.take())
            {
                window.push_notification(Notification::error(message), cx);
            }
        }

        if let Some(message) = self.watch_error.take() {
            window.push_notification(Notification::error(message), cx);
        }
    }

    fn present_disk_notice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.picking_files || window.has_active_dialog(cx) {
            return;
        }

        self.present_nonblocking_disk_messages(window, cx);

        let Some(notice) = self.next_disk_notice() else {
            return;
        };

        self.activate(notice.tab, window, cx);
        let view = cx.weak_entity();
        let notice = Rc::new(RefCell::new(notice));
        self.disk_notice = Rc::downgrade(&notice);

        window.open_dialog(cx, move |dialog, _, _| {
            // The workspace refreshes this snapshot before rendering the overlay.
            // Reading the workspace entity here would reenter its own update.
            let notice = notice.borrow().clone();

            let title = format!("{} changed on disk", notice.role.label());
            let change = match &notice.observed {
                Ok(Snapshot::Missing) => "The file is currently missing from disk. \
                    Its editor contents are still available."
                    .to_owned(),
                Ok(_) => "The file has changed outside this comparison.".to_owned(),
                Err(error) => format!("The disk version cannot currently be read: {error}"),
            };
            let reloadable = notice.reloadable && !matches!(notice.observed, Ok(Snapshot::Missing));
            let policy = if !notice.reloadable {
                "The save destination will not be reloaded into this document. \
                 Keeping it does not authorize overwriting the disk version."
            } else if !reloadable {
                "Keep the current document to continue. If the file returns, \
                 this dialog will offer reloading it."
            } else if notice.merging {
                "Restart the merge from disk or keep the current session. Restarting discards \
                 its edits and resolution decisions after confirmation."
            } else {
                "Reload from disk or keep the current document."
            };
            let detail = format!("{}\n\n{change}\n\n{policy}", notice.path.display());

            let keep_view = view.clone();
            let kept = notice.clone();
            let footer = DialogFooter::new()
                .child(Button::new("keep-current").label("Keep current").on_click(
                    move |_, window, cx| {
                        let view = keep_view.clone();
                        let notice = kept.clone();
                        window.defer(cx, move |window, cx| {
                            window.close_dialog(cx);
                            let _ = view.update(cx, |this, cx| {
                                if let Some(comparison) = this
                                    .tabs
                                    .entries
                                    .iter_mut()
                                    .find(|tab| tab.id == notice.tab)
                                    .and_then(|tab| tab.content.comparison_mut())
                                {
                                    comparison.files.dismiss(
                                        notice.role,
                                        &notice.path,
                                        notice.observed,
                                    );
                                }

                                cx.notify();
                            });
                        });
                    },
                ))
                .children(reloadable.then(|| {
                    let view = view.clone();
                    let notice = notice.clone();
                    Button::new("reload-disk")
                        .label(if notice.merging {
                            "Restart merge"
                        } else {
                            "Reload"
                        })
                        .primary()
                        .on_click(move |_, window, cx| {
                            let view = view.clone();
                            let notice = notice.clone();
                            window.defer(cx, move |window, cx| {
                                window.close_dialog(cx);
                                let _ = view.update(cx, |this, cx| {
                                    // Don't acknowledge before reload succeeds. Cancelling
                                    // discard confirmation returns to the original fence.
                                    this.request_reload(notice.tab, notice.role, window, cx);
                                });
                            });
                        })
                }));

            dialog
                .title(title.clone())
                .child(detail.clone())
                .footer(footer)
                .overlay_closable(false)
                .close_button(false)
                .keyboard(false)
                .on_cancel(|_, _, _| false)
                .on_ok(|_, _, _| false)
        });
    }
}
