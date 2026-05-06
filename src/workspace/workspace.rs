use gpui::*;
use std::path::PathBuf;

use crate::editor::EditorView;
use crate::git::{GitDiffPanel, PrDiffPanel};
use crate::pane::PaneGroup;
use crate::right_panel::RightPanelTab;
use crate::terminal::terminal::DetectedClaudeStatus;
use crate::workspace::pr_review::{self, PrReviewState, PrReviewStatus};

#[derive(Clone, Debug, PartialEq)]
pub enum ClaudeStatus {
    Idle,
    WaitingForInput,
    Working,
}

impl ClaudeStatus {
    pub fn label(&self) -> &str {
        match self {
            ClaudeStatus::Idle => "",
            ClaudeStatus::WaitingForInput => "Waiting for your response",
            ClaudeStatus::Working => "Claude is working...",
        }
    }

    pub fn from_detected(detected: &DetectedClaudeStatus) -> Self {
        match detected {
            DetectedClaudeStatus::NotRunning => ClaudeStatus::Idle,
            DetectedClaudeStatus::WaitingForInput => ClaudeStatus::WaitingForInput,
            DetectedClaudeStatus::Working => ClaudeStatus::Working,
        }
    }
}

pub struct Workspace {
    pub name: SharedString,
    pub directory: Option<PathBuf>,
    pub git_branch: Option<String>,
    /// Per-pane Claude statuses (only non-idle entries are stored).
    pub pane_statuses: Vec<ClaudeStatus>,
    pub layout: Option<Entity<PaneGroup>>,
    /// The right-panel editor for this workspace. Lazily created on first
    /// file open; persists across workspace switches so tabs, scroll, and
    /// undo state survive without reload.
    pub editor: Option<Entity<EditorView>>,
    /// Git status diff panel for this workspace. Some when a directory is set.
    pub git_diff_panel: Option<Entity<GitDiffPanel>>,
    /// Pull-request diff panel for this workspace. Some when a directory is set.
    pub pr_diff_panel: Option<Entity<PrDiffPanel>>,
    pub right_panel_tab: RightPanelTab,
    pub right_panel_width: f32,
    pub right_panel_collapsed: bool,
    pub sidebar_collapsed: bool,
    /// Background task watching the working tree for FS changes and refreshing
    /// the GitDiffPanel. Held to keep the watcher alive — dropped (cancelled)
    /// when the workspace is removed or its directory changes.
    _diff_watcher: Option<Task<()>>,
    /// If set, this workspace is a PR review (no terminals, no user-chosen
    /// directory) and should render its `PrDiffPanel` as the main view.
    pub pr_review: Option<PrReviewState>,
}

const DEFAULT_RIGHT_PANEL_WIDTH: f32 = 400.0;

impl Workspace {
    /// Create a workspace with a directory already selected (has terminal layout).
    pub fn new(name: &str, directory: PathBuf, cx: &mut Context<Self>) -> Self {
        let git_branch = Self::detect_git_branch(&directory);
        let layout = cx.new(|cx| PaneGroup::new_terminal(Some(directory.clone()), cx));
        cx.observe(&layout, |_this, _layout, cx| cx.notify()).detach();

        let git_diff_panel = cx.new(|cx| GitDiffPanel::new(directory.clone(), cx));
        cx.observe(&git_diff_panel, |_this, _p, cx| cx.notify()).detach();
        let pr_diff_panel = cx.new(|cx| PrDiffPanel::new(directory.clone(), cx));
        cx.observe(&pr_diff_panel, |_this, _p, cx| cx.notify()).detach();
        let diff_watcher = Self::start_diff_watcher(git_diff_panel.clone(), cx);

        Self {
            name: SharedString::from(name.to_string()),
            directory: Some(directory),
            git_branch,
            pane_statuses: Vec::new(),
            layout: Some(layout),
            editor: None,
            git_diff_panel: Some(git_diff_panel),
            pr_diff_panel: Some(pr_diff_panel),
            right_panel_tab: RightPanelTab::Git,
            right_panel_width: DEFAULT_RIGHT_PANEL_WIDTH,
            right_panel_collapsed: false,
            sidebar_collapsed: false,
            _diff_watcher: diff_watcher,
            pr_review: None,
        }
    }

