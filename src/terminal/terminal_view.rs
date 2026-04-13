use gpui::*;
use std::sync::Arc;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::Term;
use crate::terminal::terminal::{alac_color_to_gpui, JsonListener, TerminalModel};
use crate::theme::colors;

pub struct TerminalView {
    pub terminal: Entity<TerminalModel>,
    focus_handle: FocusHandle,
}

impl TerminalView {
    pub fn new(terminal: Entity<TerminalModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&terminal, |_this, _term, cx| cx.notify())
            .detach();
        Self {
            terminal,
            focus_handle: cx.focus_handle(),
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
            .line_height(px(18.0));

        for line_idx in 0..num_lines {
            let line = Line(line_idx as i32);
            let row = &grid[line];
            let mut line_el = div().flex().flex_row().h(px(18.0));

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

        div()
            .id("terminal-view")
            .flex()
            .flex_1()
            .size_full()
            .bg(colors::bg())
            .p_2()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
                let keystroke = &event.keystroke;

                // Let Cmd+key pass through to app actions
                if keystroke.modifiers.platform {
                    return;
                }

                let term = this.terminal.read(cx);

                // Handle special keys first
                match keystroke.key.as_str() {
                    "enter" => {
                        term.write_to_pty(b"\r");
                        return;
                    }
                    "backspace" => {
                        term.write_to_pty(b"\x7f");
                        return;
                    }
                    "tab" => {
                        term.write_to_pty(b"\t");
                        return;
                    }
                    "escape" => {
                        term.write_to_pty(b"\x1b");
                        return;
                    }
                    "up" => {
                        term.write_to_pty(b"\x1b[A");
                        return;
                    }
                    "down" => {
                        term.write_to_pty(b"\x1b[B");
                        return;
                    }
                    "right" => {
                        term.write_to_pty(b"\x1b[C");
                        return;
                    }
                    "left" => {
                        term.write_to_pty(b"\x1b[D");
                        return;
                    }
                    "home" => {
                        term.write_to_pty(b"\x1b[H");
                        return;
                    }
                    "end" => {
                        term.write_to_pty(b"\x1b[F");
                        return;
                    }
                    "delete" => {
                        term.write_to_pty(b"\x1b[3~");
                        return;
                    }
                    "space" => {
                        term.write_to_pty(b" ");
                        return;
                    }
                    _ => {}
                }

                // Handle Ctrl+key
                if keystroke.modifiers.control {
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

                // Regular character input
                if let Some(key_char) = &keystroke.key_char {
                    term.write_str_to_pty(key_char);
                }
            }))
            .child(self.render_grid(&term))
    }
}
