use gpui::*;
use std::sync::Arc;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::Term;
use crate::terminal::terminal::{alac_color_to_gpui, JsonListener, TerminalModel};
use crate::theme::colors;

const CELL_WIDTH_PX: f32 = 8.0;
const CELL_HEIGHT_PX: f32 = 16.0;
const PADDING_PX: f32 = 8.0; // p_2 = 0.5rem = 8px

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

pub struct TerminalView {
    pub terminal: Entity<TerminalModel>,
    focus_handle: FocusHandle,
    last_size: Arc<std::sync::Mutex<Option<(u16, u16)>>>,
    /// Mouse selection anchors.
    selection_start: Option<GridPos>,
    selection_end: Option<GridPos>,
}

impl TerminalView {
    pub fn new(terminal: Entity<TerminalModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&terminal, |_this, _term, cx| cx.notify())
            .detach();
        Self {
            terminal,
            focus_handle: cx.focus_handle(),
            last_size: Arc::new(std::sync::Mutex::new(None)),
            selection_start: None,
            selection_end: None,
        }
    }

    /// Convert a window-relative mouse position to a grid (line, col).
    fn mouse_to_grid(&self, pos: Point<Pixels>, bounds: Bounds<Pixels>) -> GridPos {
        let x = (pos.x - bounds.origin.x) / px(1.0) - PADDING_PX;
        let y = (pos.y - bounds.origin.y) / px(1.0) - PADDING_PX;
        let col = (x / CELL_WIDTH_PX).max(0.0) as usize;
        let line = (y / CELL_HEIGHT_PX).max(0.0) as usize;
        GridPos { line, col }
    }

    /// Returns true if the cell at (line, col) is inside the raw selection range.
    fn in_selection_range(&self, line: usize, col: usize) -> bool {
        let (Some(start), Some(end)) = (self.selection_start, self.selection_end) else {
            return false;
        };
        let lo = start.min(end);
        let hi = start.max(end);
        if lo == hi {
            return false;
        }
        let pos = (line, col);
        pos >= (lo.line, lo.col) && pos <= (hi.line, hi.col)
    }

    /// Extract the selected text from the terminal grid.
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

    /// Find the last non-empty column index for a given row, or None if the row is empty.
    fn last_content_col(row: &alacritty_terminal::grid::Row<alacritty_terminal::term::cell::Cell>, num_cols: usize) -> Option<usize> {
        for col in (0..num_cols).rev() {
            let cell = &row[Column(col)];
            let ch = cell.c;
            if ch != ' ' && ch != '\0' {
                return Some(col);
            }
        }
        None
    }

    fn render_grid(&self, term: &Arc<FairMutex<Term<JsonListener>>>) -> Div {
        let term_lock = term.lock();
        let content = term_lock.renderable_content();
        let grid = term_lock.grid();
        let num_lines = grid.screen_lines();
        let num_cols = grid.columns();
        let cursor = content.cursor;
        let term_colors = content.colors;

        let sel_bg = rgba(0x89b4fa44);

        let mut container = div()
            .flex()
            .flex_col()
            .font_family("MesloLGS NF")
            .text_size(px(13.0))
            .line_height(px(16.0));

        for line_idx in 0..num_lines {
            let line = Line(line_idx as i32);
            let row = &grid[line];
            let last_content = Self::last_content_col(row, num_cols);
            let mut line_el = div().flex().flex_row().h(px(16.0));

            // (text, fg, bg, bold, italic, underline, is_cursor)
            let mut runs: Vec<(String, Rgba, Rgba, bool, bool, bool, bool)> = Vec::new();

            let mut col = 0;
            while col < num_cols {
                let cell = &row[Column(col)];

                if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                    col += 1;
                    continue;
                }

                let is_cursor =
                    line_idx == cursor.point.line.0 as usize && col == cursor.point.column.0;

                // Only highlight selection on cells that have content (or are before last content)
                let has_content = last_content.map_or(false, |lc| col <= lc);
                let is_selected = has_content && self.in_selection_range(line_idx, col);

                let fg = if is_cursor {
                    colors::surface()
                } else {
                    alac_color_to_gpui(&cell.fg, term_colors)
                };

                let bg = if is_cursor {
                    colors::accent()
                } else if is_selected {
                    sel_bg
                } else {
                    alac_color_to_gpui(&cell.bg, term_colors)
                };

                let bold = cell.flags.contains(CellFlags::BOLD);
                let italic = cell.flags.contains(CellFlags::ITALIC);
                let underline = cell.flags.contains(CellFlags::UNDERLINE)
                    || cell.flags.contains(CellFlags::DOUBLE_UNDERLINE)
                    || cell.flags.contains(CellFlags::UNDERCURL);
                let ch = if cell.c == '\0' { ' ' } else { cell.c };

                if !is_cursor {
                    if let Some(last) = runs.last_mut() {
                        if last.1 == fg && last.2 == bg && last.3 == bold
                            && last.4 == italic && last.5 == underline && !last.6
                        {
                            last.0.push(ch);
                            col += 1;
                            continue;
                        }
                    }
                }

                runs.push((ch.to_string(), fg, bg, bold, italic, underline, is_cursor));
                col += 1;
            }

            for (text, fg, bg, bold, italic, underline, _is_cursor) in &runs {
                let has_custom_bg = *bg != colors::bg();

                let mut span = div().text_color(*fg).flex_shrink_0();

                if has_custom_bg {
                    span = span.bg(*bg);
                }

                if *bold {
                    span = span.font_weight(FontWeight::BOLD);
                }

                if *italic {
                    span = span.italic();
                }

                if *underline {
                    span = span.underline();
                }

                span = span.child(text.clone());
                line_el = line_el.child(span);
            }

            if runs.is_empty() {
                line_el = line_el.child(div().child("\u{00A0}"));
            }

            container = container.child(line_el);
        }

        container
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal = self.terminal.read(cx);
        let term = terminal.term.clone();

        let terminal_entity = self.terminal.clone();
        let last_size = self.last_size.clone();
        let size_detector = canvas(
            move |bounds: Bounds<Pixels>, _window: &mut Window, cx: &mut App| {
                let w = bounds.size.width / px(1.0);
                let h = bounds.size.height / px(1.0);
                let cols = ((w - PADDING_PX * 2.0) / CELL_WIDTH_PX).max(1.0) as u16;
                let rows = ((h - PADDING_PX * 2.0) / CELL_HEIGHT_PX).max(1.0) as u16;

                let mut cached = last_size.lock().unwrap();
                if *cached != Some((rows, cols)) {
                    *cached = Some((rows, cols));
                    let term = terminal_entity.read(cx);
                    term.resize(rows, cols);
                }
            },
            |_bounds, _state: (), _window, _cx| {},
        )
        .size_full()
        .absolute()
        .top_0()
        .left_0();

        div()
            .id("terminal-view")
            .relative()
            .flex()
            .flex_1()
            .size_full()
            .bg(colors::bg())
            .p_2()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .child(size_detector)
            // ── Mouse selection ──
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, _cx| {
                let bounds = window.bounds();
                let pos = this.mouse_to_grid(event.position, bounds);
                this.selection_start = Some(pos);
                this.selection_end = Some(pos);
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if this.selection_start.is_some() && event.pressed_button == Some(MouseButton::Left) {
                    let bounds = window.bounds();
                    let pos = this.mouse_to_grid(event.position, bounds);
                    this.selection_end = Some(pos);
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|_this, _event: &MouseUpEvent, _window, _cx| {
            }))
            // ── Scroll ──
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                let term = this.terminal.read(cx);
                let lines = match event.delta {
                    ScrollDelta::Lines(delta) => -delta.y as i32,
                    ScrollDelta::Pixels(delta) => {
                        let dy = delta.y / px(1.0);
                        (-dy / CELL_HEIGHT_PX) as i32
                    }
                };
                if lines > 0 {
                    for _ in 0..lines.min(10) {
                        term.write_to_pty(b"\x1b[A");
                    }
                } else if lines < 0 {
                    for _ in 0..(-lines).min(10) {
                        term.write_to_pty(b"\x1b[B");
                    }
                }
            }))
            // ── Keyboard ──
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
                let keystroke = &event.keystroke;

                // ── Cmd (platform) key combos ──
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
                            let term = this.terminal.read(cx);
                            term.write_to_pty(&[0x03]);
                            return;
                        }
                        "v" => {
                            let term = this.terminal.read(cx);
                            if let Some(clipboard) = cx.read_from_clipboard() {
                                let text = clipboard.text().unwrap_or_default();
                                term.write_to_pty(b"\x1b[200~");
                                if !text.is_empty() {
                                    term.write_str_to_pty(&text);
                                }
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

                // Clear selection on any non-Cmd keypress
                this.selection_start = None;
                this.selection_end = None;

                let term = this.terminal.read(cx);

                // ── Option (alt) key combos ──
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

                // ── Ctrl key combos ──
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
                            let ctrl_byte = (ch as u8) - b'a' + 1;
                            term.write_to_pty(&[ctrl_byte]);
                            return;
                        }
                    }
                }

                // ── Shift key combos ──
                if keystroke.modifiers.shift {
                    match keystroke.key.as_str() {
                        "tab" => { term.write_to_pty(b"\x1b[Z"); return; }
                        _ => {}
                    }
                }

                // ── Special keys (no modifiers) ──
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
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();
                if !path_strs.is_empty() {
                    let joined = path_strs.join(" ");
                    term.write_to_pty(b"\x1b[200~");
                    term.write_str_to_pty(&joined);
                    term.write_to_pty(b"\x1b[201~");
                }
            }))
            .child(self.render_grid(&term))
    }
}
