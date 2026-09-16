//! yori process startup and native application composition.

mod appearance;
mod comparison;
mod config;
mod editor;
mod instance;
mod storage;
mod workspace;
use comparison::ComparisonPaths;
use gpui_kit::component::{Root, WindowExt, notification::Notification};
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

fn report_startup_diagnostic<C: AppContext>(window: gpui_kit::AnyWindowHandle, cx: &mut C) {
    let _ = window.update(cx, |_, window, cx| {
        if let Some(diagnostic) = config::diagnostic(cx) {
            window.push_notification(Notification::error(diagnostic), cx);
        }
    });
}

fn dispatch_open<C: AppContext>(
    window: impl Into<gpui_kit::AnyWindowHandle>,
    workspace: &gpui_kit::Entity<Workspace>,
    comparisons: &[ComparisonPaths],
    cx: &mut C,
) -> Result<(), String> {
    let window = window.into();

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
            config::init(cx);
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

                // WindowExt updates Root internally. Release the typed Root handle
                // before reporting startup diagnostics or opening comparisons.
                let window: gpui_kit::AnyWindowHandle = window.into();

                report_startup_diagnostic(window, cx);

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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{
        Context, IntoElement, ParentElement, Render, TestAppContext, VisualContext, Window, div,
        test::TestWindowExt,
    };

    struct TestView;

    impl Render for TestView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let notifications = Root::render_notification_layer(window, cx);

            div().children(notifications)
        }
    }

    fn assert_startup_diagnostic(contents: &str, cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("yori").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();

        cx.update(|cx| {
            gpui_kit::init(cx);
            appearance::init(cx);
            config::init_for_path(path, cx);
        });

        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| TestView);
            Root::new(view, window, cx)
        });
        let window = cx.window_handle();

        report_startup_diagnostic(window, cx);

        cx.update(|window, cx| {
            window.render_frame(cx);
            let _ = window.find("notification");
        });
    }

    #[gpui_kit::test]
    fn malformed_startup_configuration_reports_without_reentrant_root_update(
        cx: &mut TestAppContext,
    ) {
        assert_startup_diagnostic("[editor\nvim_keybindings = true", cx);
    }

    #[gpui_kit::test]
    fn partially_invalid_startup_configuration_reports_without_reentrant_root_update(
        cx: &mut TestAppContext,
    ) {
        assert_startup_diagnostic(
            "[editor]\nvim_keybindings = true\nshow_whitespace = \"sometimes\"\n",
            cx,
        );
    }
}
