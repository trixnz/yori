//! Quiet document controls beneath each pane, using the existing component kit.

use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable,
    button::{Button, ButtonVariants},
    menu::{DropdownMenu, PopupMenuItem},
    tooltip::Tooltip,
};
use gpui_kit::{
    Anchor, Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, TestSupportExt, Window, div, px,
};

use super::{AlignedEditor, FOOTER_HEIGHT, Language, PaneDocument, Side, ToggleWordWrap};

impl AlignedEditor {
    pub(super) fn render_footer(
        &self,
        pane_width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("editor-footer")
            .test_support()
            .absolute()
            .bottom_0()
            .left_0()
            .w_full()
            .h(px(FOOTER_HEIGHT))
            .flex()
            .items_center()
            .overflow_hidden()
            .cursor_default()
            .font_family(cx.theme().font_family.clone())
            .text_size(px(12.0))
            .line_height(px(20.0))
            .bg(cx.theme().secondary)
            .border_t_1()
            .border_color(cx.theme().border)
            .child(self.render_pane_status(Side::Left, pane_width, cx))
            .child(
                div()
                    .w(px(self.geometry().center_width()))
                    .h_full()
                    .flex_shrink_0(),
            )
            .child(self.render_pane_status(
                Side::Right,
                pane_width
                    + if self.merge.is_none() {
                        super::scrollbar::WIDTH
                    } else {
                        0.0
                    },
                cx,
            ))
            .children(self.merge.as_ref().map(|_| {
                self.render_pane_status(Side::Incoming, pane_width + super::scrollbar::WIDTH, cx)
            }))
    }

    fn render_pane_status(
        &self,
        side: Side,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pane = self.document(side);
        let tooltip = line_ending_description(pane);
        let show_metadata = width >= 360.0;

        div()
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .overflow_hidden()
            .border_l(px(if side == Side::Left { 0.0 } else { 1.0 }))
            .border_color(cx.theme().border)
            .child(self.render_language_menu(side, cx))
            .children(show_metadata.then(|| {
                div()
                    .id(match side {
                        Side::Left => "left-line-endings",
                        Side::Right => "right-line-endings",
                        Side::Incoming => "incoming-line-endings",
                    })
                    .text_color(cx.theme().muted_foreground)
                    .child(pane.line_endings.label())
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            }))
            .child(div().flex_1())
            .children((side == Side::Right && self.can_save()).then(|| {
                Button::new("save-document")
                    .label("Save")
                    .small()
                    .ghost()
                    .disabled(self.saving || self.unresolved_count() != 0)
                    .tooltip(if self.unresolved_count() != 0 {
                        "Resolve all conflicts before saving RESULT".to_owned()
                    } else if self.needs_save() {
                        "Save changes (Ctrl+S)".to_owned()
                    } else {
                        "No unsaved changes (Ctrl+S)".to_owned()
                    })
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::workspace::Save), cx);
                    })
            }))
            .children(
                (side == Side::Incoming || (side == Side::Right && self.merge.is_none()))
                    .then(|| self.render_options_controls(cx)),
            )
    }

    fn render_language_menu(&self, side: Side, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.document(side);
        let detected = Language::detect(&pane.path);
        let selected = pane.language_override;
        let label = pane.language().label();
        let tooltip = if selected.is_none() {
            format!("Language: Auto ({label}). Click to override.")
        } else {
            format!("Language: {label} (manual override)")
        };
        let editor = cx.weak_entity();

        Button::new(match side {
            Side::Left => "left-language",
            Side::Right => "right-language",
            Side::Incoming => "incoming-language",
        })
        .label(label)
        .dropdown_caret(true)
        .ghost()
        .small()
        .accessibility_label(match side {
            Side::Left => {
                if self.merge.is_some() {
                    "Local language"
                } else {
                    "Baseline language"
                }
            }
            Side::Right => {
                if self.merge.is_some() {
                    "Result language"
                } else {
                    "Local language"
                }
            }
            Side::Incoming => "Incoming language",
        })
        .tooltip(tooltip)
        .dropdown_menu_with_anchor(Anchor::BottomLeft, move |mut menu, _, _| {
            let choices = std::iter::once(None).chain(Language::ALL.into_iter().map(Some));

            for language in choices {
                let label = language.map_or_else(
                    || format!("Auto ({})", detected.label()),
                    |language| language.label().to_owned(),
                );
                let editor = editor.clone();
                let item = PopupMenuItem::new(label)
                    .checked(language == selected)
                    .on_click(move |_, window, cx| {
                        let _ = editor.update(cx, |editor, cx| {
                            editor.choose_language(side, language, window, cx);
                        });
                    });
                menu = menu.item(item);
            }

            menu
        })
    }

    fn render_options_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = Self::vim_enabled(cx);

        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .flex_shrink_0()
            .children(enabled.then(|| {
                div()
                    .w(px(108.0))
                    .text_size(px(11.0))
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("Vim: {}", self.vim.mode().label()))
            }))
            .child(self.render_options_menu(cx))
    }

    fn render_options_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let word_wrap = self.word_wrap.enabled();
        let whitespace = self.show_whitespace;
        let connections = self.show_connections;
        let two_way = self.merge.is_none();
        let vim = Self::vim_enabled(cx);
        let editor = cx.weak_entity();

        Button::new("editor-options")
            .label("Options")
            .dropdown_caret(true)
            .ghost()
            .small()
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                let wrap_item = PopupMenuItem::new("Word wrap (this tab)")
                    .checked(word_wrap)
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(Box::new(ToggleWordWrap), cx);
                    });

                let whitespace_editor = editor.clone();
                let whitespace_item = PopupMenuItem::new("Show whitespace (all comparisons)")
                    .checked(whitespace)
                    .on_click(move |_, window, cx| {
                        let _ = whitespace_editor.update(cx, |editor, cx| {
                            editor.set_whitespace(!whitespace, window, cx);
                        });
                    });

                let connections_editor = editor.clone();
                let connections_item =
                    PopupMenuItem::new("Show change connections (all comparisons)")
                        .checked(connections)
                        .on_click(move |_, window, cx| {
                            let _ = connections_editor.update(cx, |editor, cx| {
                                editor.set_connections(!connections, window, cx);
                            });
                        });

                let vim_editor = editor.clone();
                let vim_item = PopupMenuItem::new("Vim keybindings (all tabs)")
                    .checked(vim)
                    .on_click(move |_, window, cx| {
                        let _ = vim_editor.update(cx, |editor, cx| {
                            editor.toggle_vim(!vim, window, cx);
                        });
                    });

                let menu = menu.item(whitespace_item);
                let menu = if two_way {
                    menu.item(connections_item)
                } else {
                    menu
                };
                menu.separator().item(vim_item).item(wrap_item)
            })
    }

    fn choose_language(
        &mut self,
        side: Side,
        language: Option<Language>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.document_mut(side).change_language(language) {
            self.schedule_highlighting(side, window, cx);
        }

        self.focus.focus(window, cx);
        cx.notify();
    }
}

fn line_ending_description(pane: &PaneDocument) -> String {
    let ending = pane.line_endings.description();
    if pane.document.text().is_empty() {
        return format!("{ending} · Empty file");
    }

    let final_newline = if pane.document.text().ends_with('\n') {
        "Final newline present"
    } else {
        "No final newline"
    };

    format!("{ending} · {final_newline}")
}

#[cfg(test)]
mod tests;
