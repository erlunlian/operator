use gpui::*;

use crate::terminal::TerminalModel;
use crate::terminal::terminal_view::TerminalView;

#[derive(Clone)]
pub enum PaneContent {
    Terminal(Entity<TerminalView>),
}

pub struct Pane {
    pub id: usize,
    pub content: PaneContent,
    pub focus_handle: FocusHandle,
}

impl Pane {
    pub fn new_terminal(id: usize, cx: &mut Context<Self>) -> Self {
        let terminal = cx.new(|cx| TerminalModel::new(cx));
        let terminal_view = cx.new(|cx| TerminalView::new(terminal, cx));
        Self {
            id,
            content: PaneContent::Terminal(terminal_view),
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn title(&self, _cx: &App) -> String {
        match &self.content {
            PaneContent::Terminal(_) => format!("Terminal {}", self.id),
        }
    }

    pub fn get_claude_status(&self, cx: &App) -> crate::workspace::workspace::ClaudeStatus {
        use crate::workspace::workspace::ClaudeStatus;
        match &self.content {
            PaneContent::Terminal(view) => {
                let view = view.read(cx);
                let terminal = view.terminal.read(cx);
                ClaudeStatus::from_detected(&terminal.get_claude_status())
            }
        }
    }
}

impl Render for Pane {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        match &self.content {
            PaneContent::Terminal(view) => div().flex().flex_1().size_full().child(view.clone()),
        }
    }
}
