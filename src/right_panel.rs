use gpui::*;
use std::path::PathBuf;

use crate::editor::EditorView;
use crate::git::GitDiffPanel;
use crate::git::PrDiffPanel;
use crate::theme::colors;
use crate::util;

#[cfg(target_os = "macos")]
fn start_window_drag_native() {
    unsafe {
        let app: cocoa::base::id = objc::msg_send![objc::class!(NSApplication), sharedApplication];
        let event: cocoa::base::id = objc::msg_send![app, currentEvent];
        let window: cocoa::base::id = objc::msg_send![event, window];
        if !window.is_null() {
            let _: () = objc::msg_send![window, performWindowDragWithEvent: event];
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn start_window_drag_native() {}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RightPanelTab {
    Files,
    Git,
    Pr,
}

pub struct RightPanel {
    pub active_tab: RightPanelTab,
    pub diff_panel: Entity<GitDiffPanel>,
    pub pr_diff_panel: Entity<PrDiffPanel>,
    pub editor: Option<Entity<EditorView>>,
    pub width: f32,
}

impl RightPanel {
    pub fn new(diff_panel: Entity<GitDiffPanel>, pr_diff_panel: Entity<PrDiffPanel>) -> Self {
        Self {
            active_tab: RightPanelTab::Git,
            diff_panel,
            pr_diff_panel,
            editor: None,
            width: 400.0,
        }
    }

    pub fn ensure_editor(&mut self, root_dir: PathBuf, cx: &mut Context<Self>) {
        if self.editor.is_none() {
            let editor = cx.new(|cx| EditorView::new(root_dir, cx));
            self.editor = Some(editor);
            cx.notify();
        }
    }

    /// Open a file in the editor panel, creating the editor if needed.
    pub fn open_file(&mut self, path: PathBuf, root_dir: PathBuf, line_num: Option<usize>, cx: &mut Context<Self>) {
        self.ensure_editor(root_dir, cx);
        if let Some(editor) = &self.editor {
            editor.update(cx, |view, cx| {
                view.open_file(path, cx);
                if let Some(line) = line_num {
                    view.navigate_to_line(line, cx);
                }
            });
        }
        self.active_tab = RightPanelTab::Files;
        cx.notify();
    }

    pub fn set_directory(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        // Reset editor for new directory
        let editor = cx.new(|cx| EditorView::new(dir, cx));
        self.editor = Some(editor);
        cx.notify();
    }

    fn render_tab_button(
        id: impl Into<SharedString>,
        icon: &str,
        is_active: bool,
        entity: Entity<RightPanel>,
        tab: RightPanelTab,
        tooltip_text: &'static str,
    ) -> Stateful<Div> {
        let bg = if is_active {
            colors::surface_hover()
        } else {
            colors::surface()
        };
        let text_col = if is_active {
            colors::text()
        } else {
            colors::text_muted()
        };

        div()
            .id(ElementId::Name(id.into()))
            .flex()
            .items_center()
            .justify_center()
            .w(px(28.0))
            .h(px(28.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .bg(bg)
            .child(
                div()
                    .font_family(util::ICON_FONT)
                    .text_size(px(15.0))
                    .text_color(text_col)
                    .child(icon.to_string()),
            )
            .tooltip(move |_window, cx| util::render_tooltip(tooltip_text, cx))
            .on_click(move |_, _window, cx| {
                entity.update(cx, |panel, cx| {
                    panel.active_tab = tab;
                    cx.notify();
                });
            })
    }
}

impl Render for RightPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().clone();
        let entity2 = cx.entity().clone();
        let entity3 = cx.entity().clone();
        let is_git = self.active_tab == RightPanelTab::Git;
        let is_files = self.active_tab == RightPanelTab::Files;
        let is_pr = self.active_tab == RightPanelTab::Pr;

        // Tab switcher bar at top
        let tab_bar = div()
            .flex()
            .flex_row()
            .w_full()
            .h(px(36.0))
            .bg(colors::surface())
            .border_b_1()
            .border_color(colors::border())
            .items_center()
            .px_2()
            .gap_1()
            .child(Self::render_tab_button(
                "right-panel-tab-files",
                "\u{f0c9}", // nf-fa-bars (list icon)
                is_files,
                entity,
                RightPanelTab::Files,
                "Files (Cmd+E)",
            ))
            .child(Self::render_tab_button(
                "right-panel-tab-git",
                "\u{e725}", // nf-dev-git_branch
                is_git,
                entity2,
                RightPanelTab::Git,
                "Git Diff (Cmd+Shift+G)",
            ))
            .child(Self::render_tab_button(
                "right-panel-tab-pr",
                "\u{e726}", // nf-dev-git_pull_request
                is_pr,
                entity3,
                RightPanelTab::Pr,
                "Pull Requests (Cmd+Shift+R)",
            ))
            .child(
                div()
                    .id("right-panel-drag")
                    .flex_1()
                    .h_full()
                    .on_mouse_down(MouseButton::Left, |_event, _window, _cx| {
                        start_window_drag_native();
                    })
                    .on_click(move |event, window, _cx| {
                        if event.click_count() == 2 {
                            window.titlebar_double_click();
                        }
                    }),
            );

        // Content area
        let content: AnyElement = match self.active_tab {
            RightPanelTab::Git => {
                let dp = self.diff_panel.clone();
                div()
                    .flex()
                    .flex_1()
                    .size_full()
                    .overflow_hidden()
                    .child(dp)
                    .into_any_element()
            }
            RightPanelTab::Pr => {
                let pr = self.pr_diff_panel.clone();
                div()
                    .flex()
                    .flex_1()
                    .size_full()
                    .overflow_hidden()
                    .child(pr)
                    .into_any_element()
            }
            RightPanelTab::Files => {
                if let Some(editor) = &self.editor {
                    div()
                        .flex()
                        .flex_1()
                        .size_full()
                        .overflow_hidden()
                        .child(editor.clone())
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .text_color(colors::text_muted())
                        .text_sm()
                        .child("Open a project to browse files")
                        .into_any_element()
                }
            }
        };

        div()
            .flex()
            .flex_col()
            .h_full()
            .w(px(self.width))
            .flex_shrink_0()
            .overflow_hidden()
            .bg(colors::bg())
            .border_l_1()
            .border_color(colors::border())
            .child(tab_bar)
            .child(content)
    }
}
