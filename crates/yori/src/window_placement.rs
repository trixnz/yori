//! Persistent top-level window placement, separate from user-facing preferences.

use atomicwrites::{AllowOverwrite, AtomicFile};
use gpui_kit::{App, Bounds, Context, Global, Pixels, Window, WindowBounds, point, px, size};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use toml_edit::DocumentMut;

const APPLICATION_NAME: &str = "yori";
const FILE_NAME: &str = "window-state.toml";
const FORMAT_VERSION: i64 = 1;

#[derive(Default)]
struct WindowPlacement {
    path: Option<PathBuf>,
    current: Option<WindowBounds>,
}

impl Global for WindowPlacement {}

impl WindowPlacement {
    fn persistent(path: PathBuf) -> Self {
        Self {
            current: load(&path),
            path: Some(path),
        }
    }
}

pub(crate) fn init(cx: &mut App) {
    let placement = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map_or_else(WindowPlacement::default, |directory| {
            WindowPlacement::persistent(directory.join(APPLICATION_NAME).join(FILE_NAME))
        });

    cx.set_global(placement);
}

pub(crate) fn saved(cx: &App) -> Option<WindowBounds> {
    cx.global::<WindowPlacement>().current
}

pub(crate) fn track<T: 'static>(window: &mut Window, cx: &mut Context<T>) {
    if cx.try_global::<WindowPlacement>().is_none() {
        return;
    }

    if cx.global::<WindowPlacement>().current.is_none() {
        capture(window, cx);
    }

    cx.observe_window_bounds(window, |_, window, cx| capture(window, cx))
        .detach();
}

pub(crate) fn persist(window: &Window, cx: &mut App) {
    if cx.try_global::<WindowPlacement>().is_none() {
        return;
    }

    capture(window, cx);

    let placement = cx.global::<WindowPlacement>();
    let (Some(path), Some(bounds)) = (&placement.path, placement.current) else {
        return;
    };

    if let Err(error) = save(path, bounds) {
        eprintln!("yori: cannot save window placement: {error}");
    }
}

fn capture(window: &Window, cx: &mut App) {
    let reported = window.window_bounds();
    let restore_bounds = cx
        .global::<WindowPlacement>()
        .current
        .map_or_else(|| reported.get_bounds(), |bounds| bounds.get_bounds());
    let current = if window.is_fullscreen() {
        WindowBounds::Fullscreen(restore_bounds)
    } else if window.is_maximized() {
        WindowBounds::Maximized(restore_bounds)
    } else {
        WindowBounds::Windowed(reported.get_bounds())
    };

    cx.global_mut::<WindowPlacement>().current = Some(current);
}

fn load(path: &Path) -> Option<WindowBounds> {
    let text = fs::read_to_string(path).ok()?;
    let document = text.parse::<DocumentMut>().ok()?;
    if document.get("version")?.as_integer()? != FORMAT_VERSION {
        return None;
    }

    let bounds = Bounds {
        origin: point(number(&document, "x")?, number(&document, "y")?),
        size: size(number(&document, "width")?, number(&document, "height")?),
    };
    if bounds.size.width <= Pixels::ZERO || bounds.size.height <= Pixels::ZERO {
        return None;
    }

    match document.get("state")?.as_str()? {
        "windowed" => Some(WindowBounds::Windowed(bounds)),
        "maximized" => Some(WindowBounds::Maximized(bounds)),
        "fullscreen" => Some(WindowBounds::Fullscreen(bounds)),
        _ => None,
    }
}

fn number(document: &DocumentMut, key: &str) -> Option<Pixels> {
    let value = document
        .get(key)?
        .as_value()?
        .to_string()
        .trim()
        .parse::<f32>()
        .ok()?;

    value.is_finite().then(|| px(value))
}

fn save(path: &Path, placement: WindowBounds) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("window state path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;

    let (state, bounds) = match placement {
        WindowBounds::Windowed(bounds) => ("windowed", bounds),
        WindowBounds::Maximized(bounds) => ("maximized", bounds),
        WindowBounds::Fullscreen(bounds) => ("fullscreen", bounds),
    };
    let text = format!(
        "version = {FORMAT_VERSION}\nstate = \"{state}\"\nx = {}\ny = {}\nwidth = {}\nheight = {}\n",
        f32::from(bounds.origin.x),
        f32::from(bounds.origin.y),
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
    );

    AtomicFile::new(path, AllowOverwrite)
        .write(|temporary| temporary.write_all(text.as_bytes()))
        .map_err(|error| format!("cannot update {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Bounds<Pixels> {
        Bounds {
            origin: point(px(-320.5), px(48.25)),
            size: size(px(1234.5), px(789.25)),
        }
    }

    #[test]
    fn every_window_state_round_trips_with_restore_bounds() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let placements = [
            WindowBounds::Windowed(bounds()),
            WindowBounds::Maximized(bounds()),
            WindowBounds::Fullscreen(bounds()),
        ];

        for expected in placements {
            save(&path, expected).unwrap();

            assert_eq!(load(&path), Some(expected));
        }
    }

    #[test]
    fn missing_unreadable_and_invalid_state_fall_back_to_default_placement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);

        assert_eq!(load(&path), None);

        fs::write(
            &path,
            "version = 1\nstate = \"maximized\"\nx = 10\ny = 20\nwidth = -1\nheight = 600\n",
        )
        .unwrap();
        assert_eq!(load(&path), None);

        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert_eq!(load(&path), None);
    }
}
