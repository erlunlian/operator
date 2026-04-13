use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::diff_model::*;
use super::git_repo::GitRepo;
use crate::theme::colors;
use crate::util;

/// How many context lines around each change to show by default.
const DEFAULT_CONTEXT: usize = 3;
/// How many extra context lines to reveal per click.
const EXPAND_STEP: usize = 20;

// ── Colors specific to the diff view ──

fn gutter_bg() -> Rgba {
    rgb(0x1a1a2e)
}
fn added_line_bg() -> Rgba {
    rgba(0x2ea04322)
}
fn added_gutter_bg() -> Rgba {
    rgba(0x2ea04340)
}
fn removed_line_bg() -> Rgba {
    rgba(0xf8514922)
}
fn removed_gutter_bg() -> Rgba {
    rgba(0xf8514940)
}
fn collapse_bar_bg() -> Rgba {
    rgb(0x161625)
}
fn file_header_bg() -> Rgba {
    rgb(0x1c1c30)
}

// ── Section enum ──

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffSection {
    Staged,
    Unstaged,
}

// ── Panel state ──

pub struct GitDiffPanel {
    repo: Option<GitRepo>,
    branch: String,
    staged_files: Vec<DiffFile>,
    unstaged_files: Vec<DiffFile>,
    /// Which section's diffs are shown in the content area.
    active_section: DiffSection,
    /// Files whose diff body is collapsed (header still shown).
    /// Key is (section, index-within-section).
    collapsed_files: HashSet<(DiffSection, usize)>,
    /// Collapsed directory nodes in the file tree.
    collapsed_dirs: HashSet<String>,
    /// Whether the staged / unstaged tree sections are collapsed.
    staged_tree_collapsed: bool,
    unstaged_tree_collapsed: bool,
    /// Per-hunk expanded context: key = (section, file_idx, hunk_idx).
    expanded_context: HashMap<(DiffSection, usize, usize), (usize, usize)>,
    /// Panel width in px (overall diff panel).
    pub width: f32,
    /// File tree width in px.
    tree_width: f32,
    /// Whether we're currently dragging the tree resize handle.
    resizing_tree: bool,
    /// Mouse X at drag start (window coords).
    tree_drag_start_x: f32,
    /// Tree width at drag start.
    tree_drag_start_width: f32,
    scroll_handle: ScrollHandle,
    /// Which file just had its path copied (shows checkmark briefly).
    copied_file_key: Option<(DiffSection, usize)>,
    /// Timer handle to clear the copied indicator.
    _copied_timer: Option<Task<()>>,
    /// Index to scroll to after next render.
    scroll_to_file: Option<usize>,
}

impl std::hash::Hash for DiffSection {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (*self as u8).hash(state);
    }
}

impl GitDiffPanel {
    pub fn empty() -> Self {
        Self {
            repo: None,
            branch: String::new(),
            staged_files: Vec::new(),
            unstaged_files: Vec::new(),
            active_section: DiffSection::Unstaged,
            collapsed_files: HashSet::new(),
            collapsed_dirs: HashSet::new(),
            staged_tree_collapsed: false,
            unstaged_tree_collapsed: false,
            expanded_context: HashMap::new(),
            width: 360.0,
            tree_width: 200.0,
            resizing_tree: false,
            tree_drag_start_x: 0.0,
            tree_drag_start_width: 0.0,
            scroll_handle: ScrollHandle::new(),
            copied_file_key: None,
            _copied_timer: None,
            scroll_to_file: None,
        }
    }

    pub fn new(work_dir: PathBuf) -> Self {
        let repo = GitRepo::open(&work_dir);
        let branch = repo
            .as_ref()
            .map(|r| r.current_branch())
            .unwrap_or_else(|| "no repo".to_string());
        let staged_files = repo
            .as_ref()
            .map(|r| r.staged_diff())
            .unwrap_or_default();
        let unstaged_files = repo
            .as_ref()
            .map(|r| r.unstaged_diff())
            .unwrap_or_default();

        // Default to whichever section has files
        let active_section = if !staged_files.is_empty() {
            DiffSection::Staged
        } else {
            DiffSection::Unstaged
        };

        Self {
            repo,
            branch,
            staged_files,
            unstaged_files,
            active_section,
            collapsed_files: HashSet::new(),
            collapsed_dirs: HashSet::new(),
            staged_tree_collapsed: false,
            unstaged_tree_collapsed: false,
            expanded_context: HashMap::new(),
            width: 360.0,
            tree_width: 200.0,
            resizing_tree: false,
            tree_drag_start_x: 0.0,
            tree_drag_start_width: 0.0,
            scroll_handle: ScrollHandle::new(),
            copied_file_key: None,
            _copied_timer: None,
            scroll_to_file: None,
        }
    }

