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
            ClaudeStatus::WaitingForInput => "Claude is waiting for your input",
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
    pub claude_status: ClaudeStatus,
    pub layout: Option<Entity<PaneGroup>>,
}

impl Workspace {
    /// Create a workspace with a directory already selected (has terminal layout).
    pub fn new(name: &str, directory: PathBuf, cx: &mut Context<Self>) -> Self {
        let git_branch = Self::detect_git_branch(&directory);
        let layout = cx.new(|cx| PaneGroup::new_terminal(cx));
        cx.observe(&layout, |_this, _layout, cx| cx.notify()).detach();
        Self {
            name: SharedString::from(name.to_string()),
            directory: Some(directory),
            git_branch,
            claude_status: ClaudeStatus::Idle,
            layout: Some(layout),
        }
    }

    /// Create a workspace without a directory — shows the welcome/picker screen.
    pub fn new_empty(cx: &mut Context<Self>) -> Self {
        let _ = cx;
        Self {
            name: SharedString::from("New Workspace".to_string()),
            directory: None,
            git_branch: None,
            claude_status: ClaudeStatus::Idle,
            layout: None,
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
        let layout = cx.new(|cx| PaneGroup::new_terminal(cx));
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

    pub fn refresh_claude_status(&mut self, cx: &App) {
        if let Some(layout) = &self.layout {
            let layout = layout.read(cx);
            self.claude_status = layout.get_claude_status(cx);
        }
    }

    pub fn add_tab(&mut self, cx: &mut Context<Self>) {
        self.with_layout(cx, |pg, cx| pg.root.add_tab(cx));
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

    pub fn close_tab_at(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.with_layout(cx, |pg, _cx| pg.root.close_tab_at(ix));
    }

    pub fn set_active_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.with_layout(cx, |pg, _cx| pg.root.set_active_tab(ix));
    }

    pub fn next_tab(&mut self, cx: &mut Context<Self>) {
        self.with_layout(cx, |pg, _cx| pg.root.next_tab());
    }

    pub fn prev_tab(&mut self, cx: &mut Context<Self>) {
        self.with_layout(cx, |pg, _cx| pg.root.prev_tab());
    }

    pub fn split_active_pane(&mut self, cx: &mut Context<Self>) {
        self.with_layout(cx, |pg, cx| pg.split_right(cx));
    }

    pub fn split_active_pane_vertical(&mut self, cx: &mut Context<Self>) {
        self.with_layout(cx, |pg, cx| {
            pg.split(crate::pane::pane_group::SplitAxis::Vertical, cx);
        });
    }

    pub fn reorder_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        self.with_layout(cx, |pg, _cx| pg.root.reorder_tab(from, to));
    }

    pub fn add_editor_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(dir) = self.directory.clone() {
            self.with_layout(cx, |pg, cx| pg.root.add_editor_tab(dir, cx));
        }
    }
}

