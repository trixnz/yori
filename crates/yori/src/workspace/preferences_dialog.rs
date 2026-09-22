use gpui_kit::component::{ActiveTheme, WindowExt, checkbox::Checkbox, notification::Notification};
use gpui_kit::{
    Context, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render, Role,
    SharedString, StatefulInteractiveElement, Styled, TestSupportExt, Window, div, px,
};

use crate::config::EditorConfig;

pub(super) struct PreferencesDialog {
    editor: EditorConfig,
    error: Option<SharedString>,
    focus: FocusHandle,
}

impl PreferencesDialog {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            editor: crate::config::editor(cx),
            error: None,
            focus: cx.focus_handle(),
        }
    }

    pub(super) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(super) fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match crate::config::update_editor(self.editor, cx) {
            Ok(diagnostic) => {
                if let Some(diagnostic) = diagnostic {
                    window.push_notification(Notification::error(diagnostic), cx);
                }
                true
            }
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                false
            }
        }
    }

    fn setting(
        id: &'static str,
        label: &'static str,
        description: &'static str,
        checked: bool,
        on_change: impl Fn(&mut Self, bool) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        Checkbox::new(id)
            .checked(checked)
            .label(label)
            .accessibility_label(label)
            .on_change(cx.listener(move |this, checked: &bool, _, cx| {
                on_change(this, *checked);
                this.error = None;
                cx.notify();
            }))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(cx.theme().muted_foreground)
                    .child(description),
            )
    }
}

impl Render for PreferencesDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("preferences-dialog-content")
            .test_support()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(Self::setting(
                "vim-keybindings",
                "Use Vim keybindings",
                "Applies immediately to every open editor.",
                self.editor.vim_keybindings,
                |this, checked| this.editor.vim_keybindings = checked,
                cx,
            ))
            .child(Self::setting(
                "show-whitespace",
                "Show whitespace",
                "Applies immediately to every open editor.",
                self.editor.show_whitespace,
                |this, checked| this.editor.show_whitespace = checked,
                cx,
            ))
            .child(Self::setting(
                "show-change-connections",
                "Show change connections",
                "Applies immediately to every open editor.",
                self.editor.show_change_connections,
                |this, checked| this.editor.show_change_connections = checked,
                cx,
            ))
            .children(self.error.as_ref().map(|error| {
                div()
                    .id("preferences-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(error.clone())
                    .text_size(px(12.0))
                    .text_color(cx.theme().danger)
                    .child(error.clone())
            }))
    }
}
