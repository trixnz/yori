use std::rc::Rc;

use gpui_kit::component::ActiveTheme;
use gpui_kit::{
    AppContext, Context, DragMoveEvent, EntityId, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, TestSupportExt, Window, div, px,
};

const HANDLE_WIDTH: f32 = 6.0;

struct ResizeDrag {
    owner: EntityId,
    divider: usize,
}

struct ResizePreview;

impl Render for ResizePreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub(crate) fn handle<V: 'static>(
    id: &'static str,
    divider: usize,
    position: f32,
    bottom: f32,
    resize: impl Fn(&mut V, f32, &mut Window, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> gpui_kit::AnyElement {
    let owner = cx.entity().entity_id();
    let view = cx.entity().downgrade();
    let resize = Rc::new(resize);
    let start_resize = Rc::clone(&resize);
    let highlight = cx.theme().muted_foreground;

    div()
        .id((id, divider))
        .test_support()
        .absolute()
        .top_0()
        .bottom(px(bottom))
        .left(px(position - HANDLE_WIDTH / 2.0))
        .w(px(HANDLE_WIDTH))
        .block_mouse_except_scroll()
        .cursor_col_resize()
        .hover(|style| style.bg(highlight.opacity(0.25)))
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(HANDLE_WIDTH / 2.0))
                .w(px(1.0))
                .bg(cx.theme().border),
        )
        .on_drag(
            ResizeDrag { owner, divider },
            move |_, offset, window, cx| {
                let x = position + f32::from(offset.x) - HANDLE_WIDTH / 2.0;
                view.update(cx, |view, cx| start_resize(view, x, window, cx))
                    .ok();
                cx.new(|_| ResizePreview)
            },
        )
        .on_drag_move(
            cx.listener(move |view, event: &DragMoveEvent<ResizeDrag>, window, cx| {
                let drag = event.drag(cx);
                if drag.owner == owner && drag.divider == divider {
                    let x = f32::from(event.event.position.x);
                    let center = f32::from(event.bounds.origin.x) + HANDLE_WIDTH / 2.0;
                    resize(view, position + x - center, window, cx);
                }
            }),
        )
        .into_any_element()
}
