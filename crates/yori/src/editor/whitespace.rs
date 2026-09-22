//! Paint-only whitespace hints over the unchanged shaped source text.

use std::ops::Range;

use gpui_kit::component::{ActiveTheme, WindowExt, notification::Notification};
use gpui_kit::{
    AnyElement, Bounds, Context, Font, Hsla, IntoElement, PathBuilder, Pixels, Point, SharedString,
    Styled, TextAlign, TextRun, Window, canvas, fill, point, px, size,
};
use yori::display::{DisplayLine, WhitespaceKind};
use yori_document::LineEnding;

use super::{AlignedEditor, LINE_HEIGHT};

// Leave room to reveal the longest ending label without expanding the document itself.
pub(super) const ENDING_LABEL_COLUMNS: usize = 12;

impl AlignedEditor {
    pub(super) fn set_whitespace(
        &mut self,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut config = crate::config::editor(cx);
        config.show_whitespace = visible;
        let diagnostic = match crate::config::update_editor(config, cx) {
            Ok(diagnostic) => diagnostic,
            Err(error) => {
                window.push_notification(Notification::error(error), cx);
                return;
            }
        };
        if let Some(diagnostic) = diagnostic {
            window.push_notification(Notification::error(diagnostic), cx);
        }

        self.show_whitespace = visible;
        self.horizontal_scroll = self
            .horizontal_scroll
            .min(self.max_horizontal_scroll(window, cx));

        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn render_whitespace(
        &self,
        display: &DisplayLine,
        source: &str,
        ending: LineEnding,
        segment: Range<usize>,
        top: f32,
        cx: &Context<Self>,
    ) -> AnyElement {
        let markers = display
            .whitespace_marks(source)
            .into_iter()
            .filter_map(|marker| {
                let start = marker.display.start.max(segment.start);
                let end = marker.display.end.min(segment.end);
                (start < end).then(|| {
                    (
                        start - segment.start..end - segment.start,
                        marker.kind,
                        marker.trailing,
                    )
                })
            })
            .collect::<Vec<_>>();
        let text = SharedString::from(display.text[segment.clone()].to_owned());
        let last_segment = segment.end == display.text.len();
        let label = SharedString::from(match ending {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
            LineEnding::None => "no newline",
        });
        let font = Font {
            family: cx.theme().mono_font_family.clone(),
            ..Font::default()
        };
        let font_size = cx.theme().mono_font_size;
        let muted = cx.theme().muted_foreground.opacity(0.65);
        let trailing = cx.theme().foreground.opacity(0.6);
        let scroll = self.horizontal_scroll;

        canvas(
            move |_, window, _| {
                let run = TextRun {
                    len: text.len(),
                    font: font.clone(),
                    color: muted,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let source = window
                    .text_system()
                    .shape_line(text.clone(), font_size, &[run], None);

                let run = TextRun {
                    len: label.len(),
                    font: font.clone(),
                    color: muted,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let label = window
                    .text_system()
                    .shape_line(label.clone(), px(10.0), &[run], None);

                (source, label)
            },
            move |bounds, (source, label), window, cx| {
                let origin = point(bounds.origin.x - px(scroll), bounds.origin.y);
                let center_y = origin.y + px(LINE_HEIGHT / 2.0);

                for (range, kind, is_trailing) in &markers {
                    let start = origin.x + source.x_for_index(range.start);
                    let end = origin.x + source.x_for_index(range.end);
                    if end < bounds.left() || start > bounds.right() {
                        continue;
                    }

                    let color = if *is_trailing { trailing } else { muted };
                    paint_mark(*kind, start, end, center_y, color, window);
                }

                if last_segment {
                    let label_origin = point(origin.x + source.width() + px(8.0), origin.y);
                    if let Err(error) = label.paint(
                        label_origin,
                        px(LINE_HEIGHT),
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    ) {
                        eprintln!("whitespace label paint failed: {error}");
                    }
                }
            },
        )
        .absolute()
        .top(px(top))
        .left_0()
        .w_full()
        .h(px(LINE_HEIGHT))
        .into_any_element()
    }
}

fn paint_mark(
    kind: WhitespaceKind,
    start: Pixels,
    end: Pixels,
    center_y: Pixels,
    color: Hsla,
    window: &mut Window,
) {
    if kind == WhitespaceKind::Space {
        let origin = point((start + end) / 2.0 - px(1.0), center_y - px(1.0));
        window.paint_quad(fill(Bounds::new(origin, size(px(2.0), px(2.0))), color));

        return;
    }

    let start = start + px(1.0);
    let end = (end - px(1.0)).max(start);
    let head = px(3.0).min((end - start) / 2.0);
    let mut path = PathBuilder::stroke(px(1.0));
    path.move_to(Point::new(start, center_y));
    path.line_to(Point::new(end, center_y));
    path.move_to(Point::new(end - head, center_y - head));
    path.line_to(Point::new(end, center_y));
    path.line_to(Point::new(end - head, center_y + head));

    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

#[cfg(test)]
mod tests;
