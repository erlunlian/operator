use gpui::*;
use std::rc::Rc;

use crate::theme::colors;
use crate::workspace::workspace::ClaudeStatus;

/// Data for rendering a single workspace card in the sidebar
pub struct WorkspaceCardData {
    pub name: SharedString,
    pub directory: String,
    pub git_branch: Option<String>,
    pub claude_status: ClaudeStatus,
}

pub struct WorkspaceSidebar;

impl WorkspaceSidebar {
    pub fn render(
        workspaces: &[WorkspaceCardData],
        active_ix: usize,
        on_select: Rc<dyn Fn(usize, &mut Window, &mut App)>,
        on_new: Rc<dyn Fn(&mut Window, &mut App)>,
    ) -> Stateful<Div> {
        Self::render_with_width(workspaces, active_ix, on_select, on_new, 260.0)
    }

    pub fn render_with_width(
        workspaces: &[WorkspaceCardData],
        active_ix: usize,
        on_select: Rc<dyn Fn(usize, &mut Window, &mut App)>,
        on_new: Rc<dyn Fn(&mut Window, &mut App)>,
        width: f32,
    ) -> Stateful<Div> {
        let mut sidebar = div()
            .id("workspace-sidebar")
            .flex()
            .flex_col()
            .w(px(width))
            .min_w(px(120.0))
            .h_full()
            .flex_shrink_0()
            .bg(colors::surface())
            .border_r_1()
            .border_color(colors::border())
            .overflow_hidden()
            .pt(px(36.0))
            .pb_1();

        for (ix, ws) in workspaces.iter().enumerate() {
            let is_active = ix == active_ix;
            let on_select = on_select.clone();

            let card = Self::render_card(ws, is_active, ix, on_select);
            sidebar = sidebar.child(card);
        }

        // Spacer to push content to top
        sidebar = sidebar.child(div().flex_1());

        // "New Workspace" button at the bottom
        let on_new = on_new.clone();
        sidebar = sidebar.child(
            div()
                .id("new-workspace-btn")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .w_full()
                .px_3()
                .py_2()
                .mb_1()
                .border_t_1()
                .border_color(colors::border())
                .cursor_pointer()
                .child(
                    div()
                        .text_sm()
                        .text_color(colors::text_muted())
                        .child("+"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(colors::text_muted())
                        .child("New Workspace"),
                )
                .on_click(move |_, window, cx| {
                    on_new(window, cx);
                }),
        );

        sidebar
    }

    fn render_card(
        ws: &WorkspaceCardData,
        is_active: bool,
        ix: usize,
        on_select: Rc<dyn Fn(usize, &mut Window, &mut App)>,
    ) -> Stateful<Div> {
        let active_bg = if is_active {
            colors::surface_hover()
        } else {
            colors::surface()
        };

        let active_border = if is_active {
            colors::accent()
        } else {
            rgba(0x00000000) // transparent
        };

        // Status indicator color
        let status_color = match &ws.claude_status {
            ClaudeStatus::Idle => colors::text_muted(),
            ClaudeStatus::WaitingForInput => rgb(0xf9e2af), // yellow
            ClaudeStatus::Working => rgb(0xa6e3a1),          // green
        };

        // Build the info line: "branch · ~/path"
        let mut info_parts: Vec<String> = Vec::new();
        if let Some(branch) = &ws.git_branch {
            info_parts.push(branch.clone());
        }
        info_parts.push(ws.directory.clone());
        let info_line = info_parts.join(" \u{00B7} "); // middle dot separator

        let mut card = div()
            .id(ElementId::Name(format!("ws-card-{ix}").into()))
            .flex()
            .flex_row()
            .gap_2()
            .w_full()
            .px_3()
            .py_2()
            .bg(active_bg)
            .border_l_2()
            .border_color(active_border)
            .cursor_pointer();

        // Status dot
        card = card.child(
            div()
                .w(px(8.0))
                .h(px(8.0))
                .rounded_full()
                .bg(status_color)
                .mt(px(5.0))
                .flex_shrink_0(),
        );

        // Text content
        let mut text_col = div().flex().flex_col().flex_1().overflow_x_hidden();

        // Title (bold)
        text_col = text_col.child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors::text())
                .overflow_x_hidden()
                .child(ws.name.clone()),
        );

        // Claude status line
        let status_label = ws.claude_status.label();
        if !status_label.is_empty() {
            text_col = text_col.child(
                div()
                    .text_xs()
                    .text_color(status_color)
                    .child(status_label.to_string()),
            );
        }

        // Branch + directory line
        text_col = text_col.child(
            div()
                .text_xs()
                .text_color(colors::text_muted())
                .overflow_x_hidden()
                .child(info_line),
        );

        card = card.child(text_col);

        card.on_click(move |_, window, cx| {
            on_select(ix, window, cx);
        })
    }
}
