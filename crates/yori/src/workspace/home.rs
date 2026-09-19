//! The fixed Home surface and its workspace-owned pinned control.

use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Selectable, Sizable,
    tab::{Tab, TabVariant},
    tooltip::Tooltip,
};
use gpui_kit::{
    App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, Render, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt, Window, div, prelude::FluentBuilder, px,
};

use crate::review::GitCommitSummary;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

const KEY_CONTEXT: &str = "WorkspaceHome";

const ACTIONS: [HomeAction; 5] = [
    HomeAction::ReviewGitChange,
    HomeAction::ReviewPerforceChangelist,
    HomeAction::CompareFiles,
    HomeAction::OpenThreeWayMerge,
    HomeAction::Preferences,
];

/// Actions that start work. Preferences follows them in `ACTIONS` so the
/// keyboard still reaches it, but it adjusts yori rather than starting
/// anything, so it renders apart from the others.
const WORKFLOW_ACTIONS: usize = 4;

/// Enough commits to recognise where you left off without the panel becoming
/// the whole screen. The provider offers more than this.
const RECENT_SHOWN: usize = 8;

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

    fn icon(self) -> Icon {
        match self {
            Self::ReviewGitChange => Icon::new(gpui_kit::assets::IconName::GitPullRequest),
            Self::ReviewPerforceChangelist => Icon::new(IconName::Network),
            Self::CompareFiles => Icon::new(IconName::Copy),
            Self::OpenThreeWayMerge => Icon::new(gpui_kit::assets::IconName::GitMerge),
            Self::Preferences => Icon::new(IconName::Settings),
        }
    }

    /// The binding registered in `workspace::init`, spelled for this platform.
    /// Perforce review has no binding, so it shows no hint rather than a wrong
    /// one.
    fn shortcut(self) -> Option<String> {
        let (shift, key) = match self {
            Self::ReviewGitChange => (true, "G"),
            Self::ReviewPerforceChangelist => return None,
            Self::CompareFiles => (false, "O"),
            Self::OpenThreeWayMerge => (true, "M"),
            Self::Preferences => (false, ","),
        };

        Some(if cfg!(target_os = "macos") {
            format!("⌘{}{key}", if shift { "⇧" } else { "" })
        } else {
            format!("Ctrl+{}{key}", if shift { "Shift+" } else { "" })
        })
    }
}

/// Commits from the repository yori was launched in, so the most likely reason
/// to open yori is one click away instead of behind a chooser. Absent when
/// there is no repository to read, which collapses Home to a single column.
pub(super) struct RecentCommits {
    pub repository: String,
    pub commits: Vec<GitCommitSummary>,
}

pub(super) enum HomeEvent {
    Action(HomeAction),
    WorkingChanges,
    Revision(String),
}

impl EventEmitter<HomeEvent> for Home {}

pub(super) struct Home {
    focus: FocusHandle,
    selected: usize,
    recent: Option<RecentCommits>,
}