    pub fn git_dir(&self) -> Option<std::path::PathBuf> {
        self.repo.as_ref().map(|r| r.git_dir())
    }

    pub fn refresh(&mut self) {
        if let Some(repo) = &self.repo {
            self.branch = repo.current_branch();
            self.staged_files = repo.staged_diff();
            self.unstaged_files = repo.unstaged_diff();
            self.expanded_context.clear();
        }
    }

    fn stage_file(&mut self, file_idx: usize) {
        if let Some(repo) = &self.repo {
            if let Some(file) = self.unstaged_files.get(file_idx) {
                let path = file.path.clone();
                if repo.stage_file(&path).is_ok() {
                    self.refresh();
                }
            }
        }
    }

    fn unstage_file(&mut self, file_idx: usize) {
        if let Some(repo) = &self.repo {
            if let Some(file) = self.staged_files.get(file_idx) {
                let path = file.path.clone();
                if repo.unstage_file(&path).is_ok() {
                    self.refresh();
                }
            }
        }
    }

    fn revert_file(&mut self, file_idx: usize) {
        if let Some(repo) = &self.repo {
            if let Some(file) = self.unstaged_files.get(file_idx) {
                let path = file.path.clone();
                if repo.revert_file(&path).is_ok() {
                    self.refresh();
                }
            }
        }
    }

    fn active_files(&self) -> &[DiffFile] {
        match self.active_section {
            DiffSection::Staged => &self.staged_files,
            DiffSection::Unstaged => &self.unstaged_files,
        }
    }

    fn total_additions(&self) -> usize {
        self.staged_files.iter().chain(self.unstaged_files.iter()).map(|f| f.additions()).sum()
    }

    fn total_deletions(&self) -> usize {
        self.staged_files.iter().chain(self.unstaged_files.iter()).map(|f| f.deletions()).sum()
    }

    // ── File tree (left side of the content area) ──

    fn render_file_tree(&self, cx: &mut Context<Self>) -> Div {
        let mut container = div().flex().flex_col().w_full();

        // ── Staged section ──
        let staged_count = self.staged_files.len();
        container = container.child(self.render_tree_section_header(
            DiffSection::Staged,
            staged_count,
            self.staged_tree_collapsed,
            cx,
        ));
        if !self.staged_tree_collapsed && staged_count > 0 {
            let tree = build_file_tree(&self.staged_files);
            container = self.render_tree_nodes(&tree, 0, DiffSection::Staged, cx, container);
        }

        // ── Unstaged section ──
        let unstaged_count = self.unstaged_files.len();
        container = container.child(self.render_tree_section_header(
            DiffSection::Unstaged,
            unstaged_count,
            self.unstaged_tree_collapsed,
            cx,
        ));
        if !self.unstaged_tree_collapsed && unstaged_count > 0 {
            let tree = build_file_tree(&self.unstaged_files);
            container = self.render_tree_nodes(&tree, 0, DiffSection::Unstaged, cx, container);
        }

        container
    }

