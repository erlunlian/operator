use std::rc::Rc;

use gpui::*;

use crate::theme::colors;
use crate::util;
use crate::workspace::workspace::ClaudeStatus;

/// Start a native window drag from within an on_mouse_down handler (macOS).
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

/// Data for rendering a single workspace card in the sidebar
pub struct WorkspaceCardData {
    pub name: SharedString,
    pub git_branch: Option<String>,
    /// Per-pane Claude statuses (only non-idle entries).
    pub pane_statuses: Vec<ClaudeStatus>,
}

#[derive(Clone)]
pub struct WorkspaceDragPayload {
    pub ix: usize,
    pub name: SharedString,
}

struct WorkspaceDragGhost {
    name: SharedString,
}

impl Render for WorkspaceDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .bg(colors::surface_hover())
            .rounded_md()
            .text_color(colors::text())
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .child(self.name.clone())
    }
}

/// A thin accent-colored line shown between cards during drag reorder.
fn drop_indicator() -> Div {
    div()
        .h(px(2.0))
        .w_full()
        .mx_3()
        .bg(colors::accent())
        .rounded_full()
}

pub struct WorkspaceSidebar;

impl WorkspaceSidebar {
    fn status_color(status: &ClaudeStatus) -> Rgba {
        match status {
            ClaudeStatus::Idle => colors::text_muted(),
            ClaudeStatus::WaitingForInput => rgb(0xf9e2af), // yellow
            ClaudeStatus::Working => rgb(0xa6e3a1),          // green
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_with_width(
        workspaces: &[WorkspaceCardData],
        active_ix: usize,
        on_select: Rc<dyn Fn(usize, &mut Window, &mut App)>,
        on_new: Rc<dyn Fn(&mut Window, &mut App)>,
        on_close: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
        on_reorder: Option<Rc<dyn Fn(usize, usize, &mut Window, &mut App)>>,
        drop_index: Option<usize>,
        on_drop_index_change: Rc<dyn Fn(Option<usize>, &mut Window, &mut App)>,
        on_drag_end: Rc<dyn Fn(&mut Window, &mut App)>,
        width: f32,
        update_info: Option<&crate::updater::UpdateInfo>,
    ) -> Stateful<Div> {
        let on_drag_end2 = on_drag_end.clone();

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
            .pb_1()
            .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                on_drag_end2(window, cx);
            });

        // Titlebar drag region — replaces the old pt(36px) padding.
        // Allows dragging the window from the empty sidebar top area.
        sidebar = sidebar.child(
            div()
                .id("sidebar-titlebar-drag")
                .h(px(36.0))
                .w_full()
                .flex_shrink_0()
                .on_mouse_down(MouseButton::Left, |_event, _window, _cx| {
                    start_window_drag_native();
                })
                .on_click(move |event, window, _cx| {
                    if event.click_count() == 2 {
                        window.titlebar_double_click();
                    }
                }),
        );

        for (ix, ws) in workspaces.iter().enumerate() {
            let is_active = ix == active_ix;
            let on_select = on_select.clone();
            let on_close = on_close.clone();
            let on_reorder = on_reorder.clone();
            let on_drop_index_change = on_drop_index_change.clone();
            let on_drag_end = on_drag_end.clone();

            // Show drop indicator before this card
            if drop_index == Some(ix) {
                sidebar = sidebar.child(drop_indicator());
            }

            let card = Self::render_card(
                ws, is_active, ix, workspaces.len(),
                on_select, on_close, on_reorder,
                drop_index, on_drop_index_change, on_drag_end,
            );
            sidebar = sidebar.child(card);
        }

        // Show drop indicator at the end (after last card)
        if drop_index == Some(workspaces.len()) {
            sidebar = sidebar.child(drop_indicator());
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
                        .child("New Workspace (Cmd+N)"),
                )
                .on_click(move |_, window, cx| {
                    on_new(window, cx);
                }),
        );

        // Update available indicator — compact row below New Workspace
        if let Some(info) = update_info {
            let url = info.download_url.clone();
            let version = info.latest_version.clone();
            sidebar = sidebar.child(
                div()
                    .id("update-indicator")
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .w_full()
                    .px_3()
                    .py_1()
                    .cursor_pointer()
                    .hover(|s| s.bg(colors::surface_hover()))
                    .on_click(move |_, _window, cx| {
                        cx.open_url(&url);
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors::accent())
                            .child(format!("v{version} available \u{2197}")),
                    ),
            );
        }

