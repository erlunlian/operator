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
/// How many files to render initially before showing "Load more".
const FILE_RENDER_BATCH: usize = 20;

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
    list_state: ListState,
    /// Which file just had its path copied (shows checkmark briefly).
    copied_file_key: Option<(DiffSection, usize)>,
    /// Timer handle to clear the copied indicator.
    _copied_timer: Option<Task<()>>,
    /// Index to scroll to after next render.
    scroll_to_file: Option<usize>,
    /// When set, shows a confirmation dialog before reverting.
    pending_revert: Option<RevertTarget>,
    /// Max number of files to render (grows when user clicks "Load more").
    rendered_file_limit: usize,

    // Line selection for copy
    copy_selecting: bool,
    copy_anchor_line: Option<usize>,
    copy_end_line: Option<usize>,
    copy_line_contents: Vec<String>,

    // Virtual scroll cache
    /// Pre-flattened line segments per file (indexed by file_idx within active section).
    cached_file_segments: Vec<Vec<LineSegment>>,
    /// Flat row descriptors for the list.
    flat_rows: Vec<FlatRow>,
    /// file_idx → index of its FileHeader row in flat_rows.
    flat_file_starts: Vec<usize>,
    /// Whether the flat cache needs rebuilding before next render.
    flat_cache_dirty: bool,
}