    fn render_tree_section_header(
        &self,
        section: DiffSection,
        count: usize,
        is_collapsed: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let label = match section {
            DiffSection::Staged => format!("Staged ({count})"),
            DiffSection::Unstaged => format!("Unstaged ({count})"),
        };
        let is_active = self.active_section == section;
        let entity = cx.entity().clone();
        let chevron = if is_collapsed { "\u{25B6}" } else { "\u{25BC}" };
        let id_str = match section {
            DiffSection::Staged => "tree-section-staged",
            DiffSection::Unstaged => "tree-section-unstaged",
        };

        div()
            .id(ElementId::Name(id_str.into()))
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(28.0))
            .px_2()
            .cursor_pointer()
            .border_b_1()
            .border_color(colors::border())
            .when(is_active, |d: Stateful<Div>| d.bg(colors::surface_hover()))
            .hover(|s| s.bg(colors::surface_hover()))
            .on_click(move |_, _window, cx| {
                entity.update(cx, |panel, cx| {
                    // Toggle collapsed state of the section header
                    match section {
                        DiffSection::Staged => panel.staged_tree_collapsed = !panel.staged_tree_collapsed,
                        DiffSection::Unstaged => panel.unstaged_tree_collapsed = !panel.unstaged_tree_collapsed,
                    }
                    // Also switch active section to this one (so diffs update)
                    if count > 0 {
                        panel.active_section = section;
                        panel.expanded_context.clear();
                    }
                    cx.notify();
                });
            })
            .child(
                div()
                    .text_xs()
                    .text_color(colors::text_muted())
                    .w(px(12.0))
                    .child(chevron.to_string()),
            )
            .child(
                div()
                    .ml_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors::text())
                    .child(label),
            )
    }

    fn render_tree_nodes(
        &self,
        nodes: &[TreeNode],
        depth: usize,
        section: DiffSection,
        cx: &mut Context<Self>,
        mut container: Div,
    ) -> Div {
        for node in nodes {
            match node {
                TreeNode::Dir {
                    name,
                    children,
                    full_path,
                } => {
                    let dir_key = format!("{}-{full_path}", match section {
                        DiffSection::Staged => "s",
                        DiffSection::Unstaged => "u",
                    });
                    let is_collapsed = self.collapsed_dirs.contains(&dir_key);
                    let entity = cx.entity().clone();
                    let dk = dir_key.clone();
                    let chevron = if is_collapsed { "\u{25B6}" } else { "\u{25BC}" };
                    let dir_icon = if is_collapsed { util::dir_icon() } else { util::dir_icon_open() };

                    container = container.child(
                        div()
                            .id(ElementId::Name(format!("tree-dir-{dir_key}").into()))
                            .flex()
                            .flex_row()
                            .items_center()
                            .h(px(24.0))
                            .pl(px((depth as f32) * 16.0 + 8.0))
                            .pr_2()
                            .cursor_pointer()
                            .hover(|s| s.bg(colors::surface_hover()))
                            .on_click(move |_, _window, cx| {
                                let path = dk.clone();
                                entity.update(cx, |panel, cx| {
                                    if panel.collapsed_dirs.contains(&path) {
                                        panel.collapsed_dirs.remove(&path);
                                    } else {
                                        panel.collapsed_dirs.insert(path);
                                    }
                                    cx.notify();
                                });
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors::text_muted())
                                    .w(px(12.0))
                                    .child(chevron.to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .ml_1()
                                    .child(dir_icon),
                            )
                            .child(
                                div()
                                    .ml_1()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors::text())
                                    .child(name.clone()),
                            ),
                    );

                    if !is_collapsed {
                        container =
                            self.render_tree_nodes(children, depth + 1, section, cx, container);
                    }
                }
                TreeNode::File {
                    name,
                    file_idx,
                    status: _,
                } => {
                    let idx = *file_idx;
                    let entity = cx.entity().clone();
                    let is_active_section = self.active_section == section;
                    let sec_prefix = match section {
                        DiffSection::Staged => "s",
                        DiffSection::Unstaged => "u",
                    };

                    // Action buttons on the right side
                    let mut actions = div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(2.0))
                        .flex_shrink_0();

                    match section {
                        DiffSection::Unstaged => {
                            // Revert button
                            let entity_revert = cx.entity().clone();
                            actions = actions.child(
                                action_btn(format!("tree-revert-{idx}"), "\u{21BA}")
                                    .on_click(move |_, _window, cx| {
                                        entity_revert.update(cx, |panel, cx| {
                                            panel.revert_file(idx);
                                            cx.notify();
                                        });
                                        cx.stop_propagation();
                                    }),
                            );
                            // + button (stage)
                            let entity_stage = cx.entity().clone();
                            actions = actions.child(
                                action_btn(format!("tree-stage-{idx}"), "+")
                                    .on_click(move |_, _window, cx| {
                                        entity_stage.update(cx, |panel, cx| {
                                            panel.stage_file(idx);
                                            cx.notify();
                                        });
                                        cx.stop_propagation();
                                    }),
                            );
                        }
                        DiffSection::Staged => {
                            // - button (unstage)
                            let entity_unstage = cx.entity().clone();
                            actions = actions.child(
                                action_btn(format!("tree-unstage-{idx}"), "\u{2212}")
                                    .on_click(move |_, _window, cx| {
                                        entity_unstage.update(cx, |panel, cx| {
                                            panel.unstage_file(idx);
                                            cx.notify();
                                        });
                                        cx.stop_propagation();
                                    }),
                            );
                        }
                    }

                    container = container.child(
                        div()
                            .id(ElementId::Name(format!("tree-file-{sec_prefix}-{idx}").into()))
                            .group("tree-file-row")
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .h(px(24.0))
                            .pl(px((depth as f32) * 16.0 + 8.0))
                            .pr_2()
                            .cursor_pointer()
                            .hover(|s| s.bg(colors::surface_hover()))
                            .when(!is_active_section, |d: Stateful<Div>| {
                                d.text_color(colors::text_muted())
                            })
                            .on_click(move |_, _window, cx| {
                                entity.update(cx, |panel, cx| {
                                    if panel.active_section != section {
                                        panel.active_section = section;
                                        panel.expanded_context.clear();
                                    }
                                    panel.collapsed_files.remove(&(section, idx));
                                    panel.scroll_to_file = Some(idx);
                                    cx.notify();
                                });
                            })
                            .child({
                                let file_icon = util::icon_for_file(name);
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .child(file_icon),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(colors::accent())
                                            .child(name.clone()),
                                    )
                            })
                            .child(
                                actions
                                    .id(ElementId::Name(format!("tree-actions-{sec_prefix}-{idx}").into()))
                                    .opacity(0.0)
                                    .group_hover("tree-file-row", |s| s.opacity(1.0)),
                            ),
                    );
                }
            }
        }
        container
    }

    // ── File diff section ──

    fn render_file_diff(
        &self,
        file: &DiffFile,
        file_idx: usize,
        section: DiffSection,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let adds = file.additions();
        let dels = file.deletions();
        let file_key = (section, file_idx);
        let is_collapsed = self.collapsed_files.contains(&file_key);
        let entity_hdr = cx.entity().clone();
        let file_path = file.path.clone();

        let mut container = div()
            .id(ElementId::Name(format!("fdiff-{file_idx}").into()))
            .flex()
            .flex_col()
            .w_full()
            .mb_2();

        let status_color = match file.status {
            FileStatus::Added => colors::diff_added(),
            FileStatus::Modified => colors::accent(),
            FileStatus::Deleted => colors::diff_removed(),
            FileStatus::Renamed => colors::accent(),
        };

        // File diff header — always visible
        container = container.child(
            div()
                .id(ElementId::Name(format!("fhdr-{file_idx}").into()))
                .flex()
                .flex_row()
                .items_center()
                .w_full()
                .min_h(px(32.0))
                .px_3()
                .py(px(4.0))
                .bg(file_header_bg())
                .border_1()
                .border_color(colors::border())
                .when(is_collapsed, |d: Stateful<Div>| d.rounded_md())
                .when(!is_collapsed, |d: Stateful<Div>| d.rounded_t_md())
                .cursor_pointer()
                .hover(|s| s.bg(colors::surface_hover()))
                .on_click(move |_, _window, cx| {
                    entity_hdr.update(cx, |panel, cx| {
                        if panel.collapsed_files.contains(&file_key) {
                            panel.collapsed_files.remove(&file_key);
                        } else {
                            panel.collapsed_files.insert(file_key);
                        }
                        cx.notify();
                    });
                })
                // Status dot
                .child(
                    div()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded_full()
                        .bg(status_color)
                        .flex_shrink_0()
                        .mr_2(),
                )
                // File path + copy icon (grouped together, click copies)
                .child({
                    let path_for_copy = file.path.clone();
                    let just_copied = self.copied_file_key == Some(file_key);
                    let entity_copy = cx.entity().clone();
                    let (icon, icon_color) = if just_copied {
                        ("\u{2713}", colors::diff_added()) // checkmark in green
                    } else {
                        ("\u{2750}", colors::text_muted()) // copy icon
                    };
                    div()
                        .id(ElementId::Name(format!("fcopy-{file_idx}").into()))
                        .flex()
                        .flex_row()
                        .items_center()
                        .flex_shrink_0()
                        .gap(px(5.0))
                        .cursor_pointer()
                        .text_xs()
                        .border_b_1()
                        .border_color(gpui::rgba(0x00000000))
                        .hover(|s| s.border_color(colors::accent()))
                        .on_click(move |_, _window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                path_for_copy.clone(),
                            ));
                            let entity = entity_copy.clone();
                            entity_copy.update(cx, |panel, cx| {
                                panel.copied_file_key = Some(file_key);
                                cx.notify();
                            });
                            let fk = file_key;
                            cx.defer(move |cx| {
                                entity.update(cx, |panel, cx| {
                                    panel._copied_timer = Some(cx.spawn(async move |this, cx| {
                                        cx.background_executor()
                                            .timer(std::time::Duration::from_millis(1500))
                                            .await;
                                        let _ = cx.update(|cx| {
                                            let _ = this.update(cx, |panel, cx| {
                                                if panel.copied_file_key == Some(fk) {
                                                    panel.copied_file_key = None;
                                                }
                                                cx.notify();
                                            });
                                        });
                                    }));
                                });
                            });
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors::text())
                                .child(file_path),
                        )
                        .child(
                            div()
                                .text_color(icon_color)
                                .child(icon),
                        )
                })
                // Spacer so stats + actions stay on the right
                .child(div().flex_1())
                // Stats (+N -M)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_1()
                        .flex_shrink_0()
                        .ml_2()
                        .when(adds > 0, |d: Div| {
                            d.child(
                                div()
                                    .text_xs()
                                    .text_color(colors::diff_added())
                                    .child(format!("+{adds}")),
                            )
                        })
                        .when(dels > 0, |d: Div| {
                            d.child(
                                div()
                                    .text_xs()
                                    .text_color(colors::diff_removed())
                                    .child(format!("-{dels}")),
                            )
                        }),
                )
                // Action buttons
                .child({
                    let mut actions = div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(2.0))
                        .flex_shrink_0()
                        .ml_2();
                    match section {
                        DiffSection::Unstaged => {
                            let entity_revert = cx.entity().clone();
                            actions = actions.child(
                                action_btn(format!("hdr-revert-{file_idx}"), "\u{21BA}")
                                    .on_click(move |_, _window, cx| {
                                        entity_revert.update(cx, |panel, cx| {
                                            panel.revert_file(file_idx);
                                            cx.notify();
                                        });
                                        cx.stop_propagation();
                                    }),
                            );
                            let entity_stage = cx.entity().clone();
                            actions = actions.child(
                                action_btn(format!("hdr-stage-{file_idx}"), "+")
                                    .on_click(move |_, _window, cx| {
                                        entity_stage.update(cx, |panel, cx| {
                                            panel.stage_file(file_idx);
                                            cx.notify();
                                        });
                                        cx.stop_propagation();
                                    }),
                            );
                        }
                        DiffSection::Staged => {
                            let entity_unstage = cx.entity().clone();
                            actions = actions.child(
                                action_btn(format!("hdr-unstage-{file_idx}"), "\u{2212}")
                                    .on_click(move |_, _window, cx| {
                                        entity_unstage.update(cx, |panel, cx| {
                                            panel.unstage_file(file_idx);
                                            cx.notify();
                                        });
                                        cx.stop_propagation();
                                    }),
                            );
                        }
                    }
                    actions
                }),
        );

        // Diff body — only when expanded
        if !is_collapsed {
            let all_lines = self.flatten_file_lines(file, file_idx, section);
            container = container.child(self.render_line_blocks(&all_lines, file_idx, section, cx));
        }

        container
    }

    /// Flatten a file's hunks into a list of renderable segments.
    fn flatten_file_lines(&self, file: &DiffFile, file_idx: usize, section: DiffSection) -> Vec<LineSegment> {
        let mut segments: Vec<LineSegment> = Vec::new();

        for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
            let lines = &hunk.lines;
            if lines.is_empty() {
                continue;
            }

            let (extra_top, extra_bottom) = self
                .expanded_context
                .get(&(section, file_idx, hunk_idx))
                .copied()
                .unwrap_or((0, 0));

            let change_indices: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| !matches!(l.kind, DiffLineKind::Context))
                .map(|(i, _)| i)
                .collect();

            if change_indices.is_empty() {
                segments.push(LineSegment::CollapsedContext {
                    count: lines.len(),
                    file_idx,
                    hunk_idx,
                    direction: ExpandDirection::Top,
                });
                continue;
            }

            let first_change = *change_indices.first().unwrap();
            let last_change = *change_indices.last().unwrap();

            let visible_start = first_change.saturating_sub(DEFAULT_CONTEXT + extra_top);
            let visible_end = (last_change + DEFAULT_CONTEXT + extra_bottom + 1).min(lines.len());

            if visible_start > 0 {
                segments.push(LineSegment::CollapsedContext {
                    count: visible_start,
                    file_idx,
                    hunk_idx,
                    direction: ExpandDirection::Top,
                });
            }

            for line in &lines[visible_start..visible_end] {
                segments.push(LineSegment::Line(line.clone()));
            }

            let hidden_below = lines.len().saturating_sub(visible_end);
            if hidden_below > 0 {
                segments.push(LineSegment::CollapsedContext {
                    count: hidden_below,
                    file_idx,
                    hunk_idx,
                    direction: ExpandDirection::Bottom,
                });
            }
        }

        segments
    }

    fn render_line_blocks(
        &self,
        segments: &[LineSegment],
        file_idx: usize,
        section: DiffSection,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut block = div()
            .flex()
            .flex_col()
            .w_full()
            .border_l_1()
            .border_r_1()
            .border_b_1()
            .border_color(colors::border())
            .rounded_b_md()
            .overflow_hidden();

        for (seg_idx, seg) in segments.iter().enumerate() {
            match seg {
                LineSegment::Line(line) => {
                    block = block.child(self.render_diff_line(line));
                }
                LineSegment::CollapsedContext {
                    count,
                    hunk_idx,
                    direction,
                    ..
                } => {
                    let entity = cx.entity().clone();
                    let hunk_idx = *hunk_idx;
                    let dir = *direction;
                    let label = format!("{count} unmodified lines");

                    block = block.child(
                        div()
                            .id(ElementId::Name(
                                format!("collapse-{file_idx}-{seg_idx}").into(),
                            ))
                            .flex()
                            .flex_row()
                            .items_center()
                            .w_full()
                            .h(px(26.0))
                            .bg(collapse_bar_bg())
                            .border_t_1()
                            .border_b_1()
                            .border_color(colors::border())
                            .cursor_pointer()
                            .hover(|s| s.bg(colors::surface_hover()))
                            .on_click(move |_, _window, cx| {
                                entity.update(cx, |panel, cx| {
                                    let entry = panel
                                        .expanded_context
                                        .entry((section, file_idx, hunk_idx))
                                        .or_insert((0, 0));
                                    match dir {
                                        ExpandDirection::Top => entry.0 += EXPAND_STEP,
                                        ExpandDirection::Bottom => entry.1 += EXPAND_STEP,
                                    }
                                    cx.notify();
                                });
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .w(px(32.0))
                                    .flex_shrink_0()
                                    .text_color(colors::text_muted())
                                    .text_xs()
                                    .child("\u{25BC}")
                                    .child("\u{25B2}"),
                            )
                            .child(
                                div()
                                    .ml_1()
                                    .text_xs()
                                    .text_color(colors::text_muted())
                                    .child(label),
                            ),
                    );
                }
            }
        }

        block
    }

    fn render_diff_line(&self, line: &DiffLine) -> Div {
        let (row_bg, gutter_bg_color, text_col, prefix) = match line.kind {
            DiffLineKind::Added => (
                added_line_bg(),
                added_gutter_bg(),
                colors::diff_added(),
                "+",
            ),
            DiffLineKind::Removed => (
                removed_line_bg(),
                removed_gutter_bg(),
                colors::diff_removed(),
                "-",
            ),
            DiffLineKind::Context => (
                gpui::rgba(0x00000000),
                gutter_bg(),
                colors::text_muted(),
                " ",
            ),
        };

        let line_num = match line.kind {
            DiffLineKind::Removed => line.old_lineno,
            _ => line.new_lineno.or(line.old_lineno),
        };
        let line_num_str = line_num.map(|n| format!("{n}")).unwrap_or_default();

        div()
            .flex()
            .flex_row()
            .w_full()
            .min_h(px(20.0))
            .bg(row_bg)
            .font_family("Menlo")
            .text_xs()
            .child(
                div()
                    .w(px(44.0))
                    .flex_shrink_0()
                    .text_right()
                    .pr(px(8.0))
                    .pl(px(4.0))
                    .bg(gutter_bg_color)
                    .text_color(colors::text_muted())
                    .child(line_num_str),
            )
            .child(
                div()
                    .w(px(16.0))
                    .flex_shrink_0()
                    .text_color(text_col)
                    .pl(px(4.0))
                    .child(prefix.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_color(text_col)
                    .pl(px(4.0))
                    .child(line.content.trim_end().to_string()),
            )
    }
}

// ── Shared action button helper ──

fn action_btn(id: String, label: &str) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .flex()
        .items_center()
        .justify_center()
        .w(px(20.0))
        .h(px(20.0))
        .rounded(px(3.0))
        .text_size(px(12.0))
        .line_height(px(20.0))
        .font_weight(FontWeight::BOLD)
        .text_color(colors::text_muted())
        .cursor_pointer()
        .hover(|s| s.bg(colors::border()))
        .child(label.to_string())
}

