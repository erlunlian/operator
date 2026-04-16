use gpui::*;
use std::path::PathBuf;

use crate::git::PrDiffPanel;
use crate::pane::PaneGroup;
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
    /// Editor files cached when switching away from this workspace.
    pub cached_editor_files: Vec<PathBuf>,
    /// Right panel tab cached when switching away from this workspace.
    pub cached_right_panel_tab: Option<String>,
    /// Right panel width cached when switching away from this workspace.
    pub cached_right_panel_width: Option<f32>,
    /// Whether the left sidebar was collapsed when switching away.
    pub cached_sidebar_collapsed: Option<bool>,
    /// Whether the right panel was collapsed when switching away.
    pub cached_right_panel_collapsed: Option<bool>,
    /// If set, this workspace is a PR review (no terminals, no user-chosen
    /// directory) and should render its `PrDiffPanel` as the main view.
    pub pr_review: Option<PrReviewState>,
}

impl Workspace {
    /// Create a workspace with a directory already selected (has terminal layout).
    pub fn new(name: &str, directory: PathBuf, cx: &mut Context<Self>) -> Self {
        let git_branch = Self::detect_git_branch(&directory);
        let layout = cx.new(|cx| PaneGroup::new_terminal(Some(directory.clone()), cx));
        cx.observe(&layout, |_this, _layout, cx| cx.notify()).detach();
        Self {
            name: SharedString::from(name.to_string()),
            directory: Some(directory),
            git_branch,
            pane_statuses: Vec::new(),
            layout: Some(layout),
            cached_editor_files: Vec::new(),
            cached_right_panel_tab: None,
            cached_right_panel_width: None,
            cached_sidebar_collapsed: None,
            cached_right_panel_collapsed: None,
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
            cached_editor_files: Vec::new(),
            cached_right_panel_tab: None,
            cached_right_panel_width: None,
            cached_sidebar_collapsed: None,
            cached_right_panel_collapsed: None,
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
                    let short = ready.pr.short();
                    state.pr = Some(ready.pr);
                    Some(short)
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

    /// Set the directory for this workspace, creating the terminal layout.
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

}

