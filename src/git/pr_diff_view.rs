use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use super::diff_model::*;
use super::git_repo::GitRepo;
use super::github::{self, GhStatus, PrInfo, PrReviewComment};
use super::markdown;
use crate::text_input::TextInput;
use crate::theme::colors;
use crate::util;

/// How many context lines around each change to show by default.
const DEFAULT_CONTEXT: usize = 3;
/// How many extra context lines to reveal per click.
const EXPAND_STEP: usize = 20;
/// How many files to render initially before showing "Load more".
const FILE_RENDER_BATCH: usize = 20;
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
    pub width: f32,
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
    /// Max number of files to render (grows when user clicks "Load more").
    rendered_file_limit: usize,

    // Line selection for copy
    /// Whether user is currently dragging to select lines.
    copy_selecting: bool,
    /// Global line index where selection started.
    copy_anchor_line: Option<usize>,
    /// Global line index where selection currently ends.
    copy_end_line: Option<usize>,
    /// Global line index → text content for copy (rebuilt with flat cache).
    copy_line_contents: Vec<String>,

    // Virtual scroll cache
    /// Pre-flattened line segments per file (indexed by file_idx).
    cached_file_segments: Vec<Vec<LineSegment>>,
    /// Flat row descriptors for the uniform_list.
    flat_rows: Vec<FlatRow>,
    /// file_idx → index of its FileHeader row in flat_rows.
    flat_file_starts: Vec<usize>,
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
}

