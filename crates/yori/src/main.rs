//! yori process startup and native application composition.

mod appearance;
mod comparison;
mod editor;
mod instance;
mod storage;
mod workspace;
use comparison::ComparisonPaths;
use gpui_kit::component::Root;
use gpui_kit::{AppContext, AssetSource, SharedString, WindowOptions};
use std::{borrow::Cow, env, path::PathBuf, process};
use workspace::Workspace;

gpui_kit::assets::icon_assets!(MergeIconAssets, [GitMerge]);

struct AppAssets {
    components: gpui_kit::assets::Assets,
}

impl Default for AppAssets {
    fn default() -> Self {
        Self {
            components: gpui_kit::assets::Assets::new(""),
        }
    }
}

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = MergeIconAssets.load(path)? {
            return Ok(Some(bytes));
        }

        self.components.load(path)
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut paths = self.components.list(path)?;
        paths.extend(MergeIconAssets.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

fn usage(program: &str) -> String {
    format!("usage: {program} [<baseline> <local> | <base> <local> <incoming> <result>]")
}

fn load_arguments() -> Result<Vec<ComparisonPaths>, String> {
    let mut args = env::args_os();
    let program = args
        .next()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "yori".to_owned());
    let paths = args
        .map(|path| {
            std::path::absolute(&path).map_err(|error| {
                format!("cannot resolve {}: {error}", PathBuf::from(path).display())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    ComparisonPaths::from_paths(&paths)
        .map(|comparison| vec![comparison])
        .map_err(|_| usage(&program))
}

fn dispatch_open<C: AppContext>(
    window: gpui_kit::WindowHandle<Root>,
    workspace: &gpui_kit::Entity<Workspace>,
    comparisons: &[ComparisonPaths],
    cx: &mut C,
) -> Result<(), String> {
    // The typed handle also mutably borrows Root. Workspace opening must be
    // able to read/update Root itself for dialog checks and error notifications.
    let window: gpui_kit::AnyWindowHandle = window.into();
    window
        .update(cx, |_, window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.open_comparisons(comparisons, window, cx)
            })
        })
        .unwrap_or_else(|error| Err(format!("yori's window closed: {error}")))
}

fn main() {
    let comparisons = load_arguments().unwrap_or_else(|error| {
        eprintln!("yori: {error}");
        process::exit(2);
    });

    let Some(instance) = instance::Instance::start(&comparisons).unwrap_or_else(|error| {
        eprintln!("yori: {error}");
        process::exit(1);
    }) else {
        return;
    };

    gpui_kit::application()
        .with_assets(AppAssets::default())
        .run(move |cx| {
            gpui_kit::init(cx);
            appearance::init(cx);
            editor::init(cx);
            workspace::init(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            cx.spawn(async move |cx| {
                let mut workspace = None;
                let window = cx
                    .open_window(WindowOptions::default(), |window, cx| {
                        let view = cx.new(|cx| Workspace::new(window, cx));
                        workspace = Some(view.clone());
                        cx.new(|cx| Root::new(view, window, cx))
                    })
                    .expect("failed to open yori window");
                let workspace = workspace.expect("workspace initialized with its window");

                // Root is installed now, so error notifications and editor focus
                // are available before handling either initial or forwarded files.
                let initial = dispatch_open(window, &workspace, &comparisons, cx);
                if let Err(error) = initial {
                    eprintln!("yori: {error}");
                }

                while let Ok(request) = instance.next().await {
                    let result = if request.expired() {
                        Err("request expired before yori could open it; retry".into())
                    } else {
                        dispatch_open(window, &workspace, &request.comparisons, cx)
                    };

                    request.complete(result);
                }
            })
            .detach();
        });
}