// ── Segment types ──

#[derive(Clone, Copy)]
enum ExpandDirection {
    Top,
    Bottom,
}

enum LineSegment {
    Line(DiffLine),
    CollapsedContext {
        count: usize,
        file_idx: usize,
        hunk_idx: usize,
        direction: ExpandDirection,
    },
}

// ── File tree builder ──

enum TreeNode {
    Dir {
        name: String,
        full_path: String,
        children: Vec<TreeNode>,
    },
    File {
        name: String,
        file_idx: usize,
        status: FileStatus,
    },
}

fn build_file_tree(files: &[DiffFile]) -> Vec<TreeNode> {
    let mut root: Vec<TreeNode> = Vec::new();
    for (idx, file) in files.iter().enumerate() {
        let parts: Vec<&str> = file.path.split('/').collect();
        insert_into_tree(&mut root, &parts, idx, file.status.clone(), String::new());
    }
    root
}

fn insert_into_tree(
    nodes: &mut Vec<TreeNode>,
    parts: &[&str],
    file_idx: usize,
    status: FileStatus,
    parent_path: String,
) {
    if parts.is_empty() {
        return;
    }
    if parts.len() == 1 {
        nodes.push(TreeNode::File {
            name: parts[0].to_string(),
            file_idx,
            status,
        });
        return;
    }
    let dir_name = parts[0];
    let full_path = if parent_path.is_empty() {
        dir_name.to_string()
    } else {
        format!("{parent_path}/{dir_name}")
    };
    let dir_pos = nodes
        .iter()
        .position(|n| matches!(n, TreeNode::Dir { name, .. } if name == dir_name));
    if let Some(pos) = dir_pos {
        if let TreeNode::Dir { children, .. } = &mut nodes[pos] {
            insert_into_tree(children, &parts[1..], file_idx, status, full_path);
        }
    } else {
        let mut children = Vec::new();
        insert_into_tree(
            &mut children,
            &parts[1..],
            file_idx,
            status,
            full_path.clone(),
        );
        nodes.push(TreeNode::Dir {
            name: dir_name.to_string(),
            full_path,
            children,
        });
    }
}

