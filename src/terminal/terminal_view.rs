use gpui::*;
use std::sync::Arc;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::{Term, TermMode};
use crate::terminal::terminal::{alac_color_to_gpui, JsonListener, TerminalModel};
use crate::theme::colors;

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

fn rgba_to_hsla(c: Rgba) -> Hsla {
    Hsla::from(c)
}

pub struct TerminalView {
    pub terminal: Entity<TerminalModel>,
    focus_handle: FocusHandle,
    last_size: Arc<std::sync::Mutex<Option<(u16, u16)>>>,
    selection_start: Option<GridPos>,
    selection_end: Option<GridPos>,
    cell_width: f32,
    has_been_focused: bool,
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
            selection_start: None,
            selection_end: None,
            cell_width,
            has_been_focused: false,
        }
    }

    pub fn focus(&self, window: &mut Window) {
        self.focus_handle.focus(window);
    }

    fn mouse_to_grid(&self, pos: Point<Pixels>, bounds: Bounds<Pixels>) -> GridPos {
        let x = (pos.x - bounds.origin.x) / px(1.0) - PADDING_PX;
        let y = (pos.y - bounds.origin.y) / px(1.0) - PADDING_PX;
        let col = (x / self.cell_width).max(0.0) as usize;
        let line = (y / CELL_HEIGHT_PX).max(0.0) as usize;
        GridPos { line, col }
    }

    fn in_selection_range(&self, line: usize, col: usize) -> bool {
        let (Some(start), Some(end)) = (self.selection_start, self.selection_end) else {
            return false;
        };
        let lo = start.min(end);
        let hi = start.max(end);
        if lo == hi {
            return false;
        }
        (line, col) >= (lo.line, lo.col) && (line, col) <= (hi.line, hi.col)
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

}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.has_been_focused {
            self.has_been_focused = true;
            self.focus_handle.focus(window);
        }
        let terminal = self.terminal.read(cx);
        let term = terminal.term.clone();

        let terminal_entity = self.terminal.clone();
        let last_size = self.last_size.clone();

        // Build selection state for the paint closure (which can't access &self)
        let sel_start = self.selection_start;
        let sel_end = self.selection_end;
        let cell_width = self.cell_width;

        let grid_canvas = canvas(
            // Prepaint: resize detection
            {
                let terminal_entity = terminal_entity.clone();
                let last_size = last_size.clone();
                move |bounds: Bounds<Pixels>, _window: &mut Window, cx: &mut App| {
                    let w = bounds.size.width / px(1.0);
                    let h = bounds.size.height / px(1.0);
                    let cols = ((w - PADDING_PX * 2.0) / cell_width).max(1.0) as u16;
                    let rows = ((h - PADDING_PX * 2.0) / CELL_HEIGHT_PX).max(1.0) as u16;

                    let mut cached = last_size.lock().unwrap();
                    if *cached != Some((rows, cols)) {
                        *cached = Some((rows, cols));
                        let term = terminal_entity.read(cx);
                        term.resize(rows, cols);
                    }
                    bounds
                }
            },
            // Paint: render the terminal grid
            {
                let term = term.clone();
                move |bounds: Bounds<Pixels>, _prepaint: Bounds<Pixels>, window: &mut Window, cx: &mut App| {
                    // We need to reconstruct selection state for painting
                    // Paint the terminal grid using GPUI's text shaping
                    let term_lock = term.lock();
                    let content = term_lock.renderable_content();
                    let grid = term_lock.grid();
                    let num_lines = grid.screen_lines();
                    let num_cols = grid.columns();
                    let cursor = content.cursor;
                    let term_colors = content.colors;

                    let sel_color: Rgba = rgba(0x89b4fa44);
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

                    // Selection helper
                    let in_sel = |line: usize, col: usize| -> bool {
                        let (Some(start), Some(end)) = (sel_start, sel_end) else { return false; };
                        let lo = start.min(end);
                        let hi = start.max(end);
                        if lo == hi { return false; }
                        (line, col) >= (lo.line, lo.col) && (line, col) <= (hi.line, hi.col)
                    };

                    for line_idx in 0..num_lines {
                        let row = &grid[Line(line_idx as i32)];
                        let y = bounds.origin.y + px(PADDING_PX) + line_height * line_idx;

                        let last_content_col = {
                            let mut last = None;
                            for c in (0..num_cols).rev() {
                                let ch = row[Column(c)].c;
                                if ch != ' ' && ch != '\0' { last = Some(c); break; }
                            }
                            last
                        };

                        let mut line_text = String::new();
                        let mut runs: Vec<TextRun> = Vec::new();
                        let mut bg_runs: Vec<(usize, usize, Rgba)> = Vec::new();
                        let mut current_bg: Option<(usize, Rgba)> = None;

                        let mut col = 0;
                        while col < num_cols {
                            let cell = &row[Column(col)];
                            if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) { col += 1; continue; }

                            let is_cursor = line_idx == cursor.point.line.0 as usize && col == cursor.point.column.0;
                            let has_content = last_content_col.map_or(false, |lc| col <= lc);
                            let is_selected = has_content && in_sel(line_idx, col);

                            let fg = if is_cursor { colors::surface() } else { alac_color_to_gpui(&cell.fg, term_colors) };
                            let cell_bg = if is_cursor { colors::accent() } else if is_selected { sel_color } else { alac_color_to_gpui(&cell.bg, term_colors) };

                            if cell_bg != bg_color {
                                match &mut current_bg {
                                    Some((_, ref c)) if *c == cell_bg => {}
                                    Some((start, c)) => { bg_runs.push((*start, col, *c)); current_bg = Some((col, cell_bg)); }
                                    None => { current_bg = Some((col, cell_bg)); }
                                }
                            } else if let Some((start, c)) = current_bg.take() {
                                bg_runs.push((start, col, c));
                            }

                            let bold = cell.flags.contains(CellFlags::BOLD);
                            let italic = cell.flags.contains(CellFlags::ITALIC);
                            let underline = cell.flags.contains(CellFlags::UNDERLINE)
                                || cell.flags.contains(CellFlags::DOUBLE_UNDERLINE)
                                || cell.flags.contains(CellFlags::UNDERCURL);
                            let ch = if cell.c == '\0' { ' ' } else { cell.c };
                            let char_len = ch.len_utf8();
                            line_text.push(ch);

                            let font = match (bold, italic) {
                                (true, true) => font_bold_italic.clone(),
                                (true, false) => font_bold.clone(),
                                (false, true) => font_italic.clone(),
                                (false, false) => font_normal.clone(),
                            };

                            let underline_style = if underline {
                                Some(UnderlineStyle { thickness: px(1.0), color: Some(rgba_to_hsla(fg)), wavy: cell.flags.contains(CellFlags::UNDERCURL) })
                            } else { None };

                            if let Some(last) = runs.last_mut() {
                                if last.font == font && last.color == rgba_to_hsla(fg) && last.underline == underline_style {
                                    last.len += char_len;
                                    col += 1;
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
                            col += 1;
                        }

                        if let Some((start, c)) = current_bg { bg_runs.push((start, num_cols, c)); }

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

        div()
            .id("terminal-view")
            .relative()
            .flex()
            .flex_1()
            .size_full()
            .bg(colors::bg())
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .child(grid_canvas)
            // ── Mouse selection ──
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, cx| {
                this.focus_handle.focus(window);
                let bounds = window.bounds();
                let pos = this.mouse_to_grid(event.position, bounds);
                this.selection_start = Some(pos);
                this.selection_end = Some(pos);
                cx.notify();
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
                let term_model = this.terminal.read(cx);
                let lines = match event.delta {
                    ScrollDelta::Lines(delta) => -delta.y as i32,
                    ScrollDelta::Pixels(delta) => {
                        let dy = delta.y / px(1.0);
                        (-dy / CELL_HEIGHT_PX) as i32
                    }
                };
                if lines == 0 { return; }

                let mouse_mode = {
                    let t = term_model.term.lock();
                    t.mode().contains(TermMode::MOUSE_MODE) || t.mode().contains(TermMode::ALT_SCREEN)
                };

                if mouse_mode {
                    let sgr = { let t = term_model.term.lock(); t.mode().contains(TermMode::SGR_MOUSE) };
                    let count = lines.unsigned_abs().min(10);
                    for _ in 0..count {
                        if sgr {
                            let button = if lines > 0 { 64 } else { 65 };
                            term_model.write_str_to_pty(&format!("\x1b[<{};1;1M", button));
                        } else {
                            let button: u8 = if lines > 0 { 64 } else { 65 };
                            term_model.write_to_pty(&[b'\x1b', b'[', b'M', button + 32, 33, 33]);
                        }
                    }
                } else {
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
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();
                if !path_strs.is_empty() {
                    let joined = path_strs.join(" ");
                    term.write_to_pty(b"\x1b[200~");
                    term.write_str_to_pty(&joined);
                    term.write_to_pty(b"\x1b[201~");
                }
            }))
    }
}
