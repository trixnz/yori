//! Source selections narrow merge gutter actions without changing conflict status.

use gpui_kit::component::{
    ActiveTheme, IconName, Sizable,
    button::{Button, ButtonVariants},
};
use gpui_kit::{
    Context, Div, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use yori::geometry::{EditorGeometry, display_units};
use yori_diff::{SelectionRestore, merge::MergeInput};
use yori_document::{Document, editing::TextSelection};

use super::{AlignedEditor, LINE_HEIGHT, controls::outline_bounds};
use crate::editor::{RESTORE_WIDTH, Side, completion::Placement, wrapping::WrapProjection};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LineTake {
    pub input: MergeInput,
    pub plan: SelectionRestore,
    revision: u64,
}

impl AlignedEditor {
    pub(in crate::editor) fn merge_selection_active(&self) -> bool {
        self.merge.is_some()
            && self
                .selection
                .as_ref()
                .is_some_and(|selection| !selection.range().is_empty())
    }

    pub(super) fn merge_line_take(&self, input: MergeInput) -> Option<LineTake> {
        let merge = self.merge.as_ref()?;
        let selection = self.selection.as_ref()?;
        let source_side = input_side(input);
        if selection.side != source_side && selection.side != Side::Right {
            return None;
        }

        let alignment = match input {
            MergeInput::Local => &self.alignment,
            MergeInput::Incoming => &merge.incoming_alignment,
        };
        let plan = alignment.selection_restore(
            &self.document(source_side).document,
            &self.right.document,
            TextSelection {
                anchor: selection.anchor,
                head: selection.head,
            },
            selection.side == source_side,
        )?;

        Some(LineTake {
            input,
            plan,
            revision: merge.revision,
        })
    }

    pub(super) fn apply_merge_line_take(
        &mut self,
        expected: &LineTake,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A queued click must never apply a preview from another selection or
        // an earlier result, even if a same-length edit retained the byte ranges.
        if self.merge_line_take(expected.input).as_ref() != Some(expected) {
            return;
        }

        self.cancel_vim();
        self.finish_composition();
        let selection = self
            .right_selection()
            .unwrap_or(TextSelection::caret(expected.plan.local.start));
        let anchor = self.view_anchor(window, cx);
        let update = self.merge.as_mut().expect("merge mode").session.take_lines(
            expected.input,
            selection,
            &expected.plan,
        );

        match update {
            Ok(update) => self.complete_edit(anchor, update, Placement::Transfer, window, cx),
            Err(error) => {
                eprintln!("selected-line merge take rejected: {error}");
                window.play_system_bell();
                self.focus.focus(window, cx);
            }
        }
    }

    pub(super) fn render_merge_line_controls(
        &self,
        geometry: EditorGeometry,
        projection: &WrapProjection,
        cx: &mut Context<Self>,
    ) -> Div {
        let hovered = self.merge.as_ref().expect("merge mode").hovered_lines;
        let mut controls = div().absolute().size_full();
        for input in [MergeInput::Local, MergeInput::Incoming] {
            let Some(take) = self.merge_line_take(input) else {
                continue;
            };

            let visual = projection.visual_range(take.plan.rows.clone());
            let top = display_units(visual.start) * LINE_HEIGHT - self.vertical_scroll;
            let end = display_units(visual.end) * LINE_HEIGHT - self.vertical_scroll;
            if end <= 0.0 || top >= geometry.rows_viewport_height() {
                continue;
            }

            // A result selection can offer two different plans. Hover isolates
            // the chosen source/result pair instead of combining their extents.
            if hovered.is_none() || hovered == Some(input) {
                let color = if hovered == Some(input) {
                    cx.theme().foreground
                } else {
                    cx.theme().muted_foreground
                };
                for side in [input_side(input), Side::Right] {
                    let horizontal = outline_bounds(self.pane_left(side), geometry.pane_width());
                    controls = controls.child(
                        div()
                            .absolute()
                            .left(px(horizontal.start))
                            .top(px(top))
                            .w(px(horizontal.end - horizontal.start))
                            .h(px(end - top))
                            .border_1()
                            .border_color(color),
                    );
                }
            }

            let button_top = top.max(0.0).min(end - LINE_HEIGHT);
            controls = controls.child(self.render_line_take_arrow(take, button_top, geometry, cx));
        }

        controls
    }

    fn render_line_take_arrow(
        &self,
        take: LineTake,
        top: f32,
        geometry: EditorGeometry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let input = take.input;
        let (id, icon, left, source) = match input {
            MergeInput::Local => (
                "merge-selected-local",
                IconName::ArrowRight,
                geometry.right_pane_left(),
                "local",
            ),
            MergeInput::Incoming => (
                "merge-selected-incoming",
                IconName::ArrowLeft,
                geometry.incoming_pane_left(),
                "incoming",
            ),
        };
        let source_document = &self.document(input_side(input)).document;
        let description = format!(
            "Take selected {source} {} → result {}. Resolution status unchanged; Ctrl+Z to undo.",
            range_label(source_document, &take.plan.baseline),
            range_label(&self.right.document, &take.plan.local)
        );

        div()
            .id((
                "selected-merge-target",
                usize::from(input == MergeInput::Incoming),
            ))
            .absolute()
            .left(px(left + 1.0))
            .top(px(top))
            .w(px(RESTORE_WIDTH - 2.0))
            .h(px(LINE_HEIGHT))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                let merge = this.merge.as_mut().expect("merge mode");
                if *hovering {
                    merge.hovered_lines = Some(input);
                } else if merge.hovered_lines == Some(input) {
                    merge.hovered_lines = None;
                }

                cx.notify();
            }))
            .child(
                Button::new(id)
                    .icon(icon)
                    .ghost()
                    .compact()
                    .with_size(px(LINE_HEIGHT))
                    .w(px(RESTORE_WIDTH - 2.0))
                    .accessibility_label(description.clone())
                    .tooltip(description)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.apply_merge_line_take(&take, window, cx);
                    })),
            )
    }
}

fn input_side(input: MergeInput) -> Side {
    match input {
        MergeInput::Local => Side::Left,
        MergeInput::Incoming => Side::Incoming,
    }
}

fn range_label(document: &Document, range: &std::ops::Range<usize>) -> String {
    let first = document.line_at_offset(range.start) + 1;
    if range.is_empty() {
        format!("insertion before line {first}")
    } else {
        format!(
            "lines {first}–{}",
            document.line_at_offset(range.end - 1) + 1
        )
    }
}
