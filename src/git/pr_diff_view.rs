use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use super::diff_model::*;
use super::git_repo::GitRepo;
use super::github::{self, CheckKind, GhStatus, MergeMethod, PrInfo, PrReviewComment};
use super::markdown;
use crate::actions::FindInFile;
use crate::text_input::TextInput;
use crate::theme::colors;
use crate::ui::scrollbar::{self, ScrollbarState};
use crate::util;

/// How many context lines around each change to show by default.
const DEFAULT_CONTEXT: usize = 3;
/// How many extra context lines to reveal per click.
const EXPAND_STEP: usize = 20;
/// Minimum file-tree sidebar width in pixels.
const MIN_TREE_WIDTH: f32 = 40.0;
/// Maximum file-tree sidebar width in pixels.
const MAX_TREE_WIDTH: f32 = 600.0;

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
fn comment_bg() -> Rgba {
    rgba(0x89b4fa18)
}
fn comment_border() -> Rgba {
    rgba(0x89b4fa40)
}

/// Icon char (Nerd Font) + color for a check's normalized state.
fn check_icon_style(kind: CheckKind) -> (&'static str, Rgba) {
    match kind {
        // nf-fa-check, nf-fa-close, nf-fa-dot_circle_o, nf-fa-minus
        CheckKind::Success => ("\u{f00c}", colors::diff_added()),
        CheckKind::Failure => ("\u{f00d}", colors::diff_removed()),
        CheckKind::InProgress => ("\u{f192}", rgb(0xf9e2af)),
        CheckKind::Skipped => ("\u{f068}", colors::text_muted()),
    }
}

/// Merge potentially-overlapping highlight ranges into a sorted, non-overlapping list.
/// GPUI's `StyledText::with_highlights` requires sorted non-overlapping ranges.
fn merge_highlights(
    highlights: Vec<(std::ops::Range<usize>, HighlightStyle)>,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    if highlights.len() <= 1 {
        return highlights;
    }
    let mut boundaries = std::collections::BTreeSet::new();
    for (range, _) in &highlights {
        boundaries.insert(range.start);
        boundaries.insert(range.end);
    }
    let pts: Vec<usize> = boundaries.into_iter().collect();
    if pts.len() < 2 {
        return highlights;
    }
    let mut result = Vec::new();
    for w in pts.windows(2) {
        let (seg_start, seg_end) = (w[0], w[1]);
        if seg_start == seg_end { continue; }
        let mut merged = HighlightStyle::default();
        let mut any = false;
        for (range, style) in &highlights {
            if range.start <= seg_start && range.end >= seg_end {
                if let Some(c) = style.color { merged.color = Some(c); }
                if let Some(bg) = style.background_color { merged.background_color = Some(bg); }
                if let Some(w) = style.font_weight { merged.font_weight = Some(w); }
                if let Some(s) = style.font_style { merged.font_style = Some(s); }
                if let Some(u) = style.underline { merged.underline = Some(u); }
                if let Some(s) = style.strikethrough { merged.strikethrough = Some(s); }
                if let Some(f) = style.fade_out { merged.fade_out = Some(f); }
                any = true;
            }
        }
        if any { result.push((seg_start..seg_end, merged)); }
    }
    let mut coalesced: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
    for (range, style) in result {
        if let Some(last) = coalesced.last_mut() {
            if last.0.end == range.start && last.1 == style {
                last.0.end = range.end;
                continue;
            }
        }
        coalesced.push((range, style));
    }
    coalesced
}

// ── View mode ──

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffViewMode {
    Unified,
    Split,
}

// ── Comment side ──

#[derive(Clone, Copy, PartialEq)]
pub enum CommentSide {
    Left,
    Right,
}

impl CommentSide {
    fn api_str(&self) -> &'static str {
        match self {
            CommentSide::Left => "LEFT",
            CommentSide::Right => "RIGHT",
        }
    }
}

// ── Panel state ──

pub struct PrDiffPanel {
    repo: Option<GitRepo>,
    work_dir: Option<PathBuf>,
    branch: String,
    base_ref: String,
    diff_files: Vec<DiffFile>,

    // PR state
    pr_info: Option<PrInfo>,
    pr_comments: Vec<PrReviewComment>,
    /// Index from (path, line_number, side) → indices into pr_comments.
    comment_index: HashMap<(String, u32, String), Vec<usize>>,
    gh_status: GhStatus,
    loading: bool,

    // Comment drafting — supports single line or multi-line range
    /// (path, start_line, end_line, side)
    active_comment_line: Option<(String, u32, u32, CommentSide)>,
    comment_input: Option<Entity<TextInput>>,
    submitting_comment: bool,

    // Drag-to-select for multi-line comments
    /// Set on mouse-down on the "+" gutter: (path, line, side)
    comment_drag_start: Option<(String, u32, CommentSide)>,
    /// Updated on mouse-move: the current end line of the drag
    comment_drag_end: Option<u32>,
    /// Visual row range for drag highlight (global_line_idx based, side-agnostic)
    comment_drag_start_gli: Option<usize>,
    comment_drag_end_gli: Option<usize>,

    // UI state
    collapsed_files: HashSet<usize>,
    collapsed_dirs: HashSet<String>,
    expanded_context: HashMap<(usize, usize), (usize, usize)>,
    tree_width: f32,
    tree_collapsed: bool,
    resizing_tree: bool,
    tree_drag_start_x: f32,
    tree_drag_start_width: f32,
    list_state: ListState,
    scroll_to_file: Option<usize>,
    copied_file_key: Option<usize>,
    _copied_timer: Option<Task<()>>,
    /// Whether the initial async refresh (gh check + PR detection) has been triggered.
    needs_initial_refresh: bool,

    // Line selection for copy
    /// Whether user is currently dragging to select lines.
    copy_selecting: bool,
    /// Global line index where selection started.
    copy_anchor_line: Option<usize>,
    /// Global line index where selection currently ends.
    copy_end_line: Option<usize>,
    /// Global line index → text content for copy (rebuilt with flat cache).
    copy_line_contents: Vec<String>,
    /// Running auto-scroll task when dragging near viewport edges.
    _autoscroll_task: Option<Task<()>>,
    /// Last known mouse Y (window coords) during a drag.
    last_drag_mouse_y: Option<f32>,

    // Virtual scroll cache
    /// Pre-flattened line segments per file (indexed by file_idx).
    cached_file_segments: Vec<Vec<LineSegment>>,
    /// Flat row descriptors for the uniform_list.
    flat_rows: Vec<FlatRow>,
    /// file_idx → index of its FileHeader row in flat_rows.
    flat_file_starts: Vec<usize>,
    /// Prefix sum of estimated row heights (length = `flat_rows.len() + 1`).
    /// Enables O(1) content-height and O(log N) offset↔index conversions.
    flat_row_height_prefix: Vec<f32>,
    /// Whether the flat cache needs rebuilding before next render.
    flat_cache_dirty: bool,

    // Reply to existing comment thread
    /// The comment ID we're replying to, and the index in pr_comments.
    reply_to: Option<(u64, usize)>,
    reply_input: Option<Entity<TextInput>>,
    submitting_reply: bool,

    // "Copy as prompt" feedback — key identifies which button was clicked
    copied_prompt_key: Option<String>,
    _copied_comment_timer: Option<Task<()>>,

    // Resolved thread tracking — top-level comment IDs whose threads are resolved
    resolved_thread_ids: HashSet<u64>,
    /// Map from top-level comment database ID → GraphQL thread node ID (for resolve/unresolve).
    thread_node_ids: HashMap<u64, String>,
    /// Resolved threads the user has manually expanded to view.
    expanded_resolved: HashSet<u64>,
    /// Comment threads the user has manually collapsed.
    collapsed_comments: HashSet<u64>,
    /// Unified (interleaved) vs Split (side-by-side) diff view.
    view_mode: DiffViewMode,

    // In-panel search (Cmd+F)
    focus_handle: FocusHandle,
    search_active: bool,
    search_query: String,
    search_input: Option<Entity<crate::text_input::TextInput>>,
    /// Matches: (global_line_idx, start_byte, end_byte).
    search_matches: Vec<(usize, usize, usize)>,
    search_match_ix: usize,

    // Auto-hiding scrollbar over the diff list.
    scrollbar: ScrollbarState,
    last_scroll_offset: f32,
    /// File index whose inline FileHeader row is currently being covered by
    /// the sticky overlay. The list renders an invisible placeholder for
    /// that row so we don't double-paint the same header.
    sticky_file_idx: Option<usize>,

    // Merge / checks UI state
    checks_expanded: bool,
    submitting_merge: bool,
    merge_error: Option<String>,
    /// Repeating timer that re-fetches PR status every ~60s while the panel exists.
    _poll_task: Option<Task<()>>,
}

impl PrDiffPanel {
    pub fn empty(cx: &mut Context<Self>) -> Self {
        Self {
            repo: None,
            work_dir: None,
            branch: String::new(),
            base_ref: String::new(),
            diff_files: Vec::new(),
            pr_info: None,
            pr_comments: Vec::new(),
            comment_index: HashMap::new(),
            gh_status: GhStatus::Unknown,
            loading: false,
            active_comment_line: None,
            comment_input: None,
            submitting_comment: false,
            comment_drag_start: None,
            comment_drag_end: None,
            comment_drag_start_gli: None,
            comment_drag_end_gli: None,
            collapsed_files: HashSet::new(),
            collapsed_dirs: HashSet::new(),
            expanded_context: HashMap::new(),
            tree_width: 200.0,
            tree_collapsed: false,
            resizing_tree: false,
            tree_drag_start_x: 0.0,
            tree_drag_start_width: 0.0,
            list_state: ListState::new(0, ListAlignment::Top, px(200.0)),
            scroll_to_file: None,
            copied_file_key: None,
            _copied_timer: None,
            needs_initial_refresh: false,
            copy_selecting: false,
            copy_anchor_line: None,
            copy_end_line: None,
            copy_line_contents: Vec::new(),
            _autoscroll_task: None,
            last_drag_mouse_y: None,
            cached_file_segments: Vec::new(),
            flat_rows: Vec::new(),
            flat_file_starts: Vec::new(),
            flat_row_height_prefix: vec![0.0],
            flat_cache_dirty: true,
            reply_to: None,
            reply_input: None,
            submitting_reply: false,
            copied_prompt_key: None,
            _copied_comment_timer: None,
            resolved_thread_ids: HashSet::new(),
            thread_node_ids: HashMap::new(),
            expanded_resolved: HashSet::new(),
            collapsed_comments: HashSet::new(),
            view_mode: DiffViewMode::Unified,
            focus_handle: cx.focus_handle(),
            search_active: false,
            search_query: String::new(),
            search_input: None,
            search_matches: Vec::new(),
            search_match_ix: 0,
            scrollbar: ScrollbarState::default(),
            last_scroll_offset: 0.0,
            sticky_file_idx: None,
            checks_expanded: false,
            submitting_merge: false,
            merge_error: None,
            _poll_task: None,
        }
    }

    pub fn new(work_dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let repo = GitRepo::open(&work_dir);
        let branch = repo
            .as_ref()
            .map(|r| r.current_branch())
            .unwrap_or_default();
        let base_ref = repo
            .as_ref()
            .map(|r| r.default_branch())
            .unwrap_or_else(|| "main".to_string());

        let diff_files = repo
            .as_ref()
            .map(|r| r.branch_diff(&base_ref))
            .unwrap_or_default();

        Self {
            repo,
            work_dir: Some(work_dir),
            branch,
            base_ref,
            diff_files,
            pr_info: None,
            pr_comments: Vec::new(),
            comment_index: HashMap::new(),
            gh_status: GhStatus::Unknown,
            loading: false,
            active_comment_line: None,
            comment_input: None,
            submitting_comment: false,
            comment_drag_start: None,
            comment_drag_end: None,
            comment_drag_start_gli: None,
            comment_drag_end_gli: None,
            collapsed_files: HashSet::new(),
            collapsed_dirs: HashSet::new(),
            expanded_context: HashMap::new(),
            tree_width: 200.0,
            tree_collapsed: false,
            resizing_tree: false,
            tree_drag_start_x: 0.0,
            tree_drag_start_width: 0.0,
            list_state: ListState::new(0, ListAlignment::Top, px(200.0)),
            scroll_to_file: None,
            copied_file_key: None,
            _copied_timer: None,
            needs_initial_refresh: true,
            copy_selecting: false,
            copy_anchor_line: None,
            copy_end_line: None,
            copy_line_contents: Vec::new(),
            _autoscroll_task: None,
            last_drag_mouse_y: None,
            cached_file_segments: Vec::new(),
            flat_rows: Vec::new(),
            flat_file_starts: Vec::new(),
            flat_row_height_prefix: vec![0.0],
            flat_cache_dirty: true,
            reply_to: None,
            reply_input: None,
            submitting_reply: false,
            copied_prompt_key: None,
            _copied_comment_timer: None,
            resolved_thread_ids: HashSet::new(),
            thread_node_ids: HashMap::new(),
            expanded_resolved: HashSet::new(),
            collapsed_comments: HashSet::new(),
            view_mode: DiffViewMode::Unified,
            focus_handle: cx.focus_handle(),
            search_active: false,
            search_query: String::new(),
            search_input: None,
            search_matches: Vec::new(),
            search_match_ix: 0,
            scrollbar: ScrollbarState::default(),
            last_scroll_offset: 0.0,
            sticky_file_idx: None,
            checks_expanded: false,
            submitting_merge: false,
            merge_error: None,
            _poll_task: None,
        }
    }

    /// Whether the panel is in split (side-by-side) view mode.
    pub fn is_split_view(&self) -> bool {
        self.view_mode == DiffViewMode::Split
    }

    /// Set the view mode. When `split` is true, uses side-by-side; otherwise unified.
    pub fn set_split_view(&mut self, split: bool) {
        let mode = if split { DiffViewMode::Split } else { DiffViewMode::Unified };
        if self.view_mode != mode {
            self.view_mode = mode;
            self.flat_cache_dirty = true;
        }
    }

    /// Estimate total heap bytes for all cached diff data.
    pub fn estimated_bytes(&self) -> usize {
        self.diff_files.iter().map(|f| f.estimated_bytes()).sum()
    }

    /// Number of cached diff files.
    pub fn file_count(&self) -> usize {
        self.diff_files.len()
    }

