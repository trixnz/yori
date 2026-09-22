//! One shared diff-aware scrollbar beside the aligned rows.

use std::ops::Range;

use gpui_kit::component::ActiveTheme;
use gpui_kit::{
    Bounds, Context, DispatchPhase, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Styled, TestSupportExt,
    WeakEntity, Window, canvas, div, fill, point, px, size,
};
use yori::scrollbar::{HorizontalScrollTrack, OverviewBand, ScrollTrack};

use super::merge::MergeState;
use super::{AlignedEditor, HEADER_HEIGHT, LINE_HEIGHT};
use crate::appearance;

pub(super) const WIDTH: f32 = 18.0;
pub(super) const HEIGHT: f32 = 14.0;

impl AlignedEditor {
    fn scroll_track(&self) -> ScrollTrack {
        ScrollTrack::new(
            self.alignment.rows().len(),
            LINE_HEIGHT,
            self.geometry().rows_viewport_height(),
        )
    }

    fn horizontal_scroll_track(&self, max_scroll: f32) -> HorizontalScrollTrack {
        let geometry = self.geometry();

        HorizontalScrollTrack::new(
            geometry.text_viewport_width() - 2.0,
            max_scroll,
            geometry.content_width(),
        )
    }

    fn scrollbar_y(&self, y: gpui_kit::Pixels) -> f32 {
        f32::from(y - self.content_bounds.get().origin.y) - HEADER_HEIGHT
    }

    fn horizontal_scrollbar_x(&self, x: gpui_kit::Pixels) -> f32 {
        f32::from(x - self.content_bounds.get().origin.x)
    }

    fn scrollbar_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let track = self.scroll_track();
        let y = self.scrollbar_y(event.position.y);
        let thumb = track.thumb(self.vertical_scroll);
        let grab = if thumb.contains(&y) {
            y - thumb.start
        } else {
            self.vertical_scroll = track.jump(y);
            let thumb = track.thumb(self.vertical_scroll);
            (y - thumb.start).clamp(0.0, thumb.end - thumb.start)
        };

        self.scrollbar_grab = Some(grab);

        cx.stop_propagation();
        cx.notify();
    }

    fn scrollbar_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(grab) = self.scrollbar_grab else {
            return;
        };

        if event.pressed_button == Some(MouseButton::Left) {
            let track = self.scroll_track();
            let thumb = track.thumb(self.vertical_scroll);
            let top = self.scrollbar_y(event.position.y) - grab.min(thumb.end - thumb.start);
            self.vertical_scroll = track.scroll_for_thumb(top);
        } else {
            self.scrollbar_grab = None;
        }

        cx.stop_propagation();
        cx.notify();
    }

    fn horizontal_scrollbar_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let max_scroll = self.max_horizontal_scroll(window, cx);
        let track = self.horizontal_scroll_track(max_scroll);
        let x = self.horizontal_scrollbar_x(event.position.x);
        let thumb = track.thumb(self.horizontal_scroll);
        let grab = if thumb.contains(&x) {
            x - thumb.start
        } else {
            self.horizontal_scroll = track.jump(x);
            let thumb = track.thumb(self.horizontal_scroll);
            (x - thumb.start).clamp(0.0, thumb.end - thumb.start)
        };

        self.horizontal_scrollbar_grab = Some(grab);

        cx.stop_propagation();
        cx.notify();
    }

    fn horizontal_scrollbar_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(grab) = self.horizontal_scrollbar_grab else {
            return;
        };

        if event.pressed_button == Some(MouseButton::Left) {
            let max_scroll = self.max_horizontal_scroll(window, cx);
            let track = self.horizontal_scroll_track(max_scroll);
            let thumb = track.thumb(self.horizontal_scroll);
            let left =
                self.horizontal_scrollbar_x(event.position.x) - grab.min(thumb.end - thumb.start);
            self.horizontal_scroll = track.scroll_for_thumb(left);
        } else {
            self.horizontal_scrollbar_grab = None;
        }

        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn render_scrollbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let track = self.scroll_track();
        let thumb = track.thumb(self.vertical_scroll);
        let bands = if self.merge.is_none() {
            track.bands(&self.alignment, LINE_HEIGHT)
        } else {
            Vec::new()
        };
        let merge_marks = self
            .merge
            .as_ref()
            .map_or_else(Vec::new, |merge| merge_scrollbar_marks(merge, track));
        let current = if self.merge.is_some() {
            merge_marks
                .iter()
                .find(|mark| mark.current)
                .map(|mark| mark.range.clone())
        } else {
            self.navigation
                .current(&self.alignment)
                .map(|index| track.marker(self.alignment.blocks()[index].rows.clone(), LINE_HEIGHT))
        };
        let foreground = cx.theme().foreground;
        let resolved_color = cx.theme().muted_foreground;
        let unresolved_color = cx.theme().warning;
        let thumb_color = foreground.opacity(if self.scrollbar_grab.is_some() {
            0.24
        } else {
            0.12
        });
        let editor = cx.weak_entity();

        div()
            .id("diff-scrollbar")
            .test_support()
            .absolute()
            .right_0()
            .top(px(HEADER_HEIGHT))
            .w(px(WIDTH))
            .h(px(self.geometry().rows_viewport_height()))
            .overflow_hidden()
            .cursor_default()
            .bg(cx.theme().secondary)
            .border_l_1()
            .border_color(cx.theme().border)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::scrollbar_down))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| {
                        paint_rect(bounds, 1.0..WIDTH - 1.0, thumb.clone(), thumb_color, window);
                        paint_markers(bounds, &bands, current.clone(), foreground, window);
                        for mark in &merge_marks {
                            let color = if mark.resolved {
                                resolved_color
                            } else {
                                unresolved_color
                            };
                            paint_rect(bounds, 6.0..14.0, mark.range.clone(), color, window);
                        }

                        capture_vertical_drag(editor.clone(), window);
                    },
                )
                .absolute()
                .size_full(),
            )
    }

    pub(super) fn render_horizontal_scrollbar(
        &self,
        max_scroll: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let geometry = self.geometry();
        let track = self.horizontal_scroll_track(max_scroll);
        let thumb = track.thumb(self.horizontal_scroll);
        let thumb_color =
            cx.theme()
                .foreground
                .opacity(if self.horizontal_scrollbar_grab.is_some() {
                    0.24
                } else {
                    0.12
                });
        let editor = cx.weak_entity();

        div()
            .absolute()
            .left_0()
            .top(px(HEADER_HEIGHT + geometry.rows_viewport_height()))
            .w(px(geometry.content_width() + WIDTH))
            .h(px(HEIGHT))
            .overflow_hidden()
            .cursor_default()
            .bg(cx.theme().secondary)
            .child(
                div()
                    .id("horizontal-scrollbar")
                    .test_support()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(px(geometry.content_width()))
                    .h_full()
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(Self::horizontal_scrollbar_down),
                    )
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, (), window, _| {
                                paint_rect(
                                    bounds,
                                    thumb.clone(),
                                    3.0..HEIGHT - 3.0,
                                    thumb_color,
                                    window,
                                );
                                capture_horizontal_drag(editor.clone(), window);
                            },
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w_full()
                    .h(px(1.0))
                    .bg(cx.theme().border),
            )
            .child(
                div()
                    .absolute()
                    .left(px(geometry.content_width()))
                    .top_0()
                    .w(px(1.0))
                    .h_full()
                    .bg(cx.theme().border),
            )
    }
}