impl Home {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            selected: 0,
            recent: None,
        }
    }

    pub(super) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
    }

    pub(super) fn set_recent(&mut self, recent: Option<RecentCommits>, cx: &mut Context<Self>) {
        self.recent = recent;
        cx.notify();
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
        cx.emit(HomeEvent::Action(ACTIONS[self.selected]));
    }

    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = index;
        self.focus.focus(window, cx);
        cx.emit(HomeEvent::Action(ACTIONS[index]));
        cx.notify();
    }

    fn render_wordmark(cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .text_size(px(22.0))
                    .font_weight(FontWeight::MEDIUM)
                    .child("yori"),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child("Diff, merge, and review."),
            )
    }

    fn render_action(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let action = ACTIONS[index];
        let selected = self.selected == index;
        let grouped = index + 1 < WORKFLOW_ACTIONS;

        div()
            .id(("home-action", index))
            .test_support()
            .px(px(12.0))
            .py(px(9.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .when(grouped, |row| {
                row.border_b_1().border_color(cx.theme().border)
            })
            .when(selected, |row| row.bg(cx.theme().accent))
            .when(!selected, |row| {
                row.hover(|row| row.bg(cx.theme().secondary_hover))
            })
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.choose(index, window, cx);
            }))
            .child(
                action
                    .icon()
                    .with_size(px(16.0))
                    .text_color(cx.theme().muted_foreground),
            )
            .child(div().flex_1().min_w_0().child(action.label()))
            .children(action.shortcut().map(|shortcut| {
                div()
                    .flex_shrink_0()
                    .text_size(px(11.0))
                    .text_color(cx.theme().muted_foreground)
                    .child(shortcut)
            }))
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .w(px(340.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(render_section_heading("Start", cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(6.0))
                    .overflow_hidden()
                    .children((0..WORKFLOW_ACTIONS).map(|index| self.render_action(index, cx))),
            )
            .child(
                div()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(6.0))
                    .overflow_hidden()
                    .child(self.render_action(WORKFLOW_ACTIONS, cx)),
            )
    }

    fn render_recent(recent: &RecentCommits, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = recent.commits.len().min(RECENT_SHOWN);

        div()
            .w(px(380.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(render_section_heading(
                format!("Review · {}", recent.repository),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(6.0))
                    .overflow_hidden()
                    .child(Self::render_working_changes(shown > 0, cx))
                    .children(
                        recent
                            .commits
                            .iter()
                            .take(shown)
                            .enumerate()
                            .map(|(index, commit)| Self::render_commit(index, shown, commit, cx)),
                    ),
            )
    }

    /// Uncommitted work sits above the history because it is the most likely
    /// thing to be reviewing, and because Home would otherwise offer no way to
    /// reach it at all.
    fn render_working_changes(followed: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .id("home-working-changes")
            .test_support()
            .px(px(12.0))
            .py(px(8.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .when(followed, |row| {
                row.border_b_1().border_color(cx.theme().border)
            })
            .hover(|row| row.bg(cx.theme().secondary_hover))
            .cursor_pointer()
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HomeEvent::WorkingChanges)))
            .child(div().flex_1().min_w_0().child("Working changes"))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.0))
                    .text_color(cx.theme().muted_foreground)
                    .child("uncommitted"),
            )
    }

    fn render_commit(
        index: usize,
        shown: usize,
        commit: &GitCommitSummary,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let revision = commit.revision.clone();
        let tooltip = commit.title.clone();

        div()
            .id(("home-commit", index))
            .test_support()
            .px(px(12.0))
            .py(px(8.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .when(index + 1 < shown, |row| {
                row.border_b_1().border_color(cx.theme().border)
            })
            .hover(|row| row.bg(cx.theme().secondary_hover))
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(HomeEvent::Revision(revision.clone()));
            }))
            .child(
                div()
                    .w(px(58.0))
                    .flex_shrink_0()
                    .text_color(cx.theme().muted_foreground)
                    .child(commit.short_id.clone()),
            )
            .child(
                div()
                    .id(("home-commit-title", index))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_middle()
                    .child(commit.title.clone())
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx)),
            )
            .children(commit.is_merge.then(|| {
                div()
                    .flex_shrink_0()
                    .text_size(px(11.0))
                    .text_color(cx.theme().muted_foreground)
                    .child("merge")
            }))
            .child(
                div()
                    .w(px(28.0))
                    .flex_shrink_0()
                    .text_right()
                    .text_size(px(11.0))
                    .text_color(cx.theme().muted_foreground)
                    .child(relative_age(commit.time_seconds)),
            )
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
                    .flex()
                    .flex_col()
                    .gap(px(24.0))
                    .child(Self::render_wordmark(cx))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(32.0))
                            .child(self.render_actions(cx))
                            .children(
                                self.recent
                                    .as_ref()
                                    .map(|recent| Self::render_recent(recent, cx)),
                            ),
                    ),
            )
    }
}

fn render_section_heading(label: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .text_size(px(11.0))
        .text_color(cx.theme().muted_foreground)
        .child(label.into())
}

/// Coarse commit age, in the shortest form that still reads unambiguously. A
/// commit list is scanned, not audited, so a rounded age beats a timestamp.
fn relative_age(time_seconds: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        });

    match (now - time_seconds).max(0) {
        age if age < 60 => "now".to_owned(),
        age if age < 3_600 => format!("{}m", age / 60),
        age if age < 86_400 => format!("{}h", age / 3_600),
        age if age < 2_592_000 => format!("{}d", age / 86_400),
        age => format!("{}mo", age / 2_592_000),
    }
}

#[cfg(test)]
mod age_tests {
    use super::relative_age;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn ago(seconds: i64) -> String {
        let now = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap();

        relative_age(now - seconds)
    }

    #[test]
    fn ages_round_down_to_the_largest_unit_that_fits() {
        assert_eq!(ago(5), "now");
        assert_eq!(ago(90), "1m");
        assert_eq!(ago(3 * 3_600), "3h");
        assert_eq!(ago(36 * 3_600), "1d");
        assert_eq!(ago(40 * 86_400), "1mo");
    }

    #[test]
    fn a_commit_dated_in_the_future_reads_as_now_rather_than_negative() {
        let now = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap();

        assert_eq!(relative_age(now + 3_600), "now");
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