    #[allow(dead_code)]
    pub fn git_dir(&self) -> Option<PathBuf> {
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

    /// Return the `global_line_idx` for a flat row index, if it has one.
    fn gli_for_row(&self, row_ix: usize) -> Option<usize> {
        match self.flat_rows.get(row_ix)? {
            FlatRow::Line { global_line_idx, .. } => Some(*global_line_idx),
            FlatRow::SplitLine { global_line_idx, .. } => Some(*global_line_idx),
            _ => None,
        }
    }

    fn bump_scrollbar(&mut self, cx: &mut Context<Self>) {
        self.scrollbar.visible = true;
        self.scrollbar.hide_task =
            Some(scrollbar::schedule_hide(cx, |v: &mut Self| &mut v.scrollbar));
    }

    /// Rebuild `flat_row_height_prefix` from `flat_rows`. Called once
    /// per flat-cache rebuild so the three helpers below can run in
    /// O(1) / O(log N) instead of walking all rows every render.
    fn rebuild_row_height_prefix(&mut self) {
        self.flat_row_height_prefix.clear();
        self.flat_row_height_prefix.reserve(self.flat_rows.len() + 1);
        self.flat_row_height_prefix.push(0.0);
        let mut acc: f32 = 0.0;
        for row in &self.flat_rows {
            acc += row_height_estimate_px(row);
            self.flat_row_height_prefix.push(acc);
        }
    }

    /// Sum of estimated heights across all `flat_rows`. Stable between
    /// renders, so the scrollbar thumb doesn't resize as items scroll in.
    fn estimated_content_height_px(&self) -> f32 {
        *self.flat_row_height_prefix.last().unwrap_or(&0.0)
    }

    /// Estimated pixel offset corresponding to the list's current logical
    /// scroll position.
    fn estimated_scroll_offset_px(&self) -> f32 {
        let scroll_top = self.list_state.logical_scroll_top();
        let cap = scroll_top
            .item_ix
            .min(self.flat_row_height_prefix.len().saturating_sub(1));
        self.flat_row_height_prefix[cap] + f32::from(scroll_top.offset_in_item)
    }

    /// Locate the row containing `target_px` via binary search on the
    /// prefix sums.
    /// Compute the sticky file-header overlay state for the current scroll
    /// position. See the matching method in `diff_view.rs` for the full
    /// rationale; activation is shifted by each file's `pt` wrapper so
    /// the inter-file breathing room stays visible until the card itself
    /// reaches the top edge.
    fn sticky_file_header(&self) -> Option<(usize, f32)> {
        if self.flat_file_starts.is_empty() || self.flat_row_height_prefix.is_empty() {
            return None;
        }
        let scroll_px = self.estimated_scroll_offset_px();
        let card_h = 42.0;

        let mut current: Option<usize> = None;
        for (file_idx, &start_row) in self.flat_file_starts.iter().enumerate() {
            let card_top =
                self.flat_row_height_prefix[start_row] + file_header_top_pad(file_idx);
            if card_top <= scroll_px {
                current = Some(file_idx);
            } else {
                break;
            }
        }
        let current = current?;

        let push_offset = self
            .flat_file_starts
            .get(current + 1)
            .map(|&next_start| {
                let next_card_top = self.flat_row_height_prefix[next_start]
                    + file_header_top_pad(current + 1);
                let dist = next_card_top - scroll_px;
                if dist < card_h {
                    (card_h - dist).max(0.0)
                } else {
                    0.0
                }
            })
            .unwrap_or(0.0);

        Some((current, push_offset))
    }

    fn item_ix_for_estimated_offset(&self, target_px: f32) -> (usize, f32) {
        if target_px <= 0.0 || self.flat_rows.is_empty() {
            return (0, 0.0);
        }
        let prefix = &self.flat_row_height_prefix;
        let ix = match prefix.binary_search_by(|v| {
            v.partial_cmp(&target_px).unwrap_or(std::cmp::Ordering::Equal)
        }) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let ix = ix.min(self.flat_rows.len());
        let row_start = prefix[ix.min(prefix.len() - 1)];
        (ix, (target_px - row_start).max(0.0))
    }

    /// Ensure the auto-scroll timer is running while a drag is active.
    /// The timer re-reads `last_drag_mouse_y` each tick so it works even
    /// when `on_mouse_move` stops firing (mouse left the window).
    fn ensure_autoscroll_timer(&mut self, cx: &mut Context<Self>) {
        let is_dragging = self.copy_selecting || self.comment_drag_start.is_some();
        if !is_dragging {
            self._autoscroll_task = None;
            return;
        }
        if self._autoscroll_task.is_some() {
            return;
        }

        let entity = cx.entity().clone();
        self._autoscroll_task = Some(cx.spawn(async move |_, cx| {
            const EDGE_ZONE: f32 = 40.0;
            const SCROLL_SPEED: f32 = 300.0;
            const TICK_MS: u64 = 16;

            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(TICK_MS))
                    .await;
                let Ok(should_continue) = cx.update(|cx| {
                    entity.update(cx, |panel, cx| {
                        let is_dragging = panel.copy_selecting
                            || panel.comment_drag_start.is_some();
                        if !is_dragging {
                            panel._autoscroll_task = None;
                            return false;
                        }
                        let Some(mouse_y) = panel.last_drag_mouse_y else {
                            return true;
                        };

                        let vp = panel.list_state.viewport_bounds();
                        let top = f32::from(vp.origin.y);
                        let bottom = top + f32::from(vp.size.height);

                        let scroll_delta = if mouse_y < top + EDGE_ZONE {
                            let ratio = ((top + EDGE_ZONE - mouse_y) / EDGE_ZONE).clamp(0.0, 1.0);
                            -(SCROLL_SPEED * ratio * TICK_MS as f32 / 1000.0)
                        } else if mouse_y > bottom - EDGE_ZONE {
                            let ratio = ((mouse_y - (bottom - EDGE_ZONE)) / EDGE_ZONE).clamp(0.0, 1.0);
                            SCROLL_SPEED * ratio * TICK_MS as f32 / 1000.0
                        } else {
                            return true; // not near edge, keep timer alive
                        };

                        panel.list_state.scroll_by(px(scroll_delta));

                        let scroll_top = panel.list_state.logical_scroll_top();
                        if scroll_delta < 0.0 {
                            for i in scroll_top.item_ix..panel.flat_rows.len() {
                                if let Some(gli) = panel.gli_for_row(i) {
                                    if panel.copy_selecting {
                                        panel.copy_end_line = Some(gli);
                                    }
                                    if panel.comment_drag_start.is_some() {
                                        panel.comment_drag_end_gli = Some(gli);
                                    }
                                    break;
                                }
                            }
                        } else {
                            let est_visible =
                                (f32::from(vp.size.height) / 22.0) as usize;
                            let end_ix = (scroll_top.item_ix + est_visible + 5)
                                .min(panel.flat_rows.len());
                            for i in (scroll_top.item_ix..end_ix).rev() {
                                if let Some(gli) = panel.gli_for_row(i) {
                                    if panel.copy_selecting {
                                        panel.copy_end_line = Some(gli);
                                    }
                                    if panel.comment_drag_start.is_some() {
                                        panel.comment_drag_end_gli = Some(gli);
                                    }
                                    break;
                                }
                            }
                        }
                        cx.notify();
                        true
                    })
                }) else {
                    break;
                };
                if !should_continue {
                    break;
                }
            }
        }));
    }

    /// Refresh the branch diff and reload PR data asynchronously.
    /// Phase 1 delivers gh status + PR info quickly so the title renders fast.
    /// Phase 2 fetches comments, thread info, and the remote base ref.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        // Synchronous: update branch name immediately
        if let Some(repo) = &self.repo {
            self.branch = repo.current_branch();
        }

        let Some(work_dir) = self.work_dir.clone() else {
            return;
        };

        self.loading = true;
        cx.notify();

        // Phase 1: gh status + PR detection (fast)
        let work_dir_p1 = work_dir.clone();
        cx.spawn(async move |entity, cx| {
            let (gh_status, pr_info) = cx
                .background_executor()
                .spawn(async move {
                    let status = github::check_gh();
                    let pr = if status == GhStatus::Available {
                        github::detect_pr(&work_dir_p1)
                    } else {
                        None
                    };
                    (status, pr)
                })
                .await;

            // Apply PR info immediately so the title renders
            let phase2_needed = cx.update(|cx| {
                entity.update(cx, |panel, cx| {
                    panel.gh_status = gh_status.clone();
                    if let Some(ref pr) = pr_info {
                        if pr.base_ref_name != panel.base_ref {
                            panel.base_ref = pr.base_ref_name.clone();
                        }
                    }
                    panel.pr_info = pr_info.clone();
                    cx.notify();

                    (panel.work_dir.clone(), panel.base_ref.clone(), pr_info, gh_status)
                })
            });

            let Ok(Ok((Some(work_dir), base_ref, pr_info, gh_status))) = phase2_needed else {
                let _ = cx.update(|cx| {
                    let _ = entity.update(cx, |panel, cx| {
                        if let Some(repo) = &panel.repo {
                            panel.diff_files = repo.branch_diff(&panel.base_ref);
                        }
                        panel.expanded_context.clear();
                        panel.flat_cache_dirty = true;
                        panel.loading = false;
                        cx.notify();
                    });
                });
                return;
            };

            // Phase 2: comments, thread info, git fetch
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut comments = Vec::new();
                    let mut thread_info = None;
                    if gh_status == GhStatus::Available {
                        comments = pr_info
                            .as_ref()
                            .map(|p| github::fetch_pr_comments(&work_dir, p.number))
                            .unwrap_or_default();
                        thread_info = pr_info.as_ref().and_then(|p| {
                            github::repo_owner_name(&work_dir)
                                .map(|(owner, repo)| {
                                    github::fetch_thread_info(&work_dir, &owner, &repo, p.number)
                                })
                        });
                        let fetch_ref = pr_info
                            .as_ref()
                            .map(|p| p.base_ref_name.as_str())
                            .unwrap_or(&base_ref);
                        let _ = std::process::Command::new("git")
                            .args(["fetch", "origin", fetch_ref])
                            .current_dir(&work_dir)
                            .output();
                    }
                    (comments, thread_info)
                })
                .await;

            let _ = cx.update(|cx| {
                let _ = entity.update(cx, |panel, cx| {
                    let (comments, thread_info) = result;

                    // Recompute diff now that remote base is fetched
                    if let Some(repo) = &panel.repo {
                        panel.diff_files = repo.branch_diff(&panel.base_ref);
                    }
                    panel.expanded_context.clear();
                    panel.flat_cache_dirty = true;

                    panel.pr_comments = comments;
                    if let Some(info) = thread_info {
                        panel.resolved_thread_ids = info.resolved_ids;
                        panel.thread_node_ids = info.thread_node_ids;
                    } else {
                        panel.resolved_thread_ids.clear();
                        panel.thread_node_ids.clear();
                    }
                    panel.rebuild_comment_index();
                    panel.loading = false;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Start a background timer that re-calls `refresh` every 60s. The timer
    /// self-terminates when the panel entity is dropped (leaving PR mode).
    /// Idempotent — subsequent calls are no-ops.
    fn start_poll_if_needed(&mut self, cx: &mut Context<Self>) {
        if self._poll_task.is_some() {
            return;
        }
        self._poll_task = Some(cx.spawn(async move |entity, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(60))
                    .await;
                let result = cx.update(|cx| {
                    let _ = entity.update(cx, |panel, cx| {
                        if !panel.loading {
                            panel.refresh(cx);
                        }
                    });
                });
                if result.is_err() {
                    break;
                }
            }
        }));
    }

    /// Kick off a merge action. When `auto` is true, enables auto-merge;
    /// otherwise attempts to merge immediately. On success, triggers a full
    /// refresh so the new state is reflected. On failure, stores the error
    /// for display.
    fn submit_merge(&mut self, method: MergeMethod, auto: bool, cx: &mut Context<Self>) {
        let Some(work_dir) = self.work_dir.clone() else { return };
        let Some(ref pr_info) = self.pr_info else { return };
        let pr_number = pr_info.number;

        self.submitting_merge = true;
        self.merge_error = None;
        cx.notify();

        cx.spawn(async move |entity, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if auto {
                        github::enable_auto_merge(&work_dir, pr_number, method)
                    } else {
                        github::merge_pr(&work_dir, pr_number, method)
                    }
                })
                .await;

            let _ = cx.update(|cx| {
                let _ = entity.update(cx, |panel, cx| {
                    panel.submitting_merge = false;
                    match result {
                        Ok(()) => {
                            // Optimistic UI: flip local state immediately so the
                            // button doesn't flash its old label before the
                            // refresh round-trip completes.
                            if let Some(ref mut pr) = panel.pr_info {
                                if auto {
                                    pr.auto_merge_request =
                                        Some(github::AutoMergeRequest {
                                            enabled_by: None,
                                            merge_method: Some(
                                                match method {
                                                    MergeMethod::Squash => "SQUASH",
                                                    MergeMethod::Merge => "MERGE",
                                                    MergeMethod::Rebase => "REBASE",
                                                }
                                                .to_string(),
                                            ),
                                        });
                                } else {
                                    pr.state = "MERGED".to_string();
                                }
                            }
                            panel.refresh(cx);
                        }
                        Err(e) => {
                            panel.merge_error = Some(e);
                            cx.notify();
                        }
                    }
                });
            });
        })
        .detach();
    }

    fn submit_disable_auto_merge(&mut self, cx: &mut Context<Self>) {
        let Some(work_dir) = self.work_dir.clone() else { return };
        let Some(ref pr_info) = self.pr_info else { return };
        let pr_number = pr_info.number;

        self.submitting_merge = true;
        self.merge_error = None;
        cx.notify();

        cx.spawn(async move |entity, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { github::disable_auto_merge(&work_dir, pr_number) })
                .await;

            let _ = cx.update(|cx| {
                let _ = entity.update(cx, |panel, cx| {
                    panel.submitting_merge = false;
                    match result {
                        Ok(()) => {
                            if let Some(ref mut pr) = panel.pr_info {
                                pr.auto_merge_request = None;
                            }
                            panel.refresh(cx);
                        }
                        Err(e) => {
                            panel.merge_error = Some(e);
                            cx.notify();
                        }
                    }
                });
            });
        })
        .detach();
    }

    fn rebuild_comment_index(&mut self) {
        self.comment_index.clear();
        for (idx, comment) in self.pr_comments.iter().enumerate() {
            // Only index top-level comments (not replies)
            if comment.in_reply_to_id.is_some() {
                continue;
            }
            if let Some(line) = comment.line {
                let side = comment.side.clone().unwrap_or_else(|| "RIGHT".to_string());
                self.comment_index
                    .entry((comment.path.clone(), line, side))
                    .or_default()
                    .push(idx);
            }
        }
    }

    fn can_comment(&self) -> bool {
        self.gh_status == GhStatus::Available && self.pr_info.is_some()
    }

    fn start_comment(&mut self, path: String, start_line: u32, end_line: u32, side: CommentSide, cx: &mut Context<Self>) {
        let entity = cx.entity().clone();
        let input = cx.new(|cx| {
            let mut ti = TextInput::new(cx);
            ti.set_placeholder("Write a comment...");
            let entity_submit = entity.clone();
            ti.set_on_submit(Rc::new(move |text, _window, cx| {
                if !text.trim().is_empty() {
                    let text = text.to_string();
                    let _ = entity_submit.update(cx, |panel, cx| {
                        panel.submit_comment(text, cx);
                    });
                }
            }));
            ti.set_on_cancel(Rc::new(move |_window, cx| {
                let _ = entity.update(cx, |panel, cx| {
                    panel.active_comment_line = None;
                    panel.comment_input = None;
                    cx.notify();
                });
            }));
            ti
        });
        self.active_comment_line = Some((path, start_line, end_line, side));
        self.comment_input = Some(input);
        cx.notify();
    }

    fn submit_comment(&mut self, body: String, cx: &mut Context<Self>) {
        let Some(work_dir) = self.work_dir.clone() else { return };
        let Some(ref pr_info) = self.pr_info else { return };
        let Some((ref path, start_line, end_line, side)) = self.active_comment_line else { return };
        let Some(commit_sha) = self.repo.as_ref().and_then(|r| r.head_sha()) else { return };

        let pr_number = pr_info.number;
        let path = path.clone();
        let side_str = side.api_str().to_string();
        let is_multiline = start_line != end_line;
        let start = start_line.min(end_line);
        let end = start_line.max(end_line);

        self.submitting_comment = true;
        cx.notify();

        cx.spawn(async move |entity, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    github::post_pr_comment(
                        &work_dir,
                        pr_number,
                        &body,
                        &commit_sha,
                        &path,
                        end,
                        &side_str,
                        if is_multiline { Some(start) } else { None },
                        if is_multiline { Some(&side_str) } else { None },
                    )
                })
                .await;

            let _ = cx.update(|cx| {
                let _ = entity.update(cx, |panel, cx| {
                    panel.submitting_comment = false;
                    match result {
                        Ok(comment) => {
                            let idx = panel.pr_comments.len();
                            if let Some(line) = comment.line {
                                let side = comment.side.clone().unwrap_or_else(|| "RIGHT".to_string());
                                panel
                                    .comment_index
                                    .entry((comment.path.clone(), line, side))
                                    .or_default()
                                    .push(idx);
                            }
                            panel.pr_comments.push(comment);
                            panel.active_comment_line = None;
                            panel.comment_input = None;
                        }
                        Err(_) => {
                            // Keep the input open so user can retry
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn start_reply(&mut self, comment_id: u64, comment_idx: usize, cx: &mut Context<Self>) {
        let entity = cx.entity().clone();
        let input = cx.new(|cx| {
            let mut ti = TextInput::new(cx);
            ti.set_placeholder("Write a reply...");
            let entity_submit = entity.clone();
            ti.set_on_submit(Rc::new(move |text, _window, cx| {
                if !text.trim().is_empty() {
                    let text = text.to_string();
                    let _ = entity_submit.update(cx, |panel, cx| {
                        panel.submit_reply(text, cx);
                    });
                }
            }));
            ti.set_on_cancel(Rc::new(move |_window, cx| {
                let _ = entity.update(cx, |panel, cx| {
                    panel.reply_to = None;
                    panel.reply_input = None;
                    cx.notify();
                });
            }));
            ti
        });
        self.reply_to = Some((comment_id, comment_idx));
        self.reply_input = Some(input);
        cx.notify();
    }

    fn submit_reply(&mut self, body: String, cx: &mut Context<Self>) {
        let Some(work_dir) = self.work_dir.clone() else { return };
        let Some(ref pr_info) = self.pr_info else { return };
        let Some((comment_id, _)) = self.reply_to else { return };

        let pr_number = pr_info.number;
        self.submitting_reply = true;
        cx.notify();

        cx.spawn(async move |entity, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    github::reply_to_comment(&work_dir, pr_number, comment_id, &body)
                })
                .await;

            let _ = cx.update(|cx| {
                let _ = entity.update(cx, |panel, cx| {
                    panel.submitting_reply = false;
                    match result {
                        Ok(comment) => {
                            panel.pr_comments.push(comment);
                            panel.reply_to = None;
                            panel.reply_input = None;
                        }
                        Err(_) => {
                            // Keep the input open so user can retry
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn toggle_resolve_thread(&mut self, comment_id: u64, cx: &mut Context<Self>) {
        let Some(work_dir) = self.work_dir.clone() else { return };
        let Some(thread_node_id) = self.thread_node_ids.get(&comment_id).cloned() else { return };
        let is_resolved = self.resolved_thread_ids.contains(&comment_id);

        // Optimistically toggle the UI state
        if is_resolved {
            self.resolved_thread_ids.remove(&comment_id);
        } else {
            self.resolved_thread_ids.insert(comment_id);
            self.expanded_resolved.remove(&comment_id);
        }
        cx.notify();

        cx.spawn(async move |entity, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if is_resolved {
                        github::unresolve_review_thread(&work_dir, &thread_node_id)
                    } else {
                        github::resolve_review_thread(&work_dir, &thread_node_id)
                    }
                })
                .await;

            // On failure, revert the optimistic update
            if result.is_err() {
                let _ = cx.update(|cx| {
                    let _ = entity.update(cx, |panel, cx| {
                        if is_resolved {
                            panel.resolved_thread_ids.insert(comment_id);
                        } else {
                            panel.resolved_thread_ids.remove(&comment_id);
                        }
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    fn total_additions(&self) -> usize {
        self.diff_files.iter().map(|f| f.additions()).sum()
    }

    fn total_deletions(&self) -> usize {
        self.diff_files.iter().map(|f| f.deletions()).sum()
    }

    // ── Checks / merge status bar ──

    /// Render the checks + merge action bar shown below the header when a PR
    /// is loaded. Returns None when there's no PR info yet.
    fn render_pr_status_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pr = self.pr_info.as_ref()?;

        let mut bar = div()
            .flex()
            .flex_col()
            .w_full()
            .flex_shrink_0()
            .border_b_1()
            .border_color(colors::border());

        // Checks summary
        if !pr.status_check_rollup.is_empty() {
            let mut n_success = 0;
            let mut n_failure = 0;
            let mut n_progress = 0;
            let mut n_skipped = 0;
            for c in &pr.status_check_rollup {
                match c.kind() {
                    CheckKind::Success => n_success += 1,
                    CheckKind::Failure => n_failure += 1,
                    CheckKind::InProgress => n_progress += 1,
                    CheckKind::Skipped => n_skipped += 1,
                }
            }

            let (summary_icon, summary_color) = if n_failure > 0 {
                check_icon_style(CheckKind::Failure)
            } else if n_progress > 0 {
                check_icon_style(CheckKind::InProgress)
            } else {
                check_icon_style(CheckKind::Success)
            };

            let mut parts: Vec<String> = Vec::new();
            if n_failure > 0 {
                parts.push(format!("{n_failure} failing"));
            }
            if n_progress > 0 {
                parts.push(format!("{n_progress} in progress"));
            }
            if n_skipped > 0 {
                parts.push(format!("{n_skipped} skipped"));
            }
            if n_success > 0 {
                parts.push(format!("{n_success} successful"));
            }
            let summary_text = parts.join(", ");

            let expanded = self.checks_expanded;
            // nf-fa-angle_down when open, nf-fa-angle_right when closed
            let caret = if expanded { "\u{f107}" } else { "\u{f105}" };
            let entity_toggle = cx.entity().clone();

            bar = bar.child(
                div()
                    .id("pr-checks-summary")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .px_3()
                    .py(px(6.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(colors::surface_hover()))
                    .on_click(move |_, _window, cx| {
                        entity_toggle.update(cx, |panel, cx| {
                            panel.checks_expanded = !panel.checks_expanded;
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .font_family(util::ICON_FONT)
                            .text_size(px(12.0))
                            .text_color(summary_color)
                            .child(summary_icon),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors::text())
                            .child(summary_text),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .font_family(util::ICON_FONT)
                            .text_size(px(9.0))
                            .text_color(colors::text_muted())
                            .child(caret),
                    ),
            );

            if expanded {
                let mut list = div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .bg(colors::surface());
                for (idx, check) in pr.status_check_rollup.iter().enumerate() {
                    let (icon, color) = check_icon_style(check.kind());
                    let name = check.display_name().to_string();
                    let label = match check.workflow_name.as_deref() {
                        Some(wf) if !wf.is_empty() && wf != name => format!("{wf} / {name}"),
                        _ => name,
                    };
                    let url = check.url().map(|s| s.to_string());
                    let has_url = url.is_some();
                    let mut row = div()
                        .id(("pr-check-row", idx))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .w_full()
                        .px_3()
                        .py(px(3.0))
                        .text_xs()
                        .child(
                            div()
                                .font_family(util::ICON_FONT)
                                .text_size(px(11.0))
                                .text_color(color)
                                .w(px(16.0))
                                .flex_shrink_0()
                                .child(icon),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_color(colors::text())
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(label),
                        );
                    if has_url {
                        row = row
                            .cursor_pointer()
                            .hover(|s| s.bg(colors::surface_hover()))
                            .on_click(move |_, _window, cx| {
                                if let Some(ref u) = url {
                                    cx.open_url(u);
                                }
                            });
                    }
                    list = list.child(row);
                }
                bar = bar.child(list);
            }
        }

        // Merge action row
        bar = bar.child(self.render_merge_action_row(pr, cx));

        // Error row (if any)
        if let Some(ref err) = self.merge_error {
            bar = bar.child(
                div()
                    .w_full()
                    .px_3()
                    .py(px(4.0))
                    .text_xs()
                    .text_color(colors::diff_removed())
                    .bg(rgba(0xf8514914))
                    .child(err.clone()),
            );
        }

        Some(bar.into_any_element())
    }

    fn render_merge_action_row(&self, pr: &PrInfo, cx: &mut Context<Self>) -> Div {
        let base_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .px_3()
            .py(px(8.0));

        // Closed / merged PR — no actions, just status.
        if pr.state != "OPEN" {
            let (icon, color, text) = match pr.state.as_str() {
                "MERGED" => ("\u{f00c}", colors::accent(), "Merged"),
                "CLOSED" => ("\u{f00d}", colors::diff_removed(), "Closed"),
                _ => ("\u{f192}", colors::text_muted(), pr.state.as_str()),
            };
            return base_row
                .child(
                    div()
                        .font_family(util::ICON_FONT)
                        .text_size(px(14.0))
                        .text_color(color)
                        .child(icon),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors::text())
                        .child(text.to_string()),
                );
        }

        // Auto-merge already enabled — show disable button.
        if let Some(ref req) = pr.auto_merge_request {
            let method = req
                .merge_method
                .clone()
                .unwrap_or_else(|| "SQUASH".to_string())
                .to_lowercase();
            let user = req
                .enabled_by
                .as_ref()
                .map(|u| format!("@{}", u.login))
                .unwrap_or_default();
            let text = if user.is_empty() {
                format!("Auto-merge enabled ({method})")
            } else {
                format!("Auto-merge enabled ({method}) by {user}")
            };
            let submitting = self.submitting_merge;
            let entity = cx.entity().clone();
            let label = if submitting { "Working..." } else { "Disable auto-merge" };
            return base_row
                .child(
                    div()
                        .font_family(util::ICON_FONT)
                        .text_size(px(14.0))
                        .text_color(colors::diff_added())
                        .child("\u{f00c}"),
                )
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(colors::text())
                        .child(text),
                )
                .child(
                    div()
                        .id("pr-disable-auto-merge")
                        .px_3()
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .border_1()
                        .border_color(colors::border())
                        .text_color(colors::text())
                        .when(submitting, |d: Stateful<Div>| d.opacity(0.6))
                        .when(!submitting, |d: Stateful<Div>| {
                            d.cursor_pointer().hover(|s| s.bg(colors::surface_hover()))
                        })
                        .child(label)
                        .on_click(move |_, _window, cx| {
                            entity.update(cx, |panel, cx| {
                                if !panel.submitting_merge {
                                    panel.submit_disable_auto_merge(cx);
                                }
                            });
                        }),
                );
        }

        // Open PR, no auto-merge set — compute status + primary action.
        let merge_state = pr.merge_state_status.as_deref().unwrap_or("UNKNOWN");
        let mergeable = pr.mergeable.as_deref().unwrap_or("UNKNOWN");
        let review_decision = pr.review_decision.as_deref();

        let conflicts = mergeable == "CONFLICTING" || merge_state == "DIRTY";
        let changes_requested = review_decision == Some("CHANGES_REQUESTED");
        let clean_like = matches!(merge_state, "CLEAN" | "UNSTABLE" | "HAS_HOOKS");
        let is_ready = clean_like && !pr.is_draft && !conflicts && !changes_requested;

        let (status_icon, status_color, status_text) = if pr.is_draft {
            ("\u{f040}", colors::text_muted(), "This pull request is a draft".to_string())
        } else if conflicts {
            (
                "\u{f071}",
                colors::diff_removed(),
                "This branch has conflicts that must be resolved".to_string(),
            )
        } else if changes_requested {
            (
                "\u{f071}",
                rgb(0xf9e2af),
                "Changes were requested".to_string(),
            )
        } else if merge_state == "BEHIND" {
            (
                "\u{f071}",
                rgb(0xf9e2af),
                "This branch is out-of-date with the base branch".to_string(),
            )
        } else if merge_state == "BLOCKED" {
            (
                "\u{f071}",
                rgb(0xf9e2af),
                "Merging is blocked by required checks or reviews".to_string(),
            )
        } else if merge_state == "UNSTABLE" {
            (
                "\u{f00c}",
                colors::diff_added(),
                "Non-required checks are failing, but this PR can still merge".to_string(),
            )
        } else if merge_state == "CLEAN" || merge_state == "HAS_HOOKS" {
            ("\u{f00c}", colors::diff_added(), "Ready to merge".to_string())
        } else {
            (
                "\u{f192}",
                colors::text_muted(),
                format!("Merge state: {merge_state}"),
            )
        };

        let submitting = self.submitting_merge;
        let can_enable_auto = !pr.is_draft && !conflicts;

        let mut row = base_row
            .child(
                div()
                    .font_family(util::ICON_FONT)
                    .text_size(px(14.0))
                    .text_color(status_color)
                    .flex_shrink_0()
                    .child(status_icon),
            )
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(colors::text())
                    .child(status_text),
            );

        if is_ready {
            let label = if submitting { "Merging..." } else { "Merge (squash)" };
            let entity = cx.entity().clone();
            row = row.child(
                div()
                    .id("pr-merge-btn")
                    .px_3()
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .bg(colors::diff_added())
                    .text_color(rgb(0xffffff))
                    .when(submitting, |d: Stateful<Div>| d.opacity(0.6))
                    .when(!submitting, |d: Stateful<Div>| {
                        d.cursor_pointer().hover(|s| s.opacity(0.85))
                    })
                    .child(label)
                    .on_click(move |_, _window, cx| {
                        entity.update(cx, |panel, cx| {
                            if !panel.submitting_merge {
                                panel.submit_merge(MergeMethod::Squash, false, cx);
                            }
                        });
                    }),
            );
        } else if can_enable_auto {
            let label = if submitting { "Working..." } else { "Enable auto-merge (squash)" };
            let entity = cx.entity().clone();
            row = row.child(
                div()
                    .id("pr-auto-merge-btn")
                    .px_3()
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .border_1()
                    .border_color(colors::border())
                    .text_color(colors::text())
                    .when(submitting, |d: Stateful<Div>| d.opacity(0.6))
                    .when(!submitting, |d: Stateful<Div>| {
                        d.cursor_pointer().hover(|s| s.bg(colors::surface_hover()))
                    })
                    .child(label)
                    .on_click(move |_, _window, cx| {
                        entity.update(cx, |panel, cx| {
                            if !panel.submitting_merge {
                                panel.submit_merge(MergeMethod::Squash, true, cx);
                            }
                        });
                    }),
            );
        }

        row
    }

    // ── File tree (left side) ──

    fn render_file_tree(&self, cx: &mut Context<Self>) -> Div {
        let tree = build_file_tree(&self.diff_files);
        let mut container = div().flex().flex_col().w_full();
        container = self.render_tree_nodes(&tree, 0, cx, container);
        container
    }

    fn render_tree_nodes(
        &self,
        nodes: &[TreeNode],
        depth: usize,
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
                    let dir_key = full_path.clone();
                    let is_collapsed = self.collapsed_dirs.contains(&dir_key);
                    let entity = cx.entity().clone();
                    let dk = dir_key.clone();
                    let chevron = if is_collapsed { "\u{25B6}" } else { "\u{25BC}" };
                    let dir_icon = if is_collapsed {
                        util::dir_icon()
                    } else {
                        util::dir_icon_open()
                    };

                    container = container.child(
                        div()
                            .id(ElementId::Name(format!("pr-tree-dir-{dir_key}").into()))
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
                        container = self.render_tree_nodes(children, depth + 1, cx, container);
                    }
                }
                TreeNode::File { name, file_idx, .. } => {
                    let idx = *file_idx;
                    let entity = cx.entity().clone();
                    let file = &self.diff_files[idx];
                    let adds = file.additions();
                    let dels = file.deletions();

                    // Count unresolved and resolved top-level comments on this file
                    let mut unresolved_count = 0usize;
                    let mut resolved_count = 0usize;
                    for c in &self.pr_comments {
                        if c.in_reply_to_id.is_none() && c.path == file.path {
                            if self.resolved_thread_ids.contains(&c.id) {
                                resolved_count += 1;
                            } else {
                                unresolved_count += 1;
                            }
                        }
                    }

                    container = container.child(
                        div()
                            .id(ElementId::Name(format!("pr-tree-file-{idx}").into()))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .h(px(24.0))
                            .pl(px((depth as f32) * 16.0 + 8.0))
                            .pr_2()
                            .cursor_pointer()
                            .hover(|s| s.bg(colors::surface_hover()))
                            .on_click(move |_, _window, cx| {
                                entity.update(cx, |panel, cx| {
                                    panel.collapsed_files.remove(&idx);
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
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .flex_shrink_0()
                                    .when(unresolved_count > 0, |d: Div| {
                                        d.child(
                                            div()
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .gap(px(2.0))
                                                .child(
                                                    div()
                                                        .font_family(util::ICON_FONT)
                                                        .text_size(px(10.0))
                                                        .text_color(colors::accent())
                                                        .child("\u{f075}"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.0))
                                                        .text_color(colors::accent())
                                                        .child(format!("{unresolved_count}")),
                                                ),
                                        )
                                    })
                                    .when(resolved_count > 0, |d: Div| {
                                        d.child(
                                            div()
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .gap(px(2.0))
                                                .child(
                                                    div()
                                                        .font_family(util::ICON_FONT)
                                                        .text_size(px(10.0))
                                                        .text_color(colors::text_muted())
                                                        .child("\u{f00c}"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.0))
                                                        .text_color(colors::text_muted())
                                                        .child(format!("{resolved_count}")),
                                                ),
                                        )
                                    })
                                    .when(adds > 0, |d: Div| {
                                        d.child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(colors::diff_added())
                                                .child(format!("+{adds}")),
                                        )
                                    })
                                    .when(dels > 0, |d: Div| {
                                        d.child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(colors::diff_removed())
                                                .child(format!("-{dels}")),
                                        )
                                    }),
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
        // Phase 1: flatten segments for each file
        let files_count = self.diff_files.len();
        let mut all_segments: Vec<Vec<LineSegment>> = Vec::with_capacity(files_count);
        for file_idx in 0..files_count {
            let f = &self.diff_files[file_idx];
            let skip = self.collapsed_files.contains(&file_idx)
                || f.hunks.is_empty()
                || matches!(f.status, FileStatus::Renamed);
            if skip {
                all_segments.push(Vec::new());
            } else {
                let segs = self.flatten_file_lines(f, file_idx);
                all_segments.push(segs);
            }
        }

        // Phase 2: build flat rows
        self.flat_rows.clear();
        self.flat_file_starts.clear();
        let mut copy_texts: Vec<String> = Vec::new();
        let mut global_line_idx = 0usize;
        let split_mode = self.view_mode == DiffViewMode::Split;

        for file_idx in 0..files_count {
            self.flat_file_starts.push(self.flat_rows.len());
            self.flat_rows.push(FlatRow::FileHeader { file_idx });

            if self.collapsed_files.contains(&file_idx) {
                continue;
            }

            // Renames render as a single info row (old → new) instead of
            // a full content diff — the rename itself is the change.
            let is_rename = matches!(self.diff_files[file_idx].status, FileStatus::Renamed);
            if is_rename || self.diff_files[file_idx].hunks.is_empty() {
                self.flat_rows.push(FlatRow::EmptyFile { file_idx });
                continue;
            }

            let segments = &all_segments[file_idx];
            let num_segs = segments.len();

            if split_mode {
                // Split mode: pair removed/added lines side-by-side
                let mut i = 0;
                while i < num_segs {
                    let seg = &segments[i];
                    match seg {
                        LineSegment::CollapsedContext { count, hunk_idx, direction } => {
                            self.flat_rows.push(FlatRow::CollapsedContext {
                                file_idx,
                                seg_idx: i,
                                count: *count,
                                hunk_idx: *hunk_idx,
                                direction: *direction,
                                is_last_in_file: i == num_segs - 1,
                            });
                            i += 1;
                        }
                        LineSegment::Line(line) => {
                            match line.kind {
                                DiffLineKind::Context => {
                                    copy_texts.push(line.content.trim_end().to_string());
                                    self.flat_rows.push(FlatRow::SplitLine {
                                        file_idx,
                                        left_seg: Some(i),
                                        right_seg: Some(i),
                                        global_line_idx,
                                        is_last_in_file: i == num_segs - 1,
                                    });
                                    global_line_idx += 1;
                                    i += 1;
                                }
                                DiffLineKind::Removed => {
                                    let mut removes = Vec::new();
                                    while i < num_segs {
                                        if let LineSegment::Line(l) = &segments[i] {
                                            if matches!(l.kind, DiffLineKind::Removed) {
                                                removes.push(i);
                                                i += 1;
                                                continue;
                                            }
                                        }
                                        break;
                                    }
                                    let mut adds = Vec::new();
                                    while i < num_segs {
                                        if let LineSegment::Line(l) = &segments[i] {
                                            if matches!(l.kind, DiffLineKind::Added) {
                                                adds.push(i);
                                                i += 1;
                                                continue;
                                            }
                                        }
                                        break;
                                    }
                                    let max_len = removes.len().max(adds.len());
                                    for j in 0..max_len {
                                        let left = removes.get(j).copied();
                                        let right = adds.get(j).copied();
                                        if let Some(li) = left {
                                            if let LineSegment::Line(l) = &segments[li] {
                                                copy_texts.push(l.content.trim_end().to_string());
                                            }
                                        }
                                        if let Some(ri) = right {
                                            if let LineSegment::Line(l) = &segments[ri] {
                                                copy_texts.push(l.content.trim_end().to_string());
                                            }
                                        }
                                        let remaining_in_file = i >= num_segs && j == max_len - 1;
                                        self.flat_rows.push(FlatRow::SplitLine {
                                            file_idx,
                                            left_seg: left,
                                            right_seg: right,
                                            global_line_idx,
                                            is_last_in_file: remaining_in_file,
                                        });
                                        global_line_idx += 1;
                                    }
                                }
                                DiffLineKind::Added => {
                                    copy_texts.push(line.content.trim_end().to_string());
                                    self.flat_rows.push(FlatRow::SplitLine {
                                        file_idx,
                                        left_seg: None,
                                        right_seg: Some(i),
                                        global_line_idx,
                                        is_last_in_file: i == num_segs - 1,
                                    });
                                    global_line_idx += 1;
                                    i += 1;
                                }
                            }
                        }
                    }
                }
            } else {
                // Unified mode (original)
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
        }

        self.cached_file_segments = all_segments;
        self.copy_line_contents = copy_texts;
        self.rebuild_row_height_prefix();
        let scroll_pos = self.list_state.logical_scroll_top();
        let old_count = self.list_state.item_count();
        self.list_state.splice(0..old_count, self.flat_rows.len());
        self.list_state.scroll_to(scroll_pos);
        self.flat_cache_dirty = false;

        if self.search_active {
            self.find_search_matches();
        }
    }

    /// Render a single flat row. Called from the list callback.
    fn render_flat_row(&self, row_idx: usize, entity: &Entity<Self>) -> AnyElement {
        match &self.flat_rows[row_idx] {
            FlatRow::FileHeader { file_idx } => {
                if Some(*file_idx) == self.sticky_file_idx {
                    // Inline header is covered by the sticky overlay —
                    // render an invisible placeholder with the same
                    // footprint (top_pad wrapper + header card box) so
                    // the list's measured height stays identical and
                    // toggling the sticky doesn't shift scroll position.
                    div()
                        .w_full()
                        .pt(px(file_header_top_pad(*file_idx)))
                        .child(
                            div()
                                .min_h(px(32.0))
                                .py(px(4.0))
                                .border_1()
                                .border_color(rgba(0x00000000)),
                        )
                        .into_any_element()
                } else {
                    self.render_file_header(*file_idx, entity, true)
                }
            }
            FlatRow::EmptyFile { file_idx } => {
                self.render_empty_file(*file_idx)
            }
            FlatRow::Line {
                file_idx,
                seg_idx,
                global_line_idx,
                is_last_in_file,
            } => {
                let seg = &self.cached_file_segments[*file_idx][*seg_idx];
                if let LineSegment::Line(line) = seg {
                    self.render_line_row(
                        line,
                        *file_idx,
                        *global_line_idx,
                        *is_last_in_file,
                        entity,
                    )
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
            } => {
                self.render_collapsed_row(
                    *file_idx,
                    *count,
                    *hunk_idx,
                    *direction,
                    *is_last_in_file,
                    row_idx,
                    entity,
                )
            }
            FlatRow::SplitLine {
                file_idx,
                left_seg,
                right_seg,
                global_line_idx,
                is_last_in_file,
            } => {
                let left_line = left_seg.and_then(|si| {
                    if let LineSegment::Line(l) = &self.cached_file_segments[*file_idx][si] {
                        Some(l)
                    } else {
                        None
                    }
                });
                let right_line = right_seg.and_then(|si| {
                    if let LineSegment::Line(l) = &self.cached_file_segments[*file_idx][si] {
                        Some(l)
                    } else {
                        None
                    }
                });
                self.render_split_line_row(*file_idx, left_line, right_line, *global_line_idx, *is_last_in_file, entity)
            }
        }
    }

    /// Render a file header. `with_top_pad` controls the inter-file
    /// spacing wrapper: list rows pass `true` so files visually breathe;
    /// the sticky overlay passes `false` so the card sits flush against
    /// the top edge with no padding gap above it.
    fn render_file_header(
        &self,
        file_idx: usize,
        entity: &Entity<Self>,
        with_top_pad: bool,
    ) -> AnyElement {
        let file = &self.diff_files[file_idx];
        let adds = file.additions();
        let dels = file.deletions();
        let is_collapsed = self.collapsed_files.contains(&file_idx);
        let file_path = file.path.clone();

        let status_color = match file.status {
            FileStatus::Added => colors::diff_added(),
            FileStatus::Modified => colors::accent(),
            FileStatus::Deleted => colors::diff_removed(),
            FileStatus::Renamed => colors::accent(),
        };

        let entity_hdr = entity.clone();
        let just_copied = self.copied_file_key == Some(file_idx);
        let (icon, icon_color) = if just_copied {
            ("\u{f00c}", colors::diff_added()) // nf-fa-check
        } else {
            ("\u{f0c5}", colors::text_muted()) // nf-fa-copy
        };
        let path_for_copy = file.path.clone();
        let entity_copy = entity.clone();

        let header = div()
            .id(ElementId::Name(format!("pr-fhdr-{file_idx}").into()))
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
                    if panel.collapsed_files.contains(&file_idx) {
                        panel.collapsed_files.remove(&file_idx);
                    } else {
                        // If the click came from the sticky overlay (viewport
                        // scrolled past this file's header card), preserving
                        // the raw scroll offset across the collapse would
                        // land the user mid-way through a later file. Snap
                        // back to this file's row so the click stays
                        // anchored to what was clicked.
                        if let Some(&start_row) = panel.flat_file_starts.get(file_idx) {
                            if let Some(&prefix) =
                                panel.flat_row_height_prefix.get(start_row)
                            {
                                let card_top = prefix + file_header_top_pad(file_idx);
                                if panel.estimated_scroll_offset_px() > card_top {
                                    panel.scroll_to_file = Some(file_idx);
                                }
                            }
                        }
                        panel.collapsed_files.insert(file_idx);
                    }
                    panel.flat_cache_dirty = true;
                    cx.notify();
                });
            })
            .child(
                div()
                    .w(px(8.0))
                    .h(px(8.0))
                    .rounded_full()
                    .bg(status_color)
                    .flex_shrink_0()
                    .mr_2(),
            )
            .child({
                div()
                    .id(ElementId::Name(format!("pr-fcopy-{file_idx}").into()))
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
                            panel.copied_file_key = Some(file_idx);
                            cx.notify();
                        });
                        cx.defer(move |cx| {
                            entity.update(cx, |panel, cx| {
                                panel._copied_timer = Some(cx.spawn(async move |this, cx| {
                                    cx.background_executor()
                                        .timer(std::time::Duration::from_millis(1500))
                                        .await;
                                    let _ = cx.update(|cx| {
                                        let _ = this.update(cx, |panel, cx| {
                                            if panel.copied_file_key == Some(file_idx) {
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
                            .font_family(util::ICON_FONT)
                            .text_size(px(11.0))
                            .child(icon),
                    )
            })
            .child(div().flex_1())
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
            );

        if with_top_pad {
            div()
                .pt(px(file_header_top_pad(file_idx)))
                .child(header)
                .into_any_element()
        } else {
            header.into_any_element()
        }
    }

    fn render_empty_file(&self, file_idx: usize) -> AnyElement {
        let file = &self.diff_files[file_idx];
        let body: String = match (&file.status, file.old_path.as_ref()) {
            (FileStatus::Renamed, Some(old)) => format!("Renamed from {old}"),
            (FileStatus::Renamed, None) => "Renamed".to_string(),
            _ => "This file has no content".to_string(),
        };
        div()
            .id(ElementId::Name(format!("pr-fempty-{file_idx}").into()))
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
            .child(body)
            .into_any_element()
    }

    fn render_line_row(
        &self,
        line: &DiffLine,
        file_idx: usize,
        global_line_idx: usize,
        is_last_in_file: bool,
        entity: &Entity<Self>,
    ) -> AnyElement {
        let file_path = &self.diff_files[file_idx].path;
        let line_num = match line.kind {
            DiffLineKind::Removed => line.old_lineno,
            _ => line.new_lineno.or(line.old_lineno),
        };
        let side = match line.kind {
            DiffLineKind::Removed => CommentSide::Left,
            _ => CommentSide::Right,
        };

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

        let line_num_str = line_num.map(|n| format!("{n}")).unwrap_or_default();
        let content = line.content.trim_end();
        let line_selected = self.is_line_selected(global_line_idx);
        let text_bg = if line_selected {
            rgba(0x89b4fa30)
        } else {
            rgba(0x00000000)
        };
        let search_hl = self.search_highlights_for_gli(global_line_idx);

        // Syntax highlighted content + search highlights
        let content_el: AnyElement = {
            let mut hl: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();

            if let Some(highlights) = line.highlights.as_ref() {
                for s in highlights {
                    if s.byte_range.end <= content.len()
                        && content.is_char_boundary(s.byte_range.start)
                        && content.is_char_boundary(s.byte_range.end)
                    {
                        hl.push((
                            s.byte_range.clone(),
                            HighlightStyle {
                                color: Some(Hsla::from(s.color)),
                                ..Default::default()
                            },
                        ));
                    }
                }
            }

            for &(start, end, is_active) in &search_hl {
                if end <= content.len()
                    && content.is_char_boundary(start)
                    && content.is_char_boundary(end)
                {
                    let bg = if is_active { rgba(0xf9e2af88) } else { rgba(0xf9e2af44) };
                    hl.push((
                        start..end,
                        HighlightStyle {
                            background_color: Some(Hsla::from(bg)),
                            ..Default::default()
                        },
                    ));
                }
            }

            let hl = merge_highlights(hl);

            if hl.is_empty() {
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .bg(text_bg)
                    .text_color(text_col)
                    .pl(px(4.0))
                    .child(content.to_string())
                    .into_any_element()
            } else {
                let text = SharedString::from(content.to_string());
                let styled = StyledText::new(text).with_highlights(hl);
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .bg(text_bg)
                    .text_color(text_col)
                    .pl(px(4.0))
                    .child(styled)
                    .into_any_element()
            }
        };

        // Comment "+" button
        let gli = global_line_idx;
        let can_comment = self.can_comment();
        let has_line_num = line_num.is_some();
        let comment_btn: AnyElement = if can_comment && has_line_num {
            let ln = line_num.unwrap();
            let entity_down = entity.clone();
            let entity_up = entity.clone();
            let entity_move = entity.clone();
            let path_down = file_path.to_string();
            let path_up = file_path.to_string();

            // Check if this line is within a drag selection or active comment range
            let in_drag = self.comment_drag_start.as_ref().map_or(false, |(p, start, s)| {
                if p != file_path || *s != side { return false; }
                let end = self.comment_drag_end.unwrap_or(*start);
                let lo = (*start).min(end);
                let hi = (*start).max(end);
                ln >= lo && ln <= hi
            });
            let in_active = self.active_comment_line.as_ref().map_or(false, |(p, start, end, s)| {
                if p != file_path || *s != side { return false; }
                let lo = (*start).min(*end);
                let hi = (*start).max(*end);
                ln >= lo && ln <= hi
            });
            let gutter_highlight = in_drag || in_active;

            div()
                .id(ElementId::Name(
                    format!("pr-comment-btn-{}-{ln}-{}", file_path, side.api_str()).into(),
                ))
                .w(px(18.0))
                .h_full()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.0))
                .text_color(colors::accent())
                .cursor_pointer()
                .bg(if gutter_highlight { rgba(0xf9e2af40) } else { gutter_bg_color })
                .opacity(0.0)
                .when(gutter_highlight, |d: Stateful<Div>| d.opacity(1.0))
                .group_hover("pr-diff-line", |s| s.opacity(1.0).bg(rgba(0x89b4fa30)).text_color(colors::accent()))
                .hover(|s| s.bg(rgba(0x89b4fa40)))
                .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _window, cx| {
                    let path = path_down.clone();
                    entity_down.update(cx, |panel, cx| {
                        panel.comment_drag_start = Some((path, ln, side));
                        panel.comment_drag_end = None;
                        panel.comment_drag_start_gli = Some(gli);
                        panel.comment_drag_end_gli = Some(gli);
                        panel.last_drag_mouse_y = Some(f32::from(event.position.y));
                        panel.ensure_autoscroll_timer(cx);
                        cx.notify();
                    });
                    cx.stop_propagation();
                })
                .on_mouse_up(MouseButton::Left, move |_, _window, cx| {
                    let path = path_up.clone();
                    entity_up.update(cx, |panel, cx| {
                        if let Some((ref drag_path, start_ln, drag_side)) =
                            panel.comment_drag_start.take()
                        {
                            if drag_path == &path {
                                let start = start_ln.min(ln);
                                let end = start_ln.max(ln);
                                panel.comment_drag_end = None;
                                panel.comment_drag_start_gli = None;
                                panel.comment_drag_end_gli = None;
                                panel.start_comment(path, start, end, drag_side, cx);
                            }
                        }
                    });
                    cx.stop_propagation();
                })
                .on_mouse_move(move |_, _window, cx| {
                    entity_move.update(cx, |panel, cx| {
                        if panel.comment_drag_start.is_some() {
                            let mut changed = false;
                            if panel.comment_drag_end != Some(ln) {
                                panel.comment_drag_end = Some(ln);
                                changed = true;
                            }
                            if panel.comment_drag_end_gli != Some(gli) {
                                panel.comment_drag_end_gli = Some(gli);
                                changed = true;
                            }
                            if changed { cx.notify(); }
                        }
                    });
                })
                .child("+")
                .into_any_element()
        } else {
            div()
                .w(px(18.0))
                .flex_shrink_0()
                .bg(gutter_bg_color)
                .into_any_element()
        };

        // Check if this line is within an active drag selection (for row highlight).
        // Use global_line_idx range so the visual highlight is side-agnostic —
        // all rows in the dragged visual range light up regardless of LEFT/RIGHT.
        let in_drag = match (self.comment_drag_start_gli, self.comment_drag_end_gli) {
            (Some(start_gli), Some(end_gli)) => {
                let lo = start_gli.min(end_gli);
                let hi = start_gli.max(end_gli);
                global_line_idx >= lo && global_line_idx <= hi
            }
            _ => false,
        };

        // Also highlight lines within the active comment range (after drag completes).
        // This keeps side-awareness: the comment is attached to a specific side.
        let in_active_comment = self.active_comment_line.as_ref().map_or(false, |(p, start, end, s)| {
            if p != file_path || *s != side { return false; }
            if let Some(ln) = line_num {
                let lo = (*start).min(*end);
                let hi = (*start).max(*end);
                ln >= lo && ln <= hi
            } else {
                false
            }
        });

        let drag_highlight = if in_drag || in_active_comment { rgba(0xf9e2af30) } else { row_bg };

        // Mouse handlers for line selection and comment drag
        let entity_sel_down = entity.clone();
        let entity_sel_move = entity.clone();
        let entity_row_up = entity.clone();
        let row_path = file_path.to_string();

        let line_row = div()
            .group("pr-diff-line")
            .flex()
            .flex_row()
            .w_full()
            .min_h(px(20.0))
            .bg(drag_highlight)
            .font_family("Menlo")
            .text_xs()
            .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _window, cx| {
                entity_sel_down.update(cx, |panel, cx| {
                    panel.copy_anchor_line = Some(gli);
                    panel.copy_end_line = Some(gli);
                    panel.copy_selecting = true;
                    panel.last_drag_mouse_y = Some(f32::from(event.position.y));
                    panel.ensure_autoscroll_timer(cx);
                    cx.notify();
                });
            })
            .on_mouse_move(move |event: &MouseMoveEvent, _window, cx| {
                entity_sel_move.update(cx, |panel, cx| {
                    let mut changed = false;
                    if panel.copy_selecting {
                        panel.last_drag_mouse_y = Some(f32::from(event.position.y));
                        if panel.copy_end_line != Some(gli) {
                            panel.copy_end_line = Some(gli);
                            changed = true;
                        }
                    }
                    // Track comment drag across the whole row, not just the "+" button
                    if panel.comment_drag_start.is_some() {
                        panel.last_drag_mouse_y = Some(f32::from(event.position.y));
                        if panel.comment_drag_end_gli != Some(gli) {
                            panel.comment_drag_end_gli = Some(gli);
                            changed = true;
                        }
                        if let Some(ln) = line_num {
                            if panel.comment_drag_end != Some(ln) {
                                panel.comment_drag_end = Some(ln);
                                changed = true;
                            }
                        }
                    }
                    if changed {
                        cx.notify();
                    }
                });
            })
            .on_mouse_up(MouseButton::Left, {
                let path = row_path.clone();
                move |_, _window, cx| {
                    if let Some(ln) = line_num {
                        entity_row_up.update(cx, |panel, cx| {
                            if let Some((ref drag_path, start_ln, drag_side)) = panel.comment_drag_start.take() {
                                if drag_path == &path {
                                    let start = start_ln.min(ln);
                                    let end = start_ln.max(ln);
                                    panel.comment_drag_end = None;
                                    panel.comment_drag_start_gli = None;
                                    panel.comment_drag_end_gli = None;
                                    panel.start_comment(path.clone(), start, end, drag_side, cx);
                                }
                            }
                        });
                    }
                }
            })
            .child(
                div()
                    .w(px(44.0))
                    .flex_shrink_0()
                    .text_right()
                    .pr(px(4.0))
                    .pl(px(4.0))
                    .bg(gutter_bg_color)
                    .text_color(colors::text_muted())
                    .child(line_num_str),
            )
            .child(comment_btn)
            .child(
                div()
                    .w(px(16.0))
                    .flex_shrink_0()
                    .text_color(text_col)
                    .pl(px(4.0))
                    .bg(text_bg)
                    .child(prefix.to_string()),
            )
            .child(content_el);

        // Check if this line has comments or a comment form
        let has_comment_form = self.active_comment_line.as_ref().map_or(false, |(active_path, _start, end, active_side)| {
            active_path == file_path && line_num == Some(*end) && side == *active_side
        });
        let side_str = side.api_str().to_string();
        let has_comments = line_num.and_then(|ln| self.comment_index.get(&(file_path.to_string(), ln, side_str.clone()))).map_or(false, |v| !v.is_empty());

        if !has_comment_form && !has_comments {
            // Simple case: just the line, apply borders directly
            return line_row
                .border_l_1()
                .border_r_1()
                .border_color(colors::border())
                .when(is_last_in_file, |d: Div| d.border_b_1().rounded_b_md())
                .into_any_element();
        }

        // Wrap line + comments in a vertical container so they stack properly
        let mut wrapper = div()
            .flex()
            .flex_col()
            .w_full()
            .border_l_1()
            .border_r_1()
            .border_color(colors::border())
            .when(is_last_in_file, |d: Div| d.border_b_1().rounded_b_md())
            .child(line_row);

        // Inline comment form after this line
        if has_comment_form {
            wrapper = wrapper.child(self.render_comment_form(entity));
        }

        // Existing PR comments for this line
        if let Some(ln) = line_num {
            if let Some(indices) = self.comment_index.get(&(file_path.to_string(), ln, side_str)) {
                for &idx in indices {
                    wrapper = wrapper.child(self.render_comment_bubble(idx, entity));
                }
            }
        }

        wrapper.into_any_element()
    }

    /// Render one side of a split diff row (gutter + "+" + prefix + content).
    /// `is_left` selects which side this half represents: LEFT shows old line numbers
    /// and accepts comments on the LEFT side; RIGHT shows new line numbers and accepts
    /// RIGHT-side comments.
    fn render_split_half(
        &self,
        line: Option<&DiffLine>,
        is_left: bool,
        search_hl: &[(usize, usize, bool)],
        file_path: &str,
        global_line_idx: usize,
        entity: &Entity<Self>,
    ) -> Div {
        let side = if is_left { CommentSide::Left } else { CommentSide::Right };
        let (row_bg, gutter_bg_color, text_col, prefix, line_num_str, content, line_num) = match line {
            Some(l) => {
                let (bg, gbg, tc, pfx) = match l.kind {
                    DiffLineKind::Added => (added_line_bg(), added_gutter_bg(), colors::diff_added(), "+"),
                    DiffLineKind::Removed => (removed_line_bg(), removed_gutter_bg(), colors::diff_removed(), "-"),
                    DiffLineKind::Context => (rgba(0x00000000), gutter_bg(), colors::text_muted(), " "),
                };
                let ln = if is_left {
                    l.old_lineno
                } else {
                    l.new_lineno
                };
                let ln_str = ln.map(|n| format!("{n}")).unwrap_or_default();
                (bg, gbg, tc, pfx, ln_str, Some(l), ln)
            }
            None => {
                (rgba(0x00000000), gutter_bg(), colors::text_muted(), " ", String::new(), None, None)
            }
        };

        let content_el: AnyElement = if let Some(l) = content {
            let trimmed = l.content.trim_end();
            let mut hl: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();

            if let Some(highlights) = l.highlights.as_ref() {
                for s in highlights {
                    if s.byte_range.end <= trimmed.len()
                        && trimmed.is_char_boundary(s.byte_range.start)
                        && trimmed.is_char_boundary(s.byte_range.end)
                    {
                        hl.push((
                            s.byte_range.clone(),
                            HighlightStyle {
                                color: Some(Hsla::from(s.color)),
                                ..Default::default()
                            },
                        ));
                    }
                }
            }

            for &(start, end, is_active) in search_hl {
                if end <= trimmed.len()
                    && trimmed.is_char_boundary(start)
                    && trimmed.is_char_boundary(end)
                {
                    let bg = if is_active { rgba(0xf9e2af88) } else { rgba(0xf9e2af44) };
                    hl.push((
                        start..end,
                        HighlightStyle {
                            background_color: Some(Hsla::from(bg)),
                            ..Default::default()
                        },
                    ));
                }
            }

            let hl = merge_highlights(hl);

            if hl.is_empty() {
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_color(text_col)
                    .pl(px(4.0))
                    .child(trimmed.to_string())
                    .into_any_element()
            } else {
                let text = SharedString::from(trimmed.to_string());
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
                .into_any_element()
        };

        // Comment "+" button (between gutter and prefix). Mirrors the unified
        // path's button so each split half can start a comment on its own side.
        let gli = global_line_idx;
        let can_comment = self.can_comment();
        let comment_btn: AnyElement = if let (true, Some(ln)) = (can_comment, line_num) {
            let entity_down = entity.clone();
            let entity_up = entity.clone();
            let entity_move = entity.clone();
            let path_down = file_path.to_string();
            let path_up = file_path.to_string();
            let id_path = file_path.to_string();

            let in_drag = self.comment_drag_start.as_ref().map_or(false, |(p, start, s)| {
                if p != file_path || *s != side { return false; }
                let end = self.comment_drag_end.unwrap_or(*start);
                let lo = (*start).min(end);
                let hi = (*start).max(end);
                ln >= lo && ln <= hi
            });
            let in_active = self.active_comment_line.as_ref().map_or(false, |(p, start, end, s)| {
                if p != file_path || *s != side { return false; }
                let lo = (*start).min(*end);
                let hi = (*start).max(*end);
                ln >= lo && ln <= hi
            });
            let gutter_highlight = in_drag || in_active;

            div()
                .id(ElementId::Name(
                    format!("pr-split-cbtn-{}-{ln}-{}", id_path, side.api_str()).into(),
                ))
                .w(px(18.0))
                .h_full()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.0))
                .text_color(colors::accent())
                .cursor_pointer()
                .bg(if gutter_highlight { rgba(0xf9e2af40) } else { gutter_bg_color })
                .opacity(0.0)
                .when(gutter_highlight, |d: Stateful<Div>| d.opacity(1.0))
                .group_hover("pr-split-line", |s| s.opacity(1.0).bg(rgba(0x89b4fa30)).text_color(colors::accent()))
                .hover(|s| s.bg(rgba(0x89b4fa40)))
                .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _window, cx| {
                    let path = path_down.clone();
                    entity_down.update(cx, |panel, cx| {
                        panel.comment_drag_start = Some((path, ln, side));
                        panel.comment_drag_end = None;
                        panel.comment_drag_start_gli = Some(gli);
                        panel.comment_drag_end_gli = Some(gli);
                        panel.last_drag_mouse_y = Some(f32::from(event.position.y));
                        panel.ensure_autoscroll_timer(cx);
                        cx.notify();
                    });
                    cx.stop_propagation();
                })
                .on_mouse_up(MouseButton::Left, move |_, _window, cx| {
                    let path = path_up.clone();
                    entity_up.update(cx, |panel, cx| {
                        if let Some((ref drag_path, start_ln, drag_side)) =
                            panel.comment_drag_start.take()
                        {
                            if drag_path == &path {
                                let start = start_ln.min(ln);
                                let end = start_ln.max(ln);
                                panel.comment_drag_end = None;
                                panel.comment_drag_start_gli = None;
                                panel.comment_drag_end_gli = None;
                                panel.start_comment(path, start, end, drag_side, cx);
                            }
                        }
                    });
                    cx.stop_propagation();
                })
                .on_mouse_move(move |_, _window, cx| {
                    entity_move.update(cx, |panel, cx| {
                        if panel.comment_drag_start.is_some() {
                            let mut changed = false;
                            if panel.comment_drag_end != Some(ln) {
                                panel.comment_drag_end = Some(ln);
                                changed = true;
                            }
                            if panel.comment_drag_end_gli != Some(gli) {
                                panel.comment_drag_end_gli = Some(gli);
                                changed = true;
                            }
                            if changed { cx.notify(); }
                        }
                    });
                })
                .child("+")
                .into_any_element()
        } else {
            div()
                .w(px(18.0))
                .flex_shrink_0()
                .bg(gutter_bg_color)
                .into_any_element()
        };

        div()
            .group("pr-split-line")
            .flex()
            .flex_row()
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .bg(row_bg)
            .child(
                div()
                    .w(px(44.0))
                    .flex_shrink_0()
                    .text_right()
                    .pr(px(4.0))
                    .pl(px(4.0))
                    .bg(gutter_bg_color)
                    .text_color(colors::text_muted())
                    .child(line_num_str),
            )
            .child(comment_btn)
            .child(
                div()
                    .w(px(16.0))
                    .flex_shrink_0()
                    .text_color(text_col)
                    .pl(px(4.0))
                    .child(prefix.to_string()),
            )
            .child(content_el)
    }

    /// Render a side-by-side diff row with left (old) and right (new) halves.
    fn render_split_line_row(
        &self,
        file_idx: usize,
        left: Option<&DiffLine>,
        right: Option<&DiffLine>,
        global_line_idx: usize,
        is_last_in_file: bool,
        entity: &Entity<Self>,
    ) -> AnyElement {
        let file_path = self.diff_files[file_idx].path.clone();
        let search_hl = self.search_highlights_for_gli(global_line_idx);
        let left_half = self.render_split_half(left, true, &search_hl, &file_path, global_line_idx, entity);
        let right_half = self.render_split_half(right, false, &search_hl, &file_path, global_line_idx, entity);

        let line_selected = self.is_line_selected(global_line_idx);
        let sel_bg = if line_selected { rgba(0x89b4fa30) } else { rgba(0x00000000) };

        let entity_sel_down = entity.clone();
        let entity_sel_move = entity.clone();
        let gli = global_line_idx;

        // Existing comments / active form may attach to either side at either
        // side's line number. A removed-only row has no right line; an added-only
        // row has no left line.
        let left_line_num = left.and_then(|l| l.old_lineno);
        let right_line_num = right.and_then(|l| l.new_lineno);

        let active = self.active_comment_line.as_ref();
        let has_form_left = active.map_or(false, |(p, _s, e, side)| {
            p == &file_path && *side == CommentSide::Left && left_line_num == Some(*e)
        });
        let has_form_right = active.map_or(false, |(p, _s, e, side)| {
            p == &file_path && *side == CommentSide::Right && right_line_num == Some(*e)
        });
        let has_form = has_form_left || has_form_right;

        let left_comment_indices: Vec<usize> = left_line_num
            .and_then(|ln| {
                self.comment_index
                    .get(&(file_path.clone(), ln, "LEFT".to_string()))
                    .cloned()
            })
            .unwrap_or_default();
        let right_comment_indices: Vec<usize> = right_line_num
            .and_then(|ln| {
                self.comment_index
                    .get(&(file_path.clone(), ln, "RIGHT".to_string()))
                    .cloned()
            })
            .unwrap_or_default();
        let has_comments = !left_comment_indices.is_empty() || !right_comment_indices.is_empty();

        let line_row = div()
            .flex()
            .flex_row()
            .w_full()
            .min_h(px(20.0))
            .bg(sel_bg)
            .font_family("Menlo")
            .text_xs()
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
            .child(left_half)
            .child(
                div()
                    .w(px(1.0))
                    .h_full()
                    .flex_shrink_0()
                    .bg(colors::border()),
            )
            .child(right_half);

        if !has_form && !has_comments {
            return line_row
                .border_l_1()
                .border_r_1()
                .border_color(colors::border())
                .when(is_last_in_file, |d: Div| d.border_b_1().rounded_b_md())
                .into_any_element();
        }

        let mut wrapper = div()
            .flex()
            .flex_col()
            .w_full()
            .border_l_1()
            .border_r_1()
            .border_color(colors::border())
            .when(is_last_in_file, |d: Div| d.border_b_1().rounded_b_md())
            .child(line_row);

        if has_form {
            wrapper = wrapper.child(self.render_comment_form(entity));
        }

        for idx in left_comment_indices {
            wrapper = wrapper.child(self.render_comment_bubble(idx, entity));
        }
        for idx in right_comment_indices {
            wrapper = wrapper.child(self.render_comment_bubble(idx, entity));
        }

        wrapper.into_any_element()
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

        div()
            .id(ElementId::Name(
                format!("pr-collapse-{file_idx}-{row_idx}").into(),
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
                        .entry((file_idx, hunk_idx))
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

    /// Flatten a file's hunks into renderable segments.
    /// Flatten a file's hunks into renderable segments.
    ///
    /// When `source_lines` is available, expanding context can reach beyond hunk
    /// boundaries and inter-hunk gaps are shown as collapsible bars.
    fn flatten_file_lines(&self, file: &DiffFile, file_idx: usize) -> Vec<LineSegment> {
        let source = file.source_lines.as_deref();
        let total_source = source.map(|s| s.len()).unwrap_or(0);
        let mut segments: Vec<LineSegment> = Vec::new();
        let mut last_shown_ln: usize = 0;

        for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
            let lines = &hunk.lines;
            if lines.is_empty() {
                continue;
            }

            let (extra_top, extra_bottom) = self
                .expanded_context
                .get(&(file_idx, hunk_idx))
                .copied()
                .unwrap_or((0, 0));

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
                            hunk_idx,
                            direction: ExpandDirection::Top,
                        });
                    }
                    last_shown_ln = hunk_end_ln;
                } else {
                    segments.push(LineSegment::CollapsedContext {
                        count: lines.len(),
                        hunk_idx,
                        direction: ExpandDirection::Top,
                    });
                }
                continue;
            }

            let first_change = *change_indices.first().unwrap();
            let last_change = *change_indices.last().unwrap();

            let hunk_vis_start = first_change.saturating_sub(DEFAULT_CONTEXT + extra_top);
            let hunk_vis_end = (last_change + DEFAULT_CONTEXT + extra_bottom + 1).min(lines.len());

            let overflow_top = (DEFAULT_CONTEXT + extra_top).saturating_sub(first_change);
            let hunk_bottom_ctx = lines.len().saturating_sub(last_change + 1);
            let overflow_bottom = (DEFAULT_CONTEXT + extra_bottom).saturating_sub(hunk_bottom_ctx);

            if let Some(source) = source {
                let source_top_ln = if overflow_top > 0 {
                    hunk_start_ln
                        .saturating_sub(overflow_top)
                        .max(last_shown_ln + 1)
                        .max(1)
                } else {
                    hunk_start_ln
                };

                let gap = source_top_ln.saturating_sub(last_shown_ln + 1);
                if gap > 0 {
                    segments.push(LineSegment::CollapsedContext {
                        count: gap,
                        hunk_idx,
                        direction: ExpandDirection::Top,
                    });
                }

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

                if hunk_vis_start > 0 {
                    segments.push(LineSegment::CollapsedContext {
                        count: hunk_vis_start,
                        hunk_idx,
                        direction: ExpandDirection::Top,
                    });
                }

                for line in &lines[hunk_vis_start..hunk_vis_end] {
                    segments.push(LineSegment::Line(line.clone()));
                }

                let hidden_in_hunk_below = lines.len().saturating_sub(hunk_vis_end);
                if hidden_in_hunk_below > 0 {
                    segments.push(LineSegment::CollapsedContext {
                        count: hidden_in_hunk_below,
                        hunk_idx,
                        direction: ExpandDirection::Bottom,
                    });
                }

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
                if hunk_vis_start > 0 {
                    segments.push(LineSegment::CollapsedContext {
                        count: hunk_vis_start,
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
                        hunk_idx,
                        direction: ExpandDirection::Bottom,
                    });
                }
            }
        }

        if source.is_some() && total_source > last_shown_ln && !file.hunks.is_empty() {
            let remaining = total_source - last_shown_ln;
            if remaining > 0 {
                segments.push(LineSegment::CollapsedContext {
                    count: remaining,
                    hunk_idx: file.hunks.len() - 1,
                    direction: ExpandDirection::Bottom,
                });
            }
        }

        segments
    }


    fn render_comment_form(&self, entity: &Entity<Self>) -> Div {
        let entity_cancel = entity.clone();
        let entity_submit = entity.clone();

        let mut form = div()
            .w_full()
            .px_3()
            .py_2()
            .bg(comment_bg())
            .border_t_1()
            .border_b_1()
            .border_color(comment_border())
            .flex()
            .flex_col()
            .gap_2();

        // Show line range label
        if let Some((_, start, end, side)) = &self.active_comment_line {
            let prefix = match side {
                CommentSide::Left => "L",
                CommentSide::Right => "R",
            };
            let label = if start != end {
                format!("Comment on lines {prefix}{start}\u{2013}{prefix}{end}")
            } else {
                format!("Comment on line {prefix}{start}")
            };
            form = form.child(
                div()
                    .text_xs()
                    .text_color(colors::text_muted())
                    .child(label),
            );
        }

        if let Some(ref input) = self.comment_input {
            form = form.child(input.clone());
        }

        let submitting = self.submitting_comment;

        form = form.child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(
                    div()
                        .id("pr-comment-cancel")
                        .px_2()
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .text_xs()
                        .text_color(colors::text_muted())
                        .bg(colors::surface_hover())
                        .cursor_pointer()
                        .hover(|s| s.text_color(colors::text()))
                        .child("Cancel")
                        .on_click(move |_, _window, cx| {
                            entity_cancel.update(cx, |panel, cx| {
                                panel.active_comment_line = None;
                                panel.comment_input = None;
                                cx.notify();
                            });
                        }),
                )
                .child(
                    div()
                        .id("pr-comment-submit")
                        .px_2()
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .text_xs()
                        .text_color(gpui::rgb(0xffffff))
                        .bg(colors::accent())
                        .cursor_pointer()
                        .when(!submitting, |d: Stateful<Div>| {
                            d.hover(|s| s.opacity(0.8))
                        })
                        .when(submitting, |d: Stateful<Div>| d.opacity(0.5))
                        .child(if submitting { "Posting..." } else { "Comment" })
                        .on_click(move |_, _window, cx| {
                            entity_submit.update(cx, |panel, cx| {
                                if let Some(ref input) = panel.comment_input {
                                    let text = input.read(cx).text.clone();
                                    if !text.trim().is_empty() {
                                        panel.submit_comment(text, cx);
                                    }
                                }
                            });
                        }),
                ),
        );

        form
    }

    /// Build a prompt header with file name, line numbers, and the relevant code.
    fn build_prompt_header(&self, comment: &PrReviewComment) -> String {
        let path = &comment.path;
        let start = comment.start_line.unwrap_or_else(|| comment.line.unwrap_or(0));
        let end = comment.line.unwrap_or(start);

        let line_label = if start == end {
            format!("line {end}")
        } else {
            format!("lines {start}-{end}")
        };

        let mut code_lines: Vec<String> = Vec::new();
        let is_left = comment.side.as_deref() == Some("LEFT");
        for file in &self.diff_files {
            if file.path != *path {
                continue;
            }
            for hunk in &file.hunks {
                for line in &hunk.lines {
                    let ln = if is_left {
                        line.old_lineno
                    } else {
                        line.new_lineno.or(line.old_lineno)
                    };
                    if let Some(ln) = ln {
                        if ln >= start && ln <= end {
                            code_lines.push(line.content.trim_end().to_string());
                        }
                    }
                }
            }
        }

        let code_block = if code_lines.is_empty() {
            String::new()
        } else {
            format!("\n```\n{}\n```\n", code_lines.join("\n"))
        };

        format!("Address this PR comment in {path} ({line_label}):{code_block}")
    }

    /// Build a prompt for a single comment (main or reply).
    fn build_single_comment_prompt(&self, comment: &PrReviewComment, body: &str, author: &str) -> String {
        let header = self.build_prompt_header(comment);
        let plain = markdown::markdown_to_plain_text(body);
        format!("{header}\n@{author}: {plain}")
    }

    /// Build a prompt for the full thread (main comment + all replies).
    fn build_thread_prompt(&self, comment: &PrReviewComment, replies: &[&PrReviewComment]) -> String {
        let header = self.build_prompt_header(comment);
        let body = markdown::markdown_to_plain_text(&comment.body);
        let mut thread = format!("@{}: {}", comment.user.login, body);
        for reply in replies {
            let reply_body = markdown::markdown_to_plain_text(&reply.body);
            thread.push_str(&format!("\n  @{}: {}", reply.user.login, reply_body));
        }
        format!("{header}\n{thread}")
    }

    /// Build a combined prompt for all unresolved comment threads.
    fn build_all_unresolved_prompt(&self) -> String {
        let mut sections: Vec<String> = Vec::new();
        for comment in &self.pr_comments {
            // Only top-level comments (not replies)
            if comment.in_reply_to_id.is_some() {
                continue;
            }
            // Skip resolved threads
            if self.resolved_thread_ids.contains(&comment.id) {
                continue;
            }
            let replies: Vec<&PrReviewComment> = self
                .pr_comments
                .iter()
                .filter(|c| c.in_reply_to_id == Some(comment.id))
                .collect();
            sections.push(self.build_thread_prompt(comment, &replies));
        }
        if sections.is_empty() {
            return String::new();
        }
        let count = sections.len();
        let preamble = format!(
            "Address these {count} unresolved PR comment{}:\n",
            if count == 1 { "" } else { "s" }
        );
        preamble + &sections.join("\n\n---\n\n")
    }

    /// Count unresolved top-level comment threads.
    fn unresolved_comment_count(&self) -> usize {
        self.pr_comments
            .iter()
            .filter(|c| c.in_reply_to_id.is_none() && !self.resolved_thread_ids.contains(&c.id))
            .count()
    }

    /// Render a small copy-as-prompt button with custom label.
    fn render_copy_prompt_btn(&self, key: String, label: &str, prompt_text: String, entity: &Entity<Self>) -> Stateful<Div> {
        let is_copied = self.copied_prompt_key.as_deref() == Some(&key);
        let entity = entity.clone();
        let key_for_click = key.clone();
        let display = if is_copied { "Copied!".to_string() } else { label.to_string() };
        div()
            .id(ElementId::Name(format!("pr-copy-{key}").into()))
            .px_1()
            .py(px(1.0))
            .rounded(px(3.0))
            .text_size(px(10.0))
            .text_color(colors::text_muted())
            .cursor_pointer()
            .hover(|s| {
                s.text_color(colors::accent())
                    .bg(colors::surface_hover())
            })
            .child(display)
            .on_click(move |_, _window, cx| {
                cx.stop_propagation();
                cx.write_to_clipboard(ClipboardItem::new_string(prompt_text.clone()));
                entity.update(cx, |panel, cx| {
                    panel.copy_selecting = false;
                    panel.copied_prompt_key = Some(key_for_click.clone());
                    panel._copied_comment_timer = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(1500))
                            .await;
                        let _ = cx.update(|cx| {
                            let _ = this.update(cx, |panel, cx| {
                                panel.copied_prompt_key = None;
                                cx.notify();
                            });
                        });
                    }));
                    cx.notify();
                });
            })
    }

    fn render_comment_bubble(
        &self,
        comment_idx: usize,
        entity: &Entity<Self>,
    ) -> Div {
        let comment = &self.pr_comments[comment_idx];
        let author = &comment.user.login;
        let body = &comment.body;

        // Gather replies
        let comment_id = comment.id;
        let replies: Vec<&PrReviewComment> = self
            .pr_comments
            .iter()
            .filter(|c| c.in_reply_to_id == Some(comment_id))
            .collect();

        let is_thread_resolved = self.resolved_thread_ids.contains(&comment_id);
        let is_expanded_resolved = self.expanded_resolved.contains(&comment_id);
        let is_manually_collapsed = self.collapsed_comments.contains(&comment_id);

        // Show collapsed bar when resolved (and not manually expanded) or manually collapsed
        let show_collapsed = (is_thread_resolved && !is_expanded_resolved) || is_manually_collapsed;
        if show_collapsed {
            let entity_expand = entity.clone();
            let expand_cid = comment_id;
            let reply_count = replies.len();
            let summary = format!(
                "{author}: {}",
                body.lines().next().unwrap_or("").chars().take(60).collect::<String>(),
            );

            let mut bar = div()
                .w_full()
                .px_3()
                .py(px(4.0))
                .bg(comment_bg())
                .border_t_1()
                .border_b_1()
                .border_color(comment_border())
                .flex()
                .flex_row()
                .items_center()
                .gap_2();

            // Icon: checkmark for resolved, comment icon for manually collapsed
            if is_thread_resolved {
                bar = bar.child(
                    div()
                        .text_xs()
                        .text_color(rgba(0x2ea04399))
                        .child("\u{f00c}")
                        .font_family(util::ICON_FONT)
                );
            } else {
                bar = bar.child(
                    div()
                        .text_xs()
                        .text_color(colors::text_muted())
                        .child("\u{f075}")
                        .font_family(util::ICON_FONT)
                );
            }

            // Summary text — click to expand
            bar = bar.child(
                div()
                    .id(ElementId::Name(format!("pr-collapsed-expand-{comment_idx}").into()))
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_xs()
                    .text_color(colors::text_muted())
                    .cursor_pointer()
                    .hover(|s| s.text_color(colors::text()))
                    .child(if reply_count > 0 {
                        format!("{summary}  (+{reply_count} replies)")
                    } else {
                        summary
                    })
                    .on_click(move |_, _window, cx| {
                        entity_expand.update(cx, |panel, cx| {
                            if is_thread_resolved {
                                panel.expanded_resolved.insert(expand_cid);
                            }
                            panel.collapsed_comments.remove(&expand_cid);
                            cx.notify();
                        });
                    })
            );

            // Unresolve button for resolved threads
            if is_thread_resolved {
                let entity_unresolve = entity.clone();
                let unresolve_cid = comment_id;
                bar = bar.child(
                    div()
                        .id(ElementId::Name(format!("pr-collapsed-unresolve-{comment_idx}").into()))
                        .px_2()
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .text_xs()
                        .text_color(colors::text_muted())
                        .cursor_pointer()
                        .hover(|s| s.text_color(colors::accent()).bg(colors::surface_hover()))
                        .child("Unresolve")
                        .on_click(move |_, _window, cx| {
                            entity_unresolve.update(cx, |panel, cx| {
                                panel.toggle_resolve_thread(unresolve_cid, cx);
                            });
                        })
                );
            }

            return bar;
        }

        let mut bubble = div()
            .w_full()
            .px_3()
            .py_2()
            .bg(comment_bg())
            .border_t_1()
            .border_b_1()
            .border_color(comment_border())
            .flex()
            .flex_col()
            .gap_1();

        // Line range header
        if let Some(line) = comment.line {
            let side_prefix = match comment.side.as_deref() {
                Some("LEFT") => "L",
                _ => "R",
            };
            let label = if let Some(start) = comment.start_line {
                if start != line {
                    let start_prefix = match comment.start_side.as_deref().or(comment.side.as_deref()) {
                        Some("LEFT") => "L",
                        _ => "R",
                    };
                    format!("Comment on lines {start_prefix}{start}\u{2013}{side_prefix}{line}")
                } else {
                    format!("Comment on line {side_prefix}{line}")
                }
            } else {
                format!("Comment on line {side_prefix}{line}")
            };
            bubble = bubble.child(
                div()
                    .text_xs()
                    .text_color(colors::text_muted())
                    .pb(px(2.0))
                    .child(label),
            );
        }

        // Main comment with hover copy button inline with author
        let main_prompt = self.build_single_comment_prompt(comment, body, author);
        let main_btn = self.render_copy_prompt_btn(
            format!("main-{comment_idx}"),
            "Copy as prompt",
            main_prompt,
            entity,
        );
        bubble = bubble.child(self.render_single_comment(author, body, Some(main_btn), format!("pr-comment-{comment_idx}").into()));

        // Replies, each with hover copy button inline with author
        for (i, reply) in replies.iter().enumerate() {
            let reply_prompt = self.build_single_comment_prompt(comment, &reply.body, &reply.user.login);
            let reply_btn = self.render_copy_prompt_btn(
                format!("reply-{comment_idx}-{i}"),
                "Copy as prompt",
                reply_prompt,
                entity,
            );
            bubble = bubble.child(
                div()
                    .pl_3()
                    .border_l_2()
                    .border_color(comment_border())
                    .child(self.render_single_comment(&reply.user.login, &reply.body, Some(reply_btn), format!("pr-reply-c-{comment_idx}-{i}").into())),
            );
        }

        // Inline reply form (rendered as part of the thread)
        let is_replying = self.reply_to.map_or(false, |(_, idx)| idx == comment_idx);
        if is_replying {
            if let Some(ref input) = self.reply_input {
                let entity_cancel = entity.clone();
                let entity_submit = entity.clone();
                let submitting = self.submitting_reply;

                bubble = bubble.child(
                    div()
                        .pl_3()
                        .border_l_2()
                        .border_color(comment_border())
                        .mt_1()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(input.clone())
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("pr-reply-cancel")
                                        .px_2()
                                        .py(px(4.0))
                                        .rounded(px(4.0))
                                        .text_xs()
                                        .text_color(colors::text_muted())
                                        .cursor_pointer()
                                        .hover(|s| s.text_color(colors::text()).bg(colors::surface_hover()))
                                        .child("Cancel")
                                        .on_click(move |_, _window, cx| {
                                            entity_cancel.update(cx, |panel, cx| {
                                                panel.reply_to = None;
                                                panel.reply_input = None;
                                                cx.notify();
                                            });
                                        }),
                                )
                                .child(
                                    div()
                                        .id("pr-reply-submit")
                                        .px_2()
                                        .py(px(4.0))
                                        .rounded(px(4.0))
                                        .text_xs()
                                        .text_color(gpui::rgb(0xffffff))
                                        .bg(colors::accent())
                                        .cursor_pointer()
                                        .when(!submitting, |d: Stateful<Div>| {
                                            d.hover(|s| s.opacity(0.8))
                                        })
                                        .when(submitting, |d: Stateful<Div>| d.opacity(0.5))
                                        .child(if submitting { "Posting..." } else { "Reply" })
                                        .on_click(move |_, _window, cx| {
                                            entity_submit.update(cx, |panel, cx| {
                                                if let Some(ref input) = panel.reply_input {
                                                    let text = input.read(cx).text.clone();
                                                    if !text.trim().is_empty() {
                                                        panel.submit_reply(text, cx);
                                                    }
                                                }
                                            });
                                        }),
                                ),
                        ),
                );
            }
        }

        // Thread-level action buttons row
        let has_replies = self.pr_comments.iter().any(|c| c.in_reply_to_id == Some(comment_id));
        let thread_prompt = self.build_thread_prompt(comment, &self.pr_comments.iter().filter(|c| c.in_reply_to_id == Some(comment_id)).collect::<Vec<_>>());
        let entity_reply = entity.clone();
        let cid = comment_id;

        let mut actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .mt_1();

        if !is_replying {
            actions = actions.child(
                div()
                    .id(ElementId::Name(
                        format!("pr-reply-{comment_idx}").into(),
                    ))
                    .px_2()
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .text_xs()
                    .text_color(colors::text_muted())
                    .cursor_pointer()
                    .hover(|s| {
                        s.text_color(colors::accent())
                            .bg(colors::surface_hover())
                    })
                    .child("Reply")
                    .on_click(move |_, _window, cx| {
                        entity_reply.update(cx, |panel, cx| {
                            panel.start_reply(cid, comment_idx, cx);
                        });
                    }),
            );
        }

        // Collapse button
        {
            let entity_collapse = entity.clone();
            let collapse_cid = comment_id;
            actions = actions.child(
                div()
                    .id(ElementId::Name(format!("pr-collapse-{comment_idx}").into()))
                    .px_2()
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .text_xs()
                    .text_color(colors::text_muted())
                    .cursor_pointer()
                    .hover(|s| s.text_color(colors::accent()).bg(colors::surface_hover()))
                    .child("Collapse")
                    .on_click(move |_, _window, cx| {
                        entity_collapse.update(cx, |panel, cx| {
                            panel.collapsed_comments.insert(collapse_cid);
                            panel.expanded_resolved.remove(&collapse_cid);
                            cx.notify();
                        });
                    }),
            );
        }

        // Resolve / Unresolve button
        let is_resolved = self.resolved_thread_ids.contains(&comment_id);
        let has_thread_id = self.thread_node_ids.contains_key(&comment_id);
        if has_thread_id {
            let entity_resolve = entity.clone();
            let resolve_cid = comment_id;
            actions = actions.child(
                div()
                    .id(ElementId::Name(
                        format!("pr-resolve-{comment_idx}").into(),
                    ))
                    .px_2()
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .text_xs()
                    .text_color(if is_resolved { colors::accent() } else { colors::text_muted() })
                    .cursor_pointer()
                    .hover(|s| {
                        s.text_color(colors::accent())
                            .bg(colors::surface_hover())
                    })
                    .child(if is_resolved { "Unresolve" } else { "Resolve" })
                    .on_click(move |_, _window, cx| {
                        entity_resolve.update(cx, |panel, cx| {
                            panel.toggle_resolve_thread(resolve_cid, cx);
                        });
                    }),
            );
        }

        if has_replies {
            let thread_key = format!("thread-{comment_idx}");
            let is_thread_copied = self.copied_prompt_key.as_deref() == Some(&thread_key);
            let entity_thread = entity.clone();
            actions = actions.child(
                div()
                    .id(ElementId::Name(format!("pr-copy-{thread_key}").into()))
                    .px_2()
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .text_xs()
                    .text_color(colors::text_muted())
                    .cursor_pointer()
                    .hover(|s| {
                        s.text_color(colors::accent())
                            .bg(colors::surface_hover())
                    })
                    .child(if is_thread_copied { "Copied!" } else { "Copy thread as prompt" })
                    .on_click(move |_, _window, cx| {
                        cx.stop_propagation();
                        cx.write_to_clipboard(ClipboardItem::new_string(thread_prompt.clone()));
                        entity_thread.update(cx, |panel, cx| {
                            panel.copy_selecting = false;
                            panel.copied_prompt_key = Some(thread_key.clone());
                            panel._copied_comment_timer = Some(cx.spawn(async move |this, cx| {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(1500))
                                    .await;
                                let _ = cx.update(|cx| {
                                    let _ = this.update(cx, |panel, cx| {
                                        panel.copied_prompt_key = None;
                                        cx.notify();
                                    });
                                });
                            }));
                            cx.notify();
                        });
                    }),
            );
        }

        bubble = bubble.child(actions);

        bubble
    }

    fn render_single_comment(&self, author: &str, body: &str, copy_btn: Option<Stateful<Div>>, group_name: SharedString) -> Div {
        let gn = group_name.clone();
        div()
            .group(group_name)
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .w(px(16.0))
                            .h(px(16.0))
                            .rounded_full()
                            .bg(colors::accent())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(9.0))
                            .text_color(gpui::rgb(0xffffff))
                            .child(
                                author
                                    .chars()
                                    .next()
                                    .unwrap_or('?')
                                    .to_uppercase()
                                    .to_string(),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors::text())
                            .child(author.to_string()),
                    )
                    .when_some(copy_btn, |d, btn| {
                        d.child(
                            btn
                                .ml_2()
                                .opacity(0.0)
                                .group_hover(gn, |s| s.opacity(1.0)),
                        )
                    }),
            )
            .child({
                let mut body_el = div()
                    .pl(px(20.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0));
                for el in markdown::render_markdown(body) {
                    body_el = body_el.child(el);
                }
                body_el
            })
    }
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
    /// Side-by-side row: left (old) and right (new) line indices into cached_file_segments.
    SplitLine {
        file_idx: usize,
        left_seg: Option<usize>,
        right_seg: Option<usize>,
        global_line_idx: usize,
        is_last_in_file: bool,
    },
}

/// Estimated rendered height per row type. Used to report a stable,
/// total content height to the scrollbar — GPUI's `ListState` only
/// sums measured items, so for long diffs the measured height grows
/// (and the thumb shrinks) as rows scroll into view. These estimates
/// must stay close to the actual `min_h` values in the render fns.
fn row_height_estimate_px(row: &FlatRow) -> f32 {
    match row {
        FlatRow::FileHeader { .. } => 34.0,
        FlatRow::EmptyFile { .. } => 40.0,
        FlatRow::Line { .. } | FlatRow::SplitLine { .. } => 20.0,
        FlatRow::CollapsedContext { .. } => 22.0,
    }
}

/// Inter-file breathing-room above each file header card. Centralized so
/// the renderer, sticky activation, and placeholder all stay in sync.
fn file_header_top_pad(file_idx: usize) -> f32 {
    if file_idx > 0 {
        12.0
    } else {
        16.0
    }
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

// ── Search (Cmd+F) ──

impl PrDiffPanel {
    fn find_search_matches(&mut self) {
        self.search_matches.clear();
        if self.search_query.is_empty() {
            return;
        }
        let query_lower = self.search_query.to_lowercase();
        for (gli, text) in self.copy_line_contents.iter().enumerate() {
            let text_lower = text.to_lowercase();
            let mut from = 0;
            while let Some(pos) = text_lower[from..].find(&query_lower) {
                let start = from + pos;
                let end = start + query_lower.len();
                self.search_matches.push((gli, start, end));
                from = start + 1;
            }
        }
        if !self.search_matches.is_empty() {
            self.search_match_ix = self.search_match_ix.min(self.search_matches.len() - 1);
        } else {
            self.search_match_ix = 0;
        }
    }

    fn search_next(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_match_ix = (self.search_match_ix + 1) % self.search_matches.len();
        self.scroll_to_active_match();
    }

    fn scroll_to_active_match(&mut self) {
        let Some(&(gli, _, _)) = self.search_matches.get(self.search_match_ix) else {
            return;
        };
        for (row_idx, row) in self.flat_rows.iter().enumerate() {
            let row_gli = match row {
                FlatRow::Line { global_line_idx, .. } => Some(*global_line_idx),
                FlatRow::SplitLine { global_line_idx, .. } => Some(*global_line_idx),
                _ => None,
            };
            if row_gli == Some(gli) {
                self.list_state.scroll_to(ListOffset {
                    item_ix: row_idx,
                    offset_in_item: px(0.0),
                });
                return;
            }
        }
    }

    fn search_find_nearest(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_match_ix = 0;
        self.scroll_to_active_match();
    }

    fn search_highlights_for_gli(&self, gli: usize) -> Vec<(usize, usize, bool)> {
        if !self.search_active || self.search_query.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (ix, &(m_gli, start, end)) in self.search_matches.iter().enumerate() {
            if m_gli == gli {
                out.push((start, end, ix == self.search_match_ix));
            }
        }
        out
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_active = true;
        self.search_query.clear();
        self.search_matches.clear();
        self.search_match_ix = 0;

        let entity = cx.entity().clone();
        let entity_cancel = cx.entity().clone();

        let input = cx.new(|cx| {
            let mut inp = crate::text_input::TextInput::new(cx);
            inp.set_placeholder("Search diff...");

            inp.set_on_submit(Rc::new(move |_text, _window, cx| {
                entity.update(cx, |panel, cx| {
                    panel.search_next();
                    cx.notify();
                });
            }));

            inp.set_on_cancel(Rc::new(move |window, cx| {
                entity_cancel.update(cx, |panel, cx| {
                    panel.close_search(window);
                    cx.notify();
                });
            }));

            inp
        });

        cx.observe(&input, |panel, input, cx| {
            let new_text = input.read(cx).text.clone();
            if panel.search_query != new_text {
                panel.search_query = new_text;
                panel.find_search_matches();
                panel.search_find_nearest();
                cx.notify();
            }
        })
        .detach();

        self.search_input = Some(input.clone());
        input.read(cx).focus(window);
        cx.notify();
    }

    fn close_search(&mut self, window: &mut Window) {
        self.search_active = false;
        self.search_query.clear();
        self.search_input = None;
        self.search_matches.clear();
        self.search_match_ix = 0;
        self.focus_handle.focus(window);
    }

    fn handle_find(&mut self, _: &FindInFile, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_active {
            self.close_search(window);
        } else {
            self.open_search(window, cx);
        }
        cx.notify();
    }
}

// ── Render ──

impl Render for PrDiffPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Trigger async gh check + PR detection on first render
        if self.needs_initial_refresh {
            self.needs_initial_refresh = false;
            self.refresh(cx);
            self.start_poll_if_needed(cx);
        }

        // Rebuild the flat row cache if data has changed
        if self.flat_cache_dirty {
            self.rebuild_flat_cache();
        }

        // Use GitHub's authoritative stats when available, fall back to local computation
        let (total_files, total_adds, total_dels) = if let Some(ref pr) = self.pr_info {
            (
                pr.changed_files as usize,
                pr.additions as usize,
                pr.deletions as usize,
            )
        } else {
            (
                self.diff_files.len(),
                self.total_additions(),
                self.total_deletions(),
            )
        };

        let entity_move = cx.entity().clone();
        let entity_up = cx.entity().clone();

        let mut panel = div()
            .id("pr-diff-panel")
            .flex()
            .flex_col()
            .size_full()
            .min_w(px(0.0))
            .flex_1()
            .overflow_hidden()
            .bg(colors::surface())
            .track_focus(&self.focus_handle)
            .key_context("PrDiffPanel")
            .on_action(cx.listener(Self::handle_find))
            .on_mouse_move(move |event: &MouseMoveEvent, _window, cx| {
                entity_move.update(cx, |panel, cx| {
                    if panel.resizing_tree {
                        let delta = f32::from(event.position.x) - panel.tree_drag_start_x;
                        let new_w = (panel.tree_drag_start_width + delta)
                            .clamp(MIN_TREE_WIDTH, MAX_TREE_WIDTH);
                        panel.tree_width = new_w;
                        cx.notify();
                    }
                    if panel.copy_selecting || panel.comment_drag_start.is_some() {
                        panel.last_drag_mouse_y = Some(f32::from(event.position.y));
                    }
                    if let Some(new_offset) =
                        scrollbar::drag_to_offset(&panel.scrollbar, event.position.y)
                    {
                        // Translate estimated-px target to a concrete item
                        // index; see `item_ix_for_estimated_offset` for
                        // why we don't use `set_offset_from_scrollbar`.
                        let (item_ix, within) =
                            panel.item_ix_for_estimated_offset(f32::from(new_offset));
                        panel.list_state.scroll_to(ListOffset {
                            item_ix,
                            offset_in_item: px(within),
                        });
                        panel.bump_scrollbar(cx);
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
                        // Finalize any in-progress comment drag (mouse-up may have
                        // landed on a non-line row like a file header or collapse bar).
                        if let Some((path, start_ln, drag_side)) = panel.comment_drag_start.take() {
                            let end_ln = panel.comment_drag_end.unwrap_or(start_ln);
                            let start = start_ln.min(end_ln);
                            let end = start_ln.max(end_ln);
                            panel.comment_drag_end = None;
                            panel.comment_drag_start_gli = None;
                            panel.comment_drag_end_gli = None;
                            panel._autoscroll_task = None;
                            panel.last_drag_mouse_y = None;
                            panel.start_comment(path, start, end, drag_side, cx);
                            changed = true;
                        }
                        // End text selection and copy
                        if panel.copy_selecting {
                            panel.copy_selecting = false;
                            panel._autoscroll_task = None;
                            panel.last_drag_mouse_y = None;
                            panel.copy_selected_text(cx);
                            changed = true;
                        }
                        if panel.scrollbar.drag_cursor_within_thumb.is_some() {
                            panel.scrollbar.drag_cursor_within_thumb = None;
                            panel.list_state.scrollbar_drag_ended();
                            panel.bump_scrollbar(cx);
                            changed = true;
                        }
                        if changed {
                            cx.notify();
                        }
                    });
                },
            );

        // ── Top header ──
        let _pr_url = self.pr_info.as_ref().map(|pr| pr.url.clone());
        let header_el: AnyElement = if let Some(ref pr) = self.pr_info {
            let url = pr.url.clone();
            let label = format!("PR #{} \u{00B7} {}", pr.number, pr.title);
            div()
                .id("pr-title-link")
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors::accent())
                .overflow_hidden()
                .text_ellipsis()
                .flex_shrink()
                .cursor_pointer()
                .border_b_1()
                .border_color(gpui::rgba(0x00000000))
                .hover(|s| s.border_color(colors::accent()))
                .on_click(move |_, _window, cx| {
                    cx.open_url(&url);
                })
                .child(label)
                .into_any_element()
        } else {
            let header_text = if self.branch == self.base_ref {
                "On base branch — no diff".to_string()
            } else if total_files == 0 && !self.loading {
                format!("{} \u{2192} {} \u{00B7} No changes", self.branch, self.base_ref)
            } else {
                format!("{} \u{2192} {}", self.branch, self.base_ref)
            };
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors::text())
                .overflow_hidden()
                .text_ellipsis()
                .flex_1()
                .child(header_text)
                .into_any_element()
        };

        let mut header = div()
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
            .child(header_el);

        if total_adds > 0 || total_dels > 0 || total_files > 0 {
            header = header.child(
                div()
                    .ml_2()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .flex_shrink_0()
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors::text_muted())
                            .child(format!(
                                "{total_files} file{}",
                                if total_files == 1 { "" } else { "s" }
                            )),
                    )
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
            );
        }

        // Spacer + Unified / Split toggle
        let is_split = self.view_mode == DiffViewMode::Split;
        let entity_toggle = cx.entity().clone();
        header = header
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(1.0))
                    .rounded(px(6.0))
                    .bg(colors::surface())
                    .p(px(2.0))
                    .child(
                        div()
                            .id("pr-diff-mode-unified")
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .text_xs()
                            .cursor_pointer()
                            .when(!is_split, |d: Stateful<Div>| {
                                d.bg(colors::surface_hover())
                                    .text_color(colors::text())
                            })
                            .when(is_split, |d: Stateful<Div>| {
                                d.text_color(colors::text_muted())
                                    .hover(|s| s.text_color(colors::text()))
                            })
                            .child("Unified")
                            .on_click({
                                let entity = entity_toggle.clone();
                                move |_, _window, cx| {
                                    entity.update(cx, |panel, cx| {
                                        if panel.view_mode != DiffViewMode::Unified {
                                            panel.view_mode = DiffViewMode::Unified;
                                            panel.flat_cache_dirty = true;
                                            cx.notify();
                                        }
                                    });
                                }
                            }),
                    )
                    .child(
                        div()
                            .id("pr-diff-mode-split")
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .text_xs()
                            .cursor_pointer()
                            .when(is_split, |d: Stateful<Div>| {
                                d.bg(colors::surface_hover())
                                    .text_color(colors::text())
                            })
                            .when(!is_split, |d: Stateful<Div>| {
                                d.text_color(colors::text_muted())
                                    .hover(|s| s.text_color(colors::text()))
                            })
                            .child("Split")
                            .on_click(move |_, _window, cx| {
                                entity_toggle.update(cx, |panel, cx| {
                                    if panel.view_mode != DiffViewMode::Split {
                                        panel.view_mode = DiffViewMode::Split;
                                        panel.flat_cache_dirty = true;
                                        cx.notify();
                                    }
                                });
                            }),
                    ),
            );

        // Refresh button
        let entity_refresh = cx.entity().clone();
        header = header.child(
            div()
                .id("pr-refresh-btn")
                .ml_2()
                .w(px(24.0))
                .h(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .cursor_pointer()
                .text_color(colors::text_muted())
                .hover(|s| s.bg(colors::surface_hover()).text_color(colors::text()))
                .text_xs()
                .child("\u{21BB}")
                .on_click(move |_, _window, cx| {
                    entity_refresh.update(cx, |panel, cx| {
                        panel.refresh(cx);
                    });
                }),
        );

        // "Copy all unresolved as prompt" button
        let unresolved_count = self.unresolved_comment_count();
        if unresolved_count > 0 {
            let prompt_text = self.build_all_unresolved_prompt();
            let entity_copy = cx.entity().clone();
            let copy_key = "all-unresolved".to_string();
            let is_copied = self.copied_prompt_key.as_deref() == Some("all-unresolved");
            let label = if is_copied {
                "Copied!".to_string()
            } else {
                format!("Copy {unresolved_count} unresolved as prompt")
            };
            header = header.child(
                div()
                    .id("pr-copy-all-unresolved")
                    .ml_2()
                    .px_2()
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .text_xs()
                    .text_color(colors::text_muted())
                    .flex_shrink_0()
                    .cursor_pointer()
                    .hover(|s| {
                        s.text_color(colors::accent())
                            .bg(colors::surface_hover())
                    })
                    .child(label)
                    .on_click(move |_, _window, cx| {
                        cx.stop_propagation();
                        cx.write_to_clipboard(ClipboardItem::new_string(prompt_text.clone()));
                        entity_copy.update(cx, |panel, cx| {
                            panel.copy_selecting = false;
                            panel.copied_prompt_key = Some(copy_key.clone());
                            panel._copied_comment_timer = Some(cx.spawn(async move |this, cx| {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(1500))
                                    .await;
                                let _ = cx.update(|cx| {
                                    let _ = this.update(cx, |panel, cx| {
                                        panel.copied_prompt_key = None;
                                        cx.notify();
                                    });
                                });
                            }));
                            cx.notify();
                        });
                    }),
            );
        }

        panel = panel.child(header);

        // ── Search bar (Cmd+F) ──
        if self.search_active {
            if let Some(search_input) = &self.search_input {
                let match_count = self.search_matches.len();
                let match_info = if self.search_query.is_empty() {
                    String::new()
                } else if match_count == 0 {
                    "No matches".to_string()
                } else {
                    format!("{}/{}", self.search_match_ix + 1, match_count)
                };

                panel = panel.child(
                    div()
                        .id("pr-diff-search-bar")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .bg(colors::surface())
                        .border_b_1()
                        .border_color(colors::border())
                        .child(
                            div()
                                .text_xs()
                                .text_color(colors::text_muted())
                                .child("Find:"),
                        )
                        .child(div().flex_1().child(search_input.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(colors::text_muted())
                                .pr_2()
                                .child(match_info),
                        ),
                );
            }
        }

        // ── gh status hint (only shown after we've actually checked) ──
        match self.gh_status {
            GhStatus::Unknown => {} // Haven't checked yet, don't show anything
            GhStatus::NotInstalled => {
                panel = panel.child(
                    div()
                        .w_full()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(colors::text_muted())
                        .bg(rgba(0xf9e2af10))
                        .border_b_1()
                        .border_color(colors::border())
                        .child("Install gh CLI for PR features and comments"),
                );
            }
            GhStatus::NotAuthenticated => {
                panel = panel.child(
                    div()
                        .w_full()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(colors::text_muted())
                        .bg(rgba(0xf9e2af10))
                        .border_b_1()
                        .border_color(colors::border())
                        .child("Run gh auth login to enable PR features"),
                );
            }
            GhStatus::Available => {}
        }

        // Loading indicator
        if self.loading {
            panel = panel.child(
                div()
                    .w_full()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(colors::text_muted())
                    .border_b_1()
                    .border_color(colors::border())
                    .child("Loading PR data..."),
            );
        }

        // PR status bar: checks + merge action row
        if let Some(status_bar) = self.render_pr_status_bar(cx) {
            panel = panel.child(status_bar);
        }

        // Empty state
        if total_files == 0 && !self.loading {
            panel = panel.child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_color(colors::text_muted())
                    .text_sm()
                    .child(if self.branch == self.base_ref {
                        "Switch to a feature branch to see the diff"
                    } else {
                        "No changes between branches"
                    }),
            );
            return panel;
        }

        // ── Body: file tree | resize handle | diffs ──
        let tree_width = self.tree_width;
        let tree_collapsed = self.tree_collapsed;
        let entity_down = cx.entity().clone();

        let mut body = div()
            .id("pr-diff-panel-body")
            .flex()
            .flex_row()
            .flex_1()
            .w_full()
            .min_w(px(0.0))
            .overflow_hidden();

        // File tree toggle + sidebar
        {
            let entity_toggle = cx.entity().clone();
            // nf-cod-layout_sidebar_left (same icon as workspace sidebar toggle)
            let toggle_icon = "\u{F06FD}";
            let toggle_btn = div()
                .id("pr-tree-toggle")
                .flex()
                .items_center()
                .justify_center()
                .w_full()
                .py(px(4.0))
                .border_b_1()
                .border_color(colors::border())
                .cursor_pointer()
                .font_family(util::ICON_FONT)
                .text_size(px(14.0))
                .text_color(if tree_collapsed { colors::text_muted() } else { colors::accent() })
                .hover(|s| s.text_color(colors::text()).bg(colors::surface_hover()))
                .on_click(move |_, _window, cx| {
                    entity_toggle.update(cx, |panel, cx| {
                        panel.tree_collapsed = !panel.tree_collapsed;
                        cx.notify();
                    });
                })
                .child(toggle_icon);

            if tree_collapsed {
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .h_full()
                        .w(px(24.0))
                        .border_r_1()
                        .border_color(colors::border())
                        .child(toggle_btn),
                );
            } else {
                let tree_panel = div()
                    .id("pr-diff-file-tree")
                    .flex()
                    .flex_col()
                    .w(px(tree_width))
                    .flex_shrink_0()
                    .h_full()
                    .border_r_1()
                    .border_color(colors::border())
                    .child(toggle_btn)
                    .child(
                        div()
                            .id("pr-tree-scroll")
                            .flex_1()
                            .overflow_y_scroll()
                            .child(self.render_file_tree(cx)),
                    );

                body = body.child(tree_panel);

                // Resize handle
                body = body.child(
                    div()
                        .id("pr-tree-resize-handle")
                        .w(px(8.0))
                        .mx(px(-3.0))
                        .h_full()
                        .flex_shrink_0()
                        .cursor_col_resize()
                        .on_mouse_down(
                            MouseButton::Left,
                            move |event: &MouseDownEvent, _window, cx| {
                                entity_down.update(cx, |panel, cx| {
                                    panel.resizing_tree = true;
                                    panel.tree_drag_start_x = f32::from(event.position.x);
                                    panel.tree_drag_start_width = panel.tree_width;
                                    cx.notify();
                                });
                                cx.stop_propagation();
                            },
                        ),
                );
            }
        }

        // Apply any pending scroll-to-file BEFORE computing the sticky
        // overlay state — otherwise sticky is decided against the
        // pre-scroll position and the just-collapsed file's header
        // briefly flashes as the sticky until the next render.
        if let Some(target_file) = self.scroll_to_file.take() {
            if let Some(&row_idx) = self.flat_file_starts.get(target_file) {
                self.list_state.scroll_to(ListOffset {
                    item_ix: row_idx,
                    offset_in_item: px(0.),
                });
            }
        }

        // Compute sticky overlay state once. Stashing the file index on
        // `self` lets the row renderer hide the inline header for that
        // file so the sticky and inline don't double-paint.
        let sticky = self.sticky_file_header();
        self.sticky_file_idx = sticky.map(|(idx, _)| idx);

        // Diff content — virtualized with list (supports variable row heights for comments)
        let entity_list = cx.entity().clone();

        let diff_list = list(
            self.list_state.clone(),
            move |ix, _window, cx| {
                let panel = entity_list.read(cx);
                panel.render_flat_row(ix, &entity_list)
            },
        )
        .flex_1();

        // Detect ListState scroll changes between renders so the scrollbar
        // auto-shows when the user scrolls with wheel/trackpad.
        let cur_list_offset = f32::from(-self.list_state.scroll_px_offset_for_scrollbar().y);
        if (cur_list_offset - self.last_scroll_offset).abs() > 0.5 {
            self.last_scroll_offset = cur_list_offset;
            self.bump_scrollbar(cx);
        }

        // Don't use `list_state.max_offset_for_scrollbar()` — it only
        // sums heights of measured items, which for long diffs grows as
        // the user scrolls and makes the thumb shrink mid-scroll.
        let scroll_content_height = px(self.estimated_content_height_px());
        let scroll_offset_px = px(self.estimated_scroll_offset_px());
        self.scrollbar.content_height = scroll_content_height;
        let viewport_height = self.scrollbar.track_height;
        let scroll_visible = self.scrollbar.visible;
        let scroll_dragging = self.scrollbar.drag_cursor_within_thumb.is_some();

        let entity_for_bounds = cx.entity().clone();
        let entity_for_thumb = cx.entity().clone();
        let bounds_sink: Rc<dyn Fn(Bounds<Pixels>, &mut App)> =
            Rc::new(move |bounds, cx| {
                entity_for_bounds.update(cx, |panel, _cx| {
                    panel.scrollbar.track_origin_y = bounds.origin.y;
                    panel.scrollbar.track_height = bounds.size.height;
                });
            });
        let on_thumb_down: Rc<dyn Fn(Pixels, Pixels, &mut Window, &mut App)> =
            Rc::new(move |cursor_y, thumb_top, _window, cx| {
                entity_for_thumb.update(cx, |panel, cx| {
                    panel.list_state.scrollbar_drag_started();
                    scrollbar::start_drag(&mut panel.scrollbar, cursor_y, thumb_top);
                    panel.bump_scrollbar(cx);
                    cx.notify();
                });
            });

        let mut diff_content = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .overflow_x_hidden()
            .px(px(16.0))
            .child(diff_list);

        // Sticky file-header overlay: pinned to the top of the diff
        // viewport, slid up by `push_offset` as the next file's inline
        // header approaches. The matching inline row is rendered as an
        // invisible placeholder of the same height (see `render_flat_row`).
        if let Some((sticky_idx, push_offset)) = sticky {
            let entity_sticky = cx.entity().clone();
            diff_content = diff_content.child(
                div()
                    .absolute()
                    .top(px(-push_offset))
                    .left(px(16.0))
                    .right(px(16.0))
                    // Opaque backstop so scrolling diff lines behind the
                    // overlay can never show through during the handover.
                    .bg(colors::surface())
                    .child(self.render_file_header(sticky_idx, &entity_sticky, false)),
            );
        }

        if let Some(bar) = scrollbar::render_vertical(
            "pr-diff-scrollbar",
            scrollbar::Geometry {
                scroll_offset: scroll_offset_px,
                content_height: scroll_content_height,
                viewport_height,
            },
            scroll_visible,
            scroll_dragging,
            bounds_sink,
            on_thumb_down,
        ) {
            diff_content = diff_content.child(bar);
        }

        body = body.child(diff_content);
        panel = panel.child(body);

        panel
    }
}
