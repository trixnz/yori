//! A single window of independent comparison editors, using the existing component kit.

mod decision_dialog;
mod disk_dialog;
pub(crate) mod files;
mod home;
mod perforce_chooser;
mod persistence;
mod preferences_dialog;
mod tabs;
#[cfg(test)]
mod tests;

use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Root, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    dialog::{Cancel, Confirm, DialogFooter},
    notification::Notification,
    tab::{Tab, TabBar, TabVariant},
    tooltip::Tooltip,
};
use gpui_kit::{
    App, AppContext, AsyncWindowContext, Context, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyBinding, MouseButton, ParentElement, PathPromptOptions,
    Render, ScrollHandle, StatefulInteractiveElement, Styled, Subscription, TestSupportExt, Window,
    div, px,
};
#[cfg(test)]
use yori_document::Document;

use crate::comparison::{Comparison, MergePaths};
use crate::editor::{AlignedEditor, DirtyChanged, PaneDocument};
use crate::invocation::InvocationRequest;
use crate::review::{
    GitRepository, GitSourceChooser, GitSourceChooserEvent, PerforceContext, ReviewChanged,
    ReviewSession, ReviewSource,
};
use decision_dialog::{Decision, DecisionDialog, DecisionShortcut};
use home::{Home, HomeAction, HomeControl, HomeEvent, RecentCommits};
use perforce_chooser::{PerforceSourceChooser, SourceChosen};
use preferences_dialog::PreferencesDialog;
use tabs::{TabIdentity, Tabs};

pub(crate) const KEY_CONTEXT: &str = "ComparisonWorkspace";