    /// Create a workspace without a directory — shows the welcome/picker screen.
    pub fn new_empty(cx: &mut Context<Self>) -> Self {
        let _ = cx;
        Self {
            name: SharedString::from("New Workspace".to_string()),
            directory: None,
            git_branch: None,
            pane_statuses: Vec::new(),
            layout: None,
            editor: None,
            git_diff_panel: None,
            pr_diff_panel: None,
            right_panel_tab: RightPanelTab::Git,
            right_panel_width: DEFAULT_RIGHT_PANEL_WIDTH,
            right_panel_collapsed: false,
            sidebar_collapsed: false,
            _diff_watcher: None,
            pr_review: None,
        }
    }

    /// Turn this workspace into a PR review workspace and kick off the
    /// background clone+checkout task. Safe to call on an empty workspace,
    /// an existing PR review (to switch URL), or after a refresh.
    pub fn init_pr_review(&mut self, url: String, cx: &mut Context<Self>) {
        // Reuse the existing panel entity when switching URLs / refreshing so
        // the user's split-view preference and observers survive.
        let panel = match self.pr_review.take() {
            Some(existing) => {
                existing.panel.update(cx, |p, cx| {
                    let split = p.is_split_view();
                    *p = PrDiffPanel::empty(cx);
                    p.set_split_view(split);
                    cx.notify();
                });
                existing.panel
            }
            None => {
                let panel = cx.new(|cx| PrDiffPanel::empty(cx));
                cx.observe(&panel, |_this, _p, cx| cx.notify()).detach();
                panel
            }
        };

        let preview = pr_review::parse_pr_url(&url).map(|p| p.short());
        self.name = SharedString::from(
            preview
                .clone()
                .unwrap_or_else(|| "PR Review".to_string()),
        );
        self.directory = None;
        self.git_branch = None;
        self.layout = None;
        // PR review workspaces don't use the right panel — drop any state.
        self.editor = None;
        self.git_diff_panel = None;
        self.pr_diff_panel = None;
        self._diff_watcher = None;
        self.pr_review = Some(PrReviewState {
            url: url.clone(),
            panel,
            status: PrReviewStatus::Loading("Preparing clone…".into()),
            work_dir: None,
            pr: None,
            title: None,
        });
        cx.notify();

        let url_for_task = url;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { pr_review::setup_pr_blocking(&url_for_task) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |ws, cx| {
                    ws.finalize_pr_review(result, cx);
                });
            });
        })
        .detach();
    }

    /// Re-run the setup pipeline for the current PR (fetch + re-checkout).
    /// No-op if this workspace isn't a PR review or is already loading.
    pub fn refresh_pr_review(&mut self, cx: &mut Context<Self>) {
        let url = match self.pr_review.as_ref() {
            Some(s) if !matches!(s.status, PrReviewStatus::Loading(_)) => s.url.clone(),
            _ => return,
        };
        self.init_pr_review(url, cx);
    }

    /// Apply the background setup result to this workspace's PR review state.
    fn finalize_pr_review(
        &mut self,
        result: Result<pr_review::PrReviewReady, String>,
        cx: &mut Context<Self>,
    ) {
        let new_name: Option<String> = {
            let Some(state) = self.pr_review.as_mut() else {
                return;
            };
            match result {
                Ok(ready) => {
                    let work_dir = ready.work_dir.clone();
                    state.panel.update(cx, |panel, cx| {
                        let split = panel.is_split_view();
                        *panel = PrDiffPanel::new(work_dir.clone(), cx);
                        panel.set_split_view(split);
                        cx.notify();
                    });
                    state.status = PrReviewStatus::Ready;
                    state.work_dir = Some(work_dir);
                    state.title =
                        (!ready.title.is_empty()).then_some(ready.title.clone());
                    let display_name = if ready.title.is_empty() {
                        ready.pr.short()
                    } else {
                        ready.title.clone()
                    };
                    state.pr = Some(ready.pr);
                    Some(display_name)
                }
                Err(msg) => {
                    state.status = PrReviewStatus::Error(msg);
                    None
                }
            }
        };
        if let Some(name) = new_name {
            self.name = SharedString::from(name);
        }
        cx.notify();
    }

    /// Set the directory for this workspace, creating the terminal layout
    /// and the diff panels.
    pub fn set_directory(&mut self, directory: PathBuf, cx: &mut Context<Self>) {
        let dir_name = directory
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Workspace".to_string());
        self.name = SharedString::from(dir_name);
        self.git_branch = Self::detect_git_branch(&directory);

        let layout = cx.new(|cx| PaneGroup::new_terminal(Some(directory.clone()), cx));
        cx.observe(&layout, |_this, _layout, cx| cx.notify()).detach();
        self.layout = Some(layout);

        // Replace any existing diff panels for the new directory. The
        // editor stays — it's tied to the workspace, not the directory,
        // and a workspace's directory usually doesn't change after creation.
        let split_git = self
            .git_diff_panel
            .as_ref()
            .map(|p| p.read(cx).is_split_view())
            .unwrap_or(false);
        let git_diff_panel = cx.new(|cx| {
            let mut panel = GitDiffPanel::new(directory.clone(), cx);
            panel.set_split_view(split_git);
            panel
        });
        cx.observe(&git_diff_panel, |_this, _p, cx| cx.notify()).detach();

        let split_pr = self
            .pr_diff_panel
            .as_ref()
            .map(|p| p.read(cx).is_split_view())
            .unwrap_or(false);
        let pr_diff_panel = cx.new(|cx| {
            let mut panel = PrDiffPanel::new(directory.clone(), cx);
            panel.set_split_view(split_pr);
            panel
        });
        cx.observe(&pr_diff_panel, |_this, _p, cx| cx.notify()).detach();

        self._diff_watcher = Self::start_diff_watcher(git_diff_panel.clone(), cx);
        self.git_diff_panel = Some(git_diff_panel);
        self.pr_diff_panel = Some(pr_diff_panel);

        self.directory = Some(directory);
        cx.notify();
    }

    pub fn has_directory(&self) -> bool {
        self.directory.is_some()
    }

    /// True for a freshly-created workspace that has no directory and no PR
    /// review in progress — i.e. the welcome screen is being shown.
    pub fn is_empty_welcome(&self) -> bool {
        self.directory.is_none() && self.pr_review.is_none()
    }

    pub fn is_pr_review(&self) -> bool {
        self.pr_review.is_some()
    }

    /// Open a file in this workspace's editor, creating the editor lazily.
    /// `line` optionally navigates to a specific line after opening.
    pub fn open_file_in_editor(
        &mut self,
        path: PathBuf,
        line: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let Some(dir) = self.directory.clone() else {
            return;
        };
        if self.editor.is_none() {
            let editor = cx.new(|cx| EditorView::new(dir, cx));
            cx.observe(&editor, |_this, _e, cx| cx.notify()).detach();
            self.editor = Some(editor);
        }
        if let Some(editor) = &self.editor {
            editor.update(cx, |view, cx| {
                view.open_file(path, cx);
                if let Some(line) = line {
                    view.navigate_to_line(line, None, cx);
                }
            });
        }
        self.right_panel_tab = RightPanelTab::Files;
        cx.notify();
    }

    /// Notify the editor's active viewer that it needs to re-measure (e.g.
    /// after the right panel was resized).
    pub fn notify_active_viewer(&self, cx: &mut Context<Self>) {
        if let Some(editor) = &self.editor {
            let editor = editor.read(cx);
            if let Some(viewer) = editor.pane_group.read(cx).active_viewer(cx) {
                viewer.update(cx, |_, cx| cx.notify());
            }
        }
    }

    fn detect_git_branch(dir: &PathBuf) -> Option<String> {
        git2::Repository::discover(dir)
            .ok()
            .and_then(|repo| {
                repo.head()
                    .ok()
                    .and_then(|head| head.shorthand().map(|s| s.to_string()))
            })
    }

    fn with_layout(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut PaneGroup, &mut Context<PaneGroup>),
    ) {
        if let Some(layout) = &self.layout {
            layout.update(cx, |pg, cx| {
                f(pg, cx);
                cx.notify();
            });
            cx.notify();
        }
    }

    pub fn refresh_claude_status(&mut self, is_active_workspace: bool, cx: &App) {
        if let Some(layout) = &self.layout {
            let layout = layout.read(cx);
            self.pane_statuses = layout.collect_pane_statuses(is_active_workspace, cx);
        }
        // Keep git branch fresh (detects checkout/switch in terminal)
        if let Some(dir) = &self.directory {
            self.git_branch = Self::detect_git_branch(dir);
        }
    }

    pub fn add_tab(&mut self, cx: &mut Context<Self>) {
        self.with_layout(cx, |pg, cx| {
            pg.add_tab_to_focused(cx);
        });
    }

    pub fn total_tab_count(&self, cx: &App) -> usize {
        if let Some(layout) = &self.layout {
            let pg = layout.read(cx);
            crate::pane::pane_group::count_total_tabs(&pg.root)
        } else {
            0
        }
    }

    pub fn close_tab(&mut self, cx: &mut Context<Self>) {
        self.with_layout(cx, |pg, cx| { pg.close_focused_tab(cx); });
    }

    pub fn next_tab(&mut self, cx: &mut Context<Self>) {
        self.with_layout(cx, |pg, _cx| pg.next_tab_in_focused());
    }

    pub fn prev_tab(&mut self, cx: &mut Context<Self>) {
        self.with_layout(cx, |pg, _cx| pg.prev_tab_in_focused());
    }

    pub fn split_active_pane(&mut self, cx: &mut Context<Self>) {
        self.with_layout(cx, |pg, cx| pg.split_right(cx));
    }

    pub fn split_active_pane_vertical(&mut self, cx: &mut Context<Self>) {
        self.with_layout(cx, |pg, cx| {
            pg.split(crate::pane::pane_group::SplitAxis::Vertical, cx);
        });
    }

    fn start_diff_watcher(
        diff_panel: Entity<GitDiffPanel>,
        cx: &mut Context<Self>,
    ) -> Option<Task<()>> {
        let workdir = diff_panel.read(cx).workdir()?;

        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = match notify::recommended_watcher(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    use notify::EventKind;
                    match event.kind {
                        EventKind::Create(_)
                        | EventKind::Modify(_)
                        | EventKind::Remove(_) => {
                            if event.paths.iter().all(|p| should_ignore_fs_path(p)) {
                                return;
                            }
                            let _ = tx.send(());
                        }
                        _ => {}
                    }
                }
            },
        ) {
            Ok(w) => w,
            Err(_) => return None,
        };

        use notify::Watcher;
        let _ = watcher.watch(&workdir, notify::RecursiveMode::Recursive);

        let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
        let task = cx.spawn(async move |_ws, cx| {
            let _watcher = watcher;
            loop {
                let rx = rx.clone();
                let got_event = cx
                    .background_executor()
                    .spawn(async move {
                        let rx = rx.lock().unwrap();
                        if rx.recv().is_err() {
                            return false;
                        }
                        while rx.try_recv().is_ok() {}
                        true
                    })
                    .await;

                if !got_event {
                    break;
                }

                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;

                let ok = cx.update(|cx| {
                    diff_panel.update(cx, |panel, cx| {
                        panel.refresh_async(cx);
                    })
                });
                if ok.is_err() {
                    break;
                }
            }
        });
        Some(task)
    }
}

