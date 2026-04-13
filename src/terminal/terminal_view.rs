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

pub struct TerminalView {
    pub terminal: Entity<TerminalModel>,
    focus_handle: FocusHandle,
    last_size: Arc<std::sync::Mutex<Option<(u16, u16)>>>,
}

impl TerminalView {
    pub fn new(terminal: Entity<TerminalModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&terminal, |_this, _term, cx| cx.notify())
            .detach();
        Self {
            terminal,
            focus_handle: cx.focus_handle(),
            last_size: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn render_grid(&self, term: &Arc<FairMutex<Term<JsonListener>>>) -> Div {
        let term = term.lock();
        let content = term.renderable_content();
        let grid = term.grid();
        let num_lines = grid.screen_lines();
        let num_cols = grid.columns();
        let cursor = content.cursor;
        let term_colors = content.colors;

        let mut container = div()
            .flex()
            .flex_col()
            .font_family("Menlo")
            .text_size(px(13.0))
            .line_height(px(16.0));

        for line_idx in 0..num_lines {
            let line = Line(line_idx as i32);
            let row = &grid[line];
            let mut line_el = div().flex().flex_row().h(px(16.0));

            // Group cells into runs of same style
            let mut runs: Vec<(String, Rgba, Rgba, bool, bool)> = Vec::new();

            let mut col = 0;
            while col < num_cols {
                let cell = &row[Column(col)];

                // Skip wide char spacers
                if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                    col += 1;
                    continue;
                }

                let is_cursor =
                    line_idx == cursor.point.line.0 as usize && col == cursor.point.column.0;

                let fg = if is_cursor {
                    colors::surface()
                } else {
                    alac_color_to_gpui(&cell.fg, term_colors)
                };

                let bg = if is_cursor {
                    colors::accent()
                } else {
                    let bg_color = alac_color_to_gpui(&cell.bg, term_colors);
                    bg_color
                };

                let bold = cell.flags.contains(CellFlags::BOLD);
                let ch = if cell.c == '\0' { ' ' } else { cell.c };

                // Try to merge with previous run
                if !is_cursor {
                    if let Some(last) = runs.last_mut() {
                        if last.1 == fg && last.2 == bg && last.3 == bold && !last.4 {
                            last.0.push(ch);
                            col += 1;
                            continue;
                        }
                    }
                }

                runs.push((ch.to_string(), fg, bg, bold, is_cursor));
                col += 1;
            }

            for (text, fg, bg, bold, _is_cursor) in &runs {
                let has_custom_bg = *bg != colors::bg();

                let mut span = div().text_color(*fg).flex_shrink_0();

                if has_custom_bg {
                    span = span.bg(*bg);
                }

                if *bold {
                    span = span.font_weight(FontWeight::BOLD);
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

        // Resize detection: use a zero-size canvas overlay to measure bounds
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
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
                let keystroke = &event.keystroke;

                // ── Cmd (platform) key combos ──
                if keystroke.modifiers.platform {
                    let term = this.terminal.read(cx);
                    match keystroke.key.as_str() {
                        // Cmd+V: paste with bracket paste mode
                        // Claude Code detects bracket paste and reads the system
                        // clipboard itself (including images via pbpaste).
                        "v" => {
                            if let Some(clipboard) = cx.read_from_clipboard() {
                                let text = clipboard.text().unwrap_or_default();
                                term.write_to_pty(b"\x1b[200~");
                                if !text.is_empty() {
                                    term.write_str_to_pty(&text);
                                }
                                term.write_to_pty(b"\x1b[201~");
                            }
                        }
                        // Cmd+Backspace: delete to beginning of line (Ctrl+U)
                        "backspace" => { term.write_to_pty(b"\x15"); }
                        // Cmd+Delete: delete to end of line (Ctrl+K)
                        "delete" => { term.write_to_pty(b"\x0b"); }
                        // Cmd+Left: move to beginning of line (Home)
                        "left" => { term.write_to_pty(b"\x01"); }
                        // Cmd+Right: move to end of line (End)
                        "right" => { term.write_to_pty(b"\x05"); }
                        // Let other Cmd+key pass through to app actions
                        _ => { return; }
                    }
                    return;
                }

                let term = this.terminal.read(cx);

                // ── Option (alt) key combos ──
                if keystroke.modifiers.alt {
                    match keystroke.key.as_str() {
                        // Option+Backspace: delete word backward (ESC + DEL)
                        "backspace" => { term.write_to_pty(b"\x1b\x7f"); return; }
                        // Option+Delete: delete word forward (ESC + d)
                        "delete" => { term.write_to_pty(b"\x1bd"); return; }
                        // Option+Left: move word backward (ESC + b)
                        "left" => { term.write_to_pty(b"\x1bb"); return; }
                        // Option+Right: move word forward (ESC + f)
                        "right" => { term.write_to_pty(b"\x1bf"); return; }
                        _ => {}
                    }
                    // Generic Alt+key: send ESC prefix
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

                // Regular character input
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
