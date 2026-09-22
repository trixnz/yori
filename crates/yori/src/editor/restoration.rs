//! Preview and controls for whole-block or selection-scoped baseline restores.

#[cfg(test)]
mod tests;

use gpui_kit::component::{
    ActiveTheme, IconName, Sizable,
    button::{Button, ButtonVariants},
};
use gpui_kit::{
    Context, Div, InteractiveElement, MouseButton, ParentElement, StatefulInteractiveElement,
    Styled, div, px,
};
use yori::geometry::{EditorGeometry, display_units, whole_rows};
use yori_diff::SelectionRestore;
use yori_document::{Document, editing::TextSelection};

use super::{AlignedEditor, LINE_HEIGHT, RESTORE_WIDTH, Side, wrapping::WrapProjection};

impl AlignedEditor {
    pub(super) fn selection_restore(&self) -> Option<SelectionRestore> {
        if self.merge.is_some() || !self.can_edit() {
            return None;
        }

        let selection = self.selection.as_ref()?;
        self.alignment.selection_restore(
            &self.left.document,
            &self.right.document,
            TextSelection {
                anchor: selection.anchor,
                head: selection.head,
            },
            selection.side == Side::Left,
        )
    }

    pub(super) fn restore_description(&self, plan: &SelectionRestore) -> String {
        format!(
            "Baseline {} → local {}",
            line_label(&self.left.document, &plan.baseline),
            line_label(&self.right.document, &plan.local),
        )
    }

    pub(super) fn render_restore_controls(
        &self,
        geometry: EditorGeometry,
        projection: &WrapProjection,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut controls = div()
            .absolute()
            .size_full()
            .child(self.render_connections(geometry, projection, cx));
        if !self.can_edit() {
            return controls;
        }

        let first_visual_row = whole_rows(self.vertical_scroll / LINE_HEIGHT);
        let (first_row, _) = projection.visual_location(first_visual_row);
        let viewport_end = self.vertical_scroll + geometry.rows_viewport_height();
        let button_top = |rows: &std::ops::Range<usize>| {
            let visual = projection.visual_range(rows.clone());
            if display_units(visual.start) * LINE_HEIGHT >= viewport_end
                || display_units(visual.end) * LINE_HEIGHT <= self.vertical_scroll
            {
                return None;
            }

            Some(
                ((display_units(visual.start) * LINE_HEIGHT - self.vertical_scroll).max(0.0)).min(
                    display_units(visual.end) * LINE_HEIGHT - self.vertical_scroll - LINE_HEIGHT,
                ),
            )
        };

        // Never leave a full-block action under a narrower text selection.
        if self
            .selection
            .as_ref()
            .is_some_and(|s| !s.range().is_empty())
        {
            if let Some(plan) = self.selection_restore() {
                if !self.show_connections {
                    let visual = projection.visual_range(plan.rows.clone());
                    let top = display_units(visual.start) * LINE_HEIGHT - self.vertical_scroll;
                    let height = display_units(visual.len()) * LINE_HEIGHT;
                    for left in [0.0, geometry.right_pane_left()] {
                        controls = controls.child(
                            div()
                                .absolute()
                                .left(px(left + RESTORE_WIDTH))
                                .top(px(top))
                                .w(px((geometry.pane_width() - RESTORE_WIDTH).max(0.0)))
                                .h(px(height))
                                .border_1()
                                .border_color(cx.theme().muted_foreground),
                        );
                    }
                }

                if let Some(top) = button_top(&plan.rows) {
                    let description = self.restore_description(&plan);
                    controls = controls.child(
                        self.restore_button_container(top, plan.rows.clone(), geometry, cx)
                            .child(
                                Button::new("restore-selected-gutter")
                                    .icon(IconName::ArrowRight)
                                    .accessibility_label("Restore selected lines from baseline")
                                    .ghost()
                                    .compact()
                                    .with_size(px(LINE_HEIGHT))
                                    .w(px(RESTORE_WIDTH - 2.0))
                                    .tooltip(format!("{description} (Alt+Enter; undo: Ctrl+Z)"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.apply_selection_restore(&plan, window, cx);
                                    })),
                            ),
                    );
                }
            }

            return controls;
        }

        // Keep a tall block's action visible when its first row scrolls away.
        let first_block = self
            .alignment
            .blocks()
            .partition_point(|block| block.rows.end <= first_row);
        for (index, block) in self.alignment.blocks().iter().enumerate().skip(first_block) {
            let visual = projection.visual_range(block.rows.clone());
            if display_units(visual.start) * LINE_HEIGHT >= viewport_end {
                break;
            }

            if let Some(top) = button_top(&block.rows) {
                let expected = block.clone();
                controls = controls.child(
                    self.restore_button_container(top, block.rows.clone(), geometry, cx)
                        .child(
                            Button::new(("restore-block", index))
                                .icon(IconName::ArrowRight)
                                .accessibility_label("Restore block from baseline")
                                .ghost()
                                .compact()
                                .with_size(px(LINE_HEIGHT))
                                .w(px(RESTORE_WIDTH - 2.0))
                                .tooltip("Restore this block from the left (undo: Ctrl+Z)")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.restore_block(index, &expected, window, cx);
                                })),
                        ),
                );
            }
        }

        controls
    }

    fn restore_button_container(
        &self,
        top: f32,
        rows: std::ops::Range<usize>,
        geometry: EditorGeometry,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let (left, width) = if self.show_connections {
            (geometry.pane_width(), geometry.center_width())
        } else {
            (geometry.right_pane_left() + 1.0, RESTORE_WIDTH - 2.0)
        };

        div()
            .id(("restore-target", rows.start))
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .h(px(LINE_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_hover(cx.listener(|this, _: &bool, window, cx| {
                this.update_connection_hover(window, cx);
            }))
    }
}

fn line_label(document: &Document, bytes: &std::ops::Range<usize>) -> String {
    if bytes.is_empty() {
        let before = document
            .lines()
            .partition_point(|line| line.full.end <= bytes.start);
        return if before == 0 {
            "start of file (gap)".to_owned()
        } else {
            format!("gap after line {before}")
        };
    }

    let first = document.line_at_offset(bytes.start) + 1;
    let last = document.line_at_offset(bytes.end - 1) + 1;
    if first == last {
        format!("line {first}")
    } else {
        format!("lines {first}–{last}")
    }
}
