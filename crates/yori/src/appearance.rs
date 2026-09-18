//! Shared neutral chrome and semantic diff colors for the native application.

#![expect(
    clippy::unreadable_literal,
    reason = "RGB colors use conventional 0xRRGGBB notation so they can be read and copied as hex color codes"
)]

use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, px, rgb};

pub(super) fn init(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    apply(Theme::global_mut(cx));
    Theme::sync_base(cx);
}

fn apply(theme: &mut Theme) {
    theme.background = rgb(0x191919).into();
    theme.foreground = rgb(0xdedede).into();
    theme.secondary = rgb(0x202020).into();
    theme.secondary_foreground = theme.foreground;
    theme.secondary_hover = rgb(0x303030).into();
    theme.secondary_active = rgb(0x3b3b3b).into();
    theme.border = rgb(0x353535).into();
    theme.muted = rgb(0x292929).into();
    theme.muted_foreground = rgb(0x969696).into();

    theme.accent = theme.secondary_hover;
    theme.accent_foreground = theme.foreground;
    theme.primary = rgb(0xc6c6c6).into();
    theme.primary_foreground = theme.background;
    theme.primary_hover = rgb(0xd6d6d6).into();
    theme.primary_active = rgb(0xb6b6b6).into();
    theme.ring = rgb(0xaaaaaa).into();
    theme.selection = rgb(0x4b4b4b).into();
    theme.caret = theme.foreground;

    theme.popover = theme.secondary;
    theme.popover_foreground = theme.foreground;
    theme.button = theme.secondary;
    theme.button_foreground = theme.foreground;
    theme.button_hover = theme.secondary_hover;
    theme.button_active = theme.secondary_active;
    theme.button_primary = theme.primary;
    theme.button_primary_foreground = theme.primary_foreground;
    theme.button_primary_hover = theme.primary_hover;
    theme.button_primary_active = theme.primary_active;
    theme.button_secondary = theme.secondary;
    theme.button_secondary_foreground = theme.secondary_foreground;
    theme.button_secondary_hover = theme.secondary_hover;
    theme.button_secondary_active = theme.secondary_active;

    theme.tab_bar = theme.background;
    theme.tab = theme.background;
    theme.tab_foreground = theme.muted_foreground;
    theme.tab_active = theme.secondary;
    theme.tab_active_foreground = theme.foreground;

    // Keep Component's resolved tokens and Base's mirrored theme in agreement.
    theme.tokens = theme.colors.into();
    theme.font_size = px(13.0);
    theme.mono_font_size = px(14.0);
    theme.radius = px(5.0);
}

pub(super) struct DiffColors {
    pub line: Hsla,
    pub emphasis: Hsla,
    pub marker: Hsla,
}

pub(super) fn removed() -> DiffColors {
    DiffColors {
        line: rgb(0x2e2222).into(),
        emphasis: rgb(0x603638).into(),
        marker: rgb(0xc67d7d).into(),
    }
}

pub(super) fn added() -> DiffColors {
    DiffColors {
        line: rgb(0x1e2b24).into(),
        emphasis: rgb(0x31543e).into(),
        marker: rgb(0x7caa88).into(),
    }
}

pub(super) fn gap() -> Hsla {
    rgb(0x161616).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppAssets;
    use gpui_kit::AssetSource;
    use gpui_kit::component::{IconName, IconNamed};

    #[test]
    fn chrome_is_neutral_and_component_tokens_follow_the_palette() {
        let mut theme = Theme::default();
        apply(&mut theme);

        for color in [
            theme.background,
            theme.secondary,
            theme.foreground,
            theme.muted_foreground,
            theme.accent,
            theme.primary,
            theme.ring,
            theme.selection,
        ] {
            assert!(color.s.abs() < f32::EPSILON);
        }

        assert_eq!(theme.tokens.background.color, theme.background);
        assert_eq!(theme.tokens.secondary_active.color, theme.secondary_active);
        assert_eq!(theme.tokens.button_primary.color, theme.primary);
        assert!(theme.selection.l > added().line.l);
        assert!(theme.selection.l > removed().line.l);
    }

    #[test]
    fn every_used_icon_is_available_in_the_application_bundle() {
        let assets = AppAssets::default();

        for icon in [
            IconName::ArrowUp,
            IconName::ArrowDown,
            IconName::ArrowLeft,
            IconName::ArrowRight,
            IconName::FileText,
            IconName::Plus,
            IconName::Close,
        ] {
            let path = icon.path();
            let bytes = assets.load(path.as_ref()).unwrap().unwrap();

            assert!(!bytes.is_empty(), "missing icon: {path}");
        }

        for icon in [
            gpui_kit::assets::IconName::GitMerge,
            gpui_kit::assets::IconName::GitPullRequest,
            gpui_kit::assets::IconName::RefreshCw,
        ] {
            let path = icon.path();
            let bytes = assets.load(path.as_ref()).unwrap().unwrap();

            assert!(!bytes.is_empty(), "missing icon: {path}");
        }
    }
}
