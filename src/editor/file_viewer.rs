use gpui::*;
use ignore::WalkBuilder;
use std::cell::Cell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use crate::actions::{FindInFile, SaveFile};
use crate::editor::syntax::{self, HighlightSpan};
use crate::settings::AppSettings;
use crate::theme::colors;
use crate::ui::scrollbar::{self, ScrollbarState};

// ── Pre-computed highlight data for rendering ──

struct LineHighlight {
    range: Range<usize>,
    color: Hsla,
}

struct RenderedLine {
    text: SharedString,
    highlights: Vec<LineHighlight>,
    /// The buffer row this visual line belongs to.
    buffer_row: usize,
    /// Byte offset within the buffer line where this visual line starts.
    byte_offset: usize,
}

// ── Default character width for monospace cursor positioning ──
// Menlo at text_sm (14px) on macOS. Overridden dynamically if measured.
const DEFAULT_CHAR_WIDTH: f32 = 8.4;
/// Soft-wrap width in characters for markdown files.
const MARKDOWN_WRAP_CHARS: usize = 100;

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

/// A single reference to a symbol, possibly in another file.
#[derive(Clone, Debug)]
struct FileReference {
    path: PathBuf,
    row: usize,       // 0-based line index
    col_start: usize,  // byte offset
    col_end: usize,    // byte offset (exclusive)
    line_text: String,  // the full line content (for display)
}

/// An inline "Find All References" panel shown when Cmd+clicking a definition.
#[derive(Clone)]
struct ReferencesPanel {
    /// The word being referenced.
    word: String,
    /// The row the panel is anchored to (the definition line).
    anchor_row: usize,
    /// All references grouped by file path (sorted: current file first, then alphabetical).
    groups: Vec<(PathBuf, Vec<FileReference>)>,
    /// Flat index into all references for keyboard navigation.
    selected_ix: usize,
    /// Total number of references.
    total_refs: usize,
}

/// Event emitted by FileViewer when it wants to navigate to a different file.
pub enum FileViewerEvent {
    OpenFile { path: PathBuf, line: usize, col_start: usize, col_end: usize },
}

impl EventEmitter<FileViewerEvent> for FileViewer {}

pub struct FileViewer {
    pub path: PathBuf,
    pub language: Option<String>,
    /// Workspace root for cross-file search. Set by the parent after construction.
    pub workspace_root: Option<PathBuf>,
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

    // Inline references panel (Cmd+Click on definition)
    references_panel: Option<ReferencesPanel>,

    // Navigation highlight: flash the target word when jumping to a reference.
    // (row, col_start_byte, col_end_byte)
    nav_highlight: Option<(usize, usize, usize)>,

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
    /// Width of the code content area in pixels, captured during paint.
    code_area_width: Rc<Cell<f32>>,
    /// Current wrap width in characters used for markdown soft-wrapping.
    current_wrap_chars: usize,

    // Scrollbar state (auto-hide)
    scrollbar: ScrollbarState,
    /// Last seen scroll offset, used to detect scroll events and bump
    /// scrollbar visibility.
    last_scroll_offset: f32,

    /// Watches the open file's parent directory and reloads the buffer when
    /// the file is changed by an external tool. Held to keep the watcher
    /// alive — dropped when the FileViewer drops.
    _file_watcher: Option<Task<()>>,
}