/// Returns true if a filesystem event for `path` should be ignored by the
/// diff watcher. Excludes build artifacts, dependency caches, git-internal
/// object/pack churn, and editor swap files. Hot path: a `cargo build` can
/// fire thousands of FS events per second, so this avoids string allocation.
fn should_ignore_fs_path(path: &std::path::Path) -> bool {
    use std::ffi::OsStr;
    use std::path::Component;

    // Top-level / nested "ignored directory" check via OsStr comparison —
    // byte-level on Unix, no UTF-8 round-trip through `to_string_lossy`.
    let mut prev_was_dot_git = false;
    for component in path.components() {
        if let Component::Normal(name) = component {
            if name == OsStr::new("target")
                || name == OsStr::new("node_modules")
                || name == OsStr::new(".next")
                || name == OsStr::new(".turbo")
                || name == OsStr::new("dist")
                || name == OsStr::new("build")
                || name == OsStr::new(".cache")
                || name == OsStr::new(".DS_Store")
            {
                return true;
            }
            // Inside `.git/`, only index/HEAD/refs writes affect our diff;
            // pack/object/lfs/logs churn doesn't.
            if prev_was_dot_git
                && (name == OsStr::new("objects")
                    || name == OsStr::new("lfs")
                    || name == OsStr::new("logs"))
            {
                return true;
            }
            prev_was_dot_git = name == OsStr::new(".git");
        } else {
            prev_was_dot_git = false;
        }
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if name.ends_with('~')
            || name.ends_with(".swp")
            || name.ends_with(".swx")
            || name.starts_with(".#")
            || (name.starts_with('#') && name.ends_with('#'))
        {
            return true;
        }
    }
    false
}
