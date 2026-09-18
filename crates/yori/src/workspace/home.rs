//! The fixed Home surface and its workspace-owned pinned control.

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme, Selectable, Sizable,
    tab::{Tab, TabVariant},
};
use gpui_kit::{
    App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, Render, RenderOnce, StatefulInteractiveElement, Styled,
    TestSupportExt, Window, div, prelude::FluentBuilder, px,
};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

const KEY_CONTEXT: &str = "WorkspaceHome";

const ACTIONS: [HomeAction; 5] = [
    HomeAction::ReviewGitChange,
    HomeAction::ReviewPerforceChangelist,
    HomeAction::CompareFiles,
    HomeAction::OpenThreeWayMerge,
    HomeAction::Preferences,
];

gpui_kit::actions!(
    workspace_home,
    [SelectPrevious, SelectNext, ActivateSelected]
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HomeAction {
    ReviewGitChange,
    ReviewPerforceChangelist,
    CompareFiles,
    OpenThreeWayMerge,
    Preferences,
}

impl HomeAction {
    fn label(self) -> &'static str {
        match self {
            Self::ReviewGitChange => "Review Git change",
            Self::ReviewPerforceChangelist => "Review Perforce changelist",
            Self::CompareFiles => "Compare files",
            Self::OpenThreeWayMerge => "Open three-way merge",
            Self::Preferences => "Preferences",
        }
    }
}

pub(super) struct ActionChosen(pub HomeAction);

impl EventEmitter<ActionChosen> for Home {}

pub(super) struct Home {
    focus: FocusHandle,
    selected: usize,
}

impl Home {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            selected: 0,
        }
    }

    pub(super) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
    }

    fn move_selection(&mut self, offset: isize, cx: &mut Context<Self>) {
        self.selected = self
            .selected
            .saturating_add_signed(offset)
            .min(ACTIONS.len() - 1);
        cx.notify();
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    fn activate_selected(&mut self, _: &ActivateSelected, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(ActionChosen(ACTIONS[self.selected]));
    }

    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = index;
        self.focus.focus(window, cx);
        cx.emit(ActionChosen(ACTIONS[index]));
        cx.notify();
    }
}

impl Focusable for Home {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Home {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("home")
            .test_support()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .tab_index(0)
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::activate_selected))
            .child(
                div()
                    .w(px(420.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(div().text_size(px(20.0)).child("Home"))
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child("Start a review, comparison, merge, or adjust Yori."),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(px(6.0))
                            .overflow_hidden()
                            .children(ACTIONS.into_iter().enumerate().map(|(index, action)| {
                                let selected = self.selected == index;

                                div()
                                    .id(("home-action", index))
                                    .test_support()
                                    .px(px(12.0))
                                    .py(px(10.0))
                                    .when(index + 1 < ACTIONS.len(), |row| {
                                        row.border_b_1().border_color(cx.theme().border)
                                    })
                                    .when(selected, |row| row.bg(cx.theme().accent))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.choose(index, window, cx);
                                    }))
                                    .child(action.label())
                            })),
                    ),
            )
    }
}

#[derive(IntoElement)]
pub(super) struct HomeControl {
    selected: bool,
    on_click: Option<ClickHandler>,
}

impl HomeControl {
    pub(super) fn new(selected: bool) -> Self {
        Self {
            selected,
            on_click: None,
        }
    }

    pub(super) fn on_click(
        mut self,
        listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(listener));
        self
    }
}

impl RenderOnce for HomeControl {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let tab = Tab::new()
            .label("Home")
            .aria_label("Home")
            .selected(self.selected)
            .with_variant(TabVariant::Tab)
            .with_size(px(38.0))
            .when_some(self.on_click, |tab, listener| {
                tab.on_click(move |event, window, cx| listener(event, window, cx))
            });

        div().id("home-control").test_support().child(tab)
    }
}

#[cfg(test)]
impl Home {
    pub(super) fn selected_action(&self) -> HomeAction {
        ACTIONS[self.selected]
    }

    pub(super) fn action_labels() -> Vec<&'static str> {
        ACTIONS.into_iter().map(HomeAction::label).collect()
    }
}

pub(super) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, Some(KEY_CONTEXT)),
        KeyBinding::new("k", SelectPrevious, Some(KEY_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(KEY_CONTEXT)),
        KeyBinding::new("j", SelectNext, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", ActivateSelected, Some(KEY_CONTEXT)),
        KeyBinding::new("space", ActivateSelected, Some(KEY_CONTEXT)),
    ]);
}
