//! Optional centre-channel connections for the existing baseline restore actions.

use std::ops::Range;

use gpui_kit::component::{ActiveTheme, WindowExt, notification::Notification};
use gpui_kit::{
    Bounds, Context, Div, Hsla, InteractiveElement, MouseButton, ParentElement, PathBuilder,
    Pixels, StatefulInteractiveElement, Styled, Window, canvas, div, point, px,
};
use yori::geometry::{EditorGeometry, display_units, whole_rows};
use yori_diff::Alignment;
use yori_document::Document;

use super::{AlignedEditor, HEADER_HEIGHT, LINE_HEIGHT, RESTORE_WIDTH};
use crate::appearance;

pub(super) const WIDTH: f32 = 38.0;

struct Connection {
    rows: Range<usize>,
    left: Range<usize>,
    right: Range<usize>,
}

impl Connection {
    fn new(
        alignment: &Alignment,
        left: &Document,
        right: &Document,
        rows: Range<usize>,
        baseline: &Range<usize>,
        local: &Range<usize>,
    ) -> Self {
        let source_rows = |document: &Document, bytes: &Range<usize>, left_side| {
            if bytes.is_empty() {
                // A missing side meets the top of its alignment gap, not a fake line.
                return rows.start..rows.start;
            }

            let start = alignment.row_for_offset(document, bytes.start, left_side);
            let end = alignment.row_for_offset(document, bytes.end - 1, left_side) + 1;
            start..end
        };
        let left = source_rows(left, baseline, true);
        let right = source_rows(right, local, false);

        Self { rows, left, right }
    }
}

impl AlignedEditor {
    pub(super) fn set_connections(
        &mut self,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.merge.is_some() {
            return;
        }

        let mut config = crate::config::editor(cx);
        config.show_change_connections = enabled;
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

        self.show_connections = enabled;
        self.hovered_connection = None;

        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn update_connection_hover(&mut self, window: &Window, cx: &mut Context<Self>) {
        let geometry = self.geometry();
        let position = window.mouse_position() - self.content_bounds.get().origin;
        let x = f32::from(position.x);
        let y = f32::from(position.y) - HEADER_HEIGHT;
        let inside = self.show_connections
            && (geometry.pane_width()..geometry.right_pane_left()).contains(&x)
            && (0.0..geometry.rows_viewport_height()).contains(&y);
        let row = whole_rows((y + self.vertical_scroll) / LINE_HEIGHT);

        let hovered = if !inside {
            None
        } else if self
            .selection
            .as_ref()
            .is_some_and(|selection| !selection.range().is_empty())
        {
            self.selection_restore()
                .filter(|plan| plan.rows.contains(&row))
                .map(|plan| plan.rows)
        } else {
            let index = self
                .alignment
                .blocks()
                .partition_point(|block| block.rows.end <= row);
            self.alignment
                .blocks()
                .get(index)
                .filter(|block| block.rows.contains(&row))
                .map(|block| block.rows.clone())
        };

        // The arrow and band are overlapping hit targets. Resolve their enter/leave
        // callbacks from pointer position so callback order cannot clear a valid hover.
        if hovered != self.hovered_connection {
            self.hovered_connection = hovered;
            cx.notify();
        }
    }

    fn visible_connections(&self, geometry: EditorGeometry) -> Vec<Connection> {
        let build = |rows, baseline: &Range<usize>, local: &Range<usize>| {
            Connection::new(
                &self.alignment,
                &self.left.document,
                &self.right.document,
                rows,
                baseline,
                local,
            )
        };

        // Selection narrows both the drawing and the action: never imply a full-block restore.
        if self
            .selection
            .as_ref()
            .is_some_and(|selection| !selection.range().is_empty())
        {
            return self
                .selection_restore()
                .map(|plan| build(plan.rows, &plan.baseline, &plan.local))
                .into_iter()
                .collect();
        }

        let first_row = whole_rows(self.vertical_scroll / LINE_HEIGHT);
        let end = self.vertical_scroll + geometry.rows_viewport_height();
        let first_block = self
            .alignment
            .blocks()
            .partition_point(|block| block.rows.end <= first_row);

        self.alignment
            .blocks()
            .iter()
            .skip(first_block)
            .take_while(|block| display_units(block.rows.start) * LINE_HEIGHT < end)
            .map(|block| build(block.rows.clone(), &block.left, &block.right))
            .collect()
    }

    pub(super) fn render_connections(
        &self,
        geometry: EditorGeometry,
        cx: &mut Context<Self>,
    ) -> Div {
        if !self.show_connections {
            return div();
        }

        let mut surface = div().absolute().size_full();
        let mut channel = div()
            .absolute()
            .left(px(geometry.pane_width()))
            .top_0()
            .w(px(geometry.center_width()))
            .h(px(geometry.rows_viewport_height()))
            .overflow_hidden()
            .cursor_default()
            .bg(cx.theme().secondary)
            .border_x_1()
            .border_color(cx.theme().border)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation());

        let selected = self
            .selection
            .as_ref()
            .is_some_and(|selection| !selection.range().is_empty());

        for connection in self.visible_connections(geometry) {
            let hovered = self.hovered_connection.as_ref() == Some(&connection.rows);
            let current = self
                .navigation
                .current(&self.alignment)
                .is_some_and(|index| self.alignment.blocks()[index].rows == connection.rows);

            if hovered || selected {
                for (left, rows) in [
                    (0.0, &connection.left),
                    (geometry.right_pane_left(), &connection.right),
                ] {
                    surface = surface.child(
                        div()
                            .absolute()
                            .left(px(left + RESTORE_WIDTH))
                            .top(px(
                                display_units(rows.start) * LINE_HEIGHT - self.vertical_scroll
                            ))
                            .w(px((geometry.pane_width() - RESTORE_WIDTH).max(0.0)))
                            .h(px((display_units(rows.len()) * LINE_HEIGHT).max(2.0)))
                            .border_1()
                            .border_color(cx.theme().muted_foreground),
                    );
                }
            }

            channel = channel.child(self.render_connection(connection, hovered, current, cx));
        }

        surface.child(channel)
    }

