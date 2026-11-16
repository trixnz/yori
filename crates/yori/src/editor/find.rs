//! Floating find field that searches the focused pane's document.
//!
//! The field renders beside the editor surface rather than inside it, so typed
//! keys never pass through the editor's key context or its Vim capture.

use std::ops::Range;

use gpui_kit::component::{
    ActiveTheme, Disableable, IconName,
    button::{Button, ButtonVariants},
    input::{Enter, Escape, Input, InputEvent, InputState},
};
use gpui_kit::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Styled,
    Subscription, TestSupportExt, Window, div, px,
};
use yori::display::DisplayLine;
use yori::find::{FindDirection, FindMatches};

use super::{
    AlignedEditor, FindNext, FindPrevious, HEADER_HEIGHT, OpenFind, Selection, Side, scrollbar,
};

#[cfg(test)]
mod tests;

/// Also named by the keymap's find navigation context.
const KEY_CONTEXT: &str = "FindBar";

const WIDTH: f32 = 460.0;
const INSET: f32 = 8.0;

pub(super) struct FindBar {
    input: Entity<InputState>,
    open: bool,
    side: Side,
    /// Where incremental search starts; navigation moves it to the current match.
    origin: usize,
    matches: FindMatches,
    _subscription: Subscription,
}

/// Display ranges of find matches on one source line.
#[derive(Default)]
pub(super) struct LineMatches {
    pub found: Vec<Range<usize>>,
    pub current: Option<Range<usize>>,
}

impl AlignedEditor {
    pub(super) fn open_find(&mut self, _: &OpenFind, window: &mut Window, cx: &mut Context<Self>) {
        let side = self.active_side();
        let selected = self
            .selection
            .as_ref()
            .filter(|selection| selection.side == side)
            .map(Selection::range);
        let origin = selected.as_ref().map_or(0, |range| range.start);
        let prefill = selected
            .filter(|range| !range.is_empty())
            .map(|range| self.document(side).document.copy_range(range).to_owned())
            .filter(|text| !text.contains(['\n', '\r']));

        // Typing now belongs to the find field: commit Insert-mode typing and any
        // composition so Vim resumes in Normal mode when find closes.
        self.cancel_vim();
        self.finish_composition();

        let input = self.find_bar(window, cx);
        input.update(cx, |input, cx| {
            if let Some(text) = prefill {
                input.set_value(text, window, cx);
            }
            input.select_all(window, cx);
            input.focus(window, cx);
        });

        if let Some(find) = &mut self.find {
            find.open = true;
            find.side = side;
            find.origin = origin;
        }

        self.search_find(window, cx);
    }

