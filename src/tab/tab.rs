use gpui::*;
use std::path::PathBuf;

use crate::editor::EditorView;
use crate::tab::tab_bar::TabIcon;
use crate::terminal::TerminalModel;
use crate::terminal::terminal_view::TerminalView;
use crate::theme::colors;
use crate::workspace::workspace::ClaudeStatus;

pub enum TabContent {
    Terminal(Entity<TerminalView>),
    Editor(Entity<EditorView>),
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

    pub fn new_editor(title: &str, root_dir: PathBuf, cx: &mut App) -> Self {
        let editor = cx.new(|cx| EditorView::new(root_dir, cx));
        Self {
            title: SharedString::from(title.to_string()),
            content: TabContent::Editor(editor),
        }
    }

    pub fn icon(&self) -> TabIcon {
        match &self.content {
            TabContent::Terminal(_) => TabIcon {
                glyph: "\u{e795}", //  terminal
                color: colors::text_muted(),
            },
            TabContent::Editor(_) => TabIcon {
                glyph: "\u{f07c}", //  folder open
                color: colors::text_muted(),
            },
        }
    }

    pub fn get_claude_status(&self, cx: &App) -> ClaudeStatus {
        match &self.content {
            TabContent::Terminal(view) => {
                let view = view.read(cx);
                let terminal = view.terminal.read(cx);
                ClaudeStatus::from_detected(&terminal.get_claude_status())
            }
            TabContent::Editor(_) => ClaudeStatus::Idle,
        }
    }

    /// Returns true if this tab has inner sub-tabs that should be closed first.
    pub fn has_open_sub_tabs(&self, cx: &App) -> bool {
        match &self.content {
            TabContent::Editor(view) => !view.read(cx).open_files.is_empty(),
            TabContent::Terminal(_) => false,
        }
    }

    /// Get the editor entity if this is an editor tab.
    pub fn editor_entity(&self) -> Option<&Entity<EditorView>> {
        match &self.content {
            TabContent::Editor(view) => Some(view),
            TabContent::Terminal(_) => None,
        }
    }

    pub fn render_content(&self) -> AnyElement {
        let child: AnyElement = match &self.content {
            TabContent::Terminal(view) => view.clone().into_any_element(),
            TabContent::Editor(view) => view.clone().into_any_element(),
        };
        div()
            .flex()
            .flex_1()
            .size_full()
            .child(child)
            .into_any_element()
    }
}