#[derive(Clone, Copy)]
enum RevertTarget {
    Single(usize),
    All,
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
            list_state: ListState::new(0, ListAlignment::Top, px(200.0)),
            copied_file_key: None,
            _copied_timer: None,
            scroll_to_file: None,
            pending_revert: None,
            rendered_file_limit: FILE_RENDER_BATCH,
            copy_selecting: false,
            copy_anchor_line: None,
            copy_end_line: None,
            copy_line_contents: Vec::new(),
            cached_file_segments: Vec::new(),
            flat_rows: Vec::new(),
            flat_file_starts: Vec::new(),
            flat_cache_dirty: true,
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
            list_state: ListState::new(0, ListAlignment::Top, px(200.0)),
            copied_file_key: None,
            _copied_timer: None,
            scroll_to_file: None,
            pending_revert: None,
            rendered_file_limit: FILE_RENDER_BATCH,
            copy_selecting: false,
            copy_anchor_line: None,
            copy_end_line: None,
            copy_line_contents: Vec::new(),
            cached_file_segments: Vec::new(),
            flat_rows: Vec::new(),
            flat_file_starts: Vec::new(),
            flat_cache_dirty: true,
        }
    }

    /// Estimate total heap bytes for all cached diff data.
    pub fn estimated_bytes(&self) -> usize {
        let staged: usize = self.staged_files.iter().map(|f| f.estimated_bytes()).sum();
        let unstaged: usize = self.unstaged_files.iter().map(|f| f.estimated_bytes()).sum();
        staged + unstaged
    }

    /// Number of cached diff files (staged + unstaged).
    pub fn file_count(&self) -> usize {
        self.staged_files.len() + self.unstaged_files.len()
    }

    pub fn git_dir(&self) -> Option<std::path::PathBuf> {
        self.repo.as_ref().map(|r| r.git_dir())
    }

    fn is_line_selected(&self, global_idx: usize) -> bool {
        if let (Some(a), Some(e)) = (self.copy_anchor_line, self.copy_end_line) {
            let lo = a.min(e);
            let hi = a.max(e);
            global_idx >= lo && global_idx <= hi
        } else {
            false
        }
    }

    fn copy_selected_text(&self, cx: &mut Context<Self>) {
        let (Some(a), Some(e)) = (self.copy_anchor_line, self.copy_end_line) else {
            return;
        };
        let lo = a.min(e);
        let hi = a.max(e);
        let text: String = self
            .copy_line_contents
            .iter()
            .enumerate()
            .filter(|(i, _)| *i >= lo && *i <= hi)
            .map(|(_, s)| s.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub fn refresh(&mut self) {
        if let Some(repo) = &self.repo {
            self.branch = repo.current_branch();
            self.staged_files = repo.staged_diff();
            self.unstaged_files = repo.unstaged_diff();
            // Keep expanded_context — expand state should persist across refreshes.
            // It only resets on structural changes (stage/unstage/revert) via
            // reset_expanded_context().
            self.flat_cache_dirty = true;
        }
    }

    /// Clear expanded context state. Called after structural changes
    /// (stage, unstage, revert) that shift file indices.
    fn reset_expanded_context(&mut self) {
        self.expanded_context.clear();
        self.rendered_file_limit = FILE_RENDER_BATCH;
        self.flat_cache_dirty = true;
    }

    fn stage_file(&mut self, file_idx: usize) {
        if let Some(repo) = &self.repo {
            if let Some(file) = self.unstaged_files.get(file_idx) {
                let path = file.path.clone();
                if repo.stage_file(&path).is_ok() {
                    self.refresh();
                    self.reset_expanded_context();
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
                    self.reset_expanded_context();
                }
            }
        }
    }

    fn revert_file(&mut self, file_idx: usize) {
        if let Some(repo) = &self.repo {
            if let Some(file) = self.unstaged_files.get(file_idx) {
                let path = file.path.clone();
                let status = file.status.clone();
                if repo.revert_file(&path, &status).is_ok() {
                    self.refresh();
                    self.reset_expanded_context();
                }
            }
        }
    }

    fn revert_all_files(&mut self) {
        if let Some(repo) = &self.repo {
            let files: Vec<_> = self
                .unstaged_files
                .iter()
                .map(|f| (f.path.clone(), f.status.clone()))
                .collect();
            for (path, status) in &files {
                let _ = repo.revert_file(path, status);
            }
            self.refresh();
            self.reset_expanded_context();
        }
    }

    fn stage_all_files(&mut self) {
        if let Some(repo) = &self.repo {
            let paths: Vec<_> = self.unstaged_files.iter().map(|f| f.path.clone()).collect();
            for path in &paths {
                let _ = repo.stage_file(path);
            }
            self.refresh();
            self.reset_expanded_context();
        }
    }

    fn unstage_all_files(&mut self) {
        if let Some(repo) = &self.repo {
            let paths: Vec<_> = self.staged_files.iter().map(|f| f.path.clone()).collect();
            for path in &paths {
                let _ = repo.unstage_file(path);
            }
            self.refresh();
            self.reset_expanded_context();
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

        let mut header = div()
            .id(ElementId::Name(id_str.into()))
            .group("section-header")
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
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
                    match section {
                        DiffSection::Staged => panel.staged_tree_collapsed = !panel.staged_tree_collapsed,
                        DiffSection::Unstaged => panel.unstaged_tree_collapsed = !panel.unstaged_tree_collapsed,
                    }
                    if count > 0 {
                        panel.active_section = section;
                        panel.expanded_context.clear();
                        panel.flat_cache_dirty = true;
                    }
                    cx.notify();
                });
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(8.0))
                            .text_color(colors::text_muted())
                            .w(px(10.0))
                            .child(chevron.to_string()),
                    )
                    .child(
                        div()
                            .ml_1()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors::text())
                            .child(label),
                    ),
            );

        // Bulk action buttons (shown on hover, only when section has files)
        if count > 0 {
            let mut actions = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .flex_shrink_0()
                .opacity(0.0)
                .group_hover("section-header", |s| s.opacity(1.0));

            match section {
                DiffSection::Unstaged => {
                    let entity_revert_all = cx.entity().clone();
                    actions = actions.child(
                        action_btn("tree-revert-all".to_string(), "\u{21BA}")
                            .on_click(move |_, _window, cx| {
                                entity_revert_all.update(cx, |panel, cx| {
                                    panel.pending_revert = Some(RevertTarget::All);
                                    cx.notify();
                                });
                                cx.stop_propagation();
                            }),
                    );
                    let entity_stage_all = cx.entity().clone();
                    actions = actions.child(
                        action_btn("tree-stage-all".to_string(), "+")
                            .on_click(move |_, _window, cx| {
                                entity_stage_all.update(cx, |panel, cx| {
                                    panel.stage_all_files();
                                    cx.notify();
                                });
                                cx.stop_propagation();
                            }),
                    );
                }
                DiffSection::Staged => {
                    let entity_unstage_all = cx.entity().clone();
                    actions = actions.child(
                        action_btn("tree-unstage-all".to_string(), "\u{2212}")
                            .on_click(move |_, _window, cx| {
                                entity_unstage_all.update(cx, |panel, cx| {
                                    panel.unstage_all_files();
                                    cx.notify();
                                });
                                cx.stop_propagation();
                            }),
                    );
                }
            }

            header = header.child(actions);
        }

        header
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
                                    .text_size(px(8.0))
                                    .text_color(colors::text_muted())
                                    .w(px(10.0))
                                    .child(chevron.to_string()),
                            )
                            .child(
                                div()
                                    .font_family(util::ICON_FONT)
                                    .text_size(px(14.0))
                                    .text_color(colors::text_muted())
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
                    _status: _,
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
                            // Revert button (shows confirmation dialog)
                            let entity_revert = cx.entity().clone();
                            actions = actions.child(
                                action_btn(format!("tree-revert-{idx}"), "\u{21BA}")
                                    .on_click(move |_, _window, cx| {
                                        entity_revert.update(cx, |panel, cx| {
                                            panel.pending_revert = Some(RevertTarget::Single(idx));
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
                                    // Ensure the file is within the rendered batch
                                    if idx >= panel.rendered_file_limit {
                                        panel.rendered_file_limit = idx + 1;
                                    }
                                    panel.scroll_to_file = Some(idx);
                                    panel.flat_cache_dirty = true;
                                    cx.notify();
                                });
                            })
                            .child({
                                let file_icon = util::icon_for_file(name);
                                let icon_color = util::file_icon_color(name);
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .font_family(util::ICON_FONT)
                                            .text_size(px(14.0))
                                            .text_color(icon_color)
                                            .w(px(16.0))
                                            .flex_shrink_0()
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

    // ── Virtual scroll cache ──

    /// Rebuild the flat row cache from current state.
    /// Called at the start of render when `flat_cache_dirty` is true.
    fn rebuild_flat_cache(&mut self) {
        let section = self.active_section;
        let files = self.active_files();
        let render_limit = self.rendered_file_limit.min(files.len());

        // Phase 1: flatten segments for each file
        let mut all_segments: Vec<Vec<LineSegment>> = Vec::with_capacity(render_limit);
        for file_idx in 0..render_limit {
            let file_key = (section, file_idx);
            if self.collapsed_files.contains(&file_key) || files[file_idx].hunks.is_empty() {
                all_segments.push(Vec::new());
            } else {
                let segs = self.flatten_file_lines(&files[file_idx].clone(), file_idx, section);
                all_segments.push(segs);
            }
        }

        // Phase 2: build flat rows
        self.flat_rows.clear();
        self.flat_file_starts.clear();
        let mut copy_texts: Vec<String> = Vec::new();
        let mut global_line_idx = 0usize;
        let files_len = self.active_files().len();

        for file_idx in 0..render_limit {
            self.flat_file_starts.push(self.flat_rows.len());
            self.flat_rows.push(FlatRow::FileHeader { file_idx });

            let file_key = (section, file_idx);
            if self.collapsed_files.contains(&file_key) {
                continue;
            }

            if self.active_files()[file_idx].hunks.is_empty() {
                self.flat_rows.push(FlatRow::EmptyFile { file_idx });
                continue;
            }

            let segments = &all_segments[file_idx];
            let num_segs = segments.len();

            for (seg_idx, seg) in segments.iter().enumerate() {
                let is_last = seg_idx == num_segs - 1;
                match seg {
                    LineSegment::Line(line) => {
                        copy_texts.push(line.content.trim_end().to_string());
                        self.flat_rows.push(FlatRow::Line {
                            file_idx,
                            seg_idx,
                            global_line_idx,
                            is_last_in_file: is_last,
                        });
                        global_line_idx += 1;
                    }
                    LineSegment::CollapsedContext {
                        count,
                        hunk_idx,
                        direction,
                        ..
                    } => {
                        self.flat_rows.push(FlatRow::CollapsedContext {
                            file_idx,
                            seg_idx,
                            count: *count,
                            hunk_idx: *hunk_idx,
                            direction: *direction,
                            is_last_in_file: is_last,
                        });
                    }
                }
            }
        }

        let remaining = files_len.saturating_sub(render_limit);
        if remaining > 0 {
            self.flat_rows.push(FlatRow::LoadMore { remaining });
        }

        self.cached_file_segments = all_segments;
        self.copy_line_contents = copy_texts;
        self.list_state.reset(self.flat_rows.len());
        self.flat_cache_dirty = false;
    }

    /// Render a single flat row. Called from the list callback.
    fn render_flat_row(&self, row_idx: usize, entity: &Entity<Self>) -> AnyElement {
        match &self.flat_rows[row_idx] {
            FlatRow::FileHeader { file_idx } => self.render_file_header(*file_idx, entity),
            FlatRow::EmptyFile { file_idx } => self.render_empty_file(*file_idx),
            FlatRow::Line {
                file_idx,
                seg_idx,
                global_line_idx,
                is_last_in_file,
            } => {
                let seg = &self.cached_file_segments[*file_idx][*seg_idx];
                if let LineSegment::Line(line) = seg {
                    self.render_line_row(line, *file_idx, *global_line_idx, *is_last_in_file, entity)
                } else {
                    div().into_any_element()
                }
            }
            FlatRow::CollapsedContext {
                file_idx,
                seg_idx: _,
                count,
                hunk_idx,
                direction,
                is_last_in_file,
            } => self.render_collapsed_row(
                *file_idx,
                *count,
                *hunk_idx,
                *direction,
                *is_last_in_file,
                row_idx,
                entity,
            ),
            FlatRow::LoadMore { remaining } => div()
                .w_full()
                .py_2()
                .flex()
                .justify_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(colors::text_muted())
                        .child(format!("{remaining} more files...")),
                )
                .into_any_element(),
        }
    }

    fn render_file_header(&self, file_idx: usize, entity: &Entity<Self>) -> AnyElement {
        let files = self.active_files();
        let file = &files[file_idx];
        let adds = file.additions();
        let dels = file.deletions();
        let file_key = (self.active_section, file_idx);
        let is_collapsed = self.collapsed_files.contains(&file_key);
        let file_path = file.path.clone();
        let section = self.active_section;

        let status_color = match file.status {
            FileStatus::Added => colors::diff_added(),
            FileStatus::Modified => colors::accent(),
            FileStatus::Deleted => colors::diff_removed(),
            FileStatus::Renamed => colors::accent(),
        };

        let entity_hdr = entity.clone();
        let just_copied = self.copied_file_key == Some(file_key);
        let (icon, icon_color) = if just_copied {
            ("\u{2713}", colors::diff_added())
        } else {
            ("\u{2750}", colors::text_muted())
        };
        let path_for_copy = file.path.clone();
        let entity_copy = entity.clone();

        div()
            .id(ElementId::Name(format!("fdiff-hdr-{file_idx}").into()))
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
                    panel.flat_cache_dirty = true;
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
            // File path + copy icon
            .child({
                div()
                    .id(ElementId::Name(format!("fdiff-fcopy-{file_idx}").into()))
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
                        cx.write_to_clipboard(ClipboardItem::new_string(path_for_copy.clone()));
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
                    .child(div().text_color(icon_color).child(icon))
            })
            // Spacer
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
                        let entity_revert = entity.clone();
                        actions = actions.child(
                            action_btn(format!("fdiff-revert-{file_idx}"), "\u{21BA}")
                                .on_click(move |_, _window, cx| {
                                    entity_revert.update(cx, |panel, cx| {
                                        panel.pending_revert = Some(RevertTarget::Single(file_idx));
                                        cx.notify();
                                    });
                                    cx.stop_propagation();
                                }),
                        );
                        let entity_stage = entity.clone();
                        actions = actions.child(
                            action_btn(format!("fdiff-stage-{file_idx}"), "+")
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
                        let entity_unstage = entity.clone();
                        actions = actions.child(
                            action_btn(format!("fdiff-unstage-{file_idx}"), "\u{2212}")
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
            })
            .into_any_element()
    }

    fn render_empty_file(&self, file_idx: usize) -> AnyElement {
        div()
            .id(ElementId::Name(format!("fdiff-empty-{file_idx}").into()))
            .w_full()
            .px_3()
            .py_3()
            .bg(colors::surface())
            .border_1()
            .border_t_0()
            .border_color(colors::border())
            .rounded_b_md()
            .text_xs()
            .text_color(colors::text_muted())
            .child("This file has no content")
            .into_any_element()
    }

    fn render_line_row(
        &self,
        line: &DiffLine,
        _file_idx: usize,
        global_line_idx: usize,
        is_last_in_file: bool,
        entity: &Entity<Self>,
    ) -> AnyElement {
        let (row_bg, gutter_bg_color, text_col, prefix) = match line.kind {
            DiffLineKind::Added => (added_line_bg(), added_gutter_bg(), colors::diff_added(), "+"),
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
        let content = line.content.trim_end();

        let content_el: AnyElement = if let Some(highlights) = line.highlights.as_ref() {
            if highlights.is_empty() {
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_color(text_col)
                    .pl(px(4.0))
                    .child(content.to_string())
                    .into_any_element()
            } else {
                let hl: Vec<(std::ops::Range<usize>, HighlightStyle)> = highlights
                    .iter()
                    .filter(|s| {
                        s.byte_range.end <= content.len()
                            && content.is_char_boundary(s.byte_range.start)
                            && content.is_char_boundary(s.byte_range.end)
                    })
                    .map(|s| {
                        (
                            s.byte_range.clone(),
                            HighlightStyle {
                                color: Some(Hsla::from(s.color)),
                                ..Default::default()
                            },
                        )
                    })
                    .collect();
                let text = SharedString::from(content.to_string());
                let styled = StyledText::new(text).with_highlights(hl);
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_color(text_col)
                    .pl(px(4.0))
                    .child(styled)
                    .into_any_element()
            }
        } else {
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_color(text_col)
                .pl(px(4.0))
                .child(content.to_string())
                .into_any_element()
        };

        let line_selected = self.is_line_selected(global_line_idx);
        let final_bg = if line_selected { rgba(0x89b4fa30) } else { row_bg };

        let entity_sel_down = entity.clone();
        let entity_sel_move = entity.clone();
        let gli = global_line_idx;

        div()
            .flex()
            .flex_row()
            .w_full()
            .min_h(px(20.0))
            .bg(final_bg)
            .font_family("Menlo")
            .text_xs()
            .border_l_1()
            .border_r_1()
            .border_color(colors::border())
            .when(is_last_in_file, |d: Div| d.border_b_1().rounded_b_md())
            .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                entity_sel_down.update(cx, |panel, cx| {
                    panel.copy_anchor_line = Some(gli);
                    panel.copy_end_line = Some(gli);
                    panel.copy_selecting = true;
                    cx.notify();
                });
            })
            .on_mouse_move(move |_, _window, cx| {
                entity_sel_move.update(cx, |panel, cx| {
                    if panel.copy_selecting && panel.copy_end_line != Some(gli) {
                        panel.copy_end_line = Some(gli);
                        cx.notify();
                    }
                });
            })
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
            .child(content_el)
            .into_any_element()
    }

    fn render_collapsed_row(
        &self,
        file_idx: usize,
        count: usize,
        hunk_idx: usize,
        direction: ExpandDirection,
        is_last_in_file: bool,
        row_idx: usize,
        entity: &Entity<Self>,
    ) -> AnyElement {
        let entity = entity.clone();
        let label = format!("{count} unmodified lines");
        let dir = direction;
        let section = self.active_section;

        div()
            .id(ElementId::Name(
                format!("fdiff-collapse-{file_idx}-{row_idx}").into(),
            ))
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .w_full()
            .h(px(22.0))
            .bg(collapse_bar_bg())
            .border_1()
            .border_color(colors::border())
            .when(is_last_in_file, |d: Stateful<Div>| d.rounded_b_md())
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
                    panel.flat_cache_dirty = true;
                    cx.notify();
                });
            })
            .child(
                div()
                    .text_xs()
                    .text_color(colors::text_muted())
                    .child(label),
            )
            .into_any_element()
    }

    /// Flatten a file's hunks into a list of renderable segments.
    ///
    /// When `source_lines` is available on the file, expanding context can reach
    /// beyond hunk boundaries into the full file, and inter-hunk gaps are shown
    /// as collapsible bars.
    fn flatten_file_lines(&self, file: &DiffFile, file_idx: usize, section: DiffSection) -> Vec<LineSegment> {
        let source = file.source_lines.as_deref();
        let total_source = source.map(|s| s.len()).unwrap_or(0);
        let mut segments: Vec<LineSegment> = Vec::new();
        // Track last new_lineno shown (1-indexed), for inter-hunk gap computation.
        let mut last_shown_ln: usize = 0;

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

            // New-side file line range of this hunk.
            let hunk_start_ln = lines.iter().find_map(|l| l.new_lineno).unwrap_or(1) as usize;
            let hunk_end_ln = lines.iter().rev().find_map(|l| l.new_lineno).unwrap_or(1) as usize;

            let change_indices: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| !matches!(l.kind, DiffLineKind::Context))
                .map(|(i, _)| i)
                .collect();

            if change_indices.is_empty() {
                if source.is_some() {
                    let hidden = hunk_end_ln.saturating_sub(last_shown_ln);
                    if hidden > 0 {
                        segments.push(LineSegment::CollapsedContext {
                            count: hidden,
                            _file_idx: file_idx,
                            hunk_idx,
                            direction: ExpandDirection::Top,
                        });
                    }
                    last_shown_ln = hunk_end_ln;
                } else {
                    segments.push(LineSegment::CollapsedContext {
                        count: lines.len(),
                        _file_idx: file_idx,
                        hunk_idx,
                        direction: ExpandDirection::Top,
                    });
                }
                continue;
            }

            let first_change = *change_indices.first().unwrap();
            let last_change = *change_indices.last().unwrap();

            // Hunk-local visible range (clamped to hunk bounds).
            let hunk_vis_start = first_change.saturating_sub(DEFAULT_CONTEXT + extra_top);
            let hunk_vis_end = (last_change + DEFAULT_CONTEXT + extra_bottom + 1).min(lines.len());

            // How many extra lines we want beyond the hunk boundaries.
            let overflow_top = (DEFAULT_CONTEXT + extra_top).saturating_sub(first_change);
            let hunk_bottom_ctx = lines.len().saturating_sub(last_change + 1);
            let overflow_bottom = (DEFAULT_CONTEXT + extra_bottom).saturating_sub(hunk_bottom_ctx);

            if let Some(source) = source {
                // How far back into the file to reach.
                let source_top_ln = if overflow_top > 0 {
                    hunk_start_ln
                        .saturating_sub(overflow_top)
                        .max(last_shown_ln + 1)
                        .max(1)
                } else {
                    hunk_start_ln
                };

                // Inter-hunk / pre-hunk gap collapse bar.
                let gap = source_top_ln.saturating_sub(last_shown_ln + 1);
                if gap > 0 {
                    segments.push(LineSegment::CollapsedContext {
                        count: gap,
                        _file_idx: file_idx,
                        hunk_idx,
                        direction: ExpandDirection::Top,
                    });
                }

                // Source lines before the hunk (expanded beyond its boundary).
                if overflow_top > 0 {
                    for ln in source_top_ln..hunk_start_ln {
                        let idx = ln - 1;
                        if let Some(sl) = source.get(idx) {
                            segments.push(LineSegment::Line(DiffLine {
                                kind: DiffLineKind::Context,
                                content: sl.content.clone(),
                                old_lineno: None,
                                new_lineno: Some(ln as u32),
                                highlights: sl.highlights.clone(),
                            }));
                        }
                    }
                }

                // Within-hunk hidden top (when we haven't overflowed).
                if hunk_vis_start > 0 {
                    segments.push(LineSegment::CollapsedContext {
                        count: hunk_vis_start,
                        _file_idx: file_idx,
                        hunk_idx,
                        direction: ExpandDirection::Top,
                    });
                }

                // Visible hunk lines.
                for line in &lines[hunk_vis_start..hunk_vis_end] {
                    segments.push(LineSegment::Line(line.clone()));
                }

                // Within-hunk hidden bottom.
                let hidden_in_hunk_below = lines.len().saturating_sub(hunk_vis_end);
                if hidden_in_hunk_below > 0 {
                    segments.push(LineSegment::CollapsedContext {
                        count: hidden_in_hunk_below,
                        _file_idx: file_idx,
                        hunk_idx,
                        direction: ExpandDirection::Bottom,
                    });
                }

                // Source lines after the hunk (expanded beyond its boundary).
                let source_bottom_ln = if overflow_bottom > 0 {
                    (hunk_end_ln + overflow_bottom).min(total_source)
                } else {
                    hunk_end_ln
                };
                if overflow_bottom > 0 {
                    for ln in (hunk_end_ln + 1)..=source_bottom_ln {
                        let idx = ln - 1;
                        if let Some(sl) = source.get(idx) {
                            segments.push(LineSegment::Line(DiffLine {
                                kind: DiffLineKind::Context,
                                content: sl.content.clone(),
                                old_lineno: None,
                                new_lineno: Some(ln as u32),
                                highlights: sl.highlights.clone(),
                            }));
                        }
                    }
                }

                last_shown_ln = source_bottom_ln;
            } else {
                // No source lines — original within-hunk-only logic.
                if hunk_vis_start > 0 {
                    segments.push(LineSegment::CollapsedContext {
                        count: hunk_vis_start,
                        _file_idx: file_idx,
                        hunk_idx,
                        direction: ExpandDirection::Top,
                    });
                }

                for line in &lines[hunk_vis_start..hunk_vis_end] {
                    segments.push(LineSegment::Line(line.clone()));
                }

                let hidden_below = lines.len().saturating_sub(hunk_vis_end);
                if hidden_below > 0 {
                    segments.push(LineSegment::CollapsedContext {
                        count: hidden_below,
                        _file_idx: file_idx,
                        hunk_idx,
                        direction: ExpandDirection::Bottom,
                    });
                }
            }
        }

        // Collapse bar for file lines after the last hunk.
        if source.is_some() && total_source > last_shown_ln && !file.hunks.is_empty() {
            let remaining = total_source - last_shown_ln;
            if remaining > 0 {
                segments.push(LineSegment::CollapsedContext {
                    count: remaining,
                    _file_idx: file_idx,
                    hunk_idx: file.hunks.len() - 1,
                    direction: ExpandDirection::Bottom,
                });
            }
        }

        segments
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
        #[allow(dead_code)]
        _file_idx: usize,
        hunk_idx: usize,
        direction: ExpandDirection,
    },
}

