use gpui::*;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point as GridPoint};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::search::{RegexIter, RegexSearch};
use alacritty_terminal::term::{Term, TermMode};
use regex::Regex;
use crate::actions::FindInFile;
use crate::terminal::terminal::{alac_color_to_gpui, JsonListener, TerminalModel};
use crate::theme::colors;

static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"https?://[^\s<>"{}|\\^`\[\]]+"#).unwrap()
});

/// Matches GitHub shorthand like `owner/repo#123` (PRs and issues).
static GITHUB_REF_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[a-zA-Z0-9_.-]+/[a-zA-Z0-9_.-]+#\d+").unwrap()
});

const CELL_HEIGHT_PX: f32 = 16.0;
const PADDING_PX: f32 = 8.0;
const FONT_SIZE: f32 = 13.0;
const FALLBACK_CELL_WIDTH: f32 = 8.0;

/// A (line, col) position in the terminal grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GridPos {
    line: usize,
    col: usize,
}

impl GridPos {
    fn min(self, other: GridPos) -> GridPos {
        if (self.line, self.col) <= (other.line, other.col) { self } else { other }
    }
    fn max(self, other: GridPos) -> GridPos {
        if (self.line, self.col) >= (other.line, other.col) { self } else { other }
    }
}

#[derive(Clone, Debug)]
struct HoveredUrl {
    url: String,
    line: usize,
    start_col: usize,
    end_col: usize, // exclusive
}

fn rgba_to_hsla(c: Rgba) -> Hsla {
    Hsla::from(c)
}

pub struct TerminalView {
    pub terminal: Entity<TerminalModel>,
    focus_handle: FocusHandle,
    last_size: Arc<std::sync::Mutex<Option<(u16, u16)>>>,
    last_bounds: Arc<std::sync::Mutex<Bounds<Pixels>>>,
    selection_start: Option<GridPos>,
    selection_end: Option<GridPos>,
    cell_width: f32,
    has_been_focused: bool,
    /// Accumulated scroll pixels for smooth trackpad scrolling.
    scroll_px: f32,
    /// Set by prepaint when terminal dimensions change; scroll handler resets scroll_px.
    size_changed: Arc<AtomicBool>,
    /// URL currently hovered with Cmd held (for underline + click-to-open).
    hovered_url: Option<HoveredUrl>,
    /// Running auto-scroll task when dragging selection past viewport edges.
    _autoscroll_task: Option<Task<()>>,
    /// Last known mouse Y (window coords) during a selection drag.
    last_drag_mouse_y: Option<f32>,
    /// Cmd+F search state.
    search_active: bool,
    search_query: String,
    search_input: Option<Entity<crate::text_input::TextInput>>,
    /// Matches as (line_start, col_start, line_end, col_end) in grid coords.
    /// `line` is `alacritty_terminal::index::Line(i32)`: negative = scrollback,
    /// `0..screen_lines` = viewport when `display_offset == 0`.
    search_matches: Vec<(i32, usize, i32, usize)>,
    search_match_ix: usize,
}

impl TerminalView {
    pub fn new(terminal: Entity<TerminalModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&terminal, |_this, _term, cx| cx.notify())
            .detach();

        // Measure actual monospace cell width from the font
        let cell_width = {
            let font = Font {
                family: "MesloLGS NF".into(),
                features: FontFeatures::default(),
                fallbacks: None,
                weight: FontWeight::NORMAL,
                style: FontStyle::Normal,
            };
            let text_system = cx.text_system();
            let font_id = text_system.resolve_font(&font);
            text_system.advance(font_id, px(FONT_SIZE), 'M')
                .map(|s| s.width / px(1.0))
                .unwrap_or(FALLBACK_CELL_WIDTH)
        };

