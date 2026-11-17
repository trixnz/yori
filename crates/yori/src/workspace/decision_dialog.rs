use std::rc::Rc;

use gpui_kit::component::{
    Disableable, WindowExt,
    button::{Button, ButtonVariants},
    dialog::DialogFooter,
    kbd::Kbd,
};
use gpui_kit::{
    App, Context, InteractiveElement, Keystroke, ParentElement, SharedString, TestSupportExt,
    Window, div,
};

#[derive(Clone, Copy)]
pub(super) enum DecisionShortcut {
    Enter,
    Escape,
    Mnemonic(char),
}

impl DecisionShortcut {
    fn key(self) -> String {
        match self {
            Self::Enter => "enter".to_owned(),
            Self::Escape => "escape".to_owned(),
            Self::Mnemonic(character) => character.to_string(),
        }
    }

    fn keystroke(self) -> Keystroke {
        Keystroke {
            key: self.key(),
            ..Keystroke::default()
        }
    }

    fn matches(self, keystroke: &Keystroke) -> bool {
        !keystroke.modifiers.modified() && keystroke.key.eq_ignore_ascii_case(&self.key())
    }
}

type DecisionAction = dyn Fn(&mut Window, &mut App);

#[derive(Clone)]
pub(super) struct Decision {
    id: &'static str,
    label: SharedString,
    shortcut: DecisionShortcut,
    primary: bool,
    disabled: bool,
    on_activate: Rc<DecisionAction>,
}

impl Decision {
    pub(super) fn new(
        id: &'static str,
        label: impl Into<SharedString>,
        shortcut: DecisionShortcut,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            shortcut,
            primary: false,
            disabled: false,
            on_activate: Rc::new(|_, _| {}),
        }
    }

    pub(super) fn primary(mut self) -> Self {
        self.primary = true;
        self
    }

    pub(super) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub(super) fn on_activate(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_activate = Rc::new(action);
        self
    }

    fn activate(&self, window: &mut Window, cx: &mut App) {
        if self.disabled {
            return;
        }

        window.close_dialog(cx);
        (self.on_activate)(window, cx);
    }

    fn button(&self) -> Button {
        let decision = self.clone();
        let button = Button::new(self.id)
            .accessibility_label(self.label.clone())
            .disabled(self.disabled)
            .child(self.label.clone())
            .child(Kbd::new(self.shortcut.keystroke()))
            .on_click(move |_, window, cx| decision.activate(window, cx));

        if self.primary {
            button.primary()
        } else {
            button
        }
    }
}

pub(super) struct DecisionDialog {
    title: SharedString,
    detail: SharedString,
    cancel: Decision,
    alternate: Option<Decision>,
    primary: Option<Decision>,
}

