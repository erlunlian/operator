use gpui::*;
use std::cell::Cell;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use crate::actions::{FindInFile, SaveFile};
use crate::editor::syntax::{self, HighlightSpan};
use crate::settings::AppSettings;
use crate::theme::colors;

// ── Pre-computed highlight data for rendering ──

struct LineHighlight {
    range: Range<usize>,
    color: Hsla,
}

struct RenderedLine {
    text: SharedString,
    highlights: Vec<LineHighlight>,
}

// ── Default character width for monospace cursor positioning ──
// Menlo at text_sm (14px) on macOS. Overridden dynamically if measured.
const DEFAULT_CHAR_WIDTH: f32 = 8.4;

// ── Vim mode ──

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum VimMode {
    Normal,
    Insert,
    Visual,
    VisualLine,
}

impl VimMode {
    pub fn label(&self) -> &str {
        match self {
            VimMode::Normal => "NORMAL",
            VimMode::Insert => "INSERT",
            VimMode::Visual => "VISUAL",
            VimMode::VisualLine => "V-LINE",
        }
    }
}

// ── Mouse selection ──

#[derive(Clone, Copy, Debug)]
pub struct Selection {
    pub start_row: usize,
    pub start_col: usize,
    pub end_row: usize,
    pub end_col: usize,
}

impl Selection {
    /// Returns (start_row, start_col, end_row, end_col) ordered so start <= end.
    pub fn ordered(&self) -> (usize, usize, usize, usize) {
        if self.start_row < self.end_row
            || (self.start_row == self.end_row && self.start_col <= self.end_col)
        {
            (self.start_row, self.start_col, self.end_row, self.end_col)
        } else {
            (self.end_row, self.end_col, self.start_row, self.start_col)
        }
    }

    /// Returns the selected column range for a given row, or None if this row is not selected.
    pub fn col_range_for_row(&self, row: usize, line_len: usize) -> Option<(usize, usize)> {
        let (sr, sc, er, ec) = self.ordered();
        if row < sr || row > er {
            return None;
        }
        let start = if row == sr { sc } else { 0 };
        let end = if row == er { ec } else { line_len };
        if start >= end && !(row > sr && row < er && line_len == 0) {
            // For empty lines in the middle of a selection, we still want a small highlight
            if line_len == 0 && row >= sr && row <= er {
                return Some((0, 1)); // highlight a space-width for empty lines
            }
            return None;
        }
        Some((start, end))
    }
}

// ── Undo/Redo ──

/// Classifies edits so we can coalesce consecutive same-kind operations
/// into a single undo group (like real editors do).
#[derive(Clone, Copy, PartialEq)]
enum EditKind {
    InsertChar,
    InsertWhitespace,
    InsertNewline,
    DeleteBackward,
    DeleteForward,
    /// Any edit that should always get its own undo group
    /// (paste, vim commands, cut selection, etc.)
    Other,
}

#[derive(Clone)]
struct UndoEntry {
    buffer: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
}

pub struct FileViewer {
    pub path: PathBuf,
    pub language: Option<String>,
    pub dirty: bool,

    // Editable text buffer (source of truth)
    buffer: Vec<String>,

    // Cached rendering
    rendered_lines: Rc<Vec<RenderedLine>>,

    // Cursor
    cursor_row: usize,
    cursor_col: usize, // byte offset within line, always at char boundary

    // Undo/Redo
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
    last_edit_kind: Option<EditKind>,
    last_edit_time: Instant,

    // Vim
    vim_mode: VimMode,
    vim_pending: Option<char>, // for multi-key commands like dd, dw, gg, etc.
    visual_anchor_row: usize,
    visual_anchor_col: usize,

    // Mouse selection
    mouse_selecting: bool,
    selection: Option<Selection>,

    // Word highlight (double-click or Cmd+click)
    /// The word currently highlighted across the file (all occurrences).
    highlighted_word: Option<String>,
    /// Positions of all occurrences: (row, start_col, end_col) in byte offsets.
    highlighted_occurrences: Vec<(usize, usize, usize)>,

    // In-file search (Cmd+F)
    pub search_active: bool,
    pub search_query: String,
    pub search_input: Option<Entity<crate::text_input::TextInput>>,
    /// (row, start_col, end_col) for each match
    search_matches: Vec<(usize, usize, usize)>,
    search_match_ix: usize,

    // GPUI
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    gutter_width: f32,
    /// Left edge of the code content area in window coordinates, captured during paint.
    code_area_left: Rc<Cell<f32>>,
    /// Top edge of the uniform_list content area in window coordinates, captured during paint.
    content_area_top: Rc<Cell<f32>>,
    /// Measured monospace character width, captured during paint.
    char_width: Rc<Cell<f32>>,
}

impl FileViewer {
    pub fn open(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let content = std::fs::read_to_string(&path).unwrap_or_else(|e| format!("Error: {}", e));
        let language = syntax::detect_language(path.to_str().unwrap_or(""))
            .map(|s| s.to_string());

        let highlights = if let Some(lang) = &language {
            syntax::highlight_source(&content, lang)
        } else {
            vec![]
        };

        let buffer: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        // Ensure at least one line
        let buffer = if buffer.is_empty() { vec![String::new()] } else { buffer };

        let rendered_lines = Self::precompute_lines(&content, &highlights);
        let gutter_width = format!("{}", rendered_lines.len()).len().max(3) as f32 * 8.4 + 16.0;

        Self {
            path,
            language,
            dirty: false,
            buffer,
            rendered_lines: Rc::new(rendered_lines),
            cursor_row: 0,
            cursor_col: 0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_kind: None,
            last_edit_time: Instant::now(),
            vim_mode: VimMode::Normal,
            vim_pending: None,
            visual_anchor_row: 0,
            visual_anchor_col: 0,
            mouse_selecting: false,
            selection: None,
            highlighted_word: None,
            highlighted_occurrences: Vec::new(),
            search_active: false,
            search_query: String::new(),
            search_input: None,
            search_matches: Vec::new(),
            search_match_ix: 0,
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            gutter_width,
            code_area_left: Rc::new(Cell::new(0.0)),
            content_area_top: Rc::new(Cell::new(0.0)),
            char_width: Rc::new(Cell::new(DEFAULT_CHAR_WIDTH)),
        }
    }

    pub fn new_empty(path: PathBuf, cx: &mut Context<Self>) -> Self {
        Self {
            path,
            language: None,
            dirty: true,
            buffer: vec![String::new()],
            rendered_lines: Rc::new(vec![RenderedLine {
                text: SharedString::from(" ".to_string()),
                highlights: vec![],
            }]),
            cursor_row: 0,
            cursor_col: 0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_kind: None,
            last_edit_time: Instant::now(),
            vim_mode: VimMode::Normal,
            vim_pending: None,
            visual_anchor_row: 0,
            visual_anchor_col: 0,
            mouse_selecting: false,
            selection: None,
            highlighted_word: None,
            highlighted_occurrences: Vec::new(),
            search_active: false,
            search_query: String::new(),
            search_input: None,
            search_matches: Vec::new(),
            search_match_ix: 0,
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            gutter_width: 3.0 * 8.4 + 16.0,
            code_area_left: Rc::new(Cell::new(0.0)),
            content_area_top: Rc::new(Cell::new(0.0)),
            char_width: Rc::new(Cell::new(DEFAULT_CHAR_WIDTH)),
        }
    }

    // ── Highlight precomputation ──