        Self {
            terminal,
            focus_handle: cx.focus_handle(),
            last_size: Arc::new(std::sync::Mutex::new(None)),
            last_bounds: Arc::new(std::sync::Mutex::new(Bounds::default())),
            selection_start: None,
            selection_end: None,
            cell_width,
            has_been_focused: false,
            scroll_px: 0.0,
            size_changed: Arc::new(AtomicBool::new(false)),
            hovered_url: None,
            _autoscroll_task: None,
            last_drag_mouse_y: None,
            search_active: false,
            search_query: String::new(),
            search_input: None,
            search_matches: Vec::new(),
            search_match_ix: 0,
        }
    }

    fn mouse_to_grid(&self, pos: Point<Pixels>) -> GridPos {
        let bounds = *self.last_bounds.lock().unwrap();
        let x = (pos.x - bounds.origin.x) / px(1.0) - PADDING_PX;
        let y = (pos.y - bounds.origin.y) / px(1.0) - PADDING_PX;
        let col = (x / self.cell_width).max(0.0) as usize;
        let line = (y / CELL_HEIGHT_PX).max(0.0) as usize;
        GridPos { line, col }
    }

    /// Ensure the auto-scroll timer is running while a selection drag is
    /// active.  The timer re-reads `last_drag_mouse_y` each tick so it
    /// adapts even when `on_mouse_move` stops firing (mouse left the window).
    fn ensure_autoscroll_timer(&mut self, cx: &mut Context<Self>) {
        if self.selection_start.is_none() {
            self._autoscroll_task = None;
            return;
        }
        // Already running — it will pick up the latest last_drag_mouse_y.
        if self._autoscroll_task.is_some() {
            return;
        }

        let entity = cx.entity().clone();
        self._autoscroll_task = Some(cx.spawn(async move |_, cx| {
            const EDGE_ZONE: f32 = 30.0;
            const TICK_MS: u64 = 50;

            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(TICK_MS))
                    .await;
                let Ok(should_continue) = cx.update(|cx| {
                    entity.update(cx, |view, cx| {
                        if view.selection_start.is_none() {
                            view._autoscroll_task = None;
                            return false;
                        }
                        let Some(mouse_y) = view.last_drag_mouse_y else {
                            return true; // keep timer alive, no position yet
                        };

                        let bounds = *view.last_bounds.lock().unwrap();
                        let top = f32::from(bounds.origin.y);
                        let bottom = top + f32::from(bounds.size.height);
                        let visible_rows = ((f32::from(bounds.size.height) - PADDING_PX * 2.0)
                            / CELL_HEIGHT_PX)
                            .floor()
                            .max(1.0) as usize;

                        // In alacritty: Scroll::Delta(+) = UP (history),
                        //               Scroll::Delta(-) = DOWN (recent).
                        let scroll_dir: i32 = if mouse_y < top + EDGE_ZONE {
                            1
                        } else if mouse_y > bottom - EDGE_ZONE {
                            -1
                        } else {
                            return true; // not near edge, keep timer alive but skip scroll
                        };

                        let term_model = view.terminal.read(cx);
                        let is_alt_screen = {
                            let t = term_model.term.lock();
                            t.mode().contains(TermMode::ALT_SCREEN)
                        };
                        if is_alt_screen {
                            return true;
                        }
                        {
                            let mut t = term_model.term.lock();
                            t.scroll_display(Scroll::Delta(scroll_dir));
                        }
                        // Adjust selection_start for the content shift.
                        // scroll_dir > 0 = scroll up = content shifts DOWN.
                        if let Some(ref mut start) = view.selection_start {
                            if scroll_dir > 0 {
                                start.line = start.line.saturating_add(1);
                            } else {
                                start.line = start.line.saturating_sub(1);
                            }
                        }
                        // Extend selection to the viewport edge.
                        if let Some(ref mut end) = view.selection_end {
                            if scroll_dir > 0 {
                                end.line = 0;
                                end.col = 0;
                            } else {
                                end.line = visible_rows.saturating_sub(1);
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

    /// Send a mouse event to the terminal PTY using SGR or legacy encoding.
    /// `button` is the xterm button number: 0=left, 1=middle, 2=right, 32=motion.
    /// `pressed` is true for press/motion, false for release.
    fn send_mouse_event(&self, term_model: &TerminalModel, button: u8, pos: GridPos, pressed: bool) {
        // Terminal coords are 1-based.
        let x = (pos.col + 1) as u32;
        let y = (pos.line + 1) as u32;
        let has_sgr = {
            let t = term_model.term.lock();
            t.mode().contains(TermMode::SGR_MOUSE)
        };
        if has_sgr {
            let c = if pressed { 'M' } else { 'm' };
            term_model.write_str_to_pty(&format!("\x1b[<{button};{x};{y}{c}"));
        } else {
            // Legacy X10/normal encoding: button+32, x+32, y+32 (capped at 223).
            let cb = if pressed { button + 32 } else { 3 + 32 };
            let cx = (x.min(223) as u8) + 32;
            let cy = (y.min(223) as u8) + 32;
            term_model.write_to_pty(&[b'\x1b', b'[', b'M', cb, cx, cy]);
        }
    }

    fn selected_text(&self, term: &Arc<FairMutex<Term<JsonListener>>>) -> String {
        let (Some(start), Some(end)) = (self.selection_start, self.selection_end) else {
            return String::new();
        };
        let lo = start.min(end);
        let hi = start.max(end);
        if lo == hi {
            return String::new();
        }

        let term = term.lock();
        let grid = term.grid();
        let num_lines = grid.screen_lines();
        let num_cols = grid.columns();
        let mut result = String::new();

        for line_idx in lo.line..=hi.line.min(num_lines.saturating_sub(1)) {
            let row = &grid[Line(line_idx as i32)];
            let col_start = if line_idx == lo.line { lo.col } else { 0 };
            let col_end = if line_idx == hi.line { hi.col } else { num_cols.saturating_sub(1) };

            let mut line_text = String::new();
            for col_idx in col_start..=col_end.min(num_cols.saturating_sub(1)) {
                let cell = &row[Column(col_idx)];
                if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                    continue;
                }
                let ch = if cell.c == '\0' { ' ' } else { cell.c };
                line_text.push(ch);
            }
            let trimmed = line_text.trim_end();
            result.push_str(trimmed);
            if line_idx < hi.line {
                result.push('\n');
            }
        }
        result
    }

    /// Convert a regex match span to a HoveredUrl if the cursor is within it.
    fn match_to_hovered(&self, url: &str, start_byte: usize, end_byte: usize, pos: GridPos, byte_to_col: &[usize]) -> Option<HoveredUrl> {
        let start_col = byte_to_col.get(start_byte).copied()?;
        let end_col = if end_byte > 0 {
            byte_to_col.get(end_byte - 1).map(|c| c + 1)?
        } else {
            return None;
        };
        if pos.col >= start_col && pos.col < end_col {
            Some(HoveredUrl {
                url: url.to_string(),
                line: pos.line,
                start_col,
                end_col,
            })
        } else {
            None
        }
    }

    /// Detect a URL under the given screen position by regex-matching the terminal line text.
    fn detect_url_at(&self, pos: GridPos, term: &Arc<FairMutex<Term<JsonListener>>>) -> Option<HoveredUrl> {
        let term_lock = term.lock();
        let grid = term_lock.grid();
        let display_offset = grid.display_offset();
        let num_cols = grid.columns();
        let screen_lines = grid.screen_lines();

        if pos.line >= screen_lines {
            return None;
        }

        let grid_line = Line(pos.line as i32 - display_offset as i32);
        let row = &grid[grid_line];

        // Build line text, tracking byte offset → column mapping
        let mut line_text = String::new();
        let mut byte_to_col: Vec<usize> = Vec::new();

        for col_idx in 0..num_cols {
            let cell = &row[Column(col_idx)];
            if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                continue;
            }
            let ch = if cell.c == '\0' { ' ' } else { cell.c };
            let start = line_text.len();
            line_text.push(ch);
            for _ in start..line_text.len() {
                byte_to_col.push(col_idx);
            }
        }

        // Check full URLs (https://...)
        for m in URL_REGEX.find_iter(&line_text) {
            let mut url = m.as_str();
            let mut end_byte = m.end();
            while url.ends_with(|c: char| ".,;:!?)>]".contains(c)) {
                url = &url[..url.len() - 1];
                end_byte -= 1;
            }
            if url.len() < 10 {
                continue;
            }
            if let Some(hit) = self.match_to_hovered(url, m.start(), end_byte, pos, &byte_to_col) {
                return Some(hit);
            }
        }

        // Check GitHub shorthand (owner/repo#123)
        for m in GITHUB_REF_REGEX.find_iter(&line_text) {
            let text = m.as_str();
            if let Some((owner_repo, num)) = text.rsplit_once('#') {
                let url = format!("https://github.com/{owner_repo}/issues/{num}");
                if let Some(hit) = self.match_to_hovered(&url, m.start(), m.end(), pos, &byte_to_col) {
                    return Some(hit);
                }
            }
        }

        None
    }

    // ── Cmd+F search ──

    /// Recompute matches for the current query against the full grid
    /// (scrollback + viewport). Results are stored and reused for highlighting
    /// and `Enter`/`Shift+Enter` navigation.
    const MAX_SEARCH_MATCHES: usize = 10_000;

    fn update_search_matches(&mut self, cx: &mut Context<Self>) {
        self.search_matches.clear();
        if self.search_query.is_empty() {
            self.search_match_ix = 0;
            return;
        }
        // Alacritty's search engine expects a regex; escape so we do literal
        // (substring) matching. Case-insensitivity is handled inside
        // `RegexSearch::new` when the query has no uppercase chars.
        let escaped = regex::escape(&self.search_query);
        let Ok(mut regex) = RegexSearch::new(&escaped) else {
            self.search_match_ix = 0;
            return;
        };

        let term_model = self.terminal.read(cx);
        let term = term_model.term.lock();
        let grid = term.grid();
        let start = GridPoint::new(grid.topmost_line(), Column(0));
        let end = GridPoint::new(grid.bottommost_line(), grid.last_column());

        let iter = RegexIter::new(start, end, Direction::Right, &*term, &mut regex);
        for (i, m) in iter.enumerate() {
            if i >= Self::MAX_SEARCH_MATCHES {
                break;
            }
            let s = *m.start();
            let e = *m.end();
            self.search_matches.push((s.line.0, s.column.0, e.line.0, e.column.0));
        }

        if self.search_matches.is_empty() {
            self.search_match_ix = 0;
        } else if self.search_match_ix >= self.search_matches.len() {
            self.search_match_ix = self.search_matches.len() - 1;
        }
    }

    fn jump_to_current_match(&self, cx: &mut Context<Self>) {
        let Some(&(line_start, col_start, _, _)) = self.search_matches.get(self.search_match_ix)
        else {
            return;
        };
        let term_model = self.terminal.read(cx);
        let mut term = term_model.term.lock();
        term.scroll_to_point(GridPoint::new(Line(line_start), Column(col_start)));
    }

    fn step_match(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.search_matches.is_empty() {
            return;
        }
        let n = self.search_matches.len();
        self.search_match_ix = if forward {
            (self.search_match_ix + 1) % n
        } else {
            (self.search_match_ix + n - 1) % n
        };
        self.jump_to_current_match(cx);
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_active = true;
        self.search_query.clear();
        self.search_matches.clear();
        self.search_match_ix = 0;

        let next_entity = cx.entity().clone();
        let cancel_entity = cx.entity().clone();

        let input = cx.new(|cx| {
            let mut inp = crate::text_input::TextInput::new(cx);
            inp.set_placeholder("Search terminal...");

            inp.set_on_submit(Rc::new(move |_text, _window, cx| {
                next_entity.update(cx, |view, cx| {
                    view.step_match(true, cx);
                    cx.notify();
                });
            }));

            inp.set_on_cancel(Rc::new(move |window, cx| {
                cancel_entity.update(cx, |view, cx| {
                    view.close_search(window);
                    cx.notify();
                });
            }));

            inp
        });

        cx.observe(&input, |view, input, cx| {
            let new_text = input.read(cx).text.clone();
            if view.search_query != new_text {
                view.search_query = new_text;
                view.search_match_ix = 0;
                view.update_search_matches(cx);
                view.jump_to_current_match(cx);
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

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.has_been_focused {
            self.has_been_focused = true;
            self.focus_handle.focus(window);
        }

        // Recompute matches before borrowing anything from `cx`, so highlights
        // track live PTY output while the search bar is open.
        if self.search_active {
            self.update_search_matches(cx);
        }

        let term = self.terminal.read(cx).term.clone();

        let terminal_entity = self.terminal.clone();
        let last_size = self.last_size.clone();

        let cell_width = self.cell_width;

        let last_bounds = self.last_bounds.clone();
        let size_changed = self.size_changed.clone();

        let sel_start = self.selection_start;
        let sel_end = self.selection_end;
        let hovered_url = self.hovered_url.clone();
        let show_pointer = self.hovered_url.is_some();

        let search_active = self.search_active;
        let search_match_ix = self.search_match_ix;
        let search_matches: Rc<Vec<(i32, usize, i32, usize)>> =
            Rc::new(self.search_matches.clone());

        let grid_canvas = canvas(
            // Prepaint: resize detection
            {
                let terminal_entity = terminal_entity.clone();
                let last_size = last_size.clone();
                let last_bounds = last_bounds.clone();
                let size_changed = size_changed.clone();
                move |bounds: Bounds<Pixels>, _window: &mut Window, cx: &mut App| {
                    *last_bounds.lock().unwrap() = bounds;

                    let w = f32::from(bounds.size.width);
                    let h = f32::from(bounds.size.height);
                    let cols = ((w - PADDING_PX * 2.0) / cell_width).floor().max(1.0) as u16;
                    let rows = ((h - PADDING_PX * 2.0) / CELL_HEIGHT_PX).floor().max(1.0) as u16;

                    let mut cached = last_size.lock().unwrap();
                    if *cached != Some((rows, cols)) {
                        *cached = Some((rows, cols));
                        size_changed.store(true, Ordering::Release);
                        let term = terminal_entity.read(cx);
                        term.resize(rows, cols);
                    }
                    bounds
                }
            },
            // Paint: render the terminal grid using Alacritty's display_iter
            {
                let term = term.clone();
                let search_matches = search_matches.clone();
                move |bounds: Bounds<Pixels>, _prepaint: Bounds<Pixels>, window: &mut Window, cx: &mut App| {
                    // Selection range (normalized so lo <= hi)
                    let selection = match (sel_start, sel_end) {
                        (Some(s), Some(e)) if s != e => Some((s.min(e), s.max(e))),
                        _ => None,
                    };
                    let sel_bg = colors::accent();
                    let sel_fg = colors::bg();
                    // Search match highlight colors (yellow, matching the terminal
                    // palette's Yellow / BrightBlack for active / inactive).
                    let active_match_bg = rgb(0xf9e2af);
                    let inactive_match_bg = rgb(0x585b70);
                    let match_fg = colors::bg();
                    let term_lock = term.lock();
                    let content = term_lock.renderable_content();
                    let display_offset = content.display_offset;
                    let cursor = content.cursor;
                    let term_colors = content.colors;

                    // Prefilter matches to those whose line range overlaps the viewport —
                    // keeps per-cell highlighting O(visible matches), not O(total matches).
                    let visible_matches: Vec<(i32, usize, i32, usize, bool)> = if search_active {
                        let screen_lines = term_lock.grid().screen_lines() as i32;
                        let top = -(display_offset as i32);
                        let bot = screen_lines - 1 - display_offset as i32;
                        search_matches
                            .iter()
                            .enumerate()
                            .filter(|(_, (ls, _, le, _))| *le >= top && *ls <= bot)
                            .map(|(ix, &(ls, cs, le, ce))| (ls, cs, le, ce, ix == search_match_ix))
                            .collect()
                    } else {
                        Vec::new()
                    };

                    let bg_color = colors::bg();
                    let line_height = px(CELL_HEIGHT_PX);
                    let font_size = px(FONT_SIZE);

                    let font_normal = Font {
                        family: "MesloLGS NF".into(),
                        features: FontFeatures::default(),
                        fallbacks: None,
                        weight: FontWeight::NORMAL,
                        style: FontStyle::Normal,
                    };
                    let font_bold = Font { weight: FontWeight::BOLD, ..font_normal.clone() };
                    let font_italic = Font { style: FontStyle::Italic, ..font_normal.clone() };
                    let font_bold_italic = Font { weight: FontWeight::BOLD, style: FontStyle::Italic, ..font_normal.clone() };

                    // Collect cells from display_iter, grouped by screen row
                    struct RowCell {
                        col: usize,
                        ch: char,
                        fg: Rgba,
                        bg: Rgba,
                        bold: bool,
                        italic: bool,
                        underline: bool,
                        undercurl: bool,
                        is_wide_spacer: bool,
                    }

                    let mut rows: std::collections::BTreeMap<usize, Vec<RowCell>> = std::collections::BTreeMap::new();

                    for indexed in content.display_iter {
                        let screen_row = (indexed.point.line.0 + display_offset as i32) as usize;
                        let col = indexed.point.column.0;
                        let cell = &indexed.cell;

                        rows.entry(screen_row).or_default().push(RowCell {
                            col,
                            ch: cell.c,
                            fg: alac_color_to_gpui(&cell.fg, term_colors),
                            bg: alac_color_to_gpui(&cell.bg, term_colors),
                            bold: cell.flags.contains(CellFlags::BOLD),
                            italic: cell.flags.contains(CellFlags::ITALIC),
                            underline: cell.flags.contains(CellFlags::UNDERLINE)
                                || cell.flags.contains(CellFlags::DOUBLE_UNDERLINE)
                                || cell.flags.contains(CellFlags::UNDERCURL),
                            undercurl: cell.flags.contains(CellFlags::UNDERCURL),
                            is_wide_spacer: cell.flags.contains(CellFlags::WIDE_CHAR_SPACER),
                        });
                    }

                    // Cursor screen row (only visible when not scrolled back)
                    let cursor_row = (cursor.point.line.0 as usize).wrapping_add(display_offset);

                    for (screen_row, cells) in &rows {
                        let y = bounds.origin.y + px(PADDING_PX) + line_height * *screen_row;

                        let mut line_text = String::new();
                        let mut runs: Vec<TextRun> = Vec::new();
                        let mut bg_runs: Vec<(usize, usize, Rgba)> = Vec::new();
                        let mut current_bg: Option<(usize, Rgba)> = None;

                        for rc in cells {
                            if rc.is_wide_spacer { continue; }

                            let is_cursor = display_offset == 0
                                && *screen_row == cursor_row
                                && rc.col == cursor.point.column.0;

                            let is_selected = selection.map_or(false, |(lo, hi)| {
                                let row = *screen_row;
                                let col = rc.col;
                                if row < lo.line || row > hi.line { return false; }
                                if row == lo.line && row == hi.line { return col >= lo.col && col < hi.col; }
                                if row == lo.line { return col >= lo.col; }
                                if row == hi.line { return col < hi.col; }
                                true
                            });

                            // Cell position in grid (absolute Line) coords.
                            let grid_line = *screen_row as i32 - display_offset as i32;
                            // Some(true) = active match, Some(false) = inactive.
                            let match_kind: Option<bool> = visible_matches.iter().find_map(
                                |&(ls, cs, le, ce, is_active)| {
                                    if grid_line < ls || grid_line > le { return None; }
                                    if grid_line == ls && rc.col < cs { return None; }
                                    if grid_line == le && rc.col > ce { return None; }
                                    Some(is_active)
                                },
                            );

                            let fg = if is_cursor {
                                colors::surface()
                            } else if is_selected {
                                sel_fg
                            } else if match_kind.is_some() {
                                match_fg
                            } else {
                                rc.fg
                            };
                            let cell_bg = if is_cursor {
                                colors::accent()
                            } else if is_selected {
                                sel_bg
                            } else {
                                match match_kind {
                                    Some(true) => active_match_bg,
                                    Some(false) => inactive_match_bg,
                                    None => rc.bg,
                                }
                            };

                            if cell_bg != bg_color {
                                match &mut current_bg {
                                    Some((_, ref c)) if *c == cell_bg => {}
                                    Some((start, c)) => { bg_runs.push((*start, rc.col, *c)); current_bg = Some((rc.col, cell_bg)); }
                                    None => { current_bg = Some((rc.col, cell_bg)); }
                                }
                            } else if let Some((start, c)) = current_bg.take() {
                                bg_runs.push((start, rc.col, c));
                            }

                            let ch = if rc.ch == '\0' { ' ' } else { rc.ch };
                            let char_len = ch.len_utf8();
                            line_text.push(ch);

                            let font = match (rc.bold, rc.italic) {
                                (true, true) => font_bold_italic.clone(),
                                (true, false) => font_bold.clone(),
                                (false, true) => font_italic.clone(),
                                (false, false) => font_normal.clone(),
                            };

                            let is_url_hover = hovered_url.as_ref().map_or(false, |hu| {
                                *screen_row == hu.line && rc.col >= hu.start_col && rc.col < hu.end_col
                            });

                            let underline_style = if rc.underline || is_url_hover {
                                Some(UnderlineStyle { thickness: px(1.0), color: Some(rgba_to_hsla(fg)), wavy: rc.undercurl })
                            } else { None };

                            if let Some(last) = runs.last_mut() {
                                if last.font == font && last.color == rgba_to_hsla(fg) && last.underline == underline_style {
                                    last.len += char_len;
                                    continue;
                                }
                            }

                            runs.push(TextRun {
                                len: char_len,
                                font,
                                color: rgba_to_hsla(fg),
                                background_color: None,
                                underline: underline_style,
                                strikethrough: None,
                            });
                        }

                        if let Some((start, c)) = current_bg {
                            let end_col = cells.last().map_or(start, |c| c.col + 1);
                            bg_runs.push((start, end_col, c));
                        }

                        for (start_col, end_col, color) in &bg_runs {
                            let x = bounds.origin.x + px(PADDING_PX) + px(cell_width * *start_col as f32);
                            let w = px(cell_width * (end_col - start_col) as f32);
                            window.paint_quad(fill(Bounds::new(point(x, y), size(w, line_height)), rgba_to_hsla(*color)));
                        }

                        if !line_text.is_empty() {
                            let shaped = window.text_system().shape_line(
                                SharedString::from(line_text),
                                font_size,
                                &runs,
                                None,
                            );
                            let origin = point(bounds.origin.x + px(PADDING_PX), y);
                            let _ = shaped.paint(origin, line_height, window, cx);
                        }
                    }
                }
            },
        )
        .size_full();

        let mut container = div()
            .id("terminal-view")
            .relative()
            .flex()
            .flex_1()
            .min_h(px(0.0))
            .size_full()
            .bg(colors::bg())
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .child(grid_canvas);
        if show_pointer {
            container = container.cursor(CursorStyle::PointingHand);
        }
        let container = container
            // ── Mouse ──
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, cx| {
                this.focus_handle.focus(window);
                // Cmd+Click: open URL in browser
                if event.modifiers.platform {
                    let term = this.terminal.read(cx).term.clone();
                    let pos = this.mouse_to_grid(event.position);
                    if let Some(hit) = this.detect_url_at(pos, &term) {
                        let _ = std::process::Command::new("open").arg(&hit.url).spawn();
                        this.hovered_url = None;
                        cx.notify();
                        return;
                    }
                }
                let term_model = this.terminal.read(cx);
                let has_mouse_mode = {
                    let t = term_model.term.lock();
                    t.mode().intersects(TermMode::MOUSE_MODE)
                };
                if has_mouse_mode {
                    let pos = this.mouse_to_grid(event.position);
                    this.send_mouse_event(&term_model, 0, pos, true);
                    this.selection_start = None;
                    this.selection_end = None;
                } else {
                    let pos = this.mouse_to_grid(event.position);
                    this.selection_start = Some(pos);
                    this.selection_end = Some(pos);
                    this.last_drag_mouse_y = Some(f32::from(event.position.y));
                    this.ensure_autoscroll_timer(cx);
                }
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                // URL hover detection when no button is pressed
                if event.pressed_button.is_none() {
                    if event.modifiers.platform {
                        let term = this.terminal.read(cx).term.clone();
                        let pos = this.mouse_to_grid(event.position);
                        this.hovered_url = this.detect_url_at(pos, &term);
                        cx.notify();
                    } else if this.hovered_url.take().is_some() {
                        cx.notify();
                    }
                    return;
                }
                if event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let term_model = this.terminal.read(cx);
                let has_mouse_mode = {
                    let t = term_model.term.lock();
                    t.mode().intersects(TermMode::MOUSE_MODE)
                };
                if has_mouse_mode {
                    let pos = this.mouse_to_grid(event.position);
                    this.send_mouse_event(&term_model, 32, pos, true);
                    cx.notify();
                } else if this.selection_start.is_some() {
                    let pos = this.mouse_to_grid(event.position);
                    this.selection_end = Some(pos);
                    this.last_drag_mouse_y = Some(f32::from(event.position.y));
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                this._autoscroll_task = None;
                this.last_drag_mouse_y = None;
                let term_model = this.terminal.read(cx);
                let has_mouse_mode = {
                    let t = term_model.term.lock();
                    t.mode().intersects(TermMode::MOUSE_MODE)
                };
                if has_mouse_mode {
                    let pos = this.mouse_to_grid(event.position);
                    this.send_mouse_event(&term_model, 0, pos, false);
                    cx.notify();
                }
            }))
            // ── Scroll ──
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                // Reset accumulated scroll pixels if terminal was resized,
                // since the old accumulation is based on stale dimensions.
                if this.size_changed.swap(false, Ordering::Acquire) {
                    this.scroll_px = 0.0;
                }
                let line_height = CELL_HEIGHT_PX;
                let scroll_lines: Option<i32> = match event.delta {
                    ScrollDelta::Lines(delta) => {
                        Some(delta.y as i32)
                    }
                    ScrollDelta::Pixels(delta) => {
                        match event.touch_phase {
                            TouchPhase::Started => {
                                this.scroll_px = 0.0;
                                None
                            }
                            TouchPhase::Moved => {
                                let old_offset = (this.scroll_px / line_height) as i32;
                                this.scroll_px += delta.y / px(1.0);
                                let new_offset = (this.scroll_px / line_height) as i32;
                                Some(new_offset - old_offset)
                            }
                            TouchPhase::Ended => None,
                        }
                    }
                };

                let Some(lines) = scroll_lines else { return };
                if lines == 0 { return; }

                let term_model = this.terminal.read(cx);
                let (mouse_mode, is_alt_screen, has_sgr, has_alt_scroll) = {
                    let t = term_model.term.lock();
                    (
                        t.mode().intersects(TermMode::MOUSE_MODE),
                        t.mode().contains(TermMode::ALT_SCREEN),
                        t.mode().contains(TermMode::SGR_MOUSE),
                        t.mode().contains(TermMode::ALT_SCREEN)
                            && t.mode().contains(TermMode::ALTERNATE_SCROLL),
                    )
                };

                if mouse_mode {
                    for _ in 0..lines.unsigned_abs() {
                        if has_sgr {
                            let button = if lines > 0 { 64 } else { 65 };
                            term_model.write_str_to_pty(&format!("\x1b[<{};1;1M", button));
                        } else {
                            let button: u8 = if lines > 0 { 64 } else { 65 };
                            term_model.write_to_pty(&[b'\x1b', b'[', b'M', button + 32, 33, 33]);
                        }
                    }
                } else if has_alt_scroll {
                    let cmd: u8 = if lines > 0 { b'A' } else { b'B' };
                    for _ in 0..lines.unsigned_abs() {
                        term_model.write_to_pty(&[0x1b, b'O', cmd]);
                    }
                } else if !is_alt_screen {
                    let mut t = term_model.term.lock();
                    t.scroll_display(Scroll::Delta(lines));
                    drop(t);
                }
                cx.notify();
            }))
            // ── Keyboard ──
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
                let keystroke = &event.keystroke;

                if keystroke.modifiers.platform {
                    match keystroke.key.as_str() {
                        "c" => {
                            let term = this.terminal.read(cx);
                            let text = this.selected_text(&term.term);
                            if !text.is_empty() {
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                                this.selection_start = None;
                                this.selection_end = None;
                                cx.notify();
                                return;
                            }
                            this.terminal.read(cx).write_to_pty(&[0x03]);
                            return;
                        }
                        "v" => {
                            let term = this.terminal.read(cx);
                            if let Some(clipboard) = cx.read_from_clipboard() {
                                let text = clipboard.text().unwrap_or_default();
                                term.write_to_pty(b"\x1b[200~");
                                if !text.is_empty() { term.write_str_to_pty(&text); }
                                term.write_to_pty(b"\x1b[201~");
                            }
                        }
                        "backspace" => { this.terminal.read(cx).write_to_pty(b"\x15"); }
                        "delete" => { this.terminal.read(cx).write_to_pty(b"\x0b"); }
                        "left" => { this.terminal.read(cx).write_to_pty(b"\x01"); }
                        "right" => { this.terminal.read(cx).write_to_pty(b"\x05"); }
                        _ => { return; }
                    }
                    this.selection_start = None;
                    this.selection_end = None;
                    return;
                }

                this.selection_start = None;
                this.selection_end = None;

                // Scroll to bottom on any input so the user can see what they're typing
                {
                    let term_model = this.terminal.read(cx);
                    let mut t = term_model.term.lock();
                    t.scroll_display(Scroll::Bottom);
                }

                let term = this.terminal.read(cx);

                if keystroke.modifiers.alt {
                    match keystroke.key.as_str() {
                        "backspace" => { term.write_to_pty(b"\x1b\x7f"); return; }
                        "delete" => { term.write_to_pty(b"\x1bd"); return; }
                        "left" => { term.write_to_pty(b"\x1bb"); return; }
                        "right" => { term.write_to_pty(b"\x1bf"); return; }
                        _ => {}
                    }
                    if let Some(key_char) = &keystroke.key_char {
                        let mut seq = vec![0x1b_u8];
                        seq.extend_from_slice(key_char.as_bytes());
                        term.write_to_pty(&seq);
                        return;
                    }
                    let key = keystroke.key.as_str();
                    if key.len() == 1 {
                        let mut seq = vec![0x1b_u8];
                        seq.extend_from_slice(key.as_bytes());
                        term.write_to_pty(&seq);
                        return;
                    }
                }

                if keystroke.modifiers.control {
                    match keystroke.key.as_str() {
                        "left" => { term.write_to_pty(b"\x1b[1;5D"); return; }
                        "right" => { term.write_to_pty(b"\x1b[1;5C"); return; }
                        "up" => { term.write_to_pty(b"\x1b[1;5A"); return; }
                        "down" => { term.write_to_pty(b"\x1b[1;5B"); return; }
                        _ => {}
                    }
                    let key = keystroke.key.as_str();
                    if key.len() == 1 {
                        let ch = key.chars().next().unwrap();
                        if ch.is_ascii_lowercase() {
                            term.write_to_pty(&[(ch as u8) - b'a' + 1]);
                            return;
                        }
                    }
                }

                if keystroke.modifiers.shift {
                    if keystroke.key.as_str() == "tab" { term.write_to_pty(b"\x1b[Z"); return; }
                    if keystroke.key.as_str() == "enter" { term.write_to_pty(b"\x0a"); return; }
                }

                match keystroke.key.as_str() {
                    "enter" => { term.write_to_pty(b"\r"); return; }
                    "backspace" => { term.write_to_pty(b"\x7f"); return; }
                    "tab" => { term.write_to_pty(b"\t"); return; }
                    "escape" => { term.write_to_pty(b"\x1b"); return; }
                    "up" => { term.write_to_pty(b"\x1b[A"); return; }
                    "down" => { term.write_to_pty(b"\x1b[B"); return; }
                    "right" => { term.write_to_pty(b"\x1b[C"); return; }
                    "left" => { term.write_to_pty(b"\x1b[D"); return; }
                    "home" => { term.write_to_pty(b"\x1b[H"); return; }
                    "end" => { term.write_to_pty(b"\x1b[F"); return; }
                    "delete" => { term.write_to_pty(b"\x1b[3~"); return; }
                    "space" => { term.write_to_pty(b" "); return; }
                    "pageup" => { term.write_to_pty(b"\x1b[5~"); return; }
                    "pagedown" => { term.write_to_pty(b"\x1b[6~"); return; }
                    "insert" => { term.write_to_pty(b"\x1b[2~"); return; }
                    "f1" => { term.write_to_pty(b"\x1bOP"); return; }
                    "f2" => { term.write_to_pty(b"\x1bOQ"); return; }
                    "f3" => { term.write_to_pty(b"\x1bOR"); return; }
                    "f4" => { term.write_to_pty(b"\x1bOS"); return; }
                    "f5" => { term.write_to_pty(b"\x1b[15~"); return; }
                    "f6" => { term.write_to_pty(b"\x1b[17~"); return; }
                    "f7" => { term.write_to_pty(b"\x1b[18~"); return; }
                    "f8" => { term.write_to_pty(b"\x1b[19~"); return; }
                    "f9" => { term.write_to_pty(b"\x1b[20~"); return; }
                    "f10" => { term.write_to_pty(b"\x1b[21~"); return; }
                    "f11" => { term.write_to_pty(b"\x1b[23~"); return; }
                    "f12" => { term.write_to_pty(b"\x1b[24~"); return; }
                    _ => {}
                }

                if let Some(key_char) = &keystroke.key_char {
                    term.write_str_to_pty(key_char);
                }
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                let term = this.terminal.read(cx);
                let path_strs: Vec<String> = paths
                    .paths()
                    .iter()
                    .filter_map(|p| shlex::try_quote(&p.to_string_lossy()).ok().map(|s| s.into_owned()))
                    .collect();
                if !path_strs.is_empty() {
                    let joined = path_strs.join(" ");
                    term.write_to_pty(b"\x1b[200~");
                    term.write_str_to_pty(&joined);
                    term.write_to_pty(b"\x1b[201~");
                }
            }));

        // Search bar (Cmd+F) — rendered above the terminal grid when active.
        let search_bar = if self.search_active {
            self.search_input.as_ref().map(|search_input| {
                let match_count = self.search_matches.len();
                let match_info = if self.search_query.is_empty() {
                    String::new()
                } else if match_count == 0 {
                    "No matches".to_string()
                } else if match_count >= Self::MAX_SEARCH_MATCHES {
                    format!("{}/{}+", self.search_match_ix + 1, match_count)
                } else {
                    format!("{}/{}", self.search_match_ix + 1, match_count)
                };

                div()
                    .id("terminal-search-bar")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .bg(colors::surface())
                    .border_b_1()
                    .border_color(colors::border())
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors::text_muted())
                            .pl_2()
                            .child("Find:"),
                    )
                    .child(div().flex_1().child(search_input.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors::text_muted())
                            .pr_2()
                            .child(match_info),
                    )
            })
        } else {
            None
        };

        let mut root = div()
            .key_context("Terminal")
            .on_action(cx.listener(Self::handle_find))
            .flex()
            .flex_col()
            .size_full()
            .bg(colors::bg())
            .overflow_hidden();
        if let Some(bar) = search_bar {
            root = root.child(bar);
        }
        root.child(container)
    }
}
