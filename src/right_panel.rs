use gpui::*;

use crate::theme::colors;
use crate::util;
use crate::workspace::Workspace;

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

fn render_tab_button(
    id: impl Into<SharedString>,
    icon: &str,
    is_active: bool,
    workspace: Entity<Workspace>,
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
            workspace.update(cx, |ws, cx| {
                ws.right_panel_tab = tab;
                cx.notify();
            });
        })
}

/// Render the right panel for the given workspace. Returns `None` if this
/// workspace shouldn't display a right panel (no directory or PR review).
pub fn render_right_panel(
    workspace: Entity<Workspace>,
    cx: &App,
) -> Option<AnyElement> {
    let ws = workspace.read(cx);
    let git_diff_panel = ws.git_diff_panel.clone()?;
    let pr_diff_panel = ws.pr_diff_panel.clone()?;
    let active_tab = ws.right_panel_tab;
    let editor = ws.editor.clone();
    let width = ws.right_panel_width;
    let is_git = active_tab == RightPanelTab::Git;
    let is_files = active_tab == RightPanelTab::Files;
    let is_pr = active_tab == RightPanelTab::Pr;

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
        .child(render_tab_button(
            "right-panel-tab-files",
            "\u{f0c9}",
            is_files,
            workspace.clone(),
            RightPanelTab::Files,
            "Files (Cmd+E)",
        ))
        .child(render_tab_button(
            "right-panel-tab-git",
            "\u{e725}",
            is_git,
            workspace.clone(),
            RightPanelTab::Git,
            "Git Diff (Cmd+Shift+G)",
        ))
        .child(render_tab_button(
            "right-panel-tab-pr",
            "\u{e726}",
            is_pr,
            workspace.clone(),
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

    let content: AnyElement = match active_tab {
        RightPanelTab::Git => div()
            .flex()
            .flex_1()
            .size_full()
            .overflow_hidden()
            .child(git_diff_panel)
            .into_any_element(),
        RightPanelTab::Pr => div()
            .flex()
            .flex_1()
            .size_full()
            .overflow_hidden()
            .child(pr_diff_panel)
            .into_any_element(),
        RightPanelTab::Files => {
            if let Some(editor) = editor {
                div()
                    .flex()
                    .flex_1()
                    .size_full()
                    .overflow_hidden()
                    .child(editor)
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_color(colors::text_muted())
                    .text_sm()
                    .child("Open a file (Cmd+P) to start editing")
                    .into_any_element()
            }
        }
    };

    Some(
        div()
            .flex()
            .flex_col()
            .h_full()
            .w(px(width))
            .flex_shrink_0()
            .overflow_hidden()
            .bg(colors::bg())
            .border_l_1()
            .border_color(colors::border())
            .child(tab_bar)
            .child(content)
            .into_any_element(),
    )
}
