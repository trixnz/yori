//! Keyboard-first Git review-source chooser.

use gpui_kit::component::{
    ActiveTheme,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
};
use gpui_kit::{
    AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, ScrollHandle, StatefulInteractiveElement,
    Styled, Subscription, TestSupportExt, Window, div, prelude::FluentBuilder, px,
};

use super::{GitCommitSummary, GitRepository, ReviewSource};

const KEY_CONTEXT: &str = "GitSourceChooser";

pub(crate) enum GitSourceChooserEvent {
    Chosen(ReviewSource),
    Cancelled,
}

impl EventEmitter<GitSourceChooserEvent> for GitSourceChooser {}

pub(crate) struct GitSourceChooser {
    repository: GitRepository,
    commits: Vec<GitCommitSummary>,
    selected: usize,
    focus: FocusHandle,
    scroll: ScrollHandle,
    revision: Entity<InputState>,
    _revision_subscription: Subscription,
    message: Option<String>,
}

impl GitSourceChooser {
    pub(crate) fn new(
        repository: GitRepository,
        commits: Vec<GitCommitSummary>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let revision = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Commit, branch, tag, or other revision")
        });
        let subscription = cx.subscribe(&revision, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.open_revision(cx);
            }
        });
        let focus = cx.focus_handle();
        focus.focus(window, cx);

        Self {
            repository,
            commits,
            selected: 0,
            focus,
            scroll: ScrollHandle::new(),
            revision,
            _revision_subscription: subscription,
            message: None,
        }
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
    }

    fn option_count(&self) -> usize {
        self.commits.len() + 2
    }

    fn revision_index(&self) -> usize {
        self.option_count() - 1
    }

    fn move_selection(&mut self, offset: isize, cx: &mut Context<Self>) {
        self.selected = self
            .selected
            .saturating_add_signed(offset)
            .min(self.option_count() - 1);

        if self.selected <= self.commits.len() {
            self.scroll.scroll_to_item(self.selected);
        }

        self.message = None;
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }

        if event.keystroke.key == "escape" {
            cx.emit(GitSourceChooserEvent::Cancelled);
            window.prevent_default();
            cx.stop_propagation();
            return;
        }

        if self.revision.focus_handle(cx).is_focused(window) {
            return;
        }

        match event.keystroke.key.as_str() {
            "up" | "k" => self.move_selection(-1, cx),
            "down" | "j" => self.move_selection(1, cx),
            "enter" => self.activate_selected(window, cx),
            _ => return,
        }

        window.prevent_default();
        cx.stop_propagation();
    }

    fn activate_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected == 0 {
            cx.emit(GitSourceChooserEvent::Chosen(
                self.repository.working_source(),
            ));
            return;
        }

        if self.selected == self.revision_index() {
            let revision = self.revision.read(cx).text().to_string();
            let revision = revision.trim();
            if revision.is_empty() {
                self.revision.focus_handle(cx).focus(window, cx);
            } else {
                self.open_revision(cx);
            }
            return;
        }

        let commit = &self.commits[self.selected - 1];
        match self.repository.commit_source(&commit.revision) {
            Ok(source) => cx.emit(GitSourceChooserEvent::Chosen(source)),
            Err(error) => {
                self.message = Some(error);
                cx.notify();
            }
        }
    }

    fn open_revision(&mut self, cx: &mut Context<Self>) {
        let revision = self.revision.read(cx).text().to_string();

        match self.repository.commit_source(revision.trim()) {
            Ok(source) => cx.emit(GitSourceChooserEvent::Chosen(source)),
            Err(error) => {
                self.message = Some(error);
                cx.notify();
            }
        }
    }

    fn select_revision(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.revision_index();
        self.message = None;

        self.revision.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn render_row(
        &self,
        id: gpui_kit::ElementId,
        index: usize,
        title: gpui_kit::SharedString,
        detail: gpui_kit::SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let selected = self.selected == index;

        div()
            .id(id)
            .test_support()
            .px(px(12.0))
            .py(px(9.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .border_b_1()
            .border_color(cx.theme().border)
            .when(selected, |row| row.bg(cx.theme().accent))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.selected = index;
                this.activate_selected(window, cx);
            }))
            .child(title)
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(cx.theme().muted_foreground)
                    .child(detail),
            )
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(14.0))
            .py(px(12.0))
            .border_b_1()
            .border_color(cx.theme().border)
            .child("Open Git review")
            .child(
                div()
                    .mt(px(3.0))
                    .text_size(px(11.0))
                    .text_color(cx.theme().muted_foreground)
                    .child(self.repository.work_dir().display().to_string()),
            )
    }

    fn render_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_revision = self.selected == self.revision_index();
        let message = self.message.clone();

        div()
            .w(px(620.0))
            .max_h(px(620.0))
            .flex()
            .flex_col()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(px(6.0))
            .overflow_hidden()
            .bg(cx.theme().background)
            .child(self.render_header(cx))
            .child(
                div()
                    .id("git-source-list")
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.scroll)
                    .overflow_y_scroll()
                    .child(self.render_row(
                        "git-source-working".into(),
                        0,
                        "Working changes".into(),
                        "HEAD compared with staged, unstaged, and untracked files".into(),
                        cx,
                    ))
                    .children(self.commits.iter().enumerate().map(|(offset, commit)| {
                        let detail = if commit.is_merge {
                            format!(
                                "{} · merge commit (parent selection unsupported)",
                                commit.short_id
                            )
                        } else {
                            commit.short_id.clone()
                        };

                        self.render_row(
                            ("git-source-commit", offset).into(),
                            offset + 1,
                            commit.title.clone().into(),
                            detail.into(),
                            cx,
                        )
                    })),
            )
            .child(
                div()
                    .id("git-source-revision")
                    .test_support()
                    .px(px(12.0))
                    .py(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .when(selected_revision, |row| row.bg(cx.theme().accent))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.select_revision(window, cx)),
                    )
                    .child(div().w(px(110.0)).child("Enter revision"))
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.revision)
                                .id("git-revision-input")
                                .aria_label("Git revision"),
                        ),
                    )
                    .child(
                        Button::new("open-git-revision")
                            .label("Open")
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_revision(cx);
                            })),
                    ),
            )
            .children(message.map(|message| {
                div()
                    .id("git-source-error")
                    .test_support()
                    .px(px(12.0))
                    .py(px(8.0))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_color(crate::appearance::removed().marker)
                    .child(message)
            }))
    }
}

impl Render for GitSourceChooser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("git-source-chooser")
            .test_support()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .tab_index(0)
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .on_key_down(cx.listener(Self::key_down))
            .child(self.render_panel(cx))
    }
}

#[cfg(test)]
impl GitSourceChooser {
    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    pub(crate) fn work_dir(&self) -> &std::path::Path {
        self.repository.work_dir()
    }

    pub(crate) fn revision_text(&self, cx: &gpui_kit::App) -> String {
        self.revision.read(cx).text().to_string()
    }
}