struct MergeScrollbarMark {
    range: Range<f32>,
    resolved: bool,
    current: bool,
}

fn merge_scrollbar_marks(merge: &MergeState, track: ScrollTrack) -> Vec<MergeScrollbarMark> {
    merge
        .display
        .conflicts()
        .iter()
        .map(|conflict| MergeScrollbarMark {
            range: track.marker(conflict.source_span.clone(), LINE_HEIGHT),
            resolved: merge
                .session
                .state(conflict.id)
                .expect("known conflict")
                .resolved,
            current: merge.current == Some(conflict.id),
        })
        .collect()
}

fn paint_rect(
    bounds: Bounds<Pixels>,
    x: Range<f32>,
    y: Range<f32>,
    color: Hsla,
    window: &mut Window,
) {
    let rect = Bounds::new(
        bounds.origin + point(px(x.start), px(y.start)),
        size(px(x.end - x.start), px(y.end - y.start)),
    );

    window.paint_quad(fill(rect, color));
}

fn paint_markers(
    bounds: Bounds<Pixels>,
    bands: &[OverviewBand],
    current: Option<Range<f32>>,
    foreground: Hsla,
    window: &mut Window,
) {
    for band in bands {
        if band.left {
            paint_rect(
                bounds,
                4.0..8.0,
                band.top..band.bottom,
                appearance::removed().marker,
                window,
            );
        }
        if band.right {
            paint_rect(
                bounds,
                10.0..14.0,
                band.top..band.bottom,
                appearance::added().marker,
                window,
            );
        }
    }

    if let Some(current) = current {
        paint_rect(bounds, 1.0..3.0, current, foreground.opacity(0.85), window);
    }
}

fn capture_vertical_drag(editor: WeakEntity<AlignedEditor>, window: &mut Window) {
    // Capture window-wide movement so dragging outside the rail never turns
    // into text selection. These handlers exist only for this frame.
    let dragging_editor = editor.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if phase == DispatchPhase::Capture {
            let _ = dragging_editor.update(cx, |editor, cx| {
                editor.scrollbar_move(event, cx);
            });
        }
    });

    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
        if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
            let _ = editor.update(cx, |editor, cx| {
                if editor.scrollbar_grab.take().is_some() {
                    cx.stop_propagation();
                    cx.notify();
                }
            });
        }
    });
}

fn capture_horizontal_drag(editor: WeakEntity<AlignedEditor>, window: &mut Window) {
    let dragging_editor = editor.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture {
            let _ = dragging_editor.update(cx, |editor, cx| {
                editor.horizontal_scrollbar_move(event, window, cx);
            });
        }
    });

    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
        if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
            let _ = editor.update(cx, |editor, cx| {
                if editor.horizontal_scrollbar_grab.take().is_some() {
                    cx.stop_propagation();
                    cx.notify();
                }
            });
        }
    });
}

#[cfg(test)]
mod tests;