impl DecisionDialog {
    pub(super) fn new(
        title: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        cancel: Decision,
    ) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            cancel,
            alternate: None,
            primary: None,
        }
    }

    pub(super) fn alternate(mut self, decision: Decision) -> Self {
        self.alternate = Some(decision);
        self
    }

    pub(super) fn primary(mut self, decision: Decision) -> Self {
        self.primary = Some(decision);
        self
    }

    fn alternate_for(&self, keystroke: &Keystroke) -> Option<&Decision> {
        self.alternate
            .as_ref()
            .filter(|decision| decision.shortcut.matches(keystroke))
    }

    fn footer(&self) -> DialogFooter {
        DialogFooter::new()
            .child(self.cancel.button())
            .children(self.alternate.as_ref().map(Decision::button))
            .children(self.primary.as_ref().map(Decision::button))
    }

    pub(super) fn open<T: 'static>(self, window: &mut Window, cx: &mut Context<'_, T>) {
        debug_assert!(matches!(self.cancel.shortcut, DecisionShortcut::Escape));
        debug_assert!(
            self.primary
                .as_ref()
                .is_none_or(|decision| matches!(decision.shortcut, DecisionShortcut::Enter))
        );

        let focus = cx.focus_handle();
        let render_focus = focus.clone();
        let decisions = Rc::new(self);
        let rendered = Rc::clone(&decisions);

        window.open_dialog(cx, move |dialog, _, _| {
            let keyed = Rc::clone(&rendered);
            let cancel = rendered.cancel.clone();
            let primary = rendered.primary.clone();
            let footer = div()
                .id("decision-dialog-shortcuts")
                .test_support()
                .track_focus(&render_focus)
                .on_key_down(move |event, window, cx| {
                    let Some(decision) = keyed.alternate_for(&event.keystroke) else {
                        return;
                    };

                    window.prevent_default();
                    cx.stop_propagation();
                    decision.activate(window, cx);
                })
                .child(rendered.footer());

            dialog
                .title(rendered.title.clone())
                .child(rendered.detail.clone())
                .overlay_closable(false)
                .footer(footer)
                .on_cancel(move |_, window, cx| {
                    cancel.activate(window, cx);
                    false
                })
                .on_ok(move |_, window, cx| {
                    if let Some(primary) = &primary {
                        primary.activate(window, cx);
                    }

                    false
                })
        });

        let modal_focus = window.focused(cx);
        window.defer(cx, move |window, cx| {
            if modal_focus.is_some_and(|focus| focus.is_focused(window)) {
                focus.focus(window, cx);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui_kit::component::Root;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        AppContext, Context, Entity, IntoElement, Render, Styled, TestAppContext,
        VisualTestContext, Window, div,
    };

    use super::*;

    struct Harness;

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn harness(cx: &mut TestAppContext) -> (Entity<Harness>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });

        let mut harness = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Harness);
            harness = Some(view.clone());
            Root::new(view, window, cx)
        });

        (harness.unwrap(), cx)
    }

    fn open_dialog(
        harness: &Entity<Harness>,
        primary_disabled: bool,
        decisions: &Rc<RefCell<Vec<&'static str>>>,
        cx: &mut VisualTestContext,
    ) {
        cx.update(|window, cx| {
            harness.update(cx, |_, cx| {
                let cancel_decisions = Rc::clone(decisions);
                let alternate_decisions = Rc::clone(decisions);
                let primary_decisions = Rc::clone(decisions);
                let cancel = Decision::new("cancel", "Cancel", DecisionShortcut::Escape)
                    .on_activate(move |_, _| cancel_decisions.borrow_mut().push("cancel"));
                let alternate =
                    Decision::new("alternate", "Discard", DecisionShortcut::Mnemonic('d'))
                        .on_activate(move |_, _| {
                            alternate_decisions.borrow_mut().push("alternate");
                        });
                let primary = Decision::new("primary", "Save", DecisionShortcut::Enter)
                    .primary()
                    .disabled(primary_disabled)
                    .on_activate(move |_, _| primary_decisions.borrow_mut().push("primary"));

                DecisionDialog::new("Decision", "Choose one.", cancel)
                    .alternate(alternate)
                    .primary(primary)
                    .open(window, cx);
            });
            window.render_frame(cx);
        });
        cx.run_until_parked();
        cx.update(TestWindowExt::render_frame);
    }

    #[gpui_kit::test]
    fn shortcuts_activate_the_corresponding_decision(cx: &mut TestAppContext) {
        let (harness, cx) = harness(cx);
        let decisions = Rc::new(RefCell::new(Vec::new()));

        for key in ["escape", "d", "enter"] {
            open_dialog(&harness, false, &decisions, cx);
            cx.update(|window, cx| {
                window.within("decision-dialog-shortcuts").press(key, cx);
            });
            cx.run_until_parked();
        }

        assert_eq!(*decisions.borrow(), ["cancel", "alternate", "primary"]);
    }

    #[gpui_kit::test]
    fn disabled_decision_ignores_its_shortcut(cx: &mut TestAppContext) {
        let (harness, cx) = harness(cx);
        let decisions = Rc::new(RefCell::new(Vec::new()));
        open_dialog(&harness, true, &decisions, cx);

        cx.update(|window, cx| {
            window
                .within("decision-dialog-shortcuts")
                .press("enter", cx);
            assert!(window.has_active_dialog(cx));
        });
        assert!(decisions.borrow().is_empty());

        cx.update(|window, cx| {
            window.within("decision-dialog-shortcuts").press("d", cx);
        });
        cx.run_until_parked();
        assert_eq!(*decisions.borrow(), ["alternate"]);
    }
}
