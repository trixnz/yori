use std::{num::NonZeroU32, sync::Arc};

use gpui_kit::component::{
    ActiveTheme, Disableable, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement,
};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, Render, Role as AccessibilityRole, ScrollHandle,
    StatefulInteractiveElement, Styled, Subscription, TestSupportExt, Window, div,
    prelude::FluentBuilder, px,
};
use yori_p4::ChangelistSummary;

use crate::review::{PerforceContext, ReviewSource};

const LIST_KEY_CONTEXT: &str = "PerforceSourceList";

gpui_kit::actions!(
    perforce_source_chooser,
    [SelectPrevious, SelectNext, ActivateSelected]
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Pending,
    Recent,
    Number,
}

pub(super) struct SourceChosen(pub ReviewSource);

impl EventEmitter<SourceChosen> for PerforceSourceChooser {}

pub(super) struct PerforceSourceChooser {
    context: Arc<PerforceContext>,
    mode: Mode,
    selection: usize,
    list_focus: FocusHandle,
    list_scroll: ScrollHandle,
    number: Entity<InputState>,
    _number_subscription: Subscription,
    opening_number: bool,
    message: Option<String>,
}

impl PerforceSourceChooser {
    pub(super) fn new(
        context: Arc<PerforceContext>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let number = cx.new(|cx| InputState::new(window, cx).placeholder("Changelist number"));
        let number_subscription = cx.subscribe_in(
            &number,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.open_number(window, cx);
                }
            },
        );

        Self {
            context,
            mode: Mode::Pending,
            selection: 0,
            list_focus: cx.focus_handle(),
            list_scroll: ScrollHandle::new(),
            number,
            _number_subscription: number_subscription,
            opening_number: false,
            message: None,
        }
    }

    pub(super) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_current(window, cx);
    }

    fn focus_current(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            Mode::Pending | Mode::Recent => self.list_focus.focus(window, cx),
            Mode::Number => self.number.focus_handle(cx).focus(window, cx),
        }
    }

    fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = mode;
        self.selection = 0;
        self.list_scroll.scroll_to_item(0);
        self.message = None;

        self.focus_current(window, cx);
        cx.notify();
    }

    fn summaries(&self) -> Vec<&ChangelistSummary> {
        match self.mode {
            Mode::Pending => self.context.pending().collect(),
            Mode::Recent => self.context.recent().iter().collect(),
            Mode::Number => Vec::new(),
        }
    }

    fn move_selection(&mut self, offset: isize, cx: &mut Context<Self>) {
        let count = self.summaries().len();
        if count == 0 {
            return;
        }

        self.selection = self
            .selection
            .saturating_add_signed(offset)
            .min(count.saturating_sub(1));
        self.list_scroll.scroll_to_item(self.selection);
        cx.notify();
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    fn activate_selected(
        &mut self,
        _: &ActivateSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(summary) = self.summaries().get(self.selection).copied() else {
            return;
        };

        let source = match self.mode {
            Mode::Pending => Ok(self.context.pending_source(summary)),
            Mode::Recent => self.context.submitted_source(summary),
            Mode::Number => return,
        };

        match source {
            Ok(source) => Self::complete(source, window, cx),
            Err(error) => {
                self.message = Some(error);
                cx.notify();
            }
        }
    }

    fn open_number(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening_number {
            return;
        }

        let value = self.number.read(cx).value();
        let Ok(number) = value.parse::<NonZeroU32>() else {
            self.message = Some("Enter a non-zero changelist number.".into());
            cx.notify();
            return;
        };

        self.opening_number = true;
        self.message = None;
        cx.notify();

        let context = Arc::clone(&self.context);
        let resolve = cx
            .background_executor()
            .spawn(async move { context.source_for_number(number) });
        cx.spawn_in(window, async move |view, cx| {
            let result = resolve.await;
            let _ = view.update_in(cx, |this, window, cx| {
                this.opening_number = false;
                match result {
                    Ok(source) => Self::complete(source, window, cx),
                    Err(error) => {
                        this.message = Some(format!("Cannot open changelist {number}: {error}"));
                        this.number.focus_handle(cx).focus(window, cx);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn complete(source: ReviewSource, window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(SourceChosen(source));
        window.defer(cx, Window::close_dialog);
    }

    fn mode_button(
        &self,
        id: &'static str,
        label: &'static str,
        mode: Mode,
        cx: &mut Context<Self>,
    ) -> Button {
        let button = Button::new(id)
            .label(label)
            .accessibility_label(label)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_mode(mode, window, cx);
            }));

        if self.mode == mode {
            button.primary()
        } else {
            button.ghost()
        }
    }

    fn render_list(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let summaries = self.summaries();
        if summaries.is_empty() {
            return div()
                .id("perforce-source-list")
                .test_support()
                .key_context(LIST_KEY_CONTEXT)
                .track_focus(&self.list_focus)
                .tab_index(0)
                .h(px(240.0))
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .on_action(cx.listener(Self::select_previous))
                .on_action(cx.listener(Self::select_next))
                .on_action(cx.listener(Self::activate_selected))
                .child(match self.mode {
                    Mode::Pending => "No pending changelists are available.",
                    Mode::Recent => "No submitted changelists were found in this client view.",
                    Mode::Number => unreachable!(),
                })
                .into_any_element();
        }

        let count = summaries.len();
        div()
            .id("perforce-source-list")
            .test_support()
            .key_context(LIST_KEY_CONTEXT)
            .track_focus(&self.list_focus)
            .tab_index(0)
            .role(AccessibilityRole::ListBox)
            .aria_label("Perforce review sources")
            .aria_size_of_set(count)
            .h(px(240.0))
            .track_scroll(&self.list_scroll)
            .overflow_y_scroll()
            .border_1()
            .border_color(cx.theme().border)
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::activate_selected))
            .children(summaries.into_iter().enumerate().map(|(index, summary)| {
                let selected = index == self.selection;
                let source = match self.mode {
                    Mode::Pending => Ok(self.context.pending_source(summary)),
                    Mode::Recent => self.context.submitted_source(summary),
                    Mode::Number => unreachable!(),
                };
                let id = summary.id.to_string();
                let description = summary.description.trim().to_owned();
                let metadata = format!("{} · {}", summary.user, summary.client);

                div()
                    .id(("perforce-source", index))
                    .test_support()
                    .role(AccessibilityRole::ListBoxOption)
                    .aria_selected(selected)
                    .aria_position_in_set(index + 1)
                    .aria_size_of_set(count)
                    .px(px(10.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .when(selected, |row| row.bg(cx.theme().accent))
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |this, _, window, cx| match source.clone() {
                            Ok(source) => Self::complete(source, window, cx),
                            Err(error) => {
                                this.message = Some(error);
                                cx.notify();
                            }
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(id))
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(metadata),
                            ),
                    )
                    .child(description)
            }))
            .vertical_scrollbar(&self.list_scroll)
            .into_any_element()
    }

    fn render_number(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        div()
            .h(px(240.0))
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(10.0))
            .child("Open a pending or submitted changelist by number.")
            .child(
                Input::new(&self.number)
                    .id("perforce-changelist-number")
                    .aria_label("Perforce changelist number"),
            )
            .child(
                Button::new("open-perforce-number")
                    .label(if self.opening_number {
                        "Opening…"
                    } else {
                        "Open changelist"
                    })
                    .primary()
                    .disabled(self.opening_number)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_number(window, cx);
                    })),
            )
            .into_any_element()
    }
}

