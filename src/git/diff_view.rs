use gpui::*;
use std::path::PathBuf;

use super::diff_model::*;
use super::git_repo::GitRepo;
use crate::theme::colors;

pub struct GitDiffPanel {
    repo: Option<GitRepo>,
    branch: String,
    diff_files: Vec<DiffFile>,
    expanded_files: Vec<bool>,
}

impl GitDiffPanel {
    pub fn new(work_dir: PathBuf) -> Self {
        let repo = GitRepo::open(&work_dir);
        let branch = repo
            .as_ref()
            .map(|r| r.current_branch())
            .unwrap_or_else(|| "no repo".to_string());
        let diff_files = repo
            .as_ref()
            .map(|r| r.working_diff())
            .unwrap_or_default();
        let expanded_files = vec![true; diff_files.len()];

        Self {
            repo,
            branch,
            diff_files,
            expanded_files,
        }
    }

    pub fn refresh(&mut self) {
        if let Some(repo) = &self.repo {
            self.branch = repo.current_branch();
            self.diff_files = repo.working_diff();
            self.expanded_files = vec![true; self.diff_files.len()];
        }
    }

    fn render_file(&self, file: &DiffFile, file_idx: usize, expanded: bool) -> Div {
        let status_icon = match file.status {
            FileStatus::Added => "A",
            FileStatus::Modified => "M",
            FileStatus::Deleted => "D",
            FileStatus::Renamed => "R",
        };

        let status_color = match file.status {
            FileStatus::Added => colors::diff_added(),
            FileStatus::Deleted => colors::diff_removed(),
            FileStatus::Modified => colors::accent(),
            FileStatus::Renamed => colors::accent(),
        };

        let mut file_el = div().flex().flex_col().w_full();

        // File header
        file_el = file_el.child(
            div()
                .id(ElementId::Name(format!("diff-file-{file_idx}").into()))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .cursor_pointer()
                .child(
                    div()
                        .text_color(status_color)
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .child(status_icon.to_string()),
                )
                .child(
                    div()
                        .text_color(colors::text())
                        .text_xs()
                        .child(file.path.clone()),
                ),
        );

        // Hunks
        if expanded {
            for hunk in &file.hunks {
                file_el = file_el.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(rgb(0x585b70))
                        .child(hunk.header.trim().to_string()),
                );

                for line in &hunk.lines {
                    let (bg, text_col, prefix) = match line.kind {
                        DiffLineKind::Added => (Some(rgba(0xa6e3a120)), colors::diff_added(), "+"),
                        DiffLineKind::Removed => {
                            (Some(rgba(0xf38ba820)), colors::diff_removed(), "-")
                        }
                        DiffLineKind::Context => (None, colors::text_muted(), " "),
                    };

                    let mut line_el = div()
                        .flex()
                        .flex_row()
                        .px_2()
                        .text_xs()
                        .font_family("Menlo");

                    if let Some(bg) = bg {
                        line_el = line_el.bg(bg);
                    }

                    line_el = line_el
                        .child(
                            div()
                                .w(px(12.0))
                                .text_color(text_col)
                                .child(prefix.to_string()),
                        )
                        .child(
                            div()
                                .text_color(text_col)
                                .child(line.content.trim_end().to_string()),
                        );

                    file_el = file_el.child(line_el);
                }
            }
        }

        file_el
    }
}

impl Render for GitDiffPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut panel = div()
            .id("diff-panel")
            .flex()
            .flex_col()
            .w(px(320.0))
            .min_w(px(320.0))
            .h_full()
            .flex_shrink_0()
            .bg(colors::surface())
            .border_l_1()
            .border_color(colors::border())
            .overflow_y_scroll();

        // Header
        panel = panel.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px_3()
                .h(px(36.0))
                .border_b_1()
                .border_color(colors::border())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(
                            div()
                                .text_color(colors::text())
                                .text_sm()
                                .child("Git Diff"),
                        )
                        .child(
                            div()
                                .text_color(colors::accent())
                                .text_xs()
                                .child(self.branch.clone()),
                        ),
                ),
        );

        if self.diff_files.is_empty() {
            panel = panel.child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_color(colors::text_muted())
                    .text_sm()
                    .child("No changes"),
            );
        } else {
            let file_count = format!("{} file(s) changed", self.diff_files.len());
            panel = panel.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(colors::text_muted())
                    .text_xs()
                    .child(file_count),
            );

            for (idx, file) in self.diff_files.iter().enumerate() {
                let expanded = self.expanded_files.get(idx).copied().unwrap_or(true);
                panel = panel.child(self.render_file(file, idx, expanded));
            }
        }

        panel
    }
}