    fn precompute_lines(content: &str, highlights: &[HighlightSpan]) -> Vec<RenderedLine> {
        let mut result = Vec::new();
        let mut line_start = 0;

        for line_text in content.split('\n') {
            let line_text = line_text.strip_suffix('\r').unwrap_or(line_text);
            let line_byte_start = line_start;
            let line_byte_end = line_byte_start + line_text.len();

            let line_highlights: Vec<LineHighlight> = highlights
                .iter()
                .filter(|s| {
                    s.byte_range.start < line_byte_end && s.byte_range.end > line_byte_start
                })
                .filter_map(|s| {
                    let span_start = s.byte_range.start.max(line_byte_start) - line_byte_start;
                    let span_end = s.byte_range.end.min(line_byte_end) - line_byte_start;

                    if span_start < span_end
                        && span_end <= line_text.len()
                        && line_text.is_char_boundary(span_start)
                        && line_text.is_char_boundary(span_end)
                    {
                        Some(LineHighlight {
                            range: span_start..span_end,
                            color: Hsla::from(s.color),
                        })
                    } else {
                        None
                    }
                })
                .collect();

            let text = if line_text.is_empty() {
                SharedString::from(" ".to_string())
            } else {
                SharedString::from(line_text.to_string())
            };

            result.push(RenderedLine {
                text,
                highlights: line_highlights,
            });

            line_start = line_byte_end;
            if content.as_bytes().get(line_start) == Some(&b'\r') {
                line_start += 1;
            }
            if content.as_bytes().get(line_start) == Some(&b'\n') {
                line_start += 1;
            }
        }

        result
    }

    fn recompute_highlights(&mut self) {
        // Rebuild rendered_lines from buffer without full syntax re-highlight
        // to keep typing responsive. Full highlight runs on save/undo/redo.
        let rendered: Vec<RenderedLine> = self
            .buffer
            .iter()
            .map(|line_text| {
                let text = if line_text.is_empty() {
                    SharedString::from(" ".to_string())
                } else {
                    SharedString::from(line_text.clone())
                };
                RenderedLine {
                    text,
                    highlights: vec![],
                }
            })
            .collect();
        self.gutter_width = format!("{}", rendered.len()).len().max(3) as f32 * 8.4 + 16.0;
        self.rendered_lines = Rc::new(rendered);
    }

    fn recompute_highlights_full(&mut self) {
        let content = self.buffer.join("\n");
        let highlights = if let Some(lang) = &self.language {
            syntax::highlight_source(&content, lang)
        } else {
            vec![]
        };
        let rendered = Self::precompute_lines(&content, &highlights);
        self.gutter_width = format!("{}", rendered.len()).len().max(3) as f32 * 8.4 + 16.0;
        self.rendered_lines = Rc::new(rendered);
    }

    // ── Cursor helpers ──

    fn line_len(&self, row: usize) -> usize {
        self.buffer.get(row).map(|l| l.len()).unwrap_or(0)
    }

    fn clamp_col(&mut self) {
        let len = self.line_len(self.cursor_row);
        if self.cursor_col > len {
            self.cursor_col = len;
        }
        // Ensure we're at a char boundary
        let line = &self.buffer[self.cursor_row];
        while self.cursor_col > 0 && !line.is_char_boundary(self.cursor_col) {
            self.cursor_col -= 1;
        }
    }

    fn prev_char_boundary(&self, row: usize, col: usize) -> usize {
        if col == 0 {
            return 0;
        }
        let line = &self.buffer[row];
        let mut c = col - 1;
        while c > 0 && !line.is_char_boundary(c) {
            c -= 1;
        }
        c
    }

    fn next_char_boundary(&self, row: usize, col: usize) -> usize {
        let line = &self.buffer[row];
        if col >= line.len() {
            return line.len();
        }
        let mut c = col + 1;
        while c < line.len() && !line.is_char_boundary(c) {
            c += 1;
        }
        c
    }

    // ── Cursor movement ──

    fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col = self.prev_char_boundary(self.cursor_row, self.cursor_col);
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.line_len(self.cursor_row);
        }
    }

    fn move_right(&mut self) {
        let len = self.line_len(self.cursor_row);
        if self.cursor_col < len {
            self.cursor_col = self.next_char_boundary(self.cursor_row, self.cursor_col);
        } else if self.cursor_row + 1 < self.buffer.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.clamp_col();
        }
    }

    fn move_down(&mut self) {
        if self.cursor_row + 1 < self.buffer.len() {
            self.cursor_row += 1;
            self.clamp_col();
        }
    }

    fn move_to_line_start(&mut self) {
        self.cursor_col = 0;
    }

    fn move_to_line_end(&mut self) {
        self.cursor_col = self.line_len(self.cursor_row);
    }

    // ── Text editing ──

    fn insert_text(&mut self, text: &str) {
        // Handle multi-line paste
        let parts: Vec<&str> = text.split('\n').collect();
        if parts.len() > 1 {
            // Multi-line paste always gets its own undo group
            self.save_undo();
            let rest = self.buffer[self.cursor_row].split_off(self.cursor_col);
            self.buffer[self.cursor_row].push_str(parts[0]);
            for (i, part) in parts[1..parts.len() - 1].iter().enumerate() {
                self.buffer
                    .insert(self.cursor_row + 1 + i, part.to_string());
            }
            let last_part = parts[parts.len() - 1];
            let new_row = self.cursor_row + parts.len() - 1;
            let mut last_line = last_part.to_string();
            self.cursor_col = last_line.len();
            last_line.push_str(&rest);
            self.buffer.insert(new_row, last_line);
            self.cursor_row = new_row;
        } else {
            // Single-line insert — coalesce by character type
            let kind = if text.chars().all(|c| c.is_whitespace()) {
                EditKind::InsertWhitespace
            } else {
                EditKind::InsertChar
            };
            self.save_undo_coalesced(kind);
            self.buffer[self.cursor_row].insert_str(self.cursor_col, text);
            self.cursor_col += text.len();
        }
        self.dirty = true;
    }

    fn insert_newline(&mut self) {
        self.save_undo_coalesced(EditKind::InsertNewline);
        let rest = self.buffer[self.cursor_row].split_off(self.cursor_col);
        self.buffer.insert(self.cursor_row + 1, rest);
        self.cursor_row += 1;
        self.cursor_col = 0;
        self.dirty = true;
    }

    fn delete_backward(&mut self) {
        if self.cursor_col > 0 {
            self.save_undo_coalesced(EditKind::DeleteBackward);
            let prev = self.prev_char_boundary(self.cursor_row, self.cursor_col);
            self.buffer[self.cursor_row].drain(prev..self.cursor_col);
            self.cursor_col = prev;
            self.dirty = true;
        } else if self.cursor_row > 0 {
            // Joining lines is a bigger operation — force a new undo group
            self.save_undo();
            let line = self.buffer.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = self.buffer[self.cursor_row].len();
            self.buffer[self.cursor_row].push_str(&line);
            self.dirty = true;
        }
    }

    fn delete_forward(&mut self) {
        let len = self.line_len(self.cursor_row);
        if self.cursor_col < len {
            self.save_undo_coalesced(EditKind::DeleteForward);
            let next = self.next_char_boundary(self.cursor_row, self.cursor_col);
            self.buffer[self.cursor_row].drain(self.cursor_col..next);
            self.dirty = true;
        } else if self.cursor_row + 1 < self.buffer.len() {
            // Joining lines — force new undo group
            self.save_undo();
            let next_line = self.buffer.remove(self.cursor_row + 1);
            self.buffer[self.cursor_row].push_str(&next_line);
            self.dirty = true;
        }
    }

    fn insert_tab(&mut self) {
        self.insert_text("    ");
    }

    // ── Vim motions ──

    fn move_word_forward(&mut self) {
        let line = &self.buffer[self.cursor_row];
        let len = line.len();
        if self.cursor_col >= len {
            // Move to next line start
            if self.cursor_row + 1 < self.buffer.len() {
                self.cursor_row += 1;
                self.cursor_col = 0;
            }
            return;
        }
        // Skip current word chars, then skip whitespace
        let bytes = line.as_bytes();
        let mut col = self.cursor_col;
        let is_word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        if col < len && is_word_char(bytes[col]) {
            while col < len && is_word_char(bytes[col]) {
                col += 1;
            }
        } else {
            while col < len && !is_word_char(bytes[col]) && !bytes[col].is_ascii_whitespace() {
                col += 1;
            }
        }
        while col < len && bytes[col].is_ascii_whitespace() {
            col += 1;
        }
        self.cursor_col = col;
    }

    fn move_word_backward(&mut self) {
        if self.cursor_col == 0 {
            if self.cursor_row > 0 {
                self.cursor_row -= 1;
                self.cursor_col = self.line_len(self.cursor_row);
            }
            return;
        }
        let line = &self.buffer[self.cursor_row];
        let bytes = line.as_bytes();
        let mut col = self.cursor_col;
        // Skip whitespace backwards
        while col > 0 && bytes[col - 1].is_ascii_whitespace() {
            col -= 1;
        }
        let is_word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        if col > 0 && is_word_char(bytes[col - 1]) {
            while col > 0 && is_word_char(bytes[col - 1]) {
                col -= 1;
            }
        } else {
            while col > 0 && !is_word_char(bytes[col - 1]) && !bytes[col - 1].is_ascii_whitespace()
            {
                col -= 1;
            }
        }
        self.cursor_col = col;
    }

    fn move_word_end(&mut self) {
        let line = &self.buffer[self.cursor_row];
        let len = line.len();
        if self.cursor_col + 1 >= len {
            if self.cursor_row + 1 < self.buffer.len() {
                self.cursor_row += 1;
                self.cursor_col = 0;
            }
            return;
        }
        let bytes = line.as_bytes();
        let mut col = self.cursor_col + 1;
        while col < len && bytes[col].is_ascii_whitespace() {
            col += 1;
        }
        let is_word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        if col < len && is_word_char(bytes[col]) {
            while col + 1 < len && is_word_char(bytes[col + 1]) {
                col += 1;
            }
        } else {
            while col + 1 < len
                && !is_word_char(bytes[col + 1])
                && !bytes[col + 1].is_ascii_whitespace()
            {
                col += 1;
            }
        }
        self.cursor_col = col;
    }

    fn delete_line(&mut self) {
        self.save_undo();
        if self.buffer.len() > 1 {
            self.buffer.remove(self.cursor_row);
            if self.cursor_row >= self.buffer.len() {
                self.cursor_row = self.buffer.len() - 1;
            }
        } else {
            self.buffer[0].clear();
            self.cursor_col = 0;
        }
        self.clamp_col();
        self.dirty = true;
    }

    fn open_line_below(&mut self) {
        self.save_undo();
        self.buffer.insert(self.cursor_row + 1, String::new());
        self.cursor_row += 1;
        self.cursor_col = 0;
        self.dirty = true;
        self.vim_mode = VimMode::Insert;
    }

    fn open_line_above(&mut self) {
        self.save_undo();
        self.buffer.insert(self.cursor_row, String::new());
        self.cursor_col = 0;
        self.dirty = true;
        self.vim_mode = VimMode::Insert;
    }

    fn delete_char_at_cursor(&mut self) {
        let len = self.line_len(self.cursor_row);
        if self.cursor_col < len {
            self.save_undo();
            let next = self.next_char_boundary(self.cursor_row, self.cursor_col);
            self.buffer[self.cursor_row].drain(self.cursor_col..next);
            self.dirty = true;
            self.clamp_col();
        }
    }

    fn move_to_first_non_blank(&mut self) {
        let line = &self.buffer[self.cursor_row];
        self.cursor_col = line
            .bytes()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(0);
    }

    // ── Vim key handler ──

    fn handle_vim_normal(&mut self, key: &str, key_char: Option<&str>) -> bool {
        // Check for pending two-key commands
        if let Some(pending) = self.vim_pending.take() {
            match (pending, key_char.unwrap_or(key)) {
                ('d', "d") => {
                    self.delete_line();
                    self.recompute_highlights();
                    return true;
                }
                ('g', "g") => {
                    self.cursor_row = 0;
                    self.cursor_col = 0;
                    self.clamp_col();
                    return true;
                }
                _ => {
                    // Unknown combo — ignore
                    return true;
                }
            }
        }

        let ch = key_char.unwrap_or(key);

        match ch {
            // Mode switching
            "i" => {
                self.vim_mode = VimMode::Insert;
                return true;
            }
            "a" => {
                self.move_right();
                self.vim_mode = VimMode::Insert;
                return true;
            }
            "A" => {
                self.move_to_line_end();
                self.vim_mode = VimMode::Insert;
                return true;
            }
            "I" => {
                self.move_to_first_non_blank();
                self.vim_mode = VimMode::Insert;
                return true;
            }
            "o" => {
                self.open_line_below();
                self.recompute_highlights();
                return true;
            }
            "O" => {
                self.open_line_above();
                self.recompute_highlights();
                return true;
            }
            "v" => {
                self.vim_mode = VimMode::Visual;
                self.visual_anchor_row = self.cursor_row;
                self.visual_anchor_col = self.cursor_col;
                return true;
            }
            "V" => {
                self.vim_mode = VimMode::VisualLine;
                self.visual_anchor_row = self.cursor_row;
                self.visual_anchor_col = 0;
                return true;
            }

            // Movement
            "h" => {
                self.move_left();
                return true;
            }
            "j" => {
                self.move_down();
                return true;
            }
            "k" => {
                self.move_up();
                return true;
            }
            "l" => {
                self.move_right();
                return true;
            }
            "w" => {
                self.move_word_forward();
                return true;
            }
            "b" => {
                self.move_word_backward();
                return true;
            }
            "e" => {
                self.move_word_end();
                return true;
            }
            "0" => {
                self.move_to_line_start();
                return true;
            }
            "$" => {
                self.move_to_line_end();
                return true;
            }
            "^" => {
                self.move_to_first_non_blank();
                return true;
            }
            "G" => {
                self.cursor_row = self.buffer.len().saturating_sub(1);
                self.clamp_col();
                return true;
            }

            // Editing
            "x" => {
                self.delete_char_at_cursor();
                self.recompute_highlights();
                return true;
            }
            "d" => {
                self.vim_pending = Some('d');
                return true;
            }
            "g" => {
                self.vim_pending = Some('g');
                return true;
            }
            "u" => {
                self.undo();
                return true;
            }
            "p" => {
                // TODO: paste from vim register not implemented yet
                return true;
            }

            _ => {}
        }

        // Arrow keys work in normal mode too
        match key {
            "left" => {
                self.move_left();
                true
            }
            "right" => {
                self.move_right();
                true
            }
            "up" => {
                self.move_up();
                true
            }
            "down" => {
                self.move_down();
                true
            }
            _ => false,
        }
    }

    fn handle_vim_visual(&mut self, key: &str, key_char: Option<&str>) -> bool {
        let ch = key_char.unwrap_or(key);
        match ch {
            // Movement (same as normal)
            "h" | "left" => self.move_left(),
            "j" | "down" => self.move_down(),
            "k" | "up" => self.move_up(),
            "l" | "right" => self.move_right(),
            "w" => self.move_word_forward(),
            "b" => self.move_word_backward(),
            "0" => self.move_to_line_start(),
            "$" => self.move_to_line_end(),

            // Delete selection
            "d" | "x" => {
                self.delete_visual_selection();
                self.recompute_highlights();
                self.vim_mode = VimMode::Normal;
                return true;
            }

            // Exit visual mode
            _ => {
                if key == "escape" {
                    self.vim_mode = VimMode::Normal;
                    return true;
                }
                return false;
            }
        }
        true
    }

    fn handle_vim_visual_line(&mut self, key: &str, key_char: Option<&str>) -> bool {
        let ch = key_char.unwrap_or(key);
        match ch {
            // Movement — only up/down makes sense in line mode
            "j" | "down" => self.move_down(),
            "k" | "up" => self.move_up(),
            "G" => {
                self.cursor_row = self.buffer.len().saturating_sub(1);
            }

            // Delete selected lines
            "d" | "x" => {
                self.delete_visual_line_selection();
                self.recompute_highlights();
                self.vim_mode = VimMode::Normal;
                return true;
            }

            // Yank (copy) — just exit for now, placeholder
            "y" => {
                self.vim_mode = VimMode::Normal;
                return true;
            }

            // Switch to character visual
            "v" => {
                self.vim_mode = VimMode::Visual;
                return true;
            }

            _ => {
                if key == "escape" {
                    self.vim_mode = VimMode::Normal;
                    return true;
                }
                return false;
            }
        }
        true
    }

    fn delete_visual_line_selection(&mut self) {
        self.save_undo();
        let (start_row, end_row) = self.visual_line_range();
        let count = end_row - start_row + 1;

        if count >= self.buffer.len() {
            // Deleting everything — leave one empty line
            self.buffer.clear();
            self.buffer.push(String::new());
            self.cursor_row = 0;
            self.cursor_col = 0;
        } else {
            for _ in 0..count {
                self.buffer.remove(start_row);
            }
            self.cursor_row = start_row.min(self.buffer.len().saturating_sub(1));
            self.clamp_col();
        }
        self.dirty = true;
    }

    fn visual_line_range(&self) -> (usize, usize) {
        let a = self.visual_anchor_row;
        let b = self.cursor_row;
        (a.min(b), a.max(b))
    }

    fn delete_visual_selection(&mut self) {
        self.save_undo();
        let (start_row, start_col, end_row, end_col) = self.visual_range();

        if start_row == end_row {
            // Single line selection
            let end = (end_col + 1).min(self.buffer[start_row].len());
            if start_col < end {
                self.buffer[start_row].drain(start_col..end);
            }
        } else {
            // Multi-line: keep start of first line, end of last line, remove middle
            let split_pos = (end_col + 1).min(self.buffer[end_row].len());
            let tail = self.buffer[end_row].split_off(split_pos);
            self.buffer[start_row].truncate(start_col);
            self.buffer[start_row].push_str(&tail);
            // Remove intermediate lines
            for _ in start_row + 1..=end_row {
                self.buffer.remove(start_row + 1);
            }
        }

        self.cursor_row = start_row;
        self.cursor_col = start_col;
        self.clamp_col();
        self.dirty = true;
    }

    fn visual_range(&self) -> (usize, usize, usize, usize) {
        let (sr, sc) = (self.visual_anchor_row, self.visual_anchor_col);
        let (er, ec) = (self.cursor_row, self.cursor_col);
        if sr < er || (sr == er && sc <= ec) {
            (sr, sc, er, ec)
        } else {
            (er, ec, sr, sc)
        }
    }

    // ── Undo/Redo ──

    /// Force a new undo snapshot (for vim commands, paste, multi-line ops, etc.)
    fn save_undo(&mut self) {
        self.push_undo_entry();
        self.last_edit_kind = Some(EditKind::Other);
        self.last_edit_time = Instant::now();
    }

    /// Coalescing undo: only pushes a new undo entry when the edit kind changes,
    /// a time gap occurs, or a word boundary is crossed. This way, Cmd+Z undoes
    /// a whole "typing run" at once rather than one character at a time.
    fn save_undo_coalesced(&mut self, kind: EditKind) {
        let now = Instant::now();
        let should_break = match self.last_edit_kind {
            None => true, // first edit ever
            Some(prev_kind) => {
                // Always break on kind change
                if prev_kind != kind {
                    true
                // Always break on newlines (each Enter is its own undo)
                } else if kind == EditKind::InsertNewline {
                    true
                // Break if >1 second pause between keystrokes
                } else if now.duration_since(self.last_edit_time).as_millis() > 1000 {
                    true
                // Break when transitioning from non-whitespace to whitespace typing
                // (word boundary — undo deletes back to start of current word)
                } else if prev_kind == EditKind::InsertChar && kind == EditKind::InsertWhitespace {
                    true
                } else if prev_kind == EditKind::InsertWhitespace && kind == EditKind::InsertChar {
                    true
                // "Other" always breaks
                } else if kind == EditKind::Other {
                    true
                } else {
                    false
                }
            }
        };

        if should_break {
            self.push_undo_entry();
        }

        self.last_edit_kind = Some(kind);
        self.last_edit_time = now;
    }

    fn push_undo_entry(&mut self) {
        self.undo_stack.push(UndoEntry {
            buffer: self.buffer.clone(),
            cursor_row: self.cursor_row,
            cursor_col: self.cursor_col,
        });
        // Clear redo stack on new edit
        self.redo_stack.clear();
        // Limit undo stack size
        if self.undo_stack.len() > 500 {
            self.undo_stack.remove(0);
        }
    }

    fn undo(&mut self) {
        if let Some(entry) = self.undo_stack.pop() {
            self.redo_stack.push(UndoEntry {
                buffer: self.buffer.clone(),
                cursor_row: self.cursor_row,
                cursor_col: self.cursor_col,
            });
            self.buffer = entry.buffer;
            self.cursor_row = entry.cursor_row.min(self.buffer.len().saturating_sub(1));
            self.cursor_col = entry.cursor_col;
            self.clamp_col();
            self.dirty = true;
            self.last_edit_kind = None; // break coalescing
            self.recompute_highlights_full();
        }
    }

    fn redo(&mut self) {
        if let Some(entry) = self.redo_stack.pop() {
            self.undo_stack.push(UndoEntry {
                buffer: self.buffer.clone(),
                cursor_row: self.cursor_row,
                cursor_col: self.cursor_col,
            });
            self.buffer = entry.buffer;
            self.cursor_row = entry.cursor_row.min(self.buffer.len().saturating_sub(1));
            self.cursor_col = entry.cursor_col;
            self.clamp_col();
            self.dirty = true;
            self.last_edit_kind = None; // break coalescing
            self.recompute_highlights_full();
        }
    }

    // ── Extended navigation ──

    fn move_to_buffer_start(&mut self) {
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    fn move_to_buffer_end(&mut self) {
        self.cursor_row = self.buffer.len().saturating_sub(1);
        self.cursor_col = self.line_len(self.cursor_row);
    }

    fn select_all(&mut self) {
        let last_row = self.buffer.len().saturating_sub(1);
        let last_col = self.line_len(last_row);
        self.selection = Some(Selection {
            start_row: 0,
            start_col: 0,
            end_row: last_row,
            end_col: last_col,
        });
        self.cursor_row = last_row;
        self.cursor_col = last_col;
    }

    /// Get the text of the current selection (mouse or vim visual).
    fn selected_text(&self) -> Option<String> {
        let sel = self.selection?;
        let (sr, sc, er, ec) = sel.ordered();
        if sr == er {
            let end = ec.min(self.buffer[sr].len());
            let start = sc.min(end);
            Some(self.buffer[sr][start..end].to_string())
        } else {
            let mut text = String::new();
            // First line from start_col to end
            text.push_str(&self.buffer[sr][sc..]);
            // Middle lines
            for row in sr + 1..er {
                text.push('\n');
                text.push_str(&self.buffer[row]);
            }
            // Last line from 0 to end_col
            text.push('\n');
            let end = ec.min(self.buffer[er].len());
            text.push_str(&self.buffer[er][..end]);
            Some(text)
        }
    }

    fn delete_selection(&mut self) {
        if let Some(sel) = self.selection.take() {
            let (sr, sc, er, ec) = sel.ordered();
            self.save_undo();
            if sr == er {
                let end = ec.min(self.buffer[sr].len());
                let start = sc.min(end);
                self.buffer[sr].drain(start..end);
            } else {
                let end_col = ec.min(self.buffer[er].len());
                let tail = self.buffer[er].split_off(end_col);
                self.buffer[sr].truncate(sc);
                self.buffer[sr].push_str(&tail);
                for _ in sr + 1..=er {
                    self.buffer.remove(sr + 1);
                }
            }
            self.cursor_row = sr;
            self.cursor_col = sc;
            self.clamp_col();
            self.dirty = true;
            self.recompute_highlights();
        }
    }

    // ── Save ──

    fn save(&mut self) {
        let content = self.buffer.join("\n");
        if std::fs::write(&self.path, &content).is_ok() {
            self.dirty = false;
        }
        self.recompute_highlights_full();
    }

    // ── Scroll ──

    fn ensure_cursor_visible(&self) {
        self.scroll_handle.scroll_to_item(self.cursor_row, ScrollStrategy::Top);
    }

    // ── Mouse position helpers ──

    /// Convert a screen position to a (row, col) in the buffer.
    /// Uses `content_area_top` and `code_area_left` captured during render,
    /// plus the scroll handle's offset.
    fn position_to_row_col(&self, position: Point<Pixels>) -> (usize, usize) {
        let line_height: f32 = 18.0;
        let content_top = self.content_area_top.get();
        let code_left = self.code_area_left.get();

        // Compute the scroll offset from the uniform list scroll handle
        let scroll_offset_y = {
            let state = self.scroll_handle.0.borrow();
            let offset = state.base_handle.offset();
            f32::from(offset.y) // negative when scrolled down
        };

        // y position relative to the top of the content area, accounting for scroll
        let y_in_content = f32::from(position.y) - content_top - scroll_offset_y;
        let row = (y_in_content / line_height).max(0.0) as usize;
        let row = row.min(self.buffer.len().saturating_sub(1));

        // x position relative to code area
        let x_in_code = f32::from(position.x) - code_left;
        let cw = self.char_width.get();
        let col_chars = (x_in_code / cw).max(0.0) as usize;

        // Convert character count to byte offset
        let line = &self.buffer[row];
        let mut byte_col = 0;
        for (i, (byte_idx, ch)) in line.char_indices().enumerate() {
            if i >= col_chars {
                byte_col = byte_idx;
                break;
            }
            byte_col = byte_idx + ch.len_utf8();
        }
        let col = byte_col.min(line.len());

        (row, col)
    }

    /// Clear the mouse selection (e.g. when the user starts typing).
    fn clear_selection(&mut self) {
        self.selection = None;
        self.mouse_selecting = false;
    }

    // ── Word helpers ──

    /// Returns the (start_byte, end_byte) of the word at the given byte column in the given row.
    fn word_at(&self, row: usize, col: usize) -> Option<(usize, usize)> {
        let line = self.buffer.get(row)?;
        if col > line.len() || line.is_empty() {
            return None;
        }
        let bytes = line.as_bytes();
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        // If cursor is at the end of line or not on a word char, try one back
        let pos = if col >= line.len() || !is_word(bytes[col]) {
            if col > 0 && is_word(bytes[col - 1]) {
                col - 1
            } else {
                return None;
            }
        } else {
            col
        };
        let mut start = pos;
        while start > 0 && is_word(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = pos;
        while end < line.len() && is_word(bytes[end]) {
            end += 1;
        }
        if start < end {
            Some((start, end))
        } else {
            None
        }
    }

    /// Find all occurrences of `word` in the buffer. Returns (row, start_col, end_col).
    fn find_all_occurrences(&self, word: &str) -> Vec<(usize, usize, usize)> {
        let mut results = Vec::new();
        let is_word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        for (row, line) in self.buffer.iter().enumerate() {
            let mut search_from = 0;
            while let Some(pos) = line[search_from..].find(word) {
                let start = search_from + pos;
                let end = start + word.len();
                // Ensure whole-word match
                let before_ok = start == 0 || !is_word_char(line.as_bytes()[start - 1]);
                let after_ok = end >= line.len() || !is_word_char(line.as_bytes()[end]);
                if before_ok && after_ok {
                    results.push((row, start, end));
                }
                search_from = start + 1;
            }
        }
        results
    }

    /// Highlight all occurrences of the word at cursor position.
    fn highlight_word_at_cursor(&mut self) {
        if let Some((start, end)) = self.word_at(self.cursor_row, self.cursor_col) {
            let word = self.buffer[self.cursor_row][start..end].to_string();
            if word.len() >= 2 {
                let occurrences = self.find_all_occurrences(&word);
                self.highlighted_word = Some(word);
                self.highlighted_occurrences = occurrences;
            } else {
                self.highlighted_word = None;
                self.highlighted_occurrences.clear();
            }
        } else {
            self.highlighted_word = None;
            self.highlighted_occurrences.clear();
        }
    }

    fn clear_highlights(&mut self) {
        self.highlighted_word = None;
        self.highlighted_occurrences.clear();
    }

    // ── Search helpers ──

    fn find_search_matches(&mut self) {
        self.search_matches.clear();
        if self.search_query.is_empty() {
            return;
        }
        let query = &self.search_query;
        let query_lower = query.to_lowercase();
        for (row, line) in self.buffer.iter().enumerate() {
            let line_lower = line.to_lowercase();
            let mut from = 0;
            while let Some(pos) = line_lower[from..].find(&query_lower) {
                let start = from + pos;
                let end = start + query.len();
                self.search_matches.push((row, start, end));
                from = start + 1;
            }
        }
        // Clamp match index
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
        self.jump_to_search_match();
    }

    fn jump_to_search_match(&mut self) {
        if let Some(&(row, col, _)) = self.search_matches.get(self.search_match_ix) {
            self.cursor_row = row;
            self.cursor_col = col;
            self.ensure_cursor_visible();
        }
    }

    /// Find the nearest search match at or after the cursor.
    fn search_find_nearest_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        // Find the first match at or after current cursor
        for (ix, &(row, col, _)) in self.search_matches.iter().enumerate() {
            if row > self.cursor_row || (row == self.cursor_row && col >= self.cursor_col) {
                self.search_match_ix = ix;
                self.jump_to_search_match();
                return;
            }
        }
        // Wrap to first match
        self.search_match_ix = 0;
        self.jump_to_search_match();
    }

    pub fn open_search_with_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_active = true;
        self.search_query.clear();
        self.search_matches.clear();
        self.search_match_ix = 0;

        let entity = cx.entity().clone();
        let entity_cancel = cx.entity().clone();

        let input = cx.new(|cx| {
            let mut inp = crate::text_input::TextInput::new(cx);
            inp.set_placeholder("Search...");

            inp.set_on_submit(Rc::new(move |_text, _window, cx| {
                entity.update(cx, |fv, cx| {
                    fv.search_next();
                    cx.notify();
                });
            }));

            inp.set_on_cancel(Rc::new(move |window, cx| {
                entity_cancel.update(cx, |fv, cx| {
                    fv.close_search(window);
                    cx.notify();
                });
            }));

            inp
        });

        // Observe text changes
        cx.observe(&input, |fv, input, cx| {
            let new_text = input.read(cx).text.clone();
            if fv.search_query != new_text {
                fv.search_query = new_text;
                fv.find_search_matches();
                fv.search_find_nearest_match();
                cx.notify();
            }
        })
        .detach();

        self.search_input = Some(input.clone());
        input.read(cx).focus(window);
        cx.notify();
    }

    pub fn close_search(&mut self, window: &mut Window) {
        self.search_active = false;
        self.search_query.clear();
        self.search_input = None;
        self.search_matches.clear();
        self.search_match_ix = 0;
        self.focus_handle.focus(window);
    }

    /// Navigate to a specific line (1-indexed).
    pub fn navigate_to_line(&mut self, line: usize) {
        let row = line.saturating_sub(1).min(self.buffer.len().saturating_sub(1));
        self.cursor_row = row;
        self.cursor_col = 0;
        self.ensure_cursor_visible();
    }

    // ── Event handlers ──

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;

        // If search is active, Escape closes it; all other keys go to search input
        if self.search_active {
            if ks.key.as_str() == "escape" {
                self.close_search(window);
                cx.notify();
            }
            return;
        }

        // Handle Cmd+key combos we care about before passing to action system
        if ks.modifiers.platform {
            let handled = match ks.key.as_str() {
                "z" if ks.modifiers.shift => {
                    self.redo();
                    true
                }
                "z" => {
                    self.undo();
                    true
                }
                "a" => {
                    self.select_all();
                    cx.notify();
                    return;
                }
                "c" => {
                    // Copy
                    if let Some(text) = self.selected_text() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    true
                }
                "x" => {
                    // Cut
                    if let Some(text) = self.selected_text() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        self.delete_selection();
                        self.recompute_highlights();
                    }
                    true
                }
                "v" => {
                    // Paste
                    if let Some(item) = cx.read_from_clipboard() {
                        if let Some(text) = item.text() {
                            if self.selection.is_some() {
                                self.delete_selection();
                            }
                            self.insert_text(&text);
                            self.recompute_highlights();
                        }
                    }
                    true
                }
                "left" => {
                    self.move_to_line_start();
                    true
                }
                "right" => {
                    self.move_to_line_end();
                    true
                }
                "up" => {
                    self.move_to_buffer_start();
                    true
                }
                "down" => {
                    self.move_to_buffer_end();
                    true
                }
                "backspace" => {
                    // Cmd+Backspace: delete from cursor to line start
                    self.save_undo();
                    if self.selection.is_some() {
                        self.delete_selection();
                    } else if self.cursor_col > 0 {
                        self.buffer[self.cursor_row].drain(0..self.cursor_col);
                        self.cursor_col = 0;
                        self.dirty = true;
                    }
                    self.recompute_highlights();
                    true
                }
                _ => false,
            };
            if handled {
                self.ensure_cursor_visible();
                cx.notify();
            }
            // Let unhandled Cmd combos (Cmd+S, Cmd+T, etc.) pass to action system
            return;
        }

        // Handle Option+arrow for word navigation
        if ks.modifiers.alt {
            let handled = match ks.key.as_str() {
                "left" => {
                    self.move_word_backward();
                    true
                }
                "right" => {
                    self.move_word_forward();
                    true
                }
                "backspace" => {
                    // Option+Backspace: delete word backward
                    self.save_undo();
                    let start_col = self.cursor_col;
                    self.move_word_backward();
                    if self.cursor_col < start_col {
                        self.buffer[self.cursor_row].drain(self.cursor_col..start_col);
                        self.dirty = true;
                        self.recompute_highlights();
                    }
                    true
                }
                _ => false,
            };
            if handled {
                self.clear_selection();
                self.ensure_cursor_visible();
                cx.notify();
                return;
            }
        }

        // Handle Ctrl+R for redo (vim convention)
        if ks.modifiers.control && ks.key.as_str() == "r" {
            self.redo();
            self.ensure_cursor_visible();
            cx.notify();
            return;
        }

        // If there's an active selection and user types, delete selection first
        if self.selection.is_some() && !matches!(ks.key.as_str(), "left" | "right" | "up" | "down" | "home" | "end" | "escape") {
            let vim_enabled = AppSettings::vim_mode(cx);
            let is_typing = !vim_enabled || self.vim_mode == VimMode::Insert;
            if is_typing {
                self.delete_selection();
                // If it was just backspace/delete, we're done (selection was deleted)
                if matches!(ks.key.as_str(), "backspace" | "delete") {
                    self.ensure_cursor_visible();
                    cx.notify();
                    return;
                }
            }
        }

        // Clear mouse selection and word highlights when typing
        self.clear_selection();
        self.clear_highlights();

        let vim_enabled = AppSettings::vim_mode(cx);

        if vim_enabled {
            self.handle_key_down_vim(ks, cx);
        } else {
            self.handle_key_down_normal(ks);
        }

        self.ensure_cursor_visible();
        cx.notify();
    }

    fn handle_key_down_normal(&mut self, ks: &Keystroke) {
        match ks.key.as_str() {
            "left" => { self.last_edit_kind = None; self.move_left() }
            "right" => { self.last_edit_kind = None; self.move_right() }
            "up" => { self.last_edit_kind = None; self.move_up() }
            "down" => { self.last_edit_kind = None; self.move_down() }
            "home" => { self.last_edit_kind = None; self.move_to_line_start() }
            "end" => { self.last_edit_kind = None; self.move_to_line_end() }
            "enter" => {
                self.insert_newline();
                self.recompute_highlights();
            }
            "backspace" => {
                self.delete_backward();
                self.recompute_highlights();
            }
            "delete" => {
                self.delete_forward();
                self.recompute_highlights();
            }
            "tab" => {
                self.insert_tab();
                self.recompute_highlights();
            }
            _ => {
                if let Some(ch) = &ks.key_char {
                    if !ch.is_empty() {
                        self.insert_text(ch);
                        self.recompute_highlights();
                    }
                }
            }
        }
    }

    fn handle_key_down_vim(&mut self, ks: &Keystroke, cx: &App) {
        let _ = cx;
        let key = ks.key.as_str();
        let key_char = ks.key_char.as_deref();

        match self.vim_mode {
            VimMode::Normal => {
                // Escape clears pending
                if key == "escape" {
                    self.vim_pending = None;
                    return;
                }
                self.handle_vim_normal(key, key_char);
            }
            VimMode::Insert => {
                // Escape returns to normal mode
                if key == "escape" {
                    self.vim_mode = VimMode::Normal;
                    // In vim, cursor moves back one char when exiting insert
                    if self.cursor_col > 0 {
                        self.cursor_col =
                            self.prev_char_boundary(self.cursor_row, self.cursor_col);
                    }
                    return;
                }
                // Insert mode uses normal editing keys
                self.handle_key_down_normal(ks);
            }
            VimMode::Visual => {
                self.handle_vim_visual(key, key_char);
            }
            VimMode::VisualLine => {
                self.handle_vim_visual_line(key, key_char);
            }
        }
    }

    fn handle_find(&mut self, _: &FindInFile, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_active {
            self.close_search(window);
        } else {
            self.open_search_with_input(window, cx);
        }
        cx.notify();
    }

    fn handle_save(&mut self, _: &SaveFile, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.path.exists() || self.path.file_name().map(|n| n.to_string_lossy().starts_with("Untitled")).unwrap_or(false) {
            // New unsaved file — prompt for save location
            let entity = cx.entity().clone();
            let paths_rx = cx.prompt_for_new_path(&self.path, None);
            cx.spawn(async move |_this, cx| {
                if let Ok(Ok(Some(path))) = paths_rx.await {
                    let _ = cx.update(|cx| {
                        entity.update(cx, |viewer: &mut FileViewer, cx| {
                            viewer.path = path.clone();
                            viewer.language = crate::editor::syntax::detect_language(
                                path.to_str().unwrap_or(""),
                            )
                            .map(|s| s.to_string());
                            viewer.save();
                            cx.notify();
                        });
                    });
                }
            })
            .detach();
        } else {
            self.save();
            cx.notify();
        }
    }

    fn handle_click(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
        self.last_edit_kind = None; // break undo coalescing on click

        let (row, col) = self.position_to_row_col(event.position);

        // Cmd+click: go to definition / find usages
        if event.modifiers.platform {
            self.cursor_row = row;
            self.cursor_col = col;
            self.handle_cmd_click(cx);
            cx.notify();
            return;
        }

        // Double-click: select word and highlight occurrences
        if event.click_count == 2 {
            self.cursor_row = row;
            self.cursor_col = col;
            if let Some((start, end)) = self.word_at(row, col) {
                self.selection = Some(Selection {
                    start_row: row,
                    start_col: start,
                    end_row: row,
                    end_col: end,
                });
                self.cursor_col = end;
                self.highlight_word_at_cursor();
            }
            self.mouse_selecting = false;
            cx.notify();
            return;
        }

        // Single click: start mouse selection, clear highlights
        self.clear_highlights();
        self.cursor_row = row;
        self.cursor_col = col;
        self.mouse_selecting = true;
        self.selection = Some(Selection {
            start_row: row,
            start_col: col,
            end_row: row,
            end_col: col,
        });

        cx.notify();
    }

    /// Handle Cmd+click: find the definition of the word under cursor.
    /// Simple heuristic: find occurrences of the word; navigate to the first
    /// "definition-like" occurrence (fn, struct, let, const, type, impl, enum, trait, mod, pub).
    /// If already at a definition, jump to the next usage instead.
    fn handle_cmd_click(&mut self, _cx: &mut Context<Self>) {
        if let Some((start, end)) = self.word_at(self.cursor_row, self.cursor_col) {
            let word = self.buffer[self.cursor_row][start..end].to_string();
            if word.is_empty() {
                return;
            }
            let occurrences = self.find_all_occurrences(&word);
            if occurrences.is_empty() {
                return;
            }

            // Find definition-like patterns
            let def_keywords = [
                "fn ", "struct ", "enum ", "trait ", "type ", "const ", "static ",
                "let ", "let mut ", "mod ", "pub fn ", "pub struct ", "pub enum ",
                "pub trait ", "pub type ", "pub const ", "pub static ", "pub mod ",
                "class ", "def ", "function ", "var ", "const ", "interface ",
            ];

            // Check if any occurrence is preceded by a definition keyword
            let mut def_ix: Option<usize> = None;
            for (ix, &(row, col_start, _col_end)) in occurrences.iter().enumerate() {
                let line = &self.buffer[row];
                let prefix = &line[..col_start];
                let trimmed = prefix.trim_end();
                for kw in &def_keywords {
                    if trimmed.ends_with(kw.trim()) {
                        def_ix = Some(ix);
                        break;
                    }
                }
                if def_ix.is_some() {
                    break;
                }
            }

            // If we're on the definition, jump to first usage; otherwise jump to definition
            let current_ix = occurrences
                .iter()
                .position(|&(r, s, _)| r == self.cursor_row && s == start);

            let target = if let Some(di) = def_ix {
                if current_ix == Some(di) {
                    // On definition — jump to first usage (next occurrence)
                    let next = (di + 1) % occurrences.len();
                    occurrences[next]
                } else {
                    // Jump to definition
                    occurrences[di]
                }
            } else {
                // No definition found — jump to first occurrence that isn't current
                if let Some(ci) = current_ix {
                    let next = (ci + 1) % occurrences.len();
                    occurrences[next]
                } else {
                    occurrences[0]
                }
            };

            self.cursor_row = target.0;
            self.cursor_col = target.1;
            self.selection = Some(Selection {
                start_row: target.0,
                start_col: target.1,
                end_row: target.0,
                end_col: target.2,
            });
            self.highlighted_word = Some(word);
            self.highlighted_occurrences = occurrences;
            self.ensure_cursor_visible();
        }
    }

    fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.mouse_selecting {
            return;
        }

        let (row, col) = self.position_to_row_col(event.position);
        self.cursor_row = row;
        self.cursor_col = col;

        if let Some(ref mut sel) = self.selection {
            sel.end_row = row;
            sel.end_col = col;
        }

        self.ensure_cursor_visible();
        cx.notify();
    }

    fn handle_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.mouse_selecting {
            return;
        }

        let (row, col) = self.position_to_row_col(event.position);
        self.cursor_row = row;
        self.cursor_col = col;

        if let Some(ref mut sel) = self.selection {
            sel.end_row = row;
            sel.end_col = col;
        }

        self.mouse_selecting = false;

        // If the selection is empty (click without drag), clear it
        if let Some(sel) = &self.selection {
            if sel.start_row == sel.end_row && sel.start_col == sel.end_col {
                self.selection = None;
            }
        }

        cx.notify();
    }

    // ── Rendering ──

    /// Convert a byte offset within a line to a character (grapheme) count.
    /// This is used to position the cursor overlay: left = char_count * char_width.
    fn byte_col_to_char_count(line: &str, byte_col: usize) -> usize {
        line[..byte_col.min(line.len())].chars().count()
    }

    fn render_line(
        line: &RenderedLine,
        line_num: usize,
        gutter_width: f32,
        cursor_col: Option<usize>,
        block_cursor: bool,
        code_area_left: Rc<Cell<f32>>,
        char_width: Rc<Cell<f32>>,
        selection_cols: Option<(usize, usize)>,
        line_text: &str,
        // Word occurrence highlights for this line: (start_col, end_col) pairs
        word_highlights: &[(usize, usize)],
        // Search match highlights for this line: (start_col, end_col, is_active) tuples
        search_highlights: &[(usize, usize, bool)],
    ) -> Div {
        let cw = char_width.get();
        let mut line_el = div().flex().flex_row().h(px(18.0));

        // Gutter
        line_el = line_el.child(
            div()
                .w(px(gutter_width))
                .h(px(18.0))
                .text_color(rgb(0x585b70))
                .flex_shrink_0()
                .pr_2()
                .text_right()
                .child(format!("{}", line_num)),
        );

        // Code content area — render text as one unbroken element,
        // then overlay cursor and selection as absolutely positioned elements.
        // This is the Zed approach: text doesn't get split, cursor is painted on top.
        let mut code_wrapper = div().relative().flex_1();

        // Canvas to capture the left edge of the code area in window coordinates
        code_wrapper = code_wrapper.child(
            canvas(
                move |bounds, _window, _cx| {
                    code_area_left.set(f32::from(bounds.origin.x));
                },
                |_, _, _, _| {},
            )
            .size_0(),
        );

        // Render the full line text as a single element (never split)
        let text_el: AnyElement = if line.highlights.is_empty() {
            div()
                .text_color(colors::text())
                .child(line.text.clone())
                .into_any_element()
        } else {
            let highlights: Vec<(Range<usize>, HighlightStyle)> = line
                .highlights
                .iter()
                .map(|h| {
                    (
                        h.range.clone(),
                        HighlightStyle {
                            color: Some(h.color),
                            ..Default::default()
                        },
                    )
                })
                .collect();
            let styled = StyledText::new(line.text.clone()).with_highlights(highlights);
            div()
                .text_color(colors::text())
                .child(styled)
                .into_any_element()
        };

        code_wrapper = code_wrapper.child(text_el);

        // Word occurrence highlights (subtle underline/background)
        for &(wh_start, wh_end) in word_highlights {
            let start_char = Self::byte_col_to_char_count(line_text, wh_start) as f32;
            let end_char = Self::byte_col_to_char_count(line_text, wh_end) as f32;
            let w = (end_char - start_char).max(0.5);
            code_wrapper = code_wrapper.child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(start_char * cw))
                    .w(px(w * cw))
                    .h(px(18.0))
                    .bg(gpui::rgba(0x89b4fa20)) // subtle blue tint
                    .border_b_1()
                    .border_color(gpui::rgba(0x89b4fa66)),
            );
        }

        // Search match highlights
        for &(sh_start, sh_end, is_active) in search_highlights {
            let start_char = Self::byte_col_to_char_count(line_text, sh_start) as f32;
            let end_char = Self::byte_col_to_char_count(line_text, sh_end) as f32;
            let w = (end_char - start_char).max(0.5);
            let bg = if is_active {
                gpui::rgba(0xf9e2af66) // bright yellow for active match
            } else {
                gpui::rgba(0xf9e2af33) // dimmer yellow for other matches
            };
            code_wrapper = code_wrapper.child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(start_char * cw))
                    .w(px(w * cw))
                    .h(px(18.0))
                    .bg(bg),
            );
        }

        // Selection highlight overlay (painted behind cursor, on top of text)
        if let Some((sel_start_byte, sel_end_byte)) = selection_cols {
            let sel_start_char = Self::byte_col_to_char_count(line_text, sel_start_byte) as f32;
            let sel_end_char = Self::byte_col_to_char_count(line_text, sel_end_byte) as f32;
            let sel_width = (sel_end_char - sel_start_char).max(0.5); // at least half-char for empty lines
            code_wrapper = code_wrapper.child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(sel_start_char * cw))
                    .w(px(sel_width * cw))
                    .h(px(18.0))
                    .bg(gpui::rgba(0x264f7844)),
            );
        }

        // Cursor overlay (painted on top of everything)
        if let Some(cursor_byte) = cursor_col {
            let cursor_char = Self::byte_col_to_char_count(line_text, cursor_byte) as f32;
            let cursor_left = cursor_char * cw;
            if block_cursor {
                code_wrapper = code_wrapper.child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(cursor_left))
                        .w(px(cw))
                        .h(px(18.0))
                        .bg(colors::accent())
                        .opacity(0.4),
                );
            } else {
                code_wrapper = code_wrapper.child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(cursor_left))
                        .w(px(2.0))
                        .h(px(18.0))
                        .bg(colors::accent()),
                );
            }
        }

        line_el = line_el.child(code_wrapper);
        line_el
    }
}