gpui_kit::actions!(
    workspace,
    [
        OpenComparison,
        OpenMerge,
        OpenGitReview,
        Save,
        CloseComparison,
        Quit,
        NextTab,
        PreviousTab,
        ShowHome,
        Preferences
    ]
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkspaceSelection {
    Home,
    Work,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PerforceDiscovery {
    Idle,
    Loading,
}

struct ComparisonTab {
    editor: Entity<AlignedEditor>,
    _subscription: Subscription,
    files: files::Files,
    message: Option<String>,
}

enum OpenTab {
    Comparison(ComparisonTab),
    Review {
        session: Entity<ReviewSession>,
        _subscription: Subscription,
    },
}

impl OpenTab {
    fn comparison(&self) -> Option<&ComparisonTab> {
        match self {
            Self::Comparison(tab) => Some(tab),
            Self::Review { .. } => None,
        }
    }

    fn comparison_mut(&mut self) -> Option<&mut ComparisonTab> {
        match self {
            Self::Comparison(tab) => Some(tab),
            Self::Review { .. } => None,
        }
    }

    fn needs_save(&self, cx: &App) -> bool {
        match self {
            Self::Comparison(tab) => tab.editor.read(cx).needs_save(),
            Self::Review { session, .. } => session.read(cx).needs_save(cx),
        }
    }

    fn can_save(&self, cx: &App) -> bool {
        match self {
            Self::Comparison(tab) => tab.editor.read(cx).can_save(),
            Self::Review { session, .. } => session.read(cx).can_save_all(cx),
        }
    }

    fn unresolved_count(&self, cx: &App) -> usize {
        match self {
            Self::Comparison(tab) => tab.editor.read(cx).unresolved_count(),
            Self::Review { .. } => 0,
        }
    }
}

pub(super) struct Workspace {
    tabs: Tabs<OpenTab>,
    selection: WorkspaceSelection,
    home: Entity<Home>,
    _home_subscription: Subscription,
    invocation_directory: Option<std::path::PathBuf>,
    focus: FocusHandle,
    tab_scroll: ScrollHandle,
    picking_files: bool,
    perforce_discovery: PerforceDiscovery,
    #[cfg(test)]
    test_perforce_context: Option<std::sync::Arc<PerforceContext>>,
    saving: bool,
    notice_scheduled: bool,
    disk_notice: std::rc::Weak<std::cell::RefCell<disk_dialog::DiskNotice>>,
    scan: persistence::ScanState,
    disk_epoch: u64,
    disk_watch: Option<crate::storage::FileWatch>,
    watch_error: Option<String>,
    monitor: Option<gpui_kit::Task<()>>,
    git_source_chooser: Option<(Entity<GitSourceChooser>, Subscription)>,
    preferences: Option<Entity<PreferencesDialog>>,
}

impl Workspace {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            view.update(cx, |this, cx| {
                if this.saving || window.has_active_dialog(cx) {
                    return false;
                }
                if !this.has_modified_tabs(cx) {
                    crate::window_placement::persist(window, cx);
                    return true;
                }

                this.request_close(None, window, cx);
                false
            })
            .unwrap_or(true)
        });
        crate::window_placement::track(window, cx);

        let focus = cx.focus_handle();
        let home = cx.new(Home::new);
        let home_subscription = cx.subscribe_in(
            &home,
            window,
            |this, _, event: &HomeEvent, window, cx| match event {
                HomeEvent::Action(action) => this.start_home_action(*action, window, cx),
                HomeEvent::WorkingChanges => this.open_working_changes(window, cx),
                HomeEvent::Revision(revision) => {
                    this.open_git_revision(revision, window, cx);
                }
            },
        );

        let (disk_watch, monitor) = Self::start_monitor(window, cx);
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.watch_paths(cx);
                Self::reload_config(window, cx);
                this.scan_disk(cx);
                this.refresh_reviews(window, cx);
            }
        })
        .detach();

        let mut workspace = Self {
            tabs: Tabs::default(),
            selection: WorkspaceSelection::Home,
            home: home.clone(),
            _home_subscription: home_subscription,
            invocation_directory: None,
            focus,
            tab_scroll: ScrollHandle::new(),
            picking_files: false,
            perforce_discovery: PerforceDiscovery::Idle,
            #[cfg(test)]
            test_perforce_context: None,
            saving: false,
            notice_scheduled: false,
            disk_notice: std::rc::Weak::new(),
            scan: persistence::ScanState::default(),
            disk_epoch: 0,
            watch_error: disk_watch.is_none().then(|| "Live file watching is unavailable. Disk is still checked on activation and before saving.".into()),
            disk_watch,
            monitor,
            git_source_chooser: None,
            preferences: None,
        };
        workspace.watch_paths(cx);
        home.update(cx, |home, cx| home.focus(window, cx));

        workspace
    }

    fn reload_config(window: &mut Window, cx: &mut Context<Self>) {
        if let Some(diagnostic) = crate::config::reload(cx) {
            window.push_notification(Notification::error(diagnostic), cx);
        }
    }

    #[cfg(test)]
    pub(super) fn open_paths(
        &mut self,
        left: &std::path::Path,
        right: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self.open_comparison(&Comparison::diff(left.into(), right.into()), window, cx);
        if let Err(error) = result {
            window.push_notification(Notification::error(error), cx);
        }
    }

    /// Process one invocation on the UI thread, whether it started this process or was
    /// forwarded later.
    pub(super) fn handle_invocation(
        &mut self,
        invocation: &InvocationRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.invocation_directory = Some(invocation.directory.clone());
        self.refresh_home_recents(cx);

        // An invocation with nothing to compare means "show me the start screen
        // for this directory". Home lists the same commits the chooser does, so
        // opening the chooser here only hid Home behind something that looked
        // like it. The chooser stays one action away for anything older or by
        // ref.
        if invocation.comparisons.is_empty() {
            window.activate_window();
            self.show_home(&ShowHome, window, cx);

            return Ok(());
        }

        self.git_source_chooser = None;
        self.open_comparisons(&invocation.comparisons, window, cx)
    }

    /// Completion means every comparison was loaded or rejected, not just queued;
    /// temporary files can then be released.
    fn open_comparisons(
        &mut self,
        comparisons: &[Comparison],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        window.activate_window();
        if comparisons.is_empty() {
            if !self.picking_files && !window.has_active_dialog(cx) {
                self.focus_active(window, cx);
            }
            return Ok(());
        }
        if self.picking_files || window.has_active_dialog(cx) {
            return Err("yori has a dialog open; finish or cancel it, then retry".into());
        }

        let mut errors = Vec::new();
        for paths in comparisons {
            let result = self.open_comparison(paths, window, cx);
            if let Err(error) = result {
                window.push_notification(Notification::error(error.clone()), cx);
                errors.push(error);
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }

    fn open_comparison(
        &mut self,
        paths: &Comparison,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let paths = paths.resolve()?;
        let identity = TabIdentity::Comparison(paths.clone());
        if let Some(id) = self.tabs.find(&identity) {
            self.activate(id, window, cx);
            return Ok(());
        }

        // File content and overwrite protection come from the same snapshots;
        // in-memory content crosses the same editor seam without touching disk.
        let files = files::Files::load(&paths)?;
        let editor = match &paths {
            Comparison::Diff(diff) => {
                let left = PaneDocument::new(
                    diff.baseline.logical_path().to_owned(),
                    files.document(files::Role::Baseline).clone(),
                );
                let right = PaneDocument::new(
                    diff.local.logical_path().to_owned(),
                    files.document(files::Role::Local).clone(),
                );
                let editable = diff.local.editable();
                let saveable = diff.local.save_destination().is_some();

                self.deactivate(cx);

                cx.new(|cx| AlignedEditor::new_diff(left, right, editable, saveable, window, cx))
            }
            Comparison::Merge(paths) => {
                let session = yori_diff::merge::MergeSession::new(
                    files.document(files::Role::Base).clone(),
                    files.document(files::Role::Local).clone(),
                    files.document(files::Role::Incoming).clone(),
                )
                .map_err(|error| error.to_string())?;
                self.deactivate(cx);

                cx.new(|cx| AlignedEditor::new_merge(paths, session, window, cx))
            }
        };
        let subscription = cx.subscribe(&editor, |_, _, _: &DirtyChanged, cx| cx.notify());
        self.tabs.insert(
            identity,
            OpenTab::Comparison(ComparisonTab {
                editor,
                _subscription: subscription,
                files,
                message: None,
            }),
        );
        self.selection = WorkspaceSelection::Work;

        self.disk_epoch += 1;
        self.watch_paths(cx);
        self.scan_disk(cx);
        self.update_window_title(window);
        cx.notify();
        Ok(())
    }

    pub(crate) fn open_review_source(
        &mut self,
        source: ReviewSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let identity = TabIdentity::Review {
            identity: source.identity.clone(),
            label: source.label.clone(),
        };
        if let Some(id) = self.tabs.find(&identity) {
            self.activate(id, window, cx);
            return;
        }

        self.deactivate(cx);
        let session = cx.new(|cx| ReviewSession::new(source, cx));
        let subscription = cx.subscribe_in(
            &session,
            window,
            |this, changed_session, event: &ReviewChanged, window, cx| {
                let is_active = this.selection == WorkspaceSelection::Work
                    && this.git_source_chooser.is_none()
                    && this
                        .tabs
                        .active
                        .and_then(|id| this.tabs.get(id))
                        .is_some_and(|tab| {
                            matches!(
                                &tab.content,
                                OpenTab::Review { session, .. } if session == changed_session
                            )
                        });
                if is_active {
                    match event {
                        ReviewChanged::RefreshCompleted { activate: true } => {
                            changed_session
                                .update(cx, |session, cx| session.focus_active(window, cx));
                        }
                        ReviewChanged::ActiveEditorChanged {
                            transfer_focus: true,
                        } => {
                            changed_session.update(cx, |session, cx| {
                                session.focus_navigator(window, cx);
                            });
                        }
                        ReviewChanged::State
                        | ReviewChanged::RefreshCompleted { activate: false }
                        | ReviewChanged::ActiveEditorChanged {
                            transfer_focus: false,
                        } => {}
                    }
                }

                cx.notify();
            },
        );
        self.tabs.insert(
            identity,
            OpenTab::Review {
                session: session.clone(),
                _subscription: subscription,
            },
        );
        self.selection = WorkspaceSelection::Work;
        self.focus_active(window, cx);
        session.update(cx, |session, cx| session.refresh(window, cx));
        self.update_window_title(window);

        cx.notify();
    }

    fn update_window_title(&self, window: &mut Window) {
        let title = (self.selection == WorkspaceSelection::Work)
            .then(|| self.tabs.active.map(|id| self.tabs.label(id)))
            .flatten();

        let Some(title) = title.filter(|title| !title.is_empty()) else {
            window.set_window_title("yori");
            return;
        };

        window.set_window_title(&format!("yori - {title}"));
    }

    fn choose_merge(&mut self, _: &OpenMerge, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_files(true, window, cx);
    }

    fn start_home_action(
        &mut self,
        action: HomeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            HomeAction::ReviewGitChange => {
                self.choose_git_review(&OpenGitReview, window, cx);
            }
            HomeAction::ReviewPerforceChangelist => {
                self.choose_perforce_review(window, cx);
            }
            HomeAction::CompareFiles => {
                self.choose_pair(&OpenComparison, window, cx);
            }
            HomeAction::OpenThreeWayMerge => {
                self.choose_merge(&OpenMerge, window, cx);
            }
            HomeAction::Preferences => {
                self.open_preferences(&Preferences, window, cx);
            }
        }
    }

    /// Opens a revision picked on Home. Home lists commits from the same
    /// repository the chooser would, so this is the chooser's outcome without
    /// the intermediate dialog.
    fn open_git_revision(&mut self, revision: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking_files || window.has_active_dialog(cx) {
            return;
        }

        let source = self
            .review_repository()
            .and_then(|repository| repository.commit_source(revision));
        self.open_home_source(source, window, cx);
    }

    /// Opens the repository's uncommitted work, which is the chooser's first
    /// row and the most likely thing to be reviewing.
    fn open_working_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking_files || window.has_active_dialog(cx) {
            return;
        }

        let source = self
            .review_repository()
            .map(|repository| repository.working_source());
        self.open_home_source(source, window, cx);
    }

    /// Reports a failed pick the way the chooser would, rather than leaving the
    /// click looking like it did nothing.
    fn open_home_source(
        &mut self,
        source: Result<ReviewSource, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match source {
            Ok(source) => self.open_review_source(source, window, cx),
            Err(error) => window.push_notification(Notification::error(error), cx),
        }
    }

    /// Reloads the commits Home offers. Git discovery is local and cheap, so
    /// Home can show them without being asked; Perforce stays behind its action
    /// because discovery there is a server round-trip.
    fn refresh_home_recents(&mut self, cx: &mut Context<Self>) {
        let recent = self.review_repository().ok().and_then(|repository| {
            let repository_name = repository
                .work_dir()
                .file_name()
                .unwrap_or(repository.work_dir().as_os_str())
                .to_string_lossy()
                .into_owned();

            Some(RecentCommits {
                repository: repository_name,
                commits: repository.recent_commits().ok()?,
            })
        });

        self.home.update(cx, |home, cx| home.set_recent(recent, cx));
    }

    fn choose_perforce_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.perforce_discovery == PerforceDiscovery::Loading
            || self.picking_files
            || window.has_active_dialog(cx)
        {
            return;
        }

        #[cfg(test)]
        if let Some(context) = self.test_perforce_context.clone() {
            Self::open_perforce_chooser(context, window, cx);
            return;
        }

        let Some(directory) = self.perforce_discovery_directory() else {
            window.push_notification(
                Notification::error("yori has no invocation directory for Perforce discovery."),
                cx,
            );
            return;
        };

        self.deactivate(cx);
        self.perforce_discovery = PerforceDiscovery::Loading;
        cx.notify();

        let discovery = cx
            .background_executor()
            .spawn(async move { PerforceContext::discover(&directory) });
        cx.spawn_in(window, async move |view, cx| {
            let result = discovery.await;
            let _ = view.update_in(cx, |this, window, cx| {
                this.perforce_discovery = PerforceDiscovery::Idle;

                match result {
                    Ok(context) if !window.has_active_dialog(cx) => {
                        Self::open_perforce_chooser(std::sync::Arc::new(context), window, cx);
                    }
                    Ok(_) => window.push_notification(
                        Notification::error(
                            "Another dialog opened while Perforce context was loading; close it and retry.",
                        ),
                        cx,
                    ),
                    Err(error) => window.push_notification(
                        Notification::error(format!(
                            "Cannot discover Perforce context: {error}"
                        )),
                        cx,
                    ),
                }

                cx.notify();
            });
        })
        .detach();
    }

    fn perforce_discovery_directory(&self) -> Option<std::path::PathBuf> {
        self.invocation_directory.clone()
    }

    fn open_perforce_chooser(
        context: std::sync::Arc<PerforceContext>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<PerforceSourceChooser> {
        let chooser = cx.new(|cx| PerforceSourceChooser::new(context, window, cx));
        cx.subscribe_in(
            &chooser,
            window,
            |this, _, source: &SourceChosen, window, cx| {
                this.open_review_source(source.0.clone(), window, cx);
            },
        )
        .detach();

        let rendered = chooser.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Open Perforce review")
                .overlay_closable(false)
                .child(rendered.clone())
        });
        let focused = chooser.clone();
        window.defer(cx, move |window, cx| {
            focused.update(cx, |chooser, cx| chooser.focus(window, cx));
        });

        chooser
    }

    fn choose_git_review(
        &mut self,
        _: &OpenGitReview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.picking_files
            || self.perforce_discovery == PerforceDiscovery::Loading
            || window.has_active_dialog(cx)
        {
            return;
        }

        let repository = match self.review_repository() {
            Ok(repository) => repository,
            Err(error) => {
                window.push_notification(Notification::error(error), cx);
                return;
            }
        };

        if let Err(error) = self.open_git_source_chooser(repository, window, cx) {
            window.push_notification(Notification::error(error), cx);
        }
    }

    fn review_repository(&self) -> Result<GitRepository, String> {
        let invocation_repository = self
            .invocation_directory
            .as_deref()
            .map(GitRepository::discover);
        if let Some(Ok(repository)) = &invocation_repository {
            return Ok(repository.clone());
        }

        if let Some(context) = self.active_comparison_context() {
            return GitRepository::discover(&context);
        }

        match invocation_repository {
            Some(Err(error)) => Err(error),
            Some(Ok(_)) => unreachable!("successful discovery returned above"),
            None => Err("Open a local file or invoke yori from a Git repository first.".into()),
        }
    }

    fn active_comparison_context(&self) -> Option<std::path::PathBuf> {
        self.tabs
            .active
            .and_then(|id| self.tabs.get(id))
            .and_then(|tab| tab.identity.comparison())
            .and_then(|comparison| comparison.target().parent())
            .map(std::path::Path::to_owned)
    }

    fn open_git_source_chooser(
        &mut self,
        repository: GitRepository,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let commits = repository.recent_commits()?;

        self.deactivate(cx);
        let chooser = cx.new(|cx| GitSourceChooser::new(repository, commits, window, cx));
        let subscription = cx.subscribe_in(
            &chooser,
            window,
            |this, _, event: &GitSourceChooserEvent, window, cx| match event {
                GitSourceChooserEvent::Chosen(source) => {
                    let source = source.clone();
                    this.git_source_chooser = None;
                    this.open_review_source(source, window, cx);
                }
                GitSourceChooserEvent::Cancelled => {
                    this.git_source_chooser = None;
                    this.focus_active(window, cx);
                    cx.notify();
                }
            },
        );
        self.git_source_chooser = Some((chooser.clone(), subscription));
        window.defer(cx, move |window, cx| {
            chooser.update(cx, |chooser, cx| chooser.focus(window, cx));
        });
        cx.notify();

        Ok(())
    }

    fn open_preferences(&mut self, _: &Preferences, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.picking_files {
            return;
        }
        if let Some(preferences) = &self.preferences {
            preferences.read(cx).focus_handle().focus(window, cx);
            return;
        }
        if window.has_active_dialog(cx) {
            return;
        }

        let preferences = cx.new(PreferencesDialog::new);
        let content = preferences.clone();
        let applying = preferences.clone();
        let workspace = cx.weak_entity();
        let focus = preferences.read(cx).focus_handle();
        self.preferences = Some(preferences);

        window.open_dialog(cx, move |dialog, _, _| {
            let workspace = workspace.clone();
            let applying = applying.clone();
            let footer = DialogFooter::new()
                .child(Button::new("preferences-cancel").label("Cancel").on_click(
                    |_, window, cx| {
                        window.dispatch_action(Box::new(Cancel), cx);
                    },
                ))
                .child(
                    Button::new("preferences-apply")
                        .label("Apply")
                        .primary()
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(Confirm { secondary: false }), cx);
                        }),
                );

            dialog
                .title("Preferences")
                .width(px(480.0))
                .max_w(px(480.0))
                .close_button(false)
                .overlay_closable(false)
                .footer(footer)
                .on_ok(move |_, window, cx| {
                    applying.update(cx, |preferences, cx| preferences.apply(window, cx))
                })
                .on_cancel(|_, _, _| true)
                .on_close(move |_, _, cx| {
                    let _ = workspace.update(cx, |workspace, cx| {
                        workspace.preferences = None;
                        cx.notify();
                    });
                })
                .child(content.clone())
        });

        let modal_focus = window.focused(cx);
        window.defer(cx, move |window, cx| {
            if modal_focus.is_some_and(|modal| modal.is_focused(window)) {
                focus.focus(window, cx);
            }
        });
    }

    fn deactivate(&self, cx: &mut Context<Self>) {
        if self.selection != WorkspaceSelection::Work {
            return;
        }

        let Some(tab) = self.tabs.active.and_then(|id| self.tabs.get(id)) else {
            return;
        };

        match &tab.content {
            OpenTab::Comparison(tab) => tab.editor.update(cx, AlignedEditor::deactivate),
            OpenTab::Review { session, .. } => {
                session.update(cx, |session, cx| session.deactivate(cx));
            }
        }
    }

    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((chooser, _)) = &self.git_source_chooser {
            chooser.update(cx, |chooser, cx| chooser.focus(window, cx));
            return;
        }

        if self.selection == WorkspaceSelection::Home {
            self.home.update(cx, |home, cx| home.focus(window, cx));
            return;
        }

        let Some(tab) = self.tabs.active.and_then(|id| self.tabs.get(id)) else {
            self.focus.focus(window, cx);
            return;
        };

        match &tab.content {
            OpenTab::Comparison(tab) => tab.editor.focus_handle(cx).focus(window, cx),
            OpenTab::Review { session, .. } => {
                session.update(cx, |session, cx| session.focus_active(window, cx));
            }
        }
    }

    fn show_home(&mut self, _: &ShowHome, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking_files || window.has_active_dialog(cx) {
            return;
        }

        self.deactivate(cx);
        self.git_source_chooser = None;
        self.selection = WorkspaceSelection::Home;
        self.update_window_title(window);
        self.refresh_home_recents(cx);

        self.focus_active(window, cx);
        cx.notify();
    }

    fn refresh_reviews(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sessions = self
            .tabs
            .entries
            .iter()
            .filter_map(|tab| match &tab.content {
                OpenTab::Review { session, .. } => Some(session.clone()),
                OpenTab::Comparison(_) => None,
            })
            .collect::<Vec<_>>();

        for session in sessions {
            session.update(cx, |session, cx| session.refresh(window, cx));
        }
    }

    fn activate(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking_files || window.has_active_dialog(cx) {
            return;
        }

        self.deactivate(cx);
        self.git_source_chooser = None;
        self.tabs.activate(id);
        self.selection = WorkspaceSelection::Work;
        self.update_window_title(window);

        self.focus_active(window, cx);
        cx.notify();
    }

    fn cycle(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection == WorkspaceSelection::Home {
            if let Some(id) = self.tabs.active {
                self.activate(id, window, cx);
            }
            return;
        }

        let Some(index) = self
            .tabs
            .entries
            .iter()
            .position(|tab| Some(tab.id) == self.tabs.active)
        else {
            return;
        };

        let count = self.tabs.entries.len();
        let next = if backwards {
            (index + count - 1) % count
        } else {
            (index + 1) % count
        };
        self.activate(self.tabs.entries[next].id, window, cx);
    }

    fn has_modified_tabs(&self, cx: &App) -> bool {
        self.tabs
            .requires_discard_confirmation(None, |tab| tab.needs_save(cx))
    }

    fn close(&mut self, target: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = target else {
            crate::window_placement::persist(window, cx);
            window.defer(cx, |window, _| window.remove_window());
            return;
        };

        if self.selection == WorkspaceSelection::Work && self.tabs.active == Some(id) {
            self.deactivate(cx);
        }
        self.tabs.remove(id);
        if self.tabs.entries.is_empty() {
            self.selection = WorkspaceSelection::Home;
        }
        self.update_window_title(window);
        self.disk_epoch += 1;
        self.watch_paths(cx);

        self.focus_active(window, cx);
        cx.notify();
    }

    fn request_close(
        &mut self,
        target: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.saving || window.has_active_dialog(cx) {
            return;
        }

        let modified = self
            .tabs
            .requires_discard_confirmation(target, |tab| tab.needs_save(cx));
        if !modified {
            self.close(target, window, cx);
            return;
        }

        self.deactivate(cx);
        self.focus_active(window, cx);

        let saveable = self.tabs.entries.iter().all(|tab| {
            target.is_some_and(|id| tab.id != id)
                || !tab.content.needs_save(cx)
                || tab.content.can_save(cx)
        });
        let title = if saveable {
            target.map_or_else(
                || "Save changes before closing yori?".to_owned(),
                |id| format!("Save changes to {}?", self.tabs.label(id)),
            )
        } else {
            target.map_or_else(
                || "Discard unsaved changes before closing yori?".to_owned(),
                |id| format!("Discard changes to {}?", self.tabs.label(id)),
            )
        };
        let unresolved = self.tabs.entries.iter().any(|tab| {
            target.is_none_or(|id| tab.id == id) && tab.content.unresolved_count(cx) != 0
        });
        let aggregate = target.is_none_or(|id| {
            self.tabs
                .get(id)
                .is_some_and(|tab| matches!(&tab.content, OpenTab::Review { .. }))
        });
        let detail = if !saveable {
            "At least one changed document has no save destination. Discard the changes or keep the workspace open."
        } else if unresolved {
            "There are unresolved conflicts. Resolve them before saving, or discard this session."
        } else if aggregate {
            "Your changes have not been saved. Save all, discard all, or keep the workspace open."
        } else {
            "Your changes have not been saved. Save them, discard them, or keep the workspace open."
        };
        let view = cx.weak_entity();
        let discard_view = view.clone();
        let save_view = view.clone();
        let cancel = Decision::new("cancel", "Cancel", DecisionShortcut::Escape);
        let discard = Decision::new(
            "ok",
            if aggregate { "Discard all" } else { "Discard" },
            DecisionShortcut::Mnemonic('d'),
        )
        .on_activate(move |window, cx| {
            // Restore modal focus before disposing the editor it belonged to.
            let _ = discard_view.update(cx, |this, cx| this.close(target, window, cx));
        });
        let dialog = DecisionDialog::new(title, detail, cancel);
        if saveable {
            let save = Decision::new(
                "save-and-close",
                if aggregate { "Save all" } else { "Save" },
                DecisionShortcut::Enter,
            )
            .primary()
            .disabled(unresolved)
            .on_activate(move |window, cx| {
                let _ = save_view.update(cx, |this, cx| {
                    this.save_before_close(target, window, cx);
                });
            });

            dialog.alternate(discard).primary(save).open(window, cx);
        } else {
            dialog.alternate(discard).open(window, cx);
        }
    }

    fn choose_pair(&mut self, _: &OpenComparison, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_files(false, window, cx);
    }

    fn choose_files(&mut self, merging: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking_files
            || self.perforce_discovery == PerforceDiscovery::Loading
            || window.has_active_dialog(cx)
        {
            return;
        }

        self.deactivate(cx);
        self.git_source_chooser = None;
        self.picking_files = true;
        cx.notify();

        cx.spawn_in(window, async move |view, cx| {
            let result = choose_paths(cx, merging).await;
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.picking_files = false;
                    match result {
                        Ok(Some(paths)) => {
                            let _ = this.open_comparisons(&[paths], window, cx);
                        }
                        Ok(None) => this.focus_active(window, cx),
                        Err(error) => window.push_notification(Notification::error(error), cx),
                    }

                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn render_open_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let opening = self.picking_files || self.perforce_discovery == PerforceDiscovery::Loading;

        div()
            .px(px(5.0))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(2.0))
            .child(
                Button::new("open-git-review")
                    .icon(gpui_kit::assets::IconName::GitPullRequest)
                    .ghost()
                    .with_size(px(28.0))
                    .accessibility_label("Open Git review")
                    .tooltip("Open Git review (Ctrl+Shift+G)")
                    .disabled(opening)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.choose_git_review(&OpenGitReview, window, cx);
                    })),
            )
            .child(
                Button::new("open-merge")
                    .icon(gpui_kit::assets::IconName::GitMerge)
                    .ghost()
                    .with_size(px(20.0))
                    .size(px(28.0))
                    .accessibility_label("Open merge")
                    .tooltip("Open three-way merge (Ctrl+Shift+M)")
                    .disabled(opening)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.choose_merge(&OpenMerge, window, cx);
                    })),
            )
            .child(
                Button::new("open-perforce-review")
                    .icon(gpui_kit::assets::IconName::GitPullRequest)
                    .ghost()
                    .with_size(px(28.0))
                    .accessibility_label("Open Perforce review")
                    .tooltip("Open Perforce review")
                    .disabled(opening)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.choose_perforce_review(window, cx);
                    })),
            )
            .child(
                div()
                    .mx(px(1.0))
                    .h(px(16.0))
                    .w(px(1.0))
                    .bg(cx.theme().border),
            )
            .child(
                Button::new("open-comparison")
                    .icon(IconName::Plus)
                    .ghost()
                    .with_size(px(28.0))
                    .accessibility_label("Open comparison")
                    .tooltip("Open comparison (Ctrl+O)")
                    .disabled(opening)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.choose_pair(&OpenComparison, window, cx);
                    })),
            )
    }

    fn render_tab(&self, tab: &tabs::Tab<OpenTab>, cx: &mut Context<Self>) -> Tab {
        let id = tab.id;
        let label = self.tabs.label(id);
        let description = tab.identity.description();
        let modified = tab.content.needs_save(cx);
        let accessible = format!(
            "{label}{}; {description}",
            if modified { "; modified" } else { "" }
        );

        Tab::new()
            .label(label)
            .aria_label(accessible)
            .prefix(
                div()
                    .id(("comparison-paths", id))
                    .pl(px(10.0))
                    .tooltip(move |window, cx| Tooltip::new(description.clone()).build(window, cx))
                    .child(
                        Icon::new(match &tab.identity {
                            TabIdentity::Comparison(_) => gpui_kit::assets::IconName::FileText,
                            TabIdentity::Review { .. } => {
                                gpui_kit::assets::IconName::GitPullRequest
                            }
                        })
                        .with_size(px(14.0)),
                    ),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.request_close(Some(id), window, cx);
                }),
            )
            .suffix(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().size(px(6.0)).rounded_full().bg(if modified {
                        cx.theme().foreground
                    } else {
                        cx.theme().transparent
                    }))
                    .child(
                        div()
                            .id(("tab-close-target", id))
                            .test_support()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                Button::new(("close-comparison", id))
                                    .icon(IconName::Close)
                                    .ghost()
                                    .with_size(px(22.0))
                                    .accessibility_label("Close comparison")
                                    .tooltip("Close comparison (Ctrl+W)")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.request_close(Some(id), window, cx);
                                    })),
                            ),
                    ),
            )
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> TabBar {
        let selected = (self.selection == WorkspaceSelection::Work)
            .then(|| {
                self.tabs
                    .entries
                    .iter()
                    .position(|tab| Some(tab.id) == self.tabs.active)
            })
            .flatten();
        let tabs = self
            .tabs
            .entries
            .iter()
            .map(|tab| self.render_tab(tab, cx))
            .collect::<Vec<_>>();
        let ids = self
            .tabs
            .entries
            .iter()
            .map(|tab| tab.id)
            .collect::<Vec<_>>();
        let mut bar = TabBar::new("comparisons")
            .with_variant(TabVariant::Tab)
            .with_size(px(38.0))
            .track_scroll(&self.tab_scroll)
            .max_width(px(260.0))
            .prefix(
                HomeControl::new(self.selection == WorkspaceSelection::Home).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.show_home(&ShowHome, window, cx);
                    },
                )),
            )
            .suffix(self.render_open_controls(cx))
            .children(tabs)
            .on_click(cx.listener(move |this, index: &usize, window, cx| {
                if let Some(id) = ids.get(*index) {
                    this.activate(*id, window, cx);
                }
            }));

        if let Some(index) = selected {
            bar = bar.selected_index(index);
        }

        bar
    }

    fn render_body(&self) -> impl IntoElement {
        if let Some((chooser, _)) = &self.git_source_chooser {
            div().size_full().child(chooser.clone())
        } else if self.selection == WorkspaceSelection::Home {
            div().size_full().child(self.home.clone())
        } else if let Some(tab) = self.tabs.active.and_then(|id| self.tabs.get(id)) {
            match &tab.content {
                OpenTab::Comparison(tab) => div().size_full().child(tab.editor.clone()),
                OpenTab::Review { session, .. } => div().size_full().child(session.clone()),
            }
        } else {
            div().size_full().child(self.home.clone())
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.schedule_disk_notices(window, cx);

        let bar = self.render_tab_bar(cx);
        let body = self.render_body();
        let dialogs = Root::render_dialog_layer(window, cx);
        let notifications = Root::render_notification_layer(window, cx);

        div()
            .id("workspace")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .font_family(cx.theme().font_family.clone())
            .text_size(px(13.0))
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::choose_pair))
            .on_action(cx.listener(Self::choose_merge))
            .on_action(cx.listener(Self::choose_git_review))
            .on_action(cx.listener(Self::open_preferences))
            .on_action(cx.listener(Self::show_home))
            .on_action(cx.listener(Self::save_active))
            .on_action(cx.listener(|this, _: &CloseComparison, window, cx| {
                if this.selection != WorkspaceSelection::Work {
                    return;
                }

                if let Some(id) = this.tabs.active {
                    this.request_close(Some(id), window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &Quit, window, cx| this.request_close(None, window, cx)),
            )
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle(false, window, cx)))
            .on_action(
                cx.listener(|this, _: &PreviousTab, window, cx| this.cycle(true, window, cx)),
            )
            .child(
                div()
                    .h(px(38.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .bg(cx.theme().tab_bar)
                    .child(div().flex_1().min_w_0().overflow_hidden().child(bar)),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(body),
            )
            .children(dialogs)
            .children(notifications)
    }
}

async fn choose_file(
    cx: &mut AsyncWindowContext,
    prompt: &'static str,
) -> Result<Option<std::path::PathBuf>, String> {
    let request = cx
        .update(|_, cx| {
            cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(prompt.into()),
            })
        })
        .map_err(|error| error.to_string())?;
    let result = request
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;

    Ok(result.and_then(|paths| paths.into_iter().next()))
}