impl PrDiffPanel {
    pub fn empty() -> Self {
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
            width: 360.0,
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
            rendered_file_limit: FILE_RENDER_BATCH,
            copy_selecting: false,
            copy_anchor_line: None,
            copy_end_line: None,
            copy_line_contents: Vec::new(),
            cached_file_segments: Vec::new(),
            flat_rows: Vec::new(),
            flat_file_starts: Vec::new(),
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
        }
    }

    pub fn new(work_dir: PathBuf) -> Self {
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
            width: 360.0,
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
            rendered_file_limit: FILE_RENDER_BATCH,
            copy_selecting: false,
            copy_anchor_line: None,
            copy_end_line: None,
            copy_line_contents: Vec::new(),
            cached_file_segments: Vec::new(),
            flat_rows: Vec::new(),
            flat_file_starts: Vec::new(),
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
                        panel.rendered_file_limit = FILE_RENDER_BATCH;
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
                    panel.rendered_file_limit = FILE_RENDER_BATCH;
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
        let render_limit = self.rendered_file_limit.min(self.diff_files.len());
        let mut all_segments: Vec<Vec<LineSegment>> = Vec::with_capacity(render_limit);
        for file_idx in 0..render_limit {
            if self.collapsed_files.contains(&file_idx) || self.diff_files[file_idx].hunks.is_empty()
            {
                all_segments.push(Vec::new());
            } else {
                let segs = self.flatten_file_lines(&self.diff_files[file_idx], file_idx);
                all_segments.push(segs);
            }
        }

        // Phase 2: build flat rows
        self.flat_rows.clear();
        self.flat_file_starts.clear();
        let mut copy_texts: Vec<String> = Vec::new();
        let mut global_line_idx = 0usize;
        let split_mode = self.view_mode == DiffViewMode::Split;

        for file_idx in 0..render_limit {
            self.flat_file_starts.push(self.flat_rows.len());
            self.flat_rows.push(FlatRow::FileHeader { file_idx });

            if self.collapsed_files.contains(&file_idx) {
                continue;
            }

            if self.diff_files[file_idx].hunks.is_empty() {
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

        let remaining = self.diff_files.len().saturating_sub(render_limit);
        if remaining > 0 {
            self.flat_rows.push(FlatRow::LoadMore { remaining });
        }

        self.cached_file_segments = all_segments;
        self.copy_line_contents = copy_texts;
        let scroll_pos = self.list_state.logical_scroll_top();
        let old_count = self.list_state.item_count();
        self.list_state.splice(0..old_count, self.flat_rows.len());
        self.list_state.scroll_to(scroll_pos);
        self.flat_cache_dirty = false;
    }

    /// Render a single flat row. Called from the list callback.
    fn render_flat_row(&self, row_idx: usize, entity: &Entity<Self>) -> AnyElement {
        match &self.flat_rows[row_idx] {
            FlatRow::FileHeader { file_idx } => {
                self.render_file_header(*file_idx, entity)
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
                self.render_split_line_row(left_line, right_line, *global_line_idx, *is_last_in_file, entity)
            }
            FlatRow::LoadMore { remaining } => {
                div()
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
                    .into_any_element()
            }
        }
    }

    fn render_file_header(&self, file_idx: usize, entity: &Entity<Self>) -> AnyElement {
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

        let top_pad = if file_idx > 0 { 12.0 } else { 16.0 };
        div()
            .pt(px(top_pad))
            .child(header)
            .into_any_element()
    }

    fn render_empty_file(&self, file_idx: usize) -> AnyElement {
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
            .child("This file has no content")
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

        // Syntax highlighted content
        let content_el: AnyElement = if let Some(highlights) = line.highlights.as_ref() {
            if highlights.is_empty() {
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .bg(text_bg)
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
                    .bg(text_bg)
                    .text_color(text_col)
                    .pl(px(4.0))
                    .child(styled)
                    .into_any_element()
            }
        } else {
            div()
                .flex_1()
                .min_w(px(0.0))
                .bg(text_bg)
                .text_color(text_col)
                .pl(px(4.0))
                .child(content.to_string())
                .into_any_element()
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
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    let path = path_down.clone();
                    entity_down.update(cx, |panel, cx| {
                        panel.comment_drag_start = Some((path, ln, side));
                        panel.comment_drag_end = None;
                        panel.comment_drag_start_gli = Some(gli);
                        panel.comment_drag_end_gli = Some(gli);
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
                    let mut changed = false;
                    if panel.copy_selecting && panel.copy_end_line != Some(gli) {
                        panel.copy_end_line = Some(gli);
                        changed = true;
                    }
                    // Track comment drag across the whole row, not just the "+" button
                    if panel.comment_drag_start.is_some() {
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

    /// Render one side of a split diff row (gutter + prefix + content).
    /// `is_left` determines which line number to show for context lines.
    fn render_split_half(
        line: Option<&DiffLine>,
        is_left: bool,
    ) -> Div {
        let (row_bg, gutter_bg_color, text_col, prefix, line_num_str, content) = match line {
            Some(l) => {
                let (bg, gbg, tc, pfx) = match l.kind {
                    DiffLineKind::Added => (added_line_bg(), added_gutter_bg(), colors::diff_added(), "+"),
                    DiffLineKind::Removed => (removed_line_bg(), removed_gutter_bg(), colors::diff_removed(), "-"),
                    DiffLineKind::Context => (rgba(0x00000000), gutter_bg(), colors::text_muted(), " "),
                };
                // Left side shows old line numbers, right side shows new
                let ln = if is_left {
                    l.old_lineno
                } else {
                    l.new_lineno
                };
                let ln_str = ln.map(|n| format!("{n}")).unwrap_or_default();
                (bg, gbg, tc, pfx, ln_str, Some(l))
            }
            None => {
                (rgba(0x00000000), gutter_bg(), colors::text_muted(), " ", String::new(), None)
            }
        };

        let content_el: AnyElement = if let Some(l) = content {
            let trimmed = l.content.trim_end();
            if let Some(highlights) = l.highlights.as_ref() {
                if highlights.is_empty() {
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_color(text_col)
                        .pl(px(4.0))
                        .child(trimmed.to_string())
                        .into_any_element()
                } else {
                    let hl: Vec<(std::ops::Range<usize>, HighlightStyle)> = highlights
                        .iter()
                        .filter(|s| {
                            s.byte_range.end <= trimmed.len()
                                && trimmed.is_char_boundary(s.byte_range.start)
                                && trimmed.is_char_boundary(s.byte_range.end)
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
                    .text_color(text_col)
                    .pl(px(4.0))
                    .child(trimmed.to_string())
                    .into_any_element()
            }
        } else {
            div()
                .flex_1()
                .min_w(px(0.0))
                .into_any_element()
        };

        div()
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
    }

    /// Render a side-by-side diff row with left (old) and right (new) halves.
    fn render_split_line_row(
        &self,
        left: Option<&DiffLine>,
        right: Option<&DiffLine>,
        global_line_idx: usize,
        is_last_in_file: bool,
        entity: &Entity<Self>,
    ) -> AnyElement {
        let left_half = Self::render_split_half(left, true);
        let right_half = Self::render_split_half(right, false);

        let line_selected = self.is_line_selected(global_line_idx);
        let sel_bg = if line_selected { rgba(0x89b4fa30) } else { rgba(0x00000000) };

        let entity_sel_down = entity.clone();
        let entity_sel_move = entity.clone();
        let gli = global_line_idx;

        div()
            .flex()
            .flex_row()
            .w_full()
            .min_h(px(20.0))
            .bg(sel_bg)
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
            .child(left_half)
            .child(
                div()
                    .w(px(1.0))
                    .h_full()
                    .flex_shrink_0()
                    .bg(colors::border()),
            )
            .child(right_half)
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
        format!("{header}\n@{author}: {body}")
    }

    /// Build a prompt for the full thread (main comment + all replies).
    fn build_thread_prompt(&self, comment: &PrReviewComment, replies: &[&PrReviewComment]) -> String {
        let header = self.build_prompt_header(comment);
        let mut thread = format!("@{}: {}", comment.user.login, comment.body);
        for reply in replies {
            thread.push_str(&format!("\n  @{}: {}", reply.user.login, reply.body));
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

impl Render for PrDiffPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Trigger async gh check + PR detection on first render
        if self.needs_initial_refresh {
            self.needs_initial_refresh = false;
            self.refresh(cx);
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
            .on_mouse_move(move |event: &MouseMoveEvent, _window, cx| {
                entity_move.update(cx, |panel, cx| {
                    if panel.resizing_tree {
                        let delta = f32::from(event.position.x) - panel.tree_drag_start_x;
                        let new_w = (panel.tree_drag_start_width + delta)
                            .clamp(MIN_TREE_WIDTH, MAX_TREE_WIDTH);
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
                        // Finalize any in-progress comment drag (mouse-up may have
                        // landed on a non-line row like a file header or collapse bar).
                        if let Some((path, start_ln, drag_side)) = panel.comment_drag_start.take() {
                            let end_ln = panel.comment_drag_end.unwrap_or(start_ln);
                            let start = start_ln.min(end_ln);
                            let end = start_ln.max(end_ln);
                            panel.comment_drag_end = None;
                            panel.comment_drag_start_gli = None;
                            panel.comment_drag_end_gli = None;
                            panel.start_comment(path, start, end, drag_side, cx);
                            changed = true;
                        }
                        // End text selection and copy
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

        let diff_content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .overflow_x_hidden()
            .px(px(16.0))
            .child(diff_list);

        // Auto-load more files if there are un-rendered files
        let render_limit = self.rendered_file_limit.min(self.diff_files.len());
        let remaining = self.diff_files.len().saturating_sub(render_limit);
        if remaining > 0 {
            let entity_more = cx.entity().clone();
            cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let _ = cx.update(|cx| {
                    let _ = entity_more.update(cx, |panel, cx| {
                        let total = panel.diff_files.len();
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
                self.list_state.scroll_to(ListOffset {
                    item_ix: row_idx,
                    offset_in_item: px(0.),
                });
            }
        }

        body = body.child(diff_content);
        panel = panel.child(body);

        panel
    }
}
