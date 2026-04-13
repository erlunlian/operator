use gpui::*;
use std::path::PathBuf;

use crate::pane::PaneGroup;
use crate::terminal::terminal::DetectedClaudeStatus;

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
        }
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

    fn detect_git_branch(dir: &PathBuf) -> Option<String> {
        git2::Repository::discover(dir)
            .ok()
            .and_then(|repo| {
                repo.head()
                    .ok()
                    .and_then(|head| head.shorthand().map(|s| s.to_string()))
            })
    }

    pub fn short_dir(&self) -> String {
        match &self.directory {
            Some(dir) => crate::util::short_path(dir),
            None => "No directory".to_string(),
        }
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