async fn choose_paths(
    cx: &mut AsyncWindowContext,
    merging: bool,
) -> Result<Option<Comparison>, String> {
    let Some(base) = choose_file(
        cx,
        if merging {
            "Select common ancestor (BASE)"
        } else {
            "Select baseline file"
        },
    )
    .await?
    else {
        return Ok(None);
    };
    let Some(local) = choose_file(cx, "Select local file").await? else {
        return Ok(None);
    };
    if !merging {
        return Ok(Some(Comparison::diff(base, local)));
    }

    let Some(incoming) = choose_file(cx, "Select incoming file").await? else {
        return Ok(None);
    };
    let directory = local.parent().unwrap_or_else(|| std::path::Path::new("."));
    let name = local
        .file_name()
        .unwrap_or(local.as_os_str())
        .to_string_lossy();
    let request = cx
        .update(|_, cx| cx.prompt_for_new_path(directory, Some(&name)))
        .map_err(|error| error.to_string())?;
    let result = request
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;

    Ok(result.map(|result| {
        Comparison::Merge(MergePaths {
            base,
            local,
            incoming,
            result,
        })
    }))
}

pub(crate) fn fixed_key_bindings() -> Vec<KeyBinding> {
    let mut bindings = home::fixed_key_bindings();
    bindings.extend(perforce_chooser::fixed_key_bindings());
    bindings
}

pub(super) fn init(cx: &mut App) {
    crate::keymap::init(cx);
}
