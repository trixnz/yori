//! Read-only ancestor context inside the aligned merge surface.

use gpui_kit::component::ActiveTheme;
use gpui_kit::{Context, IntoElement, ParentElement, Styled, div, px};
use yori::{display::DisplayLine, geometry::EditorGeometry};

use super::{AlignedEditor, BaseRow, LINE_HEIGHT};
use crate::editor::{GUTTER_WIDTH, TAB_WIDTH, wrapping::ProjectedRow};

impl AlignedEditor {
    pub(in crate::editor) fn render_base_preview_row(
        &self,
        row: &BaseRow,
        projected: &ProjectedRow,
        top: f32,
        geometry: EditorGeometry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let merge = self.merge.as_ref().expect("merge mode");
        let source_line = match row {
            BaseRow::SourceLine(line) => Some(*line),
            BaseRow::Caption | BaseRow::EmptyAncestor => None,
        };
        let text = match row {
            BaseRow::Caption => "BASE · Common ancestor · Read-only".to_owned(),
            BaseRow::EmptyAncestor => "(Empty ancestor)".to_owned(),
            BaseRow::SourceLine(line) => {
                DisplayLine::from_source(merge.session.base().content(*line), 0, TAB_WIDTH).text
            }
        };
        let number = source_line.map_or_else(String::new, |line| (line + 1).to_string());
        let mut text_area = div()
            .absolute()
            .left(px(GUTTER_WIDTH))
            .w(px(geometry.text_viewport_width()))
            .h(px(
                yori::geometry::display_units(projected.height) * LINE_HEIGHT
            ))
            .overflow_hidden();
        let segments = if source_line.is_some() {
            projected.base_segments().to_vec()
        } else {
            std::iter::once(0..text.len()).collect()
        };
        for (continuation, segment) in segments.iter().enumerate() {
            text_area = text_area.child(
                div()
                    .absolute()
                    .top(px(yori::geometry::display_units(continuation) * LINE_HEIGHT))
                    .left(px(if source_line.is_some() {
                        -self.horizontal_scroll
                    } else {
                        0.0
                    }))
                    .h(px(LINE_HEIGHT))
                    .whitespace_nowrap()
                    .child(text[segment.clone()].to_owned()),
            );
        }

        div()
            .absolute()
            .top(px(top))
            .left(px(geometry.right_pane_left()))
            .w(px(geometry.pane_width()))
            .h(px(
                yori::geometry::display_units(projected.height) * LINE_HEIGHT
            ))
            .overflow_hidden()
            .bg(cx.theme().secondary)
            .text_color(cx.theme().muted_foreground)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .w(px(GUTTER_WIDTH - 10.0))
                    .text_right()
                    .child(number),
            )
            .child(text_area)
    }
}
