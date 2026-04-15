use gpui::*;

use super::metrics::{format_bytes, ProcessMetrics, SubsystemMetrics};
use crate::theme::colors;

/// Floating debug overlay showing live process metrics.
pub struct DebugPanel {
    pub visible: bool,
    metrics: Option<ProcessMetrics>,
    /// Background task that periodically refreshes metrics.
    _poll_task: Option<Task<()>>,
}

impl DebugPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let poll_task = Self::start_polling(cx);
        Self {
            visible: false,
            metrics: None,
            _poll_task: Some(poll_task),
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Update the app-level counts and subsystem metrics.
    pub fn update_subsystems(
        &mut self,
        terminal_count: usize,
        workspace_count: usize,
        subsystems: SubsystemMetrics,
    ) {
        if let Some(m) = &mut self.metrics {
            m.terminal_count = terminal_count;
            m.workspace_count = workspace_count;
            m.subsystems = subsystems;
        }
    }

    fn start_polling(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async {
                    smol::Timer::after(std::time::Duration::from_secs(2)).await;
                })
                .await;

                let should_collect = this
                    .read_with(cx, |panel, _| panel.visible)
                    .unwrap_or(false);

                if should_collect {
                    let subsystems = this
                        .read_with(cx, |panel, _| {
                            panel
                                .metrics
                                .as_ref()
                                .map(|m| {
                                    (
                                        m.terminal_count,
                                        m.workspace_count,
                                        m.subsystems.clone(),
                                    )
                                })
                                .unwrap_or_default()
                        })
                        .unwrap_or_default();

                    let metrics =
                        ProcessMetrics::collect(subsystems.0, subsystems.1, subsystems.2);
                    let _ = this.update(&mut cx.clone(), |panel, cx| {
                        panel.metrics = Some(metrics);
                        cx.notify();
                    });
                }
            }
        })
    }
}

impl Render for DebugPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }

        let content = if let Some(m) = &self.metrics {
            let sub = &m.subsystems;
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                // Process-level
                .child(section_header("Process"))
                .child(metric_row("RSS", &m.resident_display()))
                .child(metric_row("Virtual", &m.virtual_display()))
                .child(metric_row("Threads", &m.thread_count.to_string()))
                // Terminals
                .child(section_header("Terminals"))
                .child(metric_row(
                    "Count",
                    &m.terminal_count.to_string(),
                ))
                .child(metric_row(
                    "Grid mem",
                    &format_bytes(sub.terminal_grid_bytes as u64),
                ))
                // Diff panels
                .child(section_header("Git Diff"))
                .child(metric_row(
                    "Files",
                    &sub.git_diff_files.to_string(),
                ))
                .child(metric_row(
                    "Data",
                    &format_bytes(sub.git_diff_bytes as u64),
                ))
                .child(section_header("PR Diff"))
                .child(metric_row(
                    "Files",
                    &sub.pr_diff_files.to_string(),
                ))
                .child(metric_row(
                    "Data",
                    &format_bytes(sub.pr_diff_bytes as u64),
                ))
                // Summary
                .child(section_header("Summary"))
                .child(metric_row(
                    "Tracked",
                    &format_bytes(m.tracked_total() as u64),
                ))
                .child(metric_row(
                    "Untracked",
                    &format_bytes(m.untracked_bytes()),
                ))
                .child(metric_row("Workspaces", &m.workspace_count.to_string()))
        } else {
            div().child(
                div()
                    .text_xs()
                    .text_color(colors::text_muted())
                    .child("Collecting..."),
            )
        };

        div()
            .id("debug-panel-overlay")
            .absolute()
            .bottom(px(8.0))
            .right(px(8.0))
            .w(px(240.0))
            .bg(colors::surface())
            .border_1()
            .border_color(colors::border())
            .rounded(px(6.0))
            .p_2()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors::accent())
                    .child("Debug"),
            )
            .child(content)
            .into_any_element()
    }
}

fn section_header(label: &str) -> Div {
    div()
        .mt(px(4.0))
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors::accent())
        .child(label.to_string())
}

fn metric_row(label: &str, value: &str) -> Div {
    div()
        .flex()
        .flex_row()
        .justify_between()
        .child(
            div()
                .text_xs()
                .text_color(colors::text_muted())
                .child(format!("  {label}")),
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors::text())
                .child(value.to_string()),
        )
}