#[cfg(test)]
impl PerforceSourceChooser {
    pub(super) fn number_text(&self, cx: &App) -> String {
        self.number.read(cx).value().to_string()
    }

    pub(super) fn list_is_scrolled(&self) -> bool {
        self.list_scroll.offset().y < px(0.0)
    }
}

impl Render for PerforceSourceChooser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let message = self.message.clone();
        let body = match self.mode {
            Mode::Pending | Mode::Recent => self.render_list(cx),
            Mode::Number => self.render_number(cx),
        };

        div()
            .id("perforce-source-chooser")
            .w(px(620.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.context.client_label()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(self.mode_button(
                        "perforce-pending",
                        "Pending changelists",
                        Mode::Pending,
                        cx,
                    ))
                    .child(self.mode_button(
                        "perforce-recent",
                        "Recent submitted",
                        Mode::Recent,
                        cx,
                    ))
                    .child(self.mode_button(
                        "perforce-number",
                        "Enter changelist number",
                        Mode::Number,
                        cx,
                    )),
            )
            .children(message.map(|message| {
                div()
                    .id("perforce-source-error")
                    .px(px(8.0))
                    .py(px(6.0))
                    .bg(cx.theme().muted)
                    .child(message)
            }))
            .child(body)
    }
}

pub(super) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, Some(LIST_KEY_CONTEXT)),
        KeyBinding::new("k", SelectPrevious, Some(LIST_KEY_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(LIST_KEY_CONTEXT)),
        KeyBinding::new("j", SelectNext, Some(LIST_KEY_CONTEXT)),
        KeyBinding::new("enter", ActivateSelected, Some(LIST_KEY_CONTEXT)),
        KeyBinding::new("space", ActivateSelected, Some(LIST_KEY_CONTEXT)),
    ]);
}
