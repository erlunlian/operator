use gpui::*;
use std::cell::Cell;
use std::rc::Rc;

use crate::theme::colors;

const CHAR_WIDTH: f32 = 8.4;

/// A reusable single-line text input component.
///
/// Supports: typing, cursor movement (arrows, Cmd+arrows, Option+arrows),
/// selection (shift+arrows, Cmd+A, double-click word, click+drag),
/// copy/cut/paste, backspace/delete (word-level with Option).
pub struct TextInput {
    pub text: String,
    pub placeholder: String,
    cursor: usize, // byte offset, always at char boundary
    selection: Option<(usize, usize)>, // (anchor, head) in byte offsets
    mouse_selecting: bool,
    focus_handle: FocusHandle,
    /// Captured left edge of text area in window coords.
    text_origin_x: Rc<Cell<f32>>,
    /// Called when the user presses Enter.
    on_submit: Option<Rc<dyn Fn(&str, &mut Window, &mut App)>>,
    /// Called when the user presses Escape.
    on_cancel: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    /// Called whenever the text changes.
    on_change: Option<Rc<dyn Fn(&str, &mut Window, &mut App)>>,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            text: String::new(),
            placeholder: String::new(),
            cursor: 0,
            selection: None,
            mouse_selecting: false,
            focus_handle: cx.focus_handle(),
            text_origin_x: Rc::new(Cell::new(0.0)),
            on_submit: None,
            on_cancel: None,
            on_change: None,
        }
    }

    pub fn set_placeholder(&mut self, placeholder: &str) {
        self.placeholder = placeholder.to_string();
    }

    pub fn set_on_submit(&mut self, f: Rc<dyn Fn(&str, &mut Window, &mut App)>) {
        self.on_submit = Some(f);
    }

    pub fn set_on_cancel(&mut self, f: Rc<dyn Fn(&mut Window, &mut App)>) {
        self.on_cancel = Some(f);
    }

    pub fn set_on_change(&mut self, f: Rc<dyn Fn(&str, &mut Window, &mut App)>) {
        self.on_change = Some(f);
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.text.len();
        self.selection = None;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.selection = None;
    }

    pub fn focus(&self, window: &mut Window) {
        self.focus_handle.focus(window);
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    // ── Char boundary helpers ──

    fn prev_boundary(&self, pos: usize) -> usize {
        if pos == 0 {
            return 0;
        }
        let mut p = pos - 1;
        while p > 0 && !self.text.is_char_boundary(p) {
            p -= 1;
        }
        p
    }

    fn next_boundary(&self, pos: usize) -> usize {
        if pos >= self.text.len() {
            return self.text.len();
        }
        let mut p = pos + 1;
        while p < self.text.len() && !self.text.is_char_boundary(p) {
            p += 1;
        }
        p
    }

    // ── Word boundary helpers ──

    fn word_start(&self, pos: usize) -> usize {
        let bytes = self.text.as_bytes();
        let mut p = pos;
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        // Skip whitespace backwards
        while p > 0 && bytes[p - 1].is_ascii_whitespace() {
            p -= 1;
        }
        if p > 0 && is_word(bytes[p - 1]) {
            while p > 0 && is_word(bytes[p - 1]) {
                p -= 1;
            }
        } else {
            while p > 0 && !is_word(bytes[p - 1]) && !bytes[p - 1].is_ascii_whitespace() {
                p -= 1;
            }
        }
        p
    }

    fn word_end(&self, pos: usize) -> usize {
        let bytes = self.text.as_bytes();
        let len = bytes.len();
        let mut p = pos;
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        // Skip whitespace forwards
        while p < len && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        if p < len && is_word(bytes[p]) {
            while p < len && is_word(bytes[p]) {
                p += 1;
            }
        } else {
            while p < len && !is_word(bytes[p]) && !bytes[p].is_ascii_whitespace() {
                p += 1;
            }
        }
        p
    }

    // ── Selection helpers ──

    /// Returns (start, end) ordered.
    fn sel_range(&self) -> Option<(usize, usize)> {
        let (a, h) = self.selection?;
        Some(if a <= h { (a, h) } else { (h, a) })
    }

    fn selected_text(&self) -> Option<String> {
        let (s, e) = self.sel_range()?;
        if s == e {
            return None;
        }
        Some(self.text[s..e].to_string())
    }

    fn delete_selection(&mut self) -> bool {
        if let Some((s, e)) = self.sel_range() {
            if s != e {
                self.text.drain(s..e);
                self.cursor = s;
                self.selection = None;
                return true;
            }
        }
        self.selection = None;
        false
    }

    fn extend_selection(&mut self, new_head: usize) {
        if let Some((anchor, _)) = self.selection {
            self.selection = Some((anchor, new_head));
        } else {
            self.selection = Some((self.cursor, new_head));
        }
        self.cursor = new_head;
    }

    fn move_cursor(&mut self, pos: usize, shift: bool) {
        if shift {
            self.extend_selection(pos);
        } else {
            self.selection = None;
            self.cursor = pos;
        }
    }

    // ── Mouse position ──

    fn x_to_offset(&self, x: f32) -> usize {
        let text_x = x - self.text_origin_x.get();
        let char_ix = (text_x / CHAR_WIDTH).max(0.0) as usize;
        // Convert char index to byte offset
        let mut byte_off = 0;
        for (i, (bi, ch)) in self.text.char_indices().enumerate() {
            if i >= char_ix {
                byte_off = bi;
                return byte_off;
            }
            byte_off = bi + ch.len_utf8();
        }
        byte_off.min(self.text.len())
    }

    fn word_at(&self, pos: usize) -> (usize, usize) {
        let bytes = self.text.as_bytes();
        if pos >= bytes.len() {
            return (self.text.len(), self.text.len());
        }
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        if !is_word(bytes[pos]) {
            return (pos, self.next_boundary(pos));
        }
        let mut start = pos;
        while start > 0 && is_word(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = pos;
        while end < bytes.len() && is_word(bytes[end]) {
            end += 1;
        }
        (start, end)
    }

    // ── Event handlers ──

    fn handle_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let shift = ks.modifiers.shift;

        if ks.modifiers.platform {
            match ks.key.as_str() {
                "a" => {
                    // Select all
                    self.selection = Some((0, self.text.len()));
                    self.cursor = self.text.len();
                    cx.notify();
                    return;
                }
                "c" => {
                    if let Some(text) = self.selected_text() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    return;
                }
                "x" => {
                    if let Some(text) = self.selected_text() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        self.delete_selection();
                        self.fire_change(window, cx);
                        cx.notify();
                    }
                    return;
                }
                "v" => {
                    if let Some(item) = cx.read_from_clipboard() {
                        if let Some(text) = item.text() {
                            self.delete_selection();
                            // Single line only
                            let line = text.lines().next().unwrap_or("");
                            self.text.insert_str(self.cursor, line);
                            self.cursor += line.len();
                            self.fire_change(window, cx);
                            cx.notify();
                        }
                    }
                    return;
                }
                "left" => {
                    self.move_cursor(0, shift);
                    cx.notify();
                    return;
                }
                "right" => {
                    self.move_cursor(self.text.len(), shift);
                    cx.notify();
                    return;
                }
                "backspace" => {
                    if !self.delete_selection() {
                        if self.cursor > 0 {
                            self.text.drain(0..self.cursor);
                            self.cursor = 0;
                        }
                    }
                    self.fire_change(window, cx);
                    cx.notify();
                    return;
                }
                _ => return, // Let other Cmd combos pass through
            }
        }

        if ks.modifiers.alt {
            match ks.key.as_str() {
                "left" => {
                    let pos = self.word_start(self.cursor);
                    self.move_cursor(pos, shift);
                    cx.notify();
                    return;
                }
                "right" => {
                    let pos = self.word_end(self.cursor);
                    self.move_cursor(pos, shift);
                    cx.notify();
                    return;
                }
                "backspace" => {
                    if !self.delete_selection() {
                        let target = self.word_start(self.cursor);
                        if target < self.cursor {
                            self.text.drain(target..self.cursor);
                            self.cursor = target;
                        }
                    }
                    self.fire_change(window, cx);
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }

        match ks.key.as_str() {
            "left" => {
                if !shift && self.sel_range().is_some() {
                    let (s, _) = self.sel_range().unwrap();
                    self.cursor = s;
                    self.selection = None;
                } else {
                    let pos = self.prev_boundary(self.cursor);
                    self.move_cursor(pos, shift);
                }
                cx.notify();
            }
            "right" => {
                if !shift && self.sel_range().is_some() {
                    let (_, e) = self.sel_range().unwrap();
                    self.cursor = e;
                    self.selection = None;
                } else {
                    let pos = self.next_boundary(self.cursor);
                    self.move_cursor(pos, shift);
                }
                cx.notify();
            }
            "home" => {
                self.move_cursor(0, shift);
                cx.notify();
            }
            "end" => {
                self.move_cursor(self.text.len(), shift);
                cx.notify();
            }
            "backspace" => {
                if !self.delete_selection() {
                    if self.cursor > 0 {
                        let prev = self.prev_boundary(self.cursor);
                        self.text.drain(prev..self.cursor);
                        self.cursor = prev;
                    }
                }
                self.fire_change(window, cx);
                cx.notify();
            }
            "delete" => {
                if !self.delete_selection() {
                    if self.cursor < self.text.len() {
                        let next = self.next_boundary(self.cursor);
                        self.text.drain(self.cursor..next);
                    }
                }
                self.fire_change(window, cx);
                cx.notify();
            }
            "enter" => {
                if let Some(cb) = self.on_submit.clone() {
                    cb(&self.text, window, cx);
                }
            }
            "escape" => {
                if let Some(cb) = self.on_cancel.clone() {
                    cb(window, cx);
                }
            }
            "tab" => {
                // Ignore tab in single-line input
            }
            _ => {
                if let Some(ch) = &ks.key_char {
                    if !ch.is_empty() {
                        self.delete_selection();
                        self.text.insert_str(self.cursor, ch);
                        self.cursor += ch.len();
                        self.fire_change(window, cx);
                        cx.notify();
                    }
                }
            }
        }
    }

    fn handle_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window);
        let offset = self.x_to_offset(f32::from(event.position.x));

        if event.click_count == 2 {
            let (start, end) = self.word_at(offset);
            self.selection = Some((start, end));
            self.cursor = end;
            self.mouse_selecting = false;
            cx.notify();
            return;
        }

        self.cursor = offset;
        self.selection = None;
        self.mouse_selecting = true;
        cx.notify();
    }

    fn handle_mouse_move(&mut self, event: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.mouse_selecting {
            return;
        }
        let offset = self.x_to_offset(f32::from(event.position.x));
        if self.selection.is_none() {
            self.selection = Some((self.cursor, offset));
        } else {
            self.selection = Some((self.selection.unwrap().0, offset));
        }
        self.cursor = offset;
        cx.notify();
    }

    fn handle_mouse_up(&mut self, _event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.mouse_selecting = false;
        // Clear empty selection
        if let Some((a, h)) = self.selection {
            if a == h {
                self.selection = None;
            }
        }
        cx.notify();
    }

    fn fire_change(&self, window: &mut Window, cx: &mut App) {
        if let Some(cb) = &self.on_change {
            cb(&self.text, window, cx);
        }
    }

    fn byte_to_char_count(text: &str, byte_pos: usize) -> usize {
        text[..byte_pos.min(text.len())].chars().count()
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text_origin = self.text_origin_x.clone();
        let cursor_char = Self::byte_to_char_count(&self.text, self.cursor) as f32;

        let sel_range = self.sel_range().filter(|(s, e)| s != e);

        let display_text: SharedString = if self.text.is_empty() {
            self.placeholder.clone().into()
        } else {
            self.text.clone().into()
        };
        let text_color = if self.text.is_empty() {
            colors::text_muted()
        } else {
            colors::text()
        };

        let mut wrapper = div()
            .id("text-input")
            .flex()
            .flex_row()
            .items_center()
            .h(px(28.0))
            .px_2()
            .font_family("Menlo")
            .text_sm()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::handle_mouse_down))
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            .cursor_text();

        // The text area with cursor overlay
        let mut text_area = div()
            .relative()
            .flex_1()
            .overflow_x_hidden()
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        text_origin.set(f32::from(bounds.origin.x));
                    },
                    |_, _, _, _| {},
                )
                .size_0(),
            )
            .child(
                div().text_color(text_color).child(display_text),
            );

        // Selection highlight
        if let Some((s, e)) = sel_range {
            let sel_start = Self::byte_to_char_count(&self.text, s) as f32;
            let sel_end = Self::byte_to_char_count(&self.text, e) as f32;
            let sel_w = (sel_end - sel_start) * CHAR_WIDTH;
            text_area = text_area.child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(sel_start * CHAR_WIDTH))
                    .w(px(sel_w))
                    .h_full()
                    .bg(gpui::rgba(0x264f7844)),
            );
        }

        // Cursor
        text_area = text_area.child(
            div()
                .absolute()
                .top(px(2.0))
                .left(px(cursor_char * CHAR_WIDTH))
                .w(px(2.0))
                .h(px(16.0))
                .bg(colors::accent()),
        );

        wrapper = wrapper.child(text_area);
        wrapper
    }
}