/// Row descriptor for the virtualized flat diff list.
enum FlatRow {
    FileHeader { file_idx: usize },
    EmptyFile { file_idx: usize },
    Line {
        file_idx: usize,
        seg_idx: usize,
        global_line_idx: usize,
        is_last_in_file: bool,
    },
    CollapsedContext {
        file_idx: usize,
        #[allow(dead_code)]
        seg_idx: usize,
        count: usize,
        hunk_idx: usize,
        direction: ExpandDirection,
        is_last_in_file: bool,
    },
    LoadMore { remaining: usize },
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
        _status: FileStatus,
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
            _status: status,
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
        // Rebuild the flat row cache if data has changed
        if self.flat_cache_dirty {
            self.rebuild_flat_cache();
        }

        let total_file_count = self.staged_files.len() + self.unstaged_files.len();
        let total_adds = self.total_additions();
        let total_dels = self.total_deletions();

        let entity_move = cx.entity().clone();
        let entity_up = cx.entity().clone();

        let mut panel = div()
            .id("diff-panel")
            .flex()
            .flex_col()
            .w_full()
            .flex_1()
            .h_full()
            .bg(colors::surface())
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
                        let mut changed = false;
                        if panel.resizing_tree {
                            panel.resizing_tree = false;
                            changed = true;
                        }
                        if panel.copy_selecting {
                            panel.copy_selecting = false;
                            panel.copy_selected_text(cx);
                            changed = true;
                        }
                        if changed {
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

        // Diff content — virtualized with list (supports variable row heights)
        let entity_list = cx.entity().clone();

        let diff_content = list(
            self.list_state.clone(),
            move |ix, _window, cx| {
                let panel = entity_list.read(cx);
                panel.render_flat_row(ix, &entity_list)
            },
        )
        .flex_1()
        .min_w(px(100.0))
        .p_2();

        // Auto-load more files if there are un-rendered files
        let render_limit = self.rendered_file_limit.min(self.active_files().len());
        let remaining = self.active_files().len().saturating_sub(render_limit);
        if remaining > 0 {
            let entity_more = cx.entity().clone();
            cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let _ = cx.update(|cx| {
                    let _ = entity_more.update(cx, |panel, cx| {
                        let total = panel.active_files().len();
                        if panel.rendered_file_limit < total {
                            panel.rendered_file_limit += FILE_RENDER_BATCH;
                            panel.flat_cache_dirty = true;
                            cx.notify();
                        }
                    });
                });
            })
            .detach();
        }

        // Handle scroll-to-file: convert file_idx to flat row index
        if let Some(target_file) = self.scroll_to_file.take() {
            if let Some(&row_idx) = self.flat_file_starts.get(target_file) {
                self.list_state.scroll_to_reveal_item(row_idx);
            }
        }

        body = body.child(diff_content);
        panel = panel.child(body);

        // ── Revert confirmation dialog ──
        if let Some(revert_target) = self.pending_revert {
            let (message, action_label) = match revert_target {
                RevertTarget::Single(idx) => {
                    if let Some(file) = self.unstaged_files.get(idx) {
                        let name = file.path.rsplit('/').next().unwrap_or(&file.path).to_string();
                        if matches!(file.status, FileStatus::Added) {
                            (format!("Delete \"{name}\"? This file is untracked and will be permanently removed."), "Delete")
                        } else {
                            (format!("Revert \"{name}\"? All unsaved changes will be lost."), "Revert")
                        }
                    } else {
                        ("Revert this file?".to_string(), "Revert")
                    }
                }
                RevertTarget::All => {
                    let count = self.unstaged_files.len();
                    (format!("Revert all {count} unstaged files? All unsaved changes will be lost and untracked files will be deleted."), "Revert All")
                }
            };

            let entity_confirm = cx.entity().clone();
            let entity_cancel = cx.entity().clone();
            let entity_backdrop = cx.entity().clone();

            panel = panel.child(
                div()
                    .id("revert-confirm-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(rgba(0x00000088u32))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_click(move |_, _window, cx| {
                        entity_backdrop.update(cx, |panel, cx| {
                            panel.pending_revert = None;
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .id("revert-confirm-dialog")
                            .w(px(320.0))
                            .bg(colors::surface())
                            .border_1()
                            .border_color(colors::border())
                            .rounded(px(8.0))
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .on_click(|_, _window, cx| {
                                cx.stop_propagation();
                            })
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors::text())
                                    .child("Confirm"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors::text_muted())
                                    .child(message),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .justify_end()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("revert-cancel-btn")
                                            .px_3()
                                            .py(px(6.0))
                                            .rounded(px(4.0))
                                            .text_xs()
                                            .text_color(colors::text_muted())
                                            .bg(colors::surface_hover())
                                            .cursor_pointer()
                                            .hover(|s| s.text_color(colors::text()))
                                            .child("Cancel")
                                            .on_click(move |_, _window, cx| {
                                                entity_cancel.update(cx, |panel, cx| {
                                                    panel.pending_revert = None;
                                                    cx.notify();
                                                });
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id("revert-confirm-btn")
                                            .px_3()
                                            .py(px(6.0))
                                            .rounded(px(4.0))
                                            .text_xs()
                                            .text_color(gpui::rgb(0xffffff))
                                            .bg(colors::diff_removed())
                                            .cursor_pointer()
                                            .hover(|s| s.opacity(0.8))
                                            .child(action_label)
                                            .on_click(move |_, _window, cx| {
                                                entity_confirm.update(cx, |panel, cx| {
                                                    if let Some(target) = panel.pending_revert.take() {
                                                        match target {
                                                            RevertTarget::Single(idx) => panel.revert_file(idx),
                                                            RevertTarget::All => panel.revert_all_files(),
                                                        }
                                                    }
                                                    cx.notify();
                                                });
                                            }),
                                    ),
                            ),
                    ),
            );
        }

        panel
    }
}