impl Render for FileViewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let line_count = self.rendered_lines.len();
        let gutter_width = self.gutter_width;
        let lines = self.rendered_lines.clone();
        let cursor_row = self.cursor_row;
        let cursor_col = self.cursor_col;
        let vim_enabled = AppSettings::vim_mode(cx);
        let vim_mode = self.vim_mode;
        // Use block cursor in vim normal/visual mode, line cursor in insert/non-vim
        let block_cursor = vim_enabled && vim_mode != VimMode::Insert;

        // Compute effective selection: mouse selection OR vim visual selection
        let selection = if let Some(sel) = self.selection {
            Some(sel)
        } else if vim_enabled && (vim_mode == VimMode::Visual || vim_mode == VimMode::VisualLine) {
            if vim_mode == VimMode::VisualLine {
                let (sr, er) = if self.visual_anchor_row <= self.cursor_row {
                    (self.visual_anchor_row, self.cursor_row)
                } else {
                    (self.cursor_row, self.visual_anchor_row)
                };
                Some(Selection {
                    start_row: sr,
                    start_col: 0,
                    end_row: er,
                    end_col: self.buffer.get(er).map(|l| l.len()).unwrap_or(0),
                })
            } else {
                // Character visual mode
                Some(Selection {
                    start_row: self.visual_anchor_row,
                    start_col: self.visual_anchor_col,
                    end_row: self.cursor_row,
                    end_col: self.cursor_col,
                })
            }
        } else {
            None
        };
        // Capture buffer line lengths and text for selection range computation and cursor positioning
        let buffer_line_lens: Vec<usize> = self.buffer.iter().map(|l| l.len()).collect();
        let buffer_lines: Vec<String> = self.buffer.clone();

        // Capture word highlights and search matches as Rc for the closure
        let word_occs: Rc<Vec<(usize, usize, usize)>> =
            Rc::new(self.highlighted_occurrences.clone());
        let search_matches: Rc<Vec<(usize, usize, usize)>> =
            Rc::new(self.search_matches.clone());
        let active_search_ix = self.search_match_ix;
        let search_active = self.search_active;

        // Title with dirty indicator
        let path_display = if self.dirty {
            format!("{} (modified)", self.path.to_string_lossy())
        } else {
            self.path.to_string_lossy().to_string()
        };

        let mut root = div()
            .id("file-editor")
            .flex()
            .flex_col()
            .size_full()
            .bg(colors::bg())
            .font_family("Menlo")
            .text_sm()
            .track_focus(&self.focus_handle)
            .key_context("FileEditor")
            .on_action(cx.listener(Self::handle_save))
            .on_action(cx.listener(Self::handle_find))
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::handle_click))
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            // File header
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_1()
                    .bg(colors::surface())
                    .border_b_1()
                    .border_color(colors::border())
                    .text_color(colors::text_muted())
                    .text_xs()
                    .child(path_display),
            );

        // Search bar (Cmd+F) — uses real TextInput entity
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

                root = root.child(
                    div()
                        .id("search-bar")
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
                        .child(
                            div()
                                .flex_1()
                                .child(search_input.clone()),
                        )
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

        // Virtual scrolling editor lines with click-to-position
        root = root.child({
                let code_left = self.code_area_left.clone();
                let content_top = self.content_area_top.clone();
                let char_width = self.char_width.clone();
                div()
                    .relative()
                    .flex_1()
                    .size_full()
                    // Canvas to capture the top of the uniform list content area
                    .child(
                        canvas(
                            move |bounds, _window, _cx| {
                                content_top.set(f32::from(bounds.origin.y));
                            },
                            |_, _, _, _| {},
                        )
                        .size_0(),
                    )
                    .child(
                        uniform_list("file-lines", line_count, move |range, _window, _cx| {
                            range
                                .map(|ix| {
                                    let cursor_on_line = if ix == cursor_row {
                                        Some(cursor_col)
                                    } else {
                                        None
                                    };

                                    // Compute selection highlight for this line
                                    let sel_cols = selection.as_ref().and_then(|sel| {
                                        let line_len = buffer_line_lens.get(ix).copied().unwrap_or(0);
                                        sel.col_range_for_row(ix, line_len)
                                    });

                                    let line_text = buffer_lines.get(ix).map(|s| s.as_str()).unwrap_or("");

                                    // Gather word occurrence highlights for this line
                                    let wh: Vec<(usize, usize)> = word_occs
                                        .iter()
                                        .filter(|&&(r, _, _)| r == ix)
                                        .map(|&(_, s, e)| (s, e))
                                        .collect();

                                    // Gather search match highlights for this line
                                    let sh: Vec<(usize, usize, bool)> = if search_active {
                                        search_matches
                                            .iter()
                                            .enumerate()
                                            .filter(|&(_, &(r, _, _))| r == ix)
                                            .map(|(mi, &(_, s, e))| (s, e, mi == active_search_ix))
                                            .collect()
                                    } else {
                                        vec![]
                                    };

                                    let line_div = Self::render_line(
                                        &lines[ix],
                                        ix + 1,
                                        gutter_width,
                                        cursor_on_line,
                                        block_cursor,
                                        code_left.clone(),
                                        char_width.clone(),
                                        sel_cols,
                                        line_text,
                                        &wh,
                                        &sh,
                                    );

                                    div()
                                        .id(ElementId::Name(format!("editor-line-{ix}").into()))
                                        .cursor_text()
                                        .child(line_div)
                                        .into_any_element()
                                })
                                .collect()
                        })
                        .flex_1()
                        .size_full()
                        .track_scroll(self.scroll_handle.clone())
                    )
            });

        // Vim status bar at the bottom
        if vim_enabled {
            let mode_color = match vim_mode {
                VimMode::Normal => colors::accent(),
                VimMode::Insert => rgb(0xa6e3a1), // green
                VimMode::Visual => rgb(0xf5c2e7), // pink
                VimMode::VisualLine => rgb(0xf5c2e7), // pink (same as visual)
            };
            let mode_text_color = rgb(0x1e1e2e); // dark

            root = root.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(22.0))
                    .border_t_1()
                    .border_color(colors::border())
                    .bg(colors::surface())
                    .child(
                        div()
                            .px_2()
                            .py(px(1.0))
                            .bg(mode_color)
                            .text_color(mode_text_color)
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .child(vim_mode.label().to_string()),
                    )
                    .child(
                        div()
                            .px_2()
                            .text_xs()
                            .text_color(colors::text_muted())
                            .child(format!(
                                "{}:{}",
                                self.cursor_row + 1,
                                self.cursor_col + 1
                            )),
                    ),
            );
        }

        root
    }
}