// ── Render ──

impl Render for GitDiffPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total_file_count = self.staged_files.len() + self.unstaged_files.len();
        let total_adds = self.total_additions();
        let total_dels = self.total_deletions();

        let entity_move = cx.entity().clone();
        let entity_up = cx.entity().clone();

        let mut panel = div()
            .id("diff-panel")
            .flex()
            .flex_col()
            .w(px(self.width))
            .min_w(px(200.0))
            .h_full()
            .flex_shrink_0()
            .bg(colors::surface())
            .border_l_1()
            .border_color(colors::border())
            // Handle tree resize drag across the whole panel
            .on_mouse_move(move |event: &MouseMoveEvent, _window, cx| {
                entity_move.update(cx, |panel, cx| {
                    if panel.resizing_tree {
                        let delta = f32::from(event.position.x) - panel.tree_drag_start_x;
                        let new_w = (panel.tree_drag_start_width + delta).clamp(80.0, panel.width - 100.0);
                        panel.tree_width = new_w;
                        cx.notify();
                    }
                });
            })
            .on_mouse_up(
                MouseButton::Left,
                move |_: &MouseUpEvent, _window, cx| {
                    entity_up.update(cx, |panel, cx| {
                        if panel.resizing_tree {
                            panel.resizing_tree = false;
                            cx.notify();
                        }
                    });
                },
            );

        // ── Top summary header ──
        let section_label = match self.active_section {
            DiffSection::Staged => "Staged",
            DiffSection::Unstaged => "Unstaged",
        };
        let active_count = self.active_files().len();
        panel = panel.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .w_full()
                .min_h(px(36.0))
                .flex_shrink_0()
                .px_3()
                .py_1()
                .border_b_1()
                .border_color(colors::border())
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors::text())
                        .child(if total_file_count == 0 {
                            "No Changes".to_string()
                        } else {
                            format!("{section_label} \u{00B7} {active_count} files")
                        }),
                )
                .when(total_adds > 0 || total_dels > 0, |d: Div| {
                    d.child(
                        div()
                            .ml_2()
                            .flex()
                            .flex_row()
                            .gap_1()
                            .when(total_adds > 0, |d: Div| {
                                d.child(
                                    div()
                                        .text_xs()
                                        .text_color(colors::diff_added())
                                        .child(format!("+{total_adds}")),
                                )
                            })
                            .when(total_dels > 0, |d: Div| {
                                d.child(
                                    div()
                                        .text_xs()
                                        .text_color(colors::diff_removed())
                                        .child(format!("-{total_dels}")),
                                )
                            }),
                    )
                }),
        );

        if total_file_count == 0 {
            panel = panel.child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_color(colors::text_muted())
                    .text_sm()
                    .child("Working tree clean"),
            );
            return panel;
        }

        // ── Body: file tree | resize handle | diffs ──
        let tree_width = self.tree_width;
        let entity_down = cx.entity().clone();

        let mut body = div()
            .id("diff-panel-body")
            .flex()
            .flex_row()
            .flex_1()
            .overflow_hidden();

        // File tree sidebar
        let tree_panel = div()
            .id("diff-file-tree")
            .flex()
            .flex_col()
            .w(px(tree_width))
            .min_w(px(80.0))
            .h_full()
            .border_r_1()
            .border_color(colors::border())
            .overflow_y_scroll()
            .child(self.render_file_tree(cx));

        body = body.child(tree_panel);

        // Resize handle between tree and diff content
        body = body.child(
            div()
                .id("tree-resize-handle")
                .w(px(8.0))
                .mx(px(-3.0))
                .h_full()
                .flex_shrink_0()
                .cursor_col_resize()
                .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _window, cx| {
                    entity_down.update(cx, |panel, cx| {
                        panel.resizing_tree = true;
                        panel.tree_drag_start_x = f32::from(event.position.x);
                        panel.tree_drag_start_width = panel.tree_width;
                        cx.notify();
                    });
                }),
        );

        // Diff content (scrollable) — only shows active section
        let section = self.active_section;
        let files = self.active_files().to_vec();
        let mut diff_content = div()
            .id("diff-panel-content")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(100.0))
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .p_2();

        for (idx, file) in files.iter().enumerate() {
            diff_content = diff_content.child(self.render_file_diff(file, idx, section, cx));
        }

        // Handle scroll-to-file request
        if let Some(target_idx) = self.scroll_to_file.take() {
            self.scroll_handle.scroll_to_item(target_idx);
        }

        body = body.child(diff_content);
        panel = panel.child(body);
        panel
    }
}