    /// Creates the field on first use and returns its input. The field and its
    /// query outlive closing so reopening offers the previous search.
    fn find_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(find) = &self.find {
            return find.input.clone();
        }

        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.search_find(window, cx);
                }
            });

        self.find = Some(FindBar {
            input: input.clone(),
            open: false,
            side: Side::Right,
            origin: 0,
            matches: FindMatches::default(),
            _subscription: subscription,
        });

        input
    }

    fn open_find_bar(&self) -> Option<&FindBar> {
        self.find.as_ref().filter(|find| find.open)
    }

    fn search_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(find) = self.open_find_bar() else {
            return;
        };

        let query = find.input.read(cx).value();
        let text = self.document(find.side).document.text();
        let matches = FindMatches::search(text, &query, find.origin);

        if let Some(find) = &mut self.find {
            find.matches = matches;
        }

        self.select_find_match(window, cx);
    }

    /// Re-searches after edits so highlights and counts follow the document
    /// without moving the selection or viewport.
    pub(super) fn refresh_find(&mut self) {
        let Some(mut find) = self.find.take() else {
            return;
        };

        if find.open {
            find.matches
                .refresh(self.document(find.side).document.text());
        }
        self.find = Some(find);
    }

    fn step_find(&mut self, direction: FindDirection, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_find();
        let Some(find) = self.find.as_mut().filter(|find| find.open) else {
            return;
        };

        if let Some(current) = find.matches.advance(direction) {
            find.origin = current.start;
        }

        self.select_find_match(window, cx);
    }

    pub(super) fn find_next(&mut self, _: &FindNext, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_find_bar().is_none() {
            cx.propagate();
            return;
        }

        self.step_find(FindDirection::Next, window, cx);
    }

    pub(super) fn find_previous(
        &mut self,
        _: &FindPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open_find_bar().is_none() {
            cx.propagate();
            return;
        }

        self.step_find(FindDirection::Previous, window, cx);
    }

    /// Places the selection on the current match and scrolls it into view.
    /// Vim keeps a caret at the match start so it stays in Normal mode.
    fn select_find_match(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((side, current)) = self
            .open_find_bar()
            .and_then(|find| Some((find.side, find.matches.current()?)))
        else {
            cx.notify();
            return;
        };

        let head = if Self::vim_enabled(cx) {
            current.start
        } else {
            current.end
        };
        self.preferred_column = None;
        self.preferred_visual_x = None;
        self.visual_affinity = None;
        self.selection = Some(Selection {
            side,
            anchor: current.start,
            head,
        });
        self.sync_vim_selection(cx);
        self.locate_source_change(side, current.start);

        // Reveal the end first so the start wins when the match is wider than the viewport.
        self.reveal_source(side, current.end, window, cx);
        self.reveal_source(side, current.start, window, cx);

        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(find) = &mut self.find {
            find.open = false;
        }

        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Find matches on one source line of `side`, in that line's display bytes.
    pub(super) fn find_line_matches(
        &self,
        side: Side,
        source_range: &Range<usize>,
        display: &DisplayLine,
    ) -> LineMatches {
        let Some(find) = self.open_find_bar().filter(|find| find.side == side) else {
            return LineMatches::default();
        };

        let to_display = |range: &Range<usize>| {
            let overlap = range.start.max(source_range.start)..range.end.min(source_range.end);
            (overlap.start < overlap.end).then(|| display.display_range(overlap))
        };

        LineMatches {
            found: find
                .matches
                .overlapping(source_range.clone())
                .iter()
                .filter_map(to_display)
                .collect(),
            current: find
                .matches
                .current()
                .and_then(|current| to_display(&current)),
        }
    }

    pub(super) fn render_find_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let find = self.open_find_bar()?;
        let has_matches = !find.matches.ranges().is_empty();
        let label = find.matches.label().unwrap_or_default();

        Some(
            div()
                .id("find-bar")
                .test_support()
                .key_context(KEY_CONTEXT)
                .occlude()
                .absolute()
                .top(px(HEADER_HEIGHT + INSET))
                .right(px(scrollbar::WIDTH + INSET))
                .w(px(WIDTH))
                .flex()
                .items_center()
                .gap(px(6.0))
                .p(px(6.0))
                .bg(cx.theme().popover)
                .text_color(cx.theme().popover_foreground)
                .border_1()
                .border_color(cx.theme().border)
                .rounded(cx.theme().radius)
                .shadow_md()
                .cursor_default()
                .font_family(cx.theme().font_family.clone())
                .text_size(px(13.0))
                .on_action(cx.listener(|this, enter: &Enter, window, cx| {
                    let direction = if enter.shift {
                        FindDirection::Previous
                    } else {
                        FindDirection::Next
                    };
                    this.step_find(direction, window, cx);
                }))
                .on_action(cx.listener(|this, _: &Escape, window, cx| {
                    this.close_find(window, cx);
                }))
                .on_action(cx.listener(Self::find_next))
                .on_action(cx.listener(Self::find_previous))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&find.input).focus_bordered(false).w_full()),
                )
                .child(
                    div()
                        .id("find-count")
                        .test_support()
                        .flex_shrink_0()
                        .min_w(px(72.0))
                        .text_center()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(
                    Button::new("find-previous")
                        .icon(IconName::ArrowUp)
                        .accessibility_label("Previous match")
                        .ghost()
                        .tooltip("Previous match (Shift+Enter)")
                        .disabled(!has_matches)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.step_find(FindDirection::Previous, window, cx);
                        })),
                )
                .child(
                    Button::new("find-next")
                        .icon(IconName::ArrowDown)
                        .accessibility_label("Next match")
                        .ghost()
                        .tooltip("Next match (Enter)")
                        .disabled(!has_matches)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.step_find(FindDirection::Next, window, cx);
                        })),
                )
                .child(
                    Button::new("find-close")
                        .icon(IconName::Close)
                        .accessibility_label("Close find")
                        .ghost()
                        .tooltip("Close (Escape)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.close_find(window, cx);
                        })),
                ),
        )
    }
}
