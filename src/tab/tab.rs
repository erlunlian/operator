use gpui::*;
use std::path::PathBuf;

use crate::editor::FileViewer;
use crate::tab::tab_bar::TabIcon;
use crate::terminal::TerminalModel;
use crate::terminal::terminal_view::TerminalView;
use crate::theme::colors;
use crate::util;
use crate::workspace::workspace::ClaudeStatus;

pub enum TabContent {
    Terminal(Entity<TerminalView>),
    File {
        path: PathBuf,
        viewer: Entity<FileViewer>,
    },
}

pub struct Tab {
    pub title: SharedString,
    pub content: TabContent,
}

impl Tab {
    pub fn new(title: &str, work_dir: Option<PathBuf>, cx: &mut App) -> Self {
        let terminal = cx.new(|cx| TerminalModel::new(work_dir, cx));
        let terminal_view = cx.new(|cx| TerminalView::new(terminal, cx));
        Self {
            title: SharedString::from(title.to_string()),
            content: TabContent::Terminal(terminal_view),
        }
    }

    pub fn new_file(path: PathBuf, cx: &mut App) -> Self {
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string());
        let viewer = cx.new(|cx| FileViewer::open(path.clone(), cx));
        Self {
            title: SharedString::from(title),
            content: TabContent::File { path, viewer },
        }
    }

    pub fn new_empty_file(path: PathBuf, cx: &mut App) -> Self {
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string());
        let viewer = cx.new(|cx| FileViewer::new_empty(path.clone(), cx));
        Self {
            title: SharedString::from(title),
            content: TabContent::File { path, viewer },
        }
    }

    pub fn icon(&self) -> TabIcon {
        match &self.content {
            TabContent::Terminal(_) => TabIcon {
                glyph: "\u{e795}", //  terminal
                color: colors::text_muted(),
            },
            TabContent::File { .. } => {
                TabIcon {
                    glyph: util::icon_for_file(&self.title),
                    color: util::file_icon_color(&self.title),
                }
            }
        }
    }

    pub fn get_claude_status(&self, cx: &App) -> ClaudeStatus {
        match &self.content {
            TabContent::Terminal(terminal_view) => {
                let view = terminal_view.read(cx);
                let terminal = view.terminal.read(cx);
                ClaudeStatus::from_detected(&terminal.get_claude_status())
            }
            TabContent::File { .. } => ClaudeStatus::Idle,
        }
    }

    pub fn mark_claude_as_read(&self, cx: &App) {
        if let TabContent::Terminal(terminal_view) = &self.content {
            let view = terminal_view.read(cx);
            view.terminal.read(cx).mark_claude_as_read();
        }
    }

    pub fn render_content(&self) -> AnyElement {
        match &self.content {
            TabContent::Terminal(terminal_view) => {
                div()
                    .flex()
                    .flex_1()
                    .size_full()
                    .child(terminal_view.clone())
                    .into_any_element()
            }
            TabContent::File { viewer, .. } => {
                div()
                    .flex()
                    .flex_1()
                    .size_full()
                    .child(viewer.clone())
                    .into_any_element()
            }
        }
    }

    /// Returns the file path if this is a file tab.
    pub fn file_path(&self) -> Option<&PathBuf> {
        match &self.content {
            TabContent::File { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Returns the file viewer if this is a file tab.
    pub fn file_viewer(&self) -> Option<&Entity<FileViewer>> {
        match &self.content {
            TabContent::File { viewer, .. } => Some(viewer),
            _ => None,
        }
    }

    /// Check if this is a file tab with dirty state.
    pub fn is_dirty(&self, cx: &App) -> bool {
        match &self.content {
            TabContent::File { viewer, .. } => viewer.read(cx).dirty,
            _ => false,
        }
    }
}