impl FileViewer {
    pub fn open(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let language = syntax::detect_language(path.to_str().unwrap_or(""))
            .map(|s| s.to_string());

        let file_watcher = Self::spawn_file_watcher(path.clone(), cx);

        // Heavy work — file I/O, tree-sitter highlighting, per-line allocation,
        // and render precomputation — runs on the background pool. The viewer
        // returns an empty placeholder immediately and gets populated via
        // `apply_loaded_buffer` once the load completes; without this, opening
        // a multi-MB source file would block the foreground thread for seconds.
        let load_path = path.clone();
        let load_lang = language.clone();
        cx.spawn(async move |this, cx| {
            let (buffer, rendered) = cx
                .background_executor()
                .spawn(async move {
                    Self::compute_buffer_and_lines(
                        &load_path,
                        load_lang.as_deref(),
                        MARKDOWN_WRAP_CHARS,
                    )
                })
                .await;
            let _ = this.update(cx, |viewer, cx| {
                viewer.apply_loaded_buffer(buffer, rendered, cx);
            });
        })
        .detach();

        Self {
            path,
            language,
            workspace_root: None,
            dirty: false,
            buffer: vec![String::new()],
            rendered_lines: Rc::new(vec![RenderedLine {
                text: SharedString::from(" ".to_string()),
                highlights: vec![],
                buffer_row: 0,
                byte_offset: 0,
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
            references_panel: None,
            nav_highlight: None,
            search_active: false,
            search_query: String::new(),
            search_input: None,
            search_matches: Vec::new(),
            search_match_ix: 0,
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            // Placeholder gutter for the 1-line empty buffer; recomputed in
            // `apply_loaded_buffer` once the real content arrives.
            gutter_width: 3.0 * DEFAULT_CHAR_WIDTH + 16.0,
            code_area_left: Rc::new(Cell::new(0.0)),
            content_area_top: Rc::new(Cell::new(0.0)),
            char_width: Rc::new(Cell::new(DEFAULT_CHAR_WIDTH)),
            code_area_width: Rc::new(Cell::new(0.0)),
            current_wrap_chars: MARKDOWN_WRAP_CHARS,
            scrollbar: ScrollbarState::default(),
            last_scroll_offset: 0.0,
            _file_watcher: file_watcher,
        }
    }

    pub fn new_empty(path: PathBuf, cx: &mut Context<Self>) -> Self {
        Self {
            path,
            language: None,
            workspace_root: None,
            dirty: true,
            buffer: vec![String::new()],
            rendered_lines: Rc::new(vec![RenderedLine {
                text: SharedString::from(" ".to_string()),
                highlights: vec![],
                buffer_row: 0,
                byte_offset: 0,
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
            references_panel: None,
            nav_highlight: None,
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
            code_area_width: Rc::new(Cell::new(0.0)),
            current_wrap_chars: MARKDOWN_WRAP_CHARS,
            scrollbar: ScrollbarState::default(),
            last_scroll_offset: 0.0,
            // No watcher for new_empty: the path doesn't exist on disk yet,
            // so there's nothing meaningful to watch until the first save.
            _file_watcher: None,
        }
    }

    /// Spawn a background watcher that reloads the buffer when the file on
    /// disk changes (e.g. an external tool or another editor writes to it).
    /// We watch the file's parent directory rather than the file itself
    /// because many tools save via atomic-replace (write-temp + rename),
    /// which invalidates a watch on the original inode.
    fn spawn_file_watcher(path: PathBuf, cx: &mut Context<Self>) -> Option<Task<()>> {
        let parent = path.parent()?.to_path_buf();
        let target = path.clone();

        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = match notify::recommended_watcher(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    use notify::EventKind;
                    match event.kind {
                        EventKind::Create(_)
                        | EventKind::Modify(_)
                        | EventKind::Remove(_) => {
                            if event.paths.iter().any(|p| p == &target) {
                                let _ = tx.send(());
                            }
                        }
                        _ => {}
                    }
                }
            },
        ) {
            Ok(w) => w,
            Err(_) => return None,
        };

        use notify::Watcher;
        if watcher
            .watch(&parent, notify::RecursiveMode::NonRecursive)
            .is_err()
        {
            return None;
        }

        let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
        Some(cx.spawn(async move |this, cx| {
            // Hold the watcher alive for the lifetime of this task.
            let _watcher = watcher;
            loop {
                let rx_clone = rx.clone();
                let got_event = cx
                    .background_executor()
                    .spawn(async move {
                        let rx = rx_clone.lock().unwrap();
                        if rx.recv().is_err() {
                            return false;
                        }
                        // Coalesce burst events (e.g. atomic-rename emits
                        // create+remove+modify in quick succession).
                        while rx.try_recv().is_ok() {}
                        true
                    })
                    .await;
                if !got_event {
                    break;
                }

                // Brief settle so the writer finishes before we read.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(75))
                    .await;

                // Snapshot the language and wrap setting from the live viewer
                // so the background load uses current values. Bails if the
                // viewer was dropped while we were waiting.
                let Ok((load_path, load_lang, wrap_chars)) = this.update(cx, |v, _| {
                    (v.path.clone(), v.language.clone(), v.current_wrap_chars)
                }) else {
                    break;
                };

                let (buffer, rendered) = cx
                    .background_executor()
                    .spawn(async move {
                        Self::compute_buffer_and_lines(
                            &load_path,
                            load_lang.as_deref(),
                            wrap_chars,
                        )
                    })
                    .await;

                let r = this.update(cx, |viewer, cx| {
                    viewer.apply_loaded_buffer(buffer, rendered, cx);
                });
                if r.is_err() {
                    break;
                }
            }
        }))
    }

    /// Reads the file and computes the line buffer plus precomputed render
    /// lines. Pure and `Send` so it can run on the background executor —
    /// keeps the foreground thread responsive when opening or reloading
    /// large files (the tree-sitter pass alone can take seconds on multi-MB
    /// sources).
    fn compute_buffer_and_lines(
        path: &Path,
        language: Option<&str>,
        wrap_chars: usize,
    ) -> (Vec<String>, Vec<RenderedLine>) {
        let content =
            std::fs::read_to_string(path).unwrap_or_else(|e| format!("Error: {}", e));
        let highlights = match language {
            Some(lang) => syntax::highlight_source(&content, lang),
            None => vec![],
        };
        let mut buffer: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        if buffer.is_empty() {
            buffer.push(String::new());
        }
        let is_markdown = language == Some("markdown");
        let rendered = Self::precompute_lines(&content, &highlights, is_markdown, wrap_chars);
        (buffer, rendered)
    }

    /// Apply a freshly-loaded buffer + render lines to this viewer. Shared
    /// between the initial async load in `open` and watcher-driven reloads.
    /// Skips when the buffer has unsaved changes (don't clobber user edits)
    /// or when the new buffer matches the current one (avoids rerender
    /// after our own `save()` round-trips through the watcher).
    fn apply_loaded_buffer(
        &mut self,
        buffer: Vec<String>,
        rendered_lines: Vec<RenderedLine>,
        cx: &mut Context<Self>,
    ) {
        if self.dirty {
            return;
        }
        if buffer == self.buffer {
            return;
        }

        self.buffer = buffer;
        self.rendered_lines = Rc::new(rendered_lines);
        self.gutter_width =
            format!("{}", self.buffer.len()).len().max(3) as f32 * self.char_width.get() + 16.0;

        // Clamp cursor to the new bounds.
        if self.cursor_row >= self.buffer.len() {
            self.cursor_row = self.buffer.len().saturating_sub(1);
        }
        if let Some(line) = self.buffer.get(self.cursor_row) {
            if self.cursor_col > line.len() {
                self.cursor_col = line.len();
            }
        }

        // The buffer changed underneath; old undo entries reference indices
        // that may no longer exist, so reset history and transient state.
        self.selection = None;
        self.search_matches.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        cx.notify();
    }

    // ── Highlight precomputation ──

    fn precompute_lines(
        content: &str,
        highlights: &[HighlightSpan],
        wrap: bool,
        wrap_chars: usize,
    ) -> Vec<RenderedLine> {
        let mut result = Vec::new();
        let mut line_start = 0;
        let mut buffer_row: usize = 0;

        // Iterate with `lines()` to match how `buffer` is built (also `lines()`).
        // `split('\n')` would emit a phantom trailing empty entry for content
        // ending in '\n', whose `buffer_row` would be out of bounds for `buffer`.
        for line_text in content.lines() {
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

            if wrap && line_text.chars().count() > wrap_chars {
                // Soft-wrap this line into visual sub-lines at word boundaries
                let chunks = Self::soft_wrap_line(line_text, wrap_chars);
                for (chunk_text, byte_offset) in chunks {
                    let chunk_end = byte_offset + chunk_text.len();
                    // Slice highlights that overlap this chunk, rebasing offsets
                    let chunk_hl: Vec<LineHighlight> = line_highlights
                        .iter()
                        .filter(|h| h.range.start < chunk_end && h.range.end > byte_offset)
                        .map(|h| LineHighlight {
                            range: h.range.start.saturating_sub(byte_offset)
                                ..h.range.end.min(chunk_end) - byte_offset,
                            color: h.color,
                        })
                        .collect();

                    let text = if chunk_text.is_empty() {
                        SharedString::from(" ".to_string())
                    } else {
                        SharedString::from(chunk_text.to_string())
                    };
                    result.push(RenderedLine {
                        text,
                        highlights: chunk_hl,
                        buffer_row,
                        byte_offset,
                    });
                }
            } else {
                let text = if line_text.is_empty() {
                    SharedString::from(" ".to_string())
                } else {
                    SharedString::from(line_text.to_string())
                };

                result.push(RenderedLine {
                    text,
                    highlights: line_highlights,
                    buffer_row,
                    byte_offset: 0,
                });
            }

            buffer_row += 1;
            line_start = line_byte_end;
            if content.as_bytes().get(line_start) == Some(&b'\r') {
                line_start += 1;
            }
            if content.as_bytes().get(line_start) == Some(&b'\n') {
                line_start += 1;
            }
        }

        // Mirror the empty-content guard in `buffer` (which becomes `[String::new()]`)
        // so `rendered_lines.len() >= buffer.len()` always holds.
        if result.is_empty() {
            result.push(RenderedLine {
                text: SharedString::from(" ".to_string()),
                highlights: vec![],
                buffer_row: 0,
                byte_offset: 0,
            });
        }

        result
    }

    /// Split a line into chunks at word boundaries, each at most `max_chars` characters.
    /// Returns (chunk_text, byte_offset_in_line) pairs.
    fn soft_wrap_line(line: &str, max_chars: usize) -> Vec<(&str, usize)> {
        let mut chunks = Vec::new();
        let mut remaining = line;
        let mut byte_offset: usize = 0;

        while remaining.chars().count() > max_chars {
            // Find the last space within the limit
            let char_end: usize = remaining
                .char_indices()
                .nth(max_chars)
                .map(|(i, _)| i)
                .unwrap_or(remaining.len());

            let break_at = remaining[..char_end]
                .rfind(' ')
                .map(|i| i + 1) // include the space in the first chunk
                .unwrap_or(char_end); // no space found, hard break at max_chars

            chunks.push((&remaining[..break_at], byte_offset));
            byte_offset += break_at;
            remaining = &remaining[break_at..];
        }

        if !remaining.is_empty() || chunks.is_empty() {
            chunks.push((remaining, byte_offset));
        }

        chunks
    }

    fn recompute_highlights(&mut self) {
        // Rebuild rendered_lines from buffer without full syntax re-highlight
        // to keep typing responsive. Full highlight runs on save/undo/redo.
        let is_markdown = self.language.as_deref() == Some("markdown");
        let wrap_chars = self.current_wrap_chars;
        let mut rendered: Vec<RenderedLine> = Vec::new();
        for (buffer_row, line_text) in self.buffer.iter().enumerate() {
            if is_markdown && line_text.chars().count() > wrap_chars {
                let chunks = Self::soft_wrap_line(line_text, wrap_chars);
                for (chunk, byte_offset) in chunks {
                    let text = if chunk.is_empty() {
                        SharedString::from(" ".to_string())
                    } else {
                        SharedString::from(chunk.to_string())
                    };
                    rendered.push(RenderedLine {
                        text,
                        highlights: vec![],
                        buffer_row,
                        byte_offset,
                    });
                }
            } else {
                let text = if line_text.is_empty() {
                    SharedString::from(" ".to_string())
                } else {
                    SharedString::from(line_text.clone())
                };
                rendered.push(RenderedLine {
                    text,
                    highlights: vec![],
                    buffer_row,
                    byte_offset: 0,
                });
            }
        }
        self.gutter_width = format!("{}", self.buffer.len()).len().max(3) as f32 * self.char_width.get() + 16.0;
        self.rendered_lines = Rc::new(rendered);
    }

    fn recompute_highlights_full(&mut self) {
        let content = self.buffer.join("\n");
        let highlights = if let Some(lang) = &self.language {
            syntax::highlight_source(&content, lang)
        } else {
            vec![]
        };
        let is_markdown = self.language.as_deref() == Some("markdown");
        let rendered = Self::precompute_lines(&content, &highlights, is_markdown, self.current_wrap_chars);
        self.gutter_width = format!("{}", self.buffer.len()).len().max(3) as f32 * self.char_width.get() + 16.0;
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

    // ── Line comments ──

    /// Returns the line-comment token for this file's language, if any.
    /// Keyed off the file extension so it works even when the highlighter
    /// falls back (e.g. `.toml` highlights as rust but uses `#` for comments).
    fn line_comment_prefix(&self) -> Option<&'static str> {
        let ext = self.path.extension()?.to_str()?;
        Some(match ext {
            "rs" | "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx"
            | "go" | "swift" | "kt" | "kts" | "java" | "c" | "cc" | "cpp" | "cxx"
            | "h" | "hh" | "hpp" | "cs" | "scala" | "dart" | "groovy" => "//",
            "py" | "pyi" | "toml" | "sh" | "bash" | "zsh" | "fish" | "rb"
            | "yaml" | "yml" | "conf" | "ini" | "r" | "pl" | "elixir" | "ex"
            | "exs" | "nix" | "dockerfile" | "makefile" | "mk" => "#",
            "lua" | "sql" | "hs" | "elm" => "--",
            _ => return None,
        })
    }

    /// Toggle line comments on the current line or all lines intersecting the
    /// active selection. Comments are inserted at the minimum indent across
    /// non-blank lines, which matches VSCode/Zed behavior.
    fn toggle_line_comment(&mut self) {
        let Some(prefix) = self.line_comment_prefix() else {
            return;
        };

        let (start_row, end_row) = if let Some(sel) = self.selection {
            let (sr, _, er, ec) = sel.ordered();
            // Exclude the trailing row when the selection ends at column 0 of a
            // later row — the caret is visually on that row but no content is
            // actually selected on it.
            let end = if er > sr && ec == 0 { er - 1 } else { er };
            (sr, end)
        } else {
            (self.cursor_row, self.cursor_row)
        };

        let end_row = end_row.min(self.buffer.len().saturating_sub(1));
        if start_row > end_row {
            return;
        }

        let is_blank = |line: &str| line.trim().is_empty();

        let min_indent = (start_row..=end_row)
            .filter_map(|r| {
                let line = &self.buffer[r];
                if is_blank(line) {
                    None
                } else {
                    Some(line.len() - line.trim_start().len())
                }
            })
            .min();

        let Some(min_indent) = min_indent else {
            return; // all lines blank — nothing to toggle
        };

        let all_commented = (start_row..=end_row).all(|r| {
            let line = &self.buffer[r];
            if is_blank(line) {
                return true;
            }
            line.get(min_indent..)
                .is_some_and(|rest| rest.starts_with(prefix))
        });

        self.save_undo();

        for r in start_row..=end_row {
            let line = &mut self.buffer[r];
            if is_blank(line) {
                continue;
            }
            if all_commented {
                let after = min_indent + prefix.len();
                let drop_space = line.as_bytes().get(after) == Some(&b' ');
                let remove_end = if drop_space { after + 1 } else { after };
                line.drain(min_indent..remove_end);
            } else {
                let insertion = format!("{} ", prefix);
                line.insert_str(min_indent, &insertion);
            }
        }

        self.clamp_col();
        self.dirty = true;
        self.recompute_highlights_full();
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

    // ── Visual ↔ Buffer line mapping ──

    /// Find the first visual line index for a given buffer row.
    fn buffer_row_to_visual(&self, buffer_row: usize) -> usize {
        self.rendered_lines
            .iter()
            .position(|rl| rl.buffer_row == buffer_row)
            .unwrap_or(buffer_row)
    }

    /// Convert a visual line index to (buffer_row, byte_offset_in_line).
    fn visual_to_buffer(&self, visual_ix: usize) -> (usize, usize) {
        self.rendered_lines
            .get(visual_ix)
            .map(|rl| (rl.buffer_row, rl.byte_offset))
            .unwrap_or((visual_ix, 0))
    }

    // ── Scroll ──

    fn ensure_cursor_visible(&self) {
        let visual_row = self.buffer_row_to_visual(self.cursor_row);
        self.scroll_handle.scroll_to_item(visual_row, ScrollStrategy::Top);
    }

    /// Make the scrollbar visible and (re)schedule it to fade out after
    /// the idle timeout. Called whenever the user scrolls or drags.
    fn bump_scrollbar(&mut self, cx: &mut Context<Self>) {
        self.scrollbar.visible = true;
        self.scrollbar.hide_task =
            Some(scrollbar::schedule_hide(cx, |v: &mut Self| &mut v.scrollbar));
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
        let visual_row = (y_in_content / line_height).max(0.0) as usize;
        let visual_row = visual_row.min(self.rendered_lines.len().saturating_sub(1));

        // Map visual row to buffer row + byte offset for wrapped lines
        let (row, byte_offset) = self.visual_to_buffer(visual_row);

        // x position relative to code area
        let x_in_code = f32::from(position.x) - code_left;
        let cw = self.char_width.get();
        let col_chars = (x_in_code / cw).max(0.0) as usize;

        // Convert character count to byte offset within the visual chunk,
        // then add byte_offset to get position in the full buffer line.
        // Clamp defensively: a stale `rendered_lines` mustn't be able to
        // crash the process via an out-of-bounds buffer index on click.
        let row = row.min(self.buffer.len().saturating_sub(1));
        let line = &self.buffer[row];
        let byte_offset = byte_offset.min(line.len());
        let chunk = &line[byte_offset..];
        let mut byte_col = 0;
        for (i, (byte_idx, ch)) in chunk.char_indices().enumerate() {
            if i >= col_chars {
                byte_col = byte_idx;
                break;
            }
            byte_col = byte_idx + ch.len_utf8();
        }
        let col = (byte_offset + byte_col).min(line.len());

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

    /// Search for whole-word occurrences of `word` across the workspace.
    /// Returns references grouped by file (current file first, then alphabetical).
    fn find_cross_file_references(&self, word: &str) -> Vec<(PathBuf, Vec<FileReference>)> {
        let is_word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';

        // Helper: find all whole-word matches in a given text, returning FileReferences
        let search_in_content = |path: &PathBuf, content: &str| -> Vec<FileReference> {
            let mut refs = Vec::new();
            for (row, line) in content.lines().enumerate() {
                let mut search_from = 0;
                while let Some(pos) = line[search_from..].find(word) {
                    let start = search_from + pos;
                    let end = start + word.len();
                    let before_ok = start == 0 || !is_word_char(line.as_bytes()[start - 1]);
                    let after_ok = end >= line.len() || !is_word_char(line.as_bytes()[end]);
                    if before_ok && after_ok {
                        refs.push(FileReference {
                            path: path.clone(),
                            row,
                            col_start: start,
                            col_end: end,
                            line_text: line.to_string(),
                        });
                    }
                    search_from = start + 1;
                }
            }
            refs
        };

        // Current file references (from buffer, which may have unsaved changes)
        let current_refs: Vec<FileReference> = self.buffer.iter().enumerate().flat_map(|(row, line)| {
            let mut refs = Vec::new();
            let mut search_from = 0;
            while let Some(pos) = line[search_from..].find(word) {
                let start = search_from + pos;
                let end = start + word.len();
                let before_ok = start == 0 || !is_word_char(line.as_bytes()[start - 1]);
                let after_ok = end >= line.len() || !is_word_char(line.as_bytes()[end]);
                if before_ok && after_ok {
                    refs.push(FileReference {
                        path: self.path.clone(),
                        row,
                        col_start: start,
                        col_end: end,
                        line_text: line.clone(),
                    });
                }
                search_from = start + 1;
            }
            refs
        }).collect();

        let mut groups: Vec<(PathBuf, Vec<FileReference>)> = Vec::new();
        if !current_refs.is_empty() {
            groups.push((self.path.clone(), current_refs));
        }

        // Cross-file search
        if let Some(root) = &self.workspace_root {
            let walker = WalkBuilder::new(root)
                .hidden(true)
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true)
                .follow_links(false)
                .build();

            for entry in walker {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if !entry.file_type().map_or(false, |ft| ft.is_file()) {
                    continue;
                }
                let file_path = entry.into_path();
                // Skip current file (already searched from buffer)
                if file_path == self.path {
                    continue;
                }
                // Only search text-like files
                let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
                let is_code = matches!(ext,
                    "rs" | "py" | "js" | "jsx" | "ts" | "tsx" | "go" | "java" | "c" | "cpp" | "h" | "hpp" |
                    "rb" | "swift" | "kt" | "scala" | "toml" | "yaml" | "yml" | "json" | "md" | "txt" |
                    "css" | "scss" | "html" | "xml" | "sh" | "bash" | "zsh" | "sql" | "lua" | "zig" |
                    "ex" | "exs" | "erl" | "hrl" | "clj" | "cljs" | "vim" | "el" | "ml" | "mli" |
                    "hs" | "cabal" | "nix" | "dart" | "svelte" | "vue" | "proto" | "tf" | "graphql"
                );
                if !is_code {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&file_path) {
                    // Quick check: does the file even contain the word?
                    if !content.contains(word) {
                        continue;
                    }
                    let refs = search_in_content(&file_path, &content);
                    if !refs.is_empty() {
                        groups.push((file_path, refs));
                    }
                }
            }
        }

        // Sort non-current files alphabetically
        if groups.len() > 1 {
            groups[1..].sort_by(|a, b| a.0.cmp(&b.0));
        }

        groups
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
        self.references_panel = None;
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

    /// Navigate to a specific line (1-indexed), optionally highlighting a column range.
    pub fn navigate_to_line(&mut self, line: usize, col_range: Option<(usize, usize)>) {
        let row = line.saturating_sub(1).min(self.buffer.len().saturating_sub(1));
        self.cursor_row = row;
        self.cursor_col = col_range.map(|(s, _)| s).unwrap_or(0);
        if let Some((col_start, col_end)) = col_range {
            self.nav_highlight = Some((row, col_start, col_end));
        }
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
        self.nav_highlight = None;

        // If search is active, Escape closes it; all other keys go to search input
        if self.search_active {
            if ks.key.as_str() == "escape" {
                self.close_search(window);
                cx.notify();
            }
            return;
        }

        // If references panel is open, handle navigation keys
        if self.references_panel.is_some() {
            match ks.key.as_str() {
                "escape" => {
                    self.references_panel = None;
                    cx.notify();
                    return;
                }
                "up" => {
                    if let Some(panel) = &mut self.references_panel {
                        if panel.selected_ix > 0 {
                            panel.selected_ix -= 1;
                        } else {
                            panel.selected_ix = panel.total_refs.saturating_sub(1);
                        }
                    }
                    cx.notify();
                    return;
                }
                "down" => {
                    if let Some(panel) = &mut self.references_panel {
                        panel.selected_ix = (panel.selected_ix + 1) % panel.total_refs.max(1);
                    }
                    cx.notify();
                    return;
                }
                "enter" => {
                    if let Some(panel) = self.references_panel.take() {
                        if let Some(file_ref) = Self::flat_ref_at(&panel.groups, panel.selected_ix) {
                            self.goto_reference(&file_ref, cx);
                        }
                    }
                    cx.notify();
                    return;
                }
                _ => {
                    // Any other key closes the panel and falls through to normal handling
                    self.references_panel = None;
                    cx.notify();
                }
            }
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
                "/" => {
                    self.toggle_line_comment();
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
        self.nav_highlight = None;

        // If the references panel is open and the click is within its bounds, skip
        // normal click handling (the panel has its own on_click handlers).
        if let Some(panel) = &self.references_panel {
            let content_top = self.content_area_top.get();
            let scroll_offset_y = {
                let state = self.scroll_handle.0.borrow();
                f32::from(state.base_handle.offset().y)
            };
            let panel_top = content_top + (panel.anchor_row as f32 + 1.0) * 18.0 + scroll_offset_y;
            let panel_height: f32 = 320.0; // max_panel_h
            // code_area_left (window-space) is where the code text starts (after gutter).
            // The panel is rendered at `left(gutter_width + 8)` inside the relative container,
            // which puts it ~8px right of the code start in window space.
            let panel_left = self.code_area_left.get();
            let panel_right = panel_left + 500.0;
            let mx = f32::from(event.position.x);
            let my = f32::from(event.position.y);
            if mx >= panel_left && mx <= panel_right && my >= panel_top && my <= panel_top + panel_height {
                return; // click is inside the panel, let panel's on_click handle it
            }
        }

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

    /// Check whether a line prefix before `col_start` ends with a definition keyword.
    fn line_is_definition(line: &str, col_start: usize) -> bool {
        const DEF_KEYWORDS: &[&str] = &[
            "fn", "struct", "enum", "trait", "type", "const", "static",
            "let", "let mut", "mod", "pub fn", "pub struct", "pub enum",
            "pub trait", "pub type", "pub const", "pub static", "pub mod",
            "pub(crate) fn", "pub(crate) struct", "pub(crate) enum",
            "pub(crate) trait", "pub(crate) type", "pub(crate) const",
            "class", "def", "function", "var", "interface",
            "export function", "export class", "export const", "export default",
        ];
        let prefix = &line[..col_start];
        let trimmed = prefix.trim_end();
        DEF_KEYWORDS.iter().any(|kw| trimmed.ends_with(kw))
    }

    /// Handle Cmd+click: "Go to Definition" or "Find All References".
    ///
    /// - Cmd+click on a **usage** → jump to the definition (same file) or emit OpenFile event.
    /// - Cmd+click on a **definition** → open an inline references panel listing all usages
    ///   across the workspace, grouped by file.
    /// - If no definition is found, show references for all occurrences.
    fn handle_cmd_click(&mut self, cx: &mut Context<Self>) {
        // Close any open references panel first
        self.references_panel = None;

        let Some((start, end)) = self.word_at(self.cursor_row, self.cursor_col) else {
            return;
        };
        let word = self.buffer[self.cursor_row][start..end].to_string();
        if word.is_empty() {
            return;
        }

        // Cross-file search
        let groups = self.find_cross_file_references(&word);
        if groups.is_empty() {
            return;
        }

        // Flat list of all references for navigation
        let all_refs: Vec<FileReference> = groups.iter()
            .flat_map(|(_, refs)| refs.iter().cloned())
            .collect();

        // Also keep local occurrences for highlighting in this file
        let local_occurrences = self.find_all_occurrences(&word);

        // Find the definition site (first definition-like occurrence across all files)
        let def_ref = all_refs.iter().find(|r| Self::line_is_definition(&r.line_text, r.col_start));

        // Is the cursor currently on the definition?
        let on_definition = def_ref.map_or(false, |dr| {
            dr.path == self.path && dr.row == self.cursor_row && dr.col_start == start
        });

        if on_definition {
            // On a definition → show references panel (excluding the definition itself)
            let filtered_groups: Vec<(PathBuf, Vec<FileReference>)> = groups.into_iter()
                .map(|(path, refs)| {
                    let filtered: Vec<FileReference> = refs.into_iter()
                        .filter(|r| !Self::line_is_definition(&r.line_text, r.col_start))
                        .collect();
                    (path, filtered)
                })
                .filter(|(_, refs)| !refs.is_empty())
                .collect();

            let total: usize = filtered_groups.iter().map(|(_, r)| r.len()).sum();
            if total == 0 {
                return;
            }

            self.highlighted_word = Some(word.clone());
            self.highlighted_occurrences = local_occurrences;
            self.references_panel = Some(ReferencesPanel {
                word,
                anchor_row: self.cursor_row,
                groups: filtered_groups,
                selected_ix: 0,
                total_refs: total,
            });
        } else if let Some(dr) = def_ref {
            // On a usage → jump to definition
            if dr.path == self.path {
                // Same file: just move cursor and highlight target
                self.cursor_row = dr.row;
                self.cursor_col = dr.col_start;
                self.nav_highlight = Some((dr.row, dr.col_start, dr.col_end));
                self.selection = Some(Selection {
                    start_row: dr.row,
                    start_col: dr.col_start,
                    end_row: dr.row,
                    end_col: dr.col_end,
                });
                self.highlighted_word = Some(word);
                self.highlighted_occurrences = local_occurrences;
                self.ensure_cursor_visible();
            } else {
                // Different file: emit event for parent to open
                cx.emit(FileViewerEvent::OpenFile {
                    path: dr.path.clone(),
                    line: dr.row + 1, // 1-based
                    col_start: dr.col_start,
                    col_end: dr.col_end,
                });
            }
        } else {
            // No definition found → show references panel for all occurrences
            let total: usize = groups.iter().map(|(_, r)| r.len()).sum();
            self.highlighted_word = Some(word.clone());
            self.highlighted_occurrences = local_occurrences;
            self.references_panel = Some(ReferencesPanel {
                word,
                anchor_row: self.cursor_row,
                groups,
                selected_ix: 0,
                total_refs: total,
            });
        }
    }

    /// Get the nth reference from a flat index across all groups.
    fn flat_ref_at(groups: &[(PathBuf, Vec<FileReference>)], flat_ix: usize) -> Option<FileReference> {
        let mut ix = flat_ix;
        for (_, refs) in groups {
            if ix < refs.len() {
                return Some(refs[ix].clone());
            }
            ix -= refs.len();
        }
        None
    }

    /// Navigate to a reference from the references panel and close it.
    fn goto_reference(&mut self, file_ref: &FileReference, cx: &mut Context<Self>) {
        if file_ref.path == self.path {
            // Same file: move cursor and highlight target
            self.cursor_row = file_ref.row;
            self.cursor_col = file_ref.col_start;
            self.nav_highlight = Some((file_ref.row, file_ref.col_start, file_ref.col_end));
            self.references_panel = None;
            self.selection = None;
            self.ensure_cursor_visible();
        } else {
            // Different file: emit event with column info for highlight
            self.references_panel = None;
            cx.emit(FileViewerEvent::OpenFile {
                path: file_ref.path.clone(),
                line: file_ref.row + 1,
                col_start: file_ref.col_start,
                col_end: file_ref.col_end,
            });
        }
    }

    fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(new_offset) = scrollbar::drag_to_offset(&self.scrollbar, event.position.y) {
            let state = self.scroll_handle.0.borrow();
            let base = state.base_handle.clone();
            let x = base.offset().x;
            drop(state);
            base.set_offset(point(x, -new_offset));
            self.bump_scrollbar(cx);
            cx.notify();
            return;
        }

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
        if self.scrollbar.drag_cursor_within_thumb.is_some() {
            self.scrollbar.drag_cursor_within_thumb = None;
            self.bump_scrollbar(cx);
            cx.notify();
        }

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
        // Navigation highlight for this line: (start_col, end_col) if this line has a nav target
        nav_highlight: Option<(usize, usize)>,
    ) -> Div {
        let cw = char_width.get();
        let mut line_el = div().flex().flex_row().h(px(18.0));

        // Gutter
        // Gutter: show line number for first visual line of buffer row, blank for continuations
        let gutter_text = if line_num > 0 {
            format!("{}", line_num)
        } else {
            String::new()
        };
        line_el = line_el.child(
            div()
                .w(px(gutter_width))
                .h(px(18.0))
                .text_color(rgb(0x585b70))
                .flex_shrink_0()
                .pr_2()
                .text_right()
                .child(gutter_text),
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

        // Navigation highlight overlay (bright flash to show jump target)
        if let Some((nh_start, nh_end)) = nav_highlight {
            let start_char = Self::byte_col_to_char_count(line_text, nh_start) as f32;
            let end_char = Self::byte_col_to_char_count(line_text, nh_end) as f32;
            let w = (end_char - start_char).max(0.5);
            code_wrapper = code_wrapper.child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(start_char * cw))
                    .w(px(w * cw))
                    .h(px(18.0))
                    .bg(gpui::rgba(0xf9e2af55)) // warm yellow highlight
                    .border_1()
                    .border_color(gpui::rgba(0xf9e2afaa)),
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Measure actual monospace character width from the font system
        {
            let font = Font {
                family: "Menlo".into(),
                features: FontFeatures::default(),
                fallbacks: None,
                weight: FontWeight::default(),
                style: FontStyle::Normal,
            };
            let run = TextRun {
                len: 1,
                font,
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window.text_system().shape_line(
                "m".into(),
                px(14.0), // text_sm = 0.875rem = 14px at default 16px base
                &[run],
                None,
            );
            let measured = f32::from(shaped.width);
            if measured > 0.0 {
                self.char_width.set(measured);
            }
        }

        // Dynamic wrap: recompute if the measured container width changed
        if self.language.as_deref() == Some("markdown") {
            let width = self.code_area_width.get();
            if width > 0.0 {
                let cw = self.char_width.get().max(1.0);
                let new_wrap = ((width - self.gutter_width) / cw).max(40.0) as usize;
                if new_wrap != self.current_wrap_chars {
                    self.current_wrap_chars = new_wrap;
                    self.recompute_highlights_full();
                }
            }
        }

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
        let nav_highlight = self.nav_highlight;

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

        // References panel data for the closure
        let refs_panel = self.references_panel.clone();
        let entity_handle = cx.entity().clone();

        // Detect scroll changes across renders and bump scrollbar visibility.
        let current_offset_y = f32::from(self.scroll_handle.0.borrow().base_handle.offset().y);
        if (current_offset_y - self.last_scroll_offset).abs() > 0.5 {
            self.last_scroll_offset = current_offset_y;
            self.bump_scrollbar(cx);
        }

        // Virtual scrolling editor lines with click-to-position
        root = root.child({
                let code_left = self.code_area_left.clone();
                let content_top = self.content_area_top.clone();
                let char_width = self.char_width.clone();
                let code_width = self.code_area_width.clone();
                let entity_for_resize = cx.entity().clone();
                let mut editor_container = div()
                    .relative()
                    .flex_1()
                    .size_full()
                    .overflow_hidden()
                    // Canvas to capture width + trigger re-render on resize
                    .child(
                        canvas({
                            let code_width = code_width.clone();
                            move |bounds, _window, cx| {
                                let new_w = f32::from(bounds.size.width);
                                let old_w = code_width.get();
                                code_width.set(new_w);
                                // If width changed significantly, trigger re-render for wrap recompute
                                if (new_w - old_w).abs() > 5.0 {
                                    entity_for_resize.update(cx, |_view, cx| {
                                        cx.notify();
                                    });
                                }
                            }
                        },
                            |_, _, _, _| {},
                        )
                        .w_full()
                        .h_0(),
                    )
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
                                    let rl = &lines[ix];
                                    let buf_row = rl.buffer_row;
                                    let byte_off = rl.byte_offset;
                                    let is_first_visual = byte_off == 0;

                                    // Cursor: show on this visual line only if the
                                    // buffer cursor_row matches AND cursor_col falls
                                    // within this chunk's byte range.
                                    let cursor_on_line = if buf_row == cursor_row {
                                        let chunk_len = rl.text.len();
                                        if cursor_col >= byte_off
                                            && cursor_col <= byte_off + chunk_len
                                        {
                                            Some(cursor_col - byte_off)
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    };

                                    // Selection: use buffer row for range check,
                                    // then adjust col range for this chunk's offset.
                                    let sel_cols = selection.as_ref().and_then(|sel| {
                                        let line_len = buffer_line_lens.get(buf_row).copied().unwrap_or(0);
                                        sel.col_range_for_row(buf_row, line_len)
                                    }).and_then(|(s, e)| {
                                        let chunk_end = byte_off + rl.text.len();
                                        if e <= byte_off || s >= chunk_end {
                                            None // selection doesn't overlap this chunk
                                        } else {
                                            Some((s.saturating_sub(byte_off), (e - byte_off).min(rl.text.len())))
                                        }
                                    });

                                    // Buffer line text for this chunk (for char width calculation)
                                    let full_line = buffer_lines.get(buf_row).map(|s| s.as_str()).unwrap_or("");
                                    let chunk_end = (byte_off + rl.text.len()).min(full_line.len());
                                    let line_text = &full_line[byte_off..chunk_end];

                                    // Word highlights: filter to buffer row, then adjust for chunk offset
                                    let wh: Vec<(usize, usize)> = word_occs
                                        .iter()
                                        .filter(|&&(r, _, _)| r == buf_row)
                                        .filter_map(|&(_, s, e)| {
                                            let chunk_end_b = byte_off + rl.text.len();
                                            if e <= byte_off || s >= chunk_end_b { return None; }
                                            Some((s.saturating_sub(byte_off), (e - byte_off).min(rl.text.len())))
                                        })
                                        .collect();

                                    // Search match highlights: same adjustment
                                    let sh: Vec<(usize, usize, bool)> = if search_active {
                                        search_matches
                                            .iter()
                                            .enumerate()
                                            .filter(|&(_, &(r, _, _))| r == buf_row)
                                            .filter_map(|(mi, &(_, s, e))| {
                                                let chunk_end_b = byte_off + rl.text.len();
                                                if e <= byte_off || s >= chunk_end_b { return None; }
                                                Some((s.saturating_sub(byte_off), (e - byte_off).min(rl.text.len()), mi == active_search_ix))
                                            })
                                            .collect()
                                    } else {
                                        vec![]
                                    };

                                    // Navigation highlight: same adjustment
                                    let nh = nav_highlight.and_then(|(r, s, e)| {
                                        if r != buf_row { return None; }
                                        let chunk_end_b = byte_off + rl.text.len();
                                        if e <= byte_off || s >= chunk_end_b { return None; }
                                        Some((s.saturating_sub(byte_off), (e - byte_off).min(rl.text.len())))
                                    });

                                    // Line number: show buffer line number only on the
                                    // first visual line of each buffer row.
                                    let display_line_num = if is_first_visual {
                                        buf_row + 1
                                    } else {
                                        0 // render_line will show blank for 0
                                    };

                                    let line_div = Self::render_line(
                                        &lines[ix],
                                        display_line_num,
                                        gutter_width,
                                        cursor_on_line,
                                        block_cursor,
                                        code_left.clone(),
                                        char_width.clone(),
                                        sel_cols,
                                        line_text,
                                        &wh,
                                        &sh,
                                        nh,
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
                    );

                // ── Auto-hiding vertical scrollbar overlay ──
                let scroll_offset_px = -self.scroll_handle.0.borrow().base_handle.offset().y;
                let content_height = px(line_count as f32 * 18.0);
                self.scrollbar.content_height = content_height;
                let viewport_height = self.scrollbar.track_height;
                let scroll_visible = self.scrollbar.visible;
                let scroll_dragging = self.scrollbar.drag_cursor_within_thumb.is_some();
                let entity_for_bounds = cx.entity().clone();
                let entity_for_thumb = cx.entity().clone();
                let bounds_sink: Rc<dyn Fn(Bounds<Pixels>, &mut App)> =
                    Rc::new(move |bounds, cx| {
                        entity_for_bounds.update(cx, |view, _cx| {
                            view.scrollbar.track_origin_y = bounds.origin.y;
                            view.scrollbar.track_height = bounds.size.height;
                        });
                    });
                let on_thumb_down: Rc<dyn Fn(Pixels, Pixels, &mut Window, &mut App)> =
                    Rc::new(move |cursor_y, thumb_top, _window, cx| {
                        entity_for_thumb.update(cx, |view, cx| {
                            scrollbar::start_drag(&mut view.scrollbar, cursor_y, thumb_top);
                            view.bump_scrollbar(cx);
                            cx.notify();
                        });
                    });
                if let Some(bar) = scrollbar::render_vertical(
                    "file-viewer-scrollbar",
                    scrollbar::Geometry {
                        scroll_offset: scroll_offset_px,
                        content_height,
                        viewport_height,
                    },
                    scroll_visible,
                    scroll_dragging,
                    bounds_sink,
                    on_thumb_down,
                ) {
                    editor_container = editor_container.child(bar);
                }

                editor_container
            });

        // ── Inline References Panel (deferred overlay, outside scroll container) ──
        if let Some(panel) = &refs_panel {
            let scroll_offset_y = {
                let state = self.scroll_handle.0.borrow();
                f32::from(state.base_handle.offset().y)
            };
            // Compute window-space position for the panel
            let panel_y = self.content_area_top.get()
                + (panel.anchor_row as f32 + 1.0) * 18.0
                + scroll_offset_y;
            let panel_x = self.code_area_left.get();

            let item_height: f32 = 22.0;
            let header_height: f32 = 28.0;
            let group_header_height: f32 = 22.0;
            let max_panel_h: f32 = 320.0;
            let ref_count = panel.total_refs;
            let selected = panel.selected_ix;
            let workspace_root = self.workspace_root.clone();

            let mut panel_el = div()
                .id("references-panel")
                .occlude()
                .w(px(500.0))
                .max_h(px(max_panel_h))
                .bg(colors::bg())
                .border_1()
                .border_color(colors::accent())
                .rounded_md()
                .shadow_lg()
                .overflow_hidden()
                .font_family("Menlo")
                .text_sm()
                // Header
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .h(px(header_height))
                        .px_3()
                        .bg(colors::surface())
                        .border_b_1()
                        .border_color(colors::border())
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(colors::text())
                                        .child(format!("\"{}\"", panel.word)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(colors::text_muted())
                                        .child(format!(
                                            "{} reference{} in {} file{}",
                                            ref_count,
                                            if ref_count == 1 { "" } else { "s" },
                                            panel.groups.len(),
                                            if panel.groups.len() == 1 { "" } else { "s" },
                                        )),
                                ),
                        )
                        .child({
                            let entity_close = entity_handle.clone();
                            div()
                                .id("refs-close-btn")
                                .cursor_pointer()
                                .text_xs()
                                .text_color(colors::text_muted())
                                .hover(|s| s.text_color(colors::text()))
                                .px_1()
                                .child("✕")
                                .on_click(move |_, _, cx| {
                                    entity_close.update(cx, |fv: &mut FileViewer, cx| {
                                        fv.references_panel = None;
                                        cx.notify();
                                    });
                                })
                        }),
                );

            // Scrollable body with grouped references
            let mut body = div()
                .id("references-body")
                .flex()
                .flex_col()
                .max_h(px(max_panel_h - header_height))
                .overflow_y_scroll();

            let mut flat_ix: usize = 0;
            for (group_path, refs) in &panel.groups {
                let display_path = if let Some(root) = &workspace_root {
                    group_path.strip_prefix(root)
                        .unwrap_or(group_path)
                        .to_string_lossy()
                        .to_string()
                } else {
                    group_path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                };

                body = body.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(px(group_header_height))
                        .px_3()
                        .bg(colors::surface())
                        .border_b_1()
                        .border_color(colors::border())
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors::text_muted())
                                .child(display_path),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(gpui::rgba(0x585b70ff))
                                .pl_2()
                                .child(format!("({})", refs.len())),
                        ),
                );

                for file_ref in refs {
                    let is_selected = flat_ix == selected;
                    let entity_nav = entity_handle.clone();
                    let nav_ref = file_ref.clone();
                    let line_num = file_ref.row + 1;

                    let trimmed = file_ref.line_text.trim().to_string();
                    let display_text = if trimmed.chars().count() > 55 {
                        let truncated: String = trimmed.chars().take(55).collect();
                        format!("{truncated}…")
                    } else {
                        trimmed
                    };

                    // Build styled text with the matched word bolded
                    let word = &panel.word;
                    let text_el: AnyElement = {
                        let mut bold_ranges: Vec<Range<usize>> = Vec::new();
                        let lower_display = display_text.to_lowercase();
                        let lower_word = word.to_lowercase();
                        let mut search_from = 0;
                        while let Some(pos) = lower_display[search_from..].find(&lower_word) {
                            let start = search_from + pos;
                            let end = start + lower_word.len();
                            bold_ranges.push(start..end);
                            search_from = end;
                        }
                        if bold_ranges.is_empty() {
                            div()
                                .text_xs()
                                .text_color(colors::text_muted())
                                .flex_1()
                                .overflow_x_hidden()
                                .child(display_text.clone())
                                .into_any_element()
                        } else {
                            let highlights: Vec<(Range<usize>, HighlightStyle)> = bold_ranges
                                .into_iter()
                                .map(|r| (r, HighlightStyle {
                                    color: Some(colors::text().into()),
                                    font_weight: Some(FontWeight::BOLD),
                                    ..Default::default()
                                }))
                                .collect();
                            let styled = StyledText::new(SharedString::from(display_text.clone()))
                                .with_highlights(highlights);
                            div()
                                .text_xs()
                                .text_color(colors::text_muted())
                                .flex_1()
                                .overflow_x_hidden()
                                .child(styled)
                                .into_any_element()
                        }
                    };

                    body = body.child(
                        div()
                            .id(ElementId::Name(format!("ref-item-{flat_ix}").into()))
                            .flex()
                            .flex_row()
                            .items_center()
                            .h(px(item_height))
                            .px_3()
                            .pl(px(20.0))
                            .gap_2()
                            .cursor_pointer()
                            .bg(if is_selected { gpui::rgba(0x89b4fa20) } else { gpui::rgba(0x00000000) })
                            .hover(|s| s.bg(gpui::rgba(0x89b4fa28)))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors::accent())
                                    .min_w(px(36.0))
                                    .child(format!("{}", line_num)),
                            )
                            .child(text_el)
                            .on_click(move |_, _, cx| {
                                entity_nav.update(cx, |fv: &mut FileViewer, cx| {
                                    fv.goto_reference(&nav_ref, cx);
                                    cx.notify();
                                });
                            }),
                    );

                    flat_ix += 1;
                }
            }

            panel_el = panel_el.child(body);

            root = root.child(
                deferred(
                    anchored()
                        .position(point(px(panel_x), px(panel_y)))
                        .child(panel_el)
                ).with_priority(1)
            );
        }

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