        sidebar
    }

    #[allow(clippy::too_many_arguments)]
    fn render_card(
        ws: &WorkspaceCardData,
        is_active: bool,
        ix: usize,
        _total: usize,
        on_select: Rc<dyn Fn(usize, &mut Window, &mut App)>,
        on_close: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
        on_reorder: Option<Rc<dyn Fn(usize, usize, &mut Window, &mut App)>>,
        drop_index: Option<usize>,
        on_drop_index_change: Rc<dyn Fn(Option<usize>, &mut Window, &mut App)>,
        on_drag_end: Rc<dyn Fn(&mut Window, &mut App)>,
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

        // Overall status color for the card dot (most active pane wins)
        let overall_status = if ws.pane_statuses.iter().any(|s| *s == ClaudeStatus::Working) {
            ClaudeStatus::Working
        } else if ws.pane_statuses.iter().any(|s| *s == ClaudeStatus::WaitingForInput) {
            ClaudeStatus::WaitingForInput
        } else {
            ClaudeStatus::Idle
        };
        let status_color = Self::status_color(&overall_status);

        let pane_statuses = ws.pane_statuses.clone();
        let git_branch = ws.git_branch.clone();

        let mut card = div()
            .id(ElementId::Name(format!("ws-card-{ix}").into()))
            .group("ws-card")
            .flex()
            .flex_row()
            .gap_2()
            .w_full()
            .px_3()
            .py_2()
            .bg(active_bg)
            .border_l_2()
            .border_color(active_border)
            .cursor_pointer()
            .hover(|s| s.bg(colors::surface_hover()));

        // Drag support
        let payload = WorkspaceDragPayload {
            ix,
            name: ws.name.clone(),
        };
        card = card.on_drag(payload, move |payload, _offset, _window, cx| {
            cx.new(|_cx| WorkspaceDragGhost {
                name: payload.name.clone(),
            })
        });

        // Track drag position for drop indicator.
        // on_drag_move fires on EVERY element that registered it (capture phase),
        // so we must check that the cursor is actually within this card's bounds.
        {
            let on_change = on_drop_index_change.clone();
            card = card.on_drag_move::<WorkspaceDragPayload>(
                move |event: &DragMoveEvent<WorkspaceDragPayload>, window, cx| {
                    let bounds = event.bounds;
                    let pos = event.event.position;
                    if !bounds.contains(&pos) {
                        return;
                    }
                    let mid_y = bounds.origin.y + bounds.size.height / 2.0;
                    let target = if pos.y < mid_y { ix } else { ix + 1 };
                    // Don't show indicator right next to the dragged item (no-op position)
                    let source = event.drag(cx).ix;
                    let effective = if target == source || target == source + 1 {
                        None
                    } else {
                        Some(target)
                    };
                    on_change(effective, window, cx);
                },
            );
        }

        // Drop support (reorder) — use the computed drop_index, not the card ix.
        if let Some(on_reorder) = on_reorder {
            let on_drag_end = on_drag_end.clone();
            card = card.on_drop(move |payload: &WorkspaceDragPayload, window, cx| {
                if let Some(target) = drop_index {
                    on_reorder(payload.ix, target, window, cx);
                }
                on_drag_end(window, cx);
            });
        }

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

        // Title: use git branch if available, otherwise workspace name
        let title = git_branch
            .as_ref()
            .map(|b| SharedString::from(b.clone()))
            .unwrap_or_else(|| ws.name.clone());
        text_col = text_col.child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors::text())
                .overflow_x_hidden()
                .text_ellipsis()
                .child(title),
        );

        // Per-pane Claude status rows
        for pane_status in &pane_statuses {
            let label = pane_status.label();
            if !label.is_empty() {
                let color = Self::status_color(pane_status);
                text_col = text_col.child(
                    div()
                        .text_xs()
                        .text_color(color)
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .child(label.to_string()),
                );
            }
        }

        // Workspace name subtitle (when branch is the title, show name below)
        if git_branch.is_some() {
            text_col = text_col.child(
                div()
                    .text_xs()
                    .text_color(colors::text_muted())
                    .overflow_x_hidden()
                    .text_ellipsis()
                    .child(ws.name.clone()),
            );
        }

        card = card.child(text_col);

        // Close button (visible on hover)
        if let Some(on_close) = on_close {
            let close_ix = ix;
            card = card.child(
                div()
                    .id(ElementId::Name(format!("ws-close-{ix}").into()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(18.0))
                    .h(px(18.0))
                    .mt(px(2.0))
                    .rounded_sm()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(colors::text_muted())
                    .invisible()
                    .group_hover("ws-card", |s| s.visible())
                    .hover(|s| s.bg(colors::surface_hover()).text_color(colors::text()))
                    .cursor_pointer()
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        on_close(close_ix, window, cx);
                    })
                    .child(
                        div()
                            .font_family(util::ICON_FONT)
                            .text_size(px(12.0))
                            .child("\u{f00d}"), // nf-fa-close (×)
                    ),
            );
        }

        card.on_click(move |_, window, cx| {
            on_select(ix, window, cx);
        })
    }
}
