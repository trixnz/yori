//! Whole-conflict actions stay next to the code, inside reserved gutter space.

use gpui_kit::component::{
    ActiveTheme, IconName, Sizable,
    button::{Button, ButtonVariants},
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    Context, Div, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use yori::geometry::{EditorGeometry, display_units};
use yori_diff::merge::{ConflictId, Take};

use super::{AlignedEditor, LINE_HEIGHT};
use crate::editor::wrapping::WrapProjection;
use crate::editor::{RESTORE_WIDTH, Side};

pub(super) fn outline_bounds(pane_left: f32, pane_width: f32) -> std::ops::Range<f32> {
    let left = pane_left + RESTORE_WIDTH;
    left..(pane_left + pane_width - 2.0).max(left)
}

#[cfg(test)]
mod tests;

impl AlignedEditor {
    pub(in crate::editor) fn render_merge_header(
        &self,
        id: ConflictId,
        top: f32,
        geometry: EditorGeometry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .absolute()
            .top(px(top))
            .left_0()
            .w(px(geometry.content_width()))
            .h(px(LINE_HEIGHT))
            .bg(cx.theme().secondary)
            .child(
                div()
                    .absolute()
                    .left(px(geometry.right_pane_left() + RESTORE_WIDTH))
                    .w(px((geometry.pane_width() - RESTORE_WIDTH).max(0.0)))
                    .h(px(LINE_HEIGHT))
                    .overflow_hidden()
                    .child(self.render_conflict_menu(id, cx)),
            )
    }

    pub(in crate::editor) fn render_merge_conflict_controls(
        &self,
        geometry: EditorGeometry,
        projection: &WrapProjection,
        cx: &mut Context<Self>,
    ) -> Div {
        if self.merge_selection_active() {
            return self.render_merge_line_controls(geometry, projection, cx);
        }

        let merge = self.merge.as_ref().expect("merge mode");
        let mut controls = div().absolute().size_full();
        for conflict in merge.display.conflicts() {
            let id = conflict.id;
            let visual = projection.visual_range(conflict.control_span.clone());
            let top = display_units(visual.start) * LINE_HEIGHT - self.vertical_scroll;
            let end = display_units(visual.end) * LINE_HEIGHT - self.vertical_scroll;
            if end <= 0.0 || top >= geometry.rows_viewport_height() {
                continue;
            }

            let resolved = merge.session.state(id).expect("known conflict").resolved;
            let color = if resolved {
                cx.theme().muted_foreground
            } else {
                cx.theme().warning
            };
            let hovered = merge.hovered.filter(|(target, _)| *target == id);
            let active = merge.current == Some(id) || hovered.is_some();
            for side in [Side::Left, Side::Right, Side::Incoming] {
                let preview = hovered.is_some_and(|(_, take)| {
                    side == Side::Right
                        || matches!(
                            (side, take),
                            (Side::Left, Take::Local) | (Side::Incoming, Take::Incoming)
                        )
                });
                let strength = if preview {
                    0.9
                } else if active {
                    0.45
                } else {
                    0.0
                };
                let horizontal = outline_bounds(self.pane_left(side), geometry.pane_width());
                controls = controls.child(
                    div()
                        .absolute()
                        .left(px(horizontal.start))
                        .top(px(top))
                        .w(px(horizontal.end - horizontal.start))
                        .h(px((end - top).max(LINE_HEIGHT)))
                        .border_1()
                        .border_color(color.opacity(strength)),
                );
            }

            // Like two-way restoration, arrows stay reachable inside a tall
            // conflict after its first row has scrolled above the viewport.
            let button_top = top.max(0.0).min(end - LINE_HEIGHT);
            controls = controls
                .child(self.render_conflict_arrow(id, Take::Local, button_top, geometry, cx))
                .child(self.render_conflict_arrow(id, Take::Incoming, button_top, geometry, cx));
        }

        controls
    }

    fn render_conflict_arrow(
        &self,
        id: ConflictId,
        take: Take,
        top: f32,
        geometry: EditorGeometry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let local = take == Take::Local;
        let (name, icon, left, source) = if local {
            (
                "merge-take-local",
                IconName::ArrowRight,
                geometry.right_pane_left(),
                "local",
            )
        } else {
            (
                "merge-take-incoming",
                IconName::ArrowLeft,
                geometry.incoming_pane_left(),
                "incoming",
            )
        };
        let merge = self.merge.as_ref().expect("merge mode");
        let range = &merge.session.state(id).expect("known conflict").result;
        let result = merge.session.result();
        let first = result.line_at_offset(range.start) + 1;
        let affected = if range.is_empty() {
            format!("insert at result line {first}")
        } else {
            format!(
                "replace result lines {first}–{}",
                result.line_at_offset(range.end - 1) + 1
            )
        };
        let description = format!(
            "Take {source} for whole conflict {}: {affected}. Marks resolved; Ctrl+Z to undo.",
            id.0 + 1
        );

        div()
            .id((name, id.0))
            .absolute()
            .left(px(left + 1.0))
            .top(px(top))
            .w(px(RESTORE_WIDTH - 2.0))
            .h(px(LINE_HEIGHT))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let merge = this.merge.as_mut().expect("merge mode");
                if *hovered {
                    merge.hovered = Some((id, take));
                } else if merge.hovered == Some((id, take)) {
                    merge.hovered = None;
                }

                cx.notify();
            }))
            .child(
                Button::new((
                    if local {
                        "merge-local-button"
                    } else {
                        "merge-incoming-button"
                    },
                    id.0,
                ))
                .icon(icon)
                .ghost()
                .compact()
                .with_size(px(LINE_HEIGHT))
                .w(px(RESTORE_WIDTH - 2.0))
                .accessibility_label(description.clone())
                .tooltip(description)
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.merge_take(id, take, window, cx);
                })),
            )
    }

    fn render_conflict_menu(&self, id: ConflictId, cx: &mut Context<Self>) -> impl IntoElement {
        let merge = self.merge.as_ref().expect("merge mode");
        let resolved = merge.session.state(id).expect("known conflict").resolved;
        let showing_base = merge.show_base && merge.current == Some(id);
        let selection_active = self.merge_selection_active();
        let status = if resolved { "Resolved" } else { "Unresolved" };
        let color = if resolved {
            cx.theme().muted_foreground
        } else {
            cx.theme().warning
        };
        let editor = cx.weak_entity();

        div()
            .id(("merge-conflict-menu", id.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .font_family(cx.theme().font_family.clone())
            .child(
                Button::new(("merge-conflict-options", id.0))
                    .label(format!("Conflict {} · {status}", id.0 + 1))
                    .text_color(color)
                    .dropdown_caret(true)
                    .ghost()
                    .compact()
                    .with_size(px(LINE_HEIGHT))
                    .tooltip("Actions for this whole conflict")
                    .dropdown_menu(move |mut menu, _, _| {
                        for (label, take) in [
                            ("Take local then incoming", Take::LocalThenIncoming),
                            ("Take incoming then local", Take::IncomingThenLocal),
                        ] {
                            let editor = editor.clone();
                            menu = menu.item(
                                PopupMenuItem::new(label)
                                    .disabled(selection_active)
                                    .on_click(move |_, window, cx| {
                                        let _ = editor.update(cx, |this, cx| {
                                            this.merge_take(id, take, window, cx);
                                        });
                                    }),
                            );
                        }

                        let marking = editor.clone();
                        let resetting = editor.clone();
                        let base = editor.clone();
                        let copying = editor.clone();
                        menu.separator()
                            .item(
                                PopupMenuItem::new(if resolved {
                                    "Mark unresolved"
                                } else {
                                    "Mark resolved"
                                })
                                .on_click(move |_, window, cx| {
                                    let _ = marking.update(cx, |this, cx| {
                                        this.merge_mark(id, !resolved, window, cx);
                                    });
                                }),
                            )
                            .item(
                                PopupMenuItem::new("Reset conflict to local (unresolved)")
                                    .on_click(move |_, window, cx| {
                                        let _ = resetting.update(cx, |this, cx| {
                                            this.merge_reset(id, window, cx);
                                        });
                                    }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new(if showing_base {
                                    "Hide base"
                                } else {
                                    "Show base"
                                })
                                .on_click(move |_, window, cx| {
                                    let _ = base.update(cx, |this, cx| {
                                        this.toggle_conflict_base(id, window, cx);
                                    });
                                }),
                            )
                            .item(PopupMenuItem::new("Copy base").on_click(move |_, _, cx| {
                                let _ = copying.update(cx, |this, cx| {
                                    this.copy_merge_base(id, cx);
                                });
                            }))
                    }),
            )
    }

    fn toggle_conflict_base(
        &mut self,
        id: ConflictId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let merge = self.merge.as_mut().expect("merge mode");
        if merge.current != Some(id) {
            merge.current = Some(id);
            merge.show_base = false;
        }

        self.toggle_merge_base(window, cx);
    }
}
