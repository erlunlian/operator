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
    /// Index from (path, line_number) → indices into pr_comments.
    comment_index: HashMap<(String, u32), Vec<usize>>,
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

    // UI state
    collapsed_files: HashSet<usize>,
    collapsed_dirs: HashSet<String>,
    expanded_context: HashMap<(usize, usize), (usize, usize)>,
    pub width: f32,
    tree_width: f32,
    resizing_tree: bool,
    tree_drag_start_x: f32,
    tree_drag_start_width: f32,
    scroll_handle: ScrollHandle,
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
    /// Rebuilt each render: global line index → text content for copy.
    copy_line_contents: Vec<String>,

    // Reply to existing comment thread
    /// The comment ID we're replying to, and the index in pr_comments.
    reply_to: Option<(u64, usize)>,
    reply_input: Option<Entity<TextInput>>,
    submitting_reply: bool,

    // "Copy as prompt" feedback — key identifies which button was clicked
    copied_prompt_key: Option<String>,
    _copied_comment_timer: Option<Task<()>>,
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
            collapsed_files: HashSet::new(),
            collapsed_dirs: HashSet::new(),
            expanded_context: HashMap::new(),
            width: 360.0,
            tree_width: 200.0,
            resizing_tree: false,
            tree_drag_start_x: 0.0,
            tree_drag_start_width: 0.0,
            scroll_handle: ScrollHandle::new(),
            scroll_to_file: None,
            copied_file_key: None,
            _copied_timer: None,
            needs_initial_refresh: false,
            rendered_file_limit: FILE_RENDER_BATCH,
            copy_selecting: false,
            copy_anchor_line: None,
            copy_end_line: None,
            copy_line_contents: Vec::new(),
            reply_to: None,
            reply_input: None,
            submitting_reply: false,
            copied_prompt_key: None,
            _copied_comment_timer: None,
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
            collapsed_files: HashSet::new(),
            collapsed_dirs: HashSet::new(),
            expanded_context: HashMap::new(),
            width: 360.0,
            tree_width: 200.0,
            resizing_tree: false,
            tree_drag_start_x: 0.0,
            tree_drag_start_width: 0.0,
            scroll_handle: ScrollHandle::new(),
            scroll_to_file: None,
            copied_file_key: None,
            _copied_timer: None,
            needs_initial_refresh: true,
            rendered_file_limit: FILE_RENDER_BATCH,
            copy_selecting: false,
            copy_anchor_line: None,
            copy_end_line: None,
            copy_line_contents: Vec::new(),
            reply_to: None,
            reply_input: None,
            submitting_reply: false,
            copied_prompt_key: None,
            _copied_comment_timer: None,
        }
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
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        // Synchronous: recompute branch diff
        if let Some(repo) = &self.repo {
            self.branch = repo.current_branch();
            self.diff_files = repo.branch_diff(&self.base_ref);
            self.expanded_context.clear();
            self.rendered_file_limit = FILE_RENDER_BATCH;
        }

        // Async: check gh and fetch PR data
        let Some(work_dir) = self.work_dir.clone() else {
            return;
        };

        // If base_ref changed via PR info, recompute diff on the new base
        let current_base = self.base_ref.clone();

        self.loading = true;
        cx.notify();

        cx.spawn(async move |entity, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let gh_status = github::check_gh();
                    let (pr_info, comments) = if gh_status == GhStatus::Available {
                        let pr = github::detect_pr(&work_dir);
                        let comments = pr
                            .as_ref()
                            .map(|p| github::fetch_pr_comments(&work_dir, p.number))
                            .unwrap_or_default();
                        (pr, comments)
                    } else {
                        (None, Vec::new())
                    };
                    (gh_status, pr_info, comments)
                })
                .await;

            let _ = cx.update(|cx| {
                let _ = entity.update(cx, |panel, cx| {
                    let (gh_status, pr_info, comments) = result;
                    panel.gh_status = gh_status;

                    // If PR detected and base differs, recompute diff
                    if let Some(ref pr) = pr_info {
                        if pr.base_ref_name != current_base {
                            panel.base_ref = pr.base_ref_name.clone();
                            if let Some(repo) = &panel.repo {
                                panel.diff_files = repo.branch_diff(&panel.base_ref);
                            }
                        }
                    }

                    panel.pr_info = pr_info;
                    panel.pr_comments = comments;
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
                self.comment_index
                    .entry((comment.path.clone(), line))
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
                                panel
                                    .comment_index
                                    .entry((comment.path.clone(), line))
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
                                    .gap_1()
                                    .flex_shrink_0()
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

    // ── File diff section ──

    fn render_file_diff(
        &self,
        file: &DiffFile,
        file_idx: usize,
        line_counter: &std::cell::Cell<usize>,
        line_texts: &std::cell::RefCell<Vec<String>>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let adds = file.additions();
        let dels = file.deletions();
        let is_collapsed = self.collapsed_files.contains(&file_idx);
        let entity_hdr = cx.entity().clone();
        let file_path = file.path.clone();

        let mut container = div()
            .id(ElementId::Name(format!("pr-fdiff-{file_idx}").into()))
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

        // File header — assign a global line index for copy selection
        let hdr_gli = line_counter.get();
        line_counter.set(hdr_gli + 1);
        {
            let mut hdr_text = file.path.clone();
            if adds > 0 { hdr_text.push_str(&format!(" +{adds}")); }
            if dels > 0 { hdr_text.push_str(&format!(" -{dels}")); }
            line_texts.borrow_mut().push(hdr_text);
        }
        let hdr_selected = self.is_line_selected(hdr_gli);
        let hdr_bg = if hdr_selected { rgba(0x89b4fa30) } else { file_header_bg() };
        let entity_sel_down_hdr = cx.entity().clone();
        let entity_sel_move_hdr = cx.entity().clone();

        container = container.child(
            div()
                .id(ElementId::Name(format!("pr-fhdr-{file_idx}").into()))
                .flex()
                .flex_row()
                .items_center()
                .w_full()
                .min_h(px(32.0))
                .px_3()
                .py(px(4.0))
                .bg(hdr_bg)
                .border_1()
                .border_color(colors::border())
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    entity_sel_down_hdr.update(cx, |panel, cx| {
                        panel.copy_anchor_line = Some(hdr_gli);
                        panel.copy_end_line = Some(hdr_gli);
                        panel.copy_selecting = true;
                        cx.notify();
                    });
                })
                .on_mouse_move(move |_, _window, cx| {
                    entity_sel_move_hdr.update(cx, |panel, cx| {
                        if panel.copy_selecting && panel.copy_end_line != Some(hdr_gli) {
                            panel.copy_end_line = Some(hdr_gli);
                            cx.notify();
                        }
                    });
                })
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
                    let path_for_copy = file.path.clone();
                    let just_copied = self.copied_file_key == Some(file_idx);
                    let entity_copy = cx.entity().clone();
                    let (icon, icon_color) = if just_copied {
                        ("\u{2713}", colors::diff_added())
                    } else {
                        ("\u{2750}", colors::text_muted())
                    };
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
                        .child(div().text_color(icon_color).child(icon))
                })
                .child(div().flex_1())
                // Stats
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
                ),
        );

        // Diff body
        if !is_collapsed {
            if file.hunks.is_empty() {
                container = container.child(
                    div()
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
                        .child("This file has no content"),
                );
            } else {
                let all_lines = self.flatten_file_lines(file, file_idx);
                container = container.child(self.render_line_blocks(
                    &all_lines,
                    file_idx,
                    &file.path,
                    line_counter,
                    line_texts,
                    cx,
                ));
            }
        }

        container
    }

    /// Flatten a file's hunks into renderable segments.
    fn flatten_file_lines(&self, file: &DiffFile, file_idx: usize) -> Vec<LineSegment> {
        let mut segments: Vec<LineSegment> = Vec::new();

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

            let change_indices: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| !matches!(l.kind, DiffLineKind::Context))
                .map(|(i, _)| i)
                .collect();

            if change_indices.is_empty() {
                segments.push(LineSegment::CollapsedContext {
                    count: lines.len(),
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
        file_path: &str,
        line_counter: &std::cell::Cell<usize>,
        line_texts: &std::cell::RefCell<Vec<String>>,
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
                    let line_num = match line.kind {
                        DiffLineKind::Removed => line.old_lineno,
                        _ => line.new_lineno.or(line.old_lineno),
                    };
                    let side = match line.kind {
                        DiffLineKind::Removed => CommentSide::Left,
                        _ => CommentSide::Right,
                    };

                    let gli = line_counter.get();
                    line_counter.set(gli + 1);
                    line_texts
                        .borrow_mut()
                        .push(line.content.trim_end().to_string());

                    block = block
                        .child(self.render_diff_line(line, file_path, line_num, side, gli, cx));

                    // Render inline comment form after the last line of the range
                    if let Some((ref active_path, _start, end, active_side)) =
                        self.active_comment_line
                    {
                        if active_path == file_path && line_num == Some(end) && side == active_side
                        {
                            block = block.child(self.render_comment_form(cx));
                        }
                    }

                    // Render existing comments for this line
                    if let Some(line_num) = line_num {
                        if let Some(indices) =
                            self.comment_index.get(&(file_path.to_string(), line_num))
                        {
                            for &idx in indices {
                                block = block.child(self.render_comment_bubble(idx, line_counter, line_texts, cx));
                            }
                        }
                    }
                }
                LineSegment::CollapsedContext {
                    count,
                    hunk_idx,
                    direction,
                } => {
                    let entity = cx.entity().clone();
                    let hunk_idx = *hunk_idx;
                    let dir = *direction;
                    let label = format!("{count} unmodified lines");

                    block = block.child(
                        div()
                            .id(ElementId::Name(
                                format!("pr-collapse-{file_idx}-{seg_idx}").into(),
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
                                        .entry((file_idx, hunk_idx))
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

    fn render_diff_line(
        &self,
        line: &DiffLine,
        file_path: &str,
        line_num: Option<u32>,
        side: CommentSide,
        global_line_idx: usize,
        cx: &mut Context<Self>,
    ) -> Div {
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

        // The "+" comment button on the left gutter — supports drag for multi-line
        let can_comment = self.can_comment();
        let has_line_num = line_num.is_some();
        let comment_btn: AnyElement = if can_comment && has_line_num {
            let ln = line_num.unwrap();
            let entity_down = cx.entity().clone();
            let entity_up = cx.entity().clone();
            let entity_move = cx.entity().clone();
            let path_down = file_path.to_string();
            let path_up = file_path.to_string();

            // Check if this line is within a drag selection or active comment range
            let in_drag = self.comment_drag_start.as_ref().map_or(false, |(p, start, _)| {
                if p != file_path { return false; }
                let end = self.comment_drag_end.unwrap_or(*start);
                let lo = (*start).min(end);
                let hi = (*start).max(end);
                ln >= lo && ln <= hi
            });
            let in_active = self.active_comment_line.as_ref().map_or(false, |(p, start, end, _)| {
                if p != file_path { return false; }
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
                        cx.notify();
                    });
                    cx.stop_propagation();
                })
                .on_mouse_up(MouseButton::Left, move |_, _window, cx| {
                    let path = path_up.clone();
                    entity_up.update(cx, |panel, cx| {
                        if let Some((ref drag_path, start_ln, drag_side)) = panel.comment_drag_start.take() {
                            if drag_path == &path {
                                let start = start_ln.min(ln);
                                let end = start_ln.max(ln);
                                panel.comment_drag_end = None;
                                panel.start_comment(path, start, end, drag_side, cx);
                            }
                        }
                    });
                    cx.stop_propagation();
                })
                .on_mouse_move(move |_, _window, cx| {
                    entity_move.update(cx, |panel, cx| {
                        if panel.comment_drag_start.is_some() {
                            if panel.comment_drag_end != Some(ln) {
                                panel.comment_drag_end = Some(ln);
                                cx.notify();
                            }
                        }
                    });
                })
                .child("+")
                .into_any_element()
        } else {
            div().w(px(18.0)).flex_shrink_0().bg(gutter_bg_color).into_any_element()
        };

        // Check if this line is within an active drag selection (for row highlight)
        let in_drag = self.comment_drag_start.as_ref().map_or(false, |(p, start, _)| {
            if p != file_path { return false; }
            if let Some(ln) = line_num {
                let end = self.comment_drag_end.unwrap_or(*start);
                let lo = (*start).min(end);
                let hi = (*start).max(end);
                ln >= lo && ln <= hi
            } else {
                false
            }
        });

        // Also highlight lines within the active comment range (after drag completes)
        let in_active_comment = self.active_comment_line.as_ref().map_or(false, |(p, start, end, _)| {
            if p != file_path { return false; }
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
        let entity_sel_down = cx.entity().clone();
        let entity_sel_move = cx.entity().clone();
        let entity_row_up = cx.entity().clone();
        let row_path = file_path.to_string();
        let gli = global_line_idx;

        div()
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
                    if let Some(ln) = line_num {
                        if panel.comment_drag_start.is_some() && panel.comment_drag_end != Some(ln) {
                            panel.comment_drag_end = Some(ln);
                            changed = true;
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
                                    panel.start_comment(path.clone(), start, end, drag_side, cx);
                                }
                            }
                        });
                    }
                }
            })
            // Line number on the far left
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
            // Comment "+" button next to line number
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
            .child(content_el)
    }

    fn render_comment_form(&self, cx: &mut Context<Self>) -> Div {
        let entity_cancel = cx.entity().clone();
        let entity_submit = cx.entity().clone();

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

    /// Render a small copy-as-prompt button with custom label.
    fn render_copy_prompt_btn(&self, key: String, label: &str, prompt_text: String, cx: &mut Context<Self>) -> Stateful<Div> {
        let is_copied = self.copied_prompt_key.as_deref() == Some(&key);
        let entity = cx.entity().clone();
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
        line_counter: &std::cell::Cell<usize>,
        line_texts: &std::cell::RefCell<Vec<String>>,
        cx: &mut Context<Self>,
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

        // Assign a global line index for the whole comment thread
        let comment_gli = line_counter.get();
        line_counter.set(comment_gli + 1);
        // Build copy text: author + body, then replies
        let mut copy_text = format!("@{author}: {body}");
        for reply in &replies {
            copy_text.push_str(&format!("\n  @{}: {}", reply.user.login, reply.body));
        }
        line_texts.borrow_mut().push(copy_text);

        let comment_selected = self.is_line_selected(comment_gli);
        let bg = if comment_selected { rgba(0x89b4fa30) } else { comment_bg() };
        let entity_sel_down = cx.entity().clone();
        let entity_sel_move = cx.entity().clone();

        let mut bubble = div()
            .w_full()
            .px_3()
            .py_2()
            .bg(bg)
            .border_t_1()
            .border_b_1()
            .border_color(comment_border())
            .flex()
            .flex_col()
            .gap_1()
            .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                entity_sel_down.update(cx, |panel, cx| {
                    panel.copy_anchor_line = Some(comment_gli);
                    panel.copy_end_line = Some(comment_gli);
                    panel.copy_selecting = true;
                    cx.notify();
                });
            })
            .on_mouse_move(move |_, _window, cx| {
                entity_sel_move.update(cx, |panel, cx| {
                    if panel.copy_selecting && panel.copy_end_line != Some(comment_gli) {
                        panel.copy_end_line = Some(comment_gli);
                        cx.notify();
                    }
                });
            });

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
            cx,
        );
        bubble = bubble.child(self.render_single_comment(author, body, Some(main_btn), format!("pr-comment-{comment_idx}").into()));

        // Replies, each with hover copy button inline with author
        for (i, reply) in replies.iter().enumerate() {
            let reply_prompt = self.build_single_comment_prompt(comment, &reply.body, &reply.user.login);
            let reply_btn = self.render_copy_prompt_btn(
                format!("reply-{comment_idx}-{i}"),
                "Copy as prompt",
                reply_prompt,
                cx,
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
                let entity_cancel = cx.entity().clone();
                let entity_submit = cx.entity().clone();
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
        let entity_reply = cx.entity().clone();
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

        if has_replies {
            let thread_key = format!("thread-{comment_idx}");
            let is_thread_copied = self.copied_prompt_key.as_deref() == Some(&thread_key);
            let entity_thread = cx.entity().clone();
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
                    .when_some(copy_btn, |row, btn| {
                        row.child(
                            btn.opacity(0.0)
                                .group_hover(gn, |s| s.opacity(1.0))
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

        let total_files = self.diff_files.len();
        let total_adds = self.total_additions();
        let total_dels = self.total_deletions();

        let entity_move = cx.entity().clone();
        let entity_up = cx.entity().clone();

        let mut panel = div()
            .id("pr-diff-panel")
            .flex()
            .flex_col()
            .w_full()
            .flex_1()
            .h_full()
            .bg(colors::surface())
            .on_mouse_move(move |event: &MouseMoveEvent, _window, cx| {
                entity_move.update(cx, |panel, cx| {
                    if panel.resizing_tree {
                        let delta = f32::from(event.position.x) - panel.tree_drag_start_x;
                        let new_w = (panel.tree_drag_start_width + delta)
                            .clamp(80.0, panel.width - 100.0);
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
                        // Finalize comment drag if active
                        if let Some((path, start_ln, drag_side)) = panel.comment_drag_start.take() {
                            let end_ln = panel.comment_drag_end.take().unwrap_or(start_ln);
                            let start = start_ln.min(end_ln);
                            let end = start_ln.max(end_ln);
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
                format!(
                    "{} \u{2192} {} \u{00B7} {} files",
                    self.branch, self.base_ref, total_files
                )
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

        if total_adds > 0 || total_dels > 0 {
            header = header.child(
                div()
                    .ml_2()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .flex_shrink_0()
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
        let entity_down = cx.entity().clone();

        let mut body = div()
            .id("pr-diff-panel-body")
            .flex()
            .flex_row()
            .flex_1()
            .overflow_hidden();

        // File tree sidebar
        let tree_panel = div()
            .id("pr-diff-file-tree")
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
                    },
                ),
        );

        // Diff content (scrollable) — paginated for large diffs
        let files = self.diff_files.clone();
        let render_limit = self.rendered_file_limit.min(files.len());
        let line_counter = std::cell::Cell::new(0usize);
        let line_texts = std::cell::RefCell::new(Vec::<String>::new());

        let mut diff_content = div()
            .id("pr-diff-panel-content")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(100.0))
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .p_2();

        for (idx, file) in files.iter().enumerate().take(render_limit) {
            diff_content = diff_content.child(
                self.render_file_diff(file, idx, &line_counter, &line_texts, cx),
            );
        }

        // Store collected line contents for copy support
        self.copy_line_contents = line_texts.into_inner();

        // Auto-load more files: show a sentinel and schedule the next batch
        let remaining = files.len().saturating_sub(render_limit);
        if remaining > 0 {
            diff_content = diff_content.child(
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
                    ),
            );

            // Schedule the next batch load after a brief delay
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
                            cx.notify();
                        }
                    });
                });
            })
            .detach();
        }

        if let Some(target_idx) = self.scroll_to_file.take() {
            self.scroll_handle.scroll_to_item(target_idx);
        }

        body = body.child(diff_content);
        panel = panel.child(body);

        panel
    }
}