    fn render_connection(
        &self,
        connection: Connection,
        hovered: bool,
        current: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let top = display_units(connection.rows.start) * LINE_HEIGHT - self.vertical_scroll;
        let height = display_units(connection.rows.len()) * LINE_HEIGHT;
        let relative = |rows: &Range<usize>| {
            let start = display_units(rows.start - connection.rows.start) * LINE_HEIGHT;
            let end = display_units(rows.end - connection.rows.start) * LINE_HEIGHT;
            start..end
        };
        let left = relative(&connection.left);
        let right = relative(&connection.right);

        let color = if connection.left.is_empty() {
            appearance::added().marker
        } else if connection.right.is_empty() {
            appearance::removed().marker
        } else {
            cx.theme().muted_foreground
        };
        let emphasis = if hovered {
            0.24
        } else if current {
            0.15
        } else {
            0.08
        };
        let rows = connection.rows;

        div()
            .id(("change-connection", rows.start))
            .absolute()
            .top(px(top))
            .left_0()
            .w_full()
            .h(px(height))
            .on_hover(cx.listener(|this, _: &bool, window, cx| {
                this.update_connection_hover(window, cx);
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| {
                        paint_connection(
                            bounds,
                            &left,
                            &right,
                            color.opacity(emphasis),
                            false,
                            window,
                        );
                        paint_connection(
                            bounds,
                            &left,
                            &right,
                            color.opacity(if hovered { 0.65 } else { 0.3 }),
                            true,
                            window,
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
    }
}

fn paint_connection(
    bounds: Bounds<Pixels>,
    left: &Range<f32>,
    right: &Range<f32>,
    color: Hsla,
    stroke: bool,
    window: &mut Window,
) {
    let mut path = if stroke {
        PathBuilder::stroke(px(1.0))
    } else {
        PathBuilder::fill()
    };
    let start = bounds.left();
    let end = bounds.right();
    let middle = start + bounds.size.width / 2.0;
    let y = |offset| bounds.top() + px(offset);

    path.move_to(point(start, y(left.start)));
    path.cubic_bezier_to(
        point(end, y(right.start)),
        point(middle, y(left.start)),
        point(middle, y(right.start)),
    );
    path.line_to(point(end, y(right.end)));
    path.cubic_bezier_to(
        point(start, y(left.end)),
        point(middle, y(right.end)),
        point(middle, y(left.end)),
    );
    path.close();

    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

#[cfg(test)]
mod tests;
