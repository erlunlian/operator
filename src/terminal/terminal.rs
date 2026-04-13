use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{Event as AlacEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color as AlacColor, NamedColor, Rgb as AlacRgb};
use gpui::*;

use crate::theme::colors;

const TERM_ROWS: u16 = 40;
const TERM_COLS: u16 = 120;
const CELL_WIDTH: u16 = 8;
const CELL_HEIGHT: u16 = 16;

// ── Event listener that forwards terminal events ──

#[derive(Clone)]
pub struct JsonListener {
    events: Arc<std::sync::Mutex<Vec<AlacEvent>>>,
}

impl JsonListener {
    fn new() -> Self {
        Self {
            events: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn take_events(&self) -> Vec<AlacEvent> {
        let mut events = self.events.lock().unwrap();
        std::mem::take(&mut *events)
    }
}

impl EventListener for JsonListener {
    fn send_event(&self, event: AlacEvent) {
        self.events.lock().unwrap().push(event);
    }
}

// ── Terminal dimensions ──

struct TermDimensions {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermDimensions {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

// ── Claude status detection ──

#[derive(Clone, Debug, PartialEq)]
pub enum DetectedClaudeStatus {
    NotRunning,
    WaitingForInput,
    Working,
}

// ── Terminal model ──

pub struct TerminalModel {
    pub term: Arc<FairMutex<Term<JsonListener>>>,
    event_loop_sender: EventLoopSender,
    _listener: JsonListener,
    pub claude_status: Arc<std::sync::Mutex<DetectedClaudeStatus>>,
    /// True when Claude finished responding but the user hasn't focused this tab yet.
    has_unread_response: Arc<std::sync::Mutex<bool>>,
    /// Pending PTY resize, debounced to avoid SIGWINCH storms during interactive split resizing.
    /// Tuple: (rows, cols, timestamp of last change).
    pending_pty_resize: Arc<std::sync::Mutex<Option<(u16, u16, std::time::Instant)>>>,
}

impl TerminalModel {
    pub fn new(work_dir: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let listener = JsonListener::new();
        let dimensions = TermDimensions {
            cols: TERM_COLS as usize,
            rows: TERM_ROWS as usize,
        };

        let config = TermConfig {
            scrolling_history: 10000,
            ..Default::default()
        };

        let term = Term::new(config, &dimensions, listener.clone());
        let term = Arc::new(FairMutex::new(term));

        let window_size = WindowSize {
            num_lines: TERM_ROWS,
            num_cols: TERM_COLS,
            cell_width: CELL_WIDTH,
            cell_height: CELL_HEIGHT,
        };

        let mut env = std::collections::HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());

        // Use the user's preferred shell and start it as a login shell
        // so that PATH and other profile-level config are loaded.
        let user_shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let login_flag = if user_shell.ends_with("fish") {
            "--login".to_string()
        } else {
            "-l".to_string()
        };

        let pty_config = tty::Options {
            shell: Some(tty::Shell::new(user_shell, vec![login_flag])),
            working_directory: Some(work_dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_default())),
            env,
            ..Default::default()
        };

        let pty = tty::new(&pty_config, window_size, 0).expect("Failed to create PTY");

        let event_loop = EventLoop::new(
            term.clone(),
            listener.clone(),
            pty,
            false,
            false,
        )
        .expect("Failed to create event loop");

        let event_loop_sender = event_loop.channel();
        let _join_handle = event_loop.spawn();

        let claude_status = Arc::new(std::sync::Mutex::new(DetectedClaudeStatus::NotRunning));
        let has_unread_response = Arc::new(std::sync::Mutex::new(false));
        let pending_pty_resize: Arc<std::sync::Mutex<Option<(u16, u16, std::time::Instant)>>> =
            Arc::new(std::sync::Mutex::new(None));

        // Periodic render refresh — only notify when content changes
        let term_clone = term.clone();
        let listener_clone = listener.clone();
        let claude_status_clone = claude_status.clone();
        let unread_clone = has_unread_response.clone();
        let pending_resize_clone = pending_pty_resize.clone();
        let sender_clone = event_loop_sender.clone();
        let entity = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            let mut last_content_hash: u64 = 0;
            let mut prev_claude_status = DetectedClaudeStatus::NotRunning;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;

                // Flush debounced PTY resize after 200ms of stability.
                // The grid was already resized (without reflow) in resize().
                // Now do a proper Term::resize (with reflow) so the grid state
                // is fully correct, then send SIGWINCH so the shell redraws.
                let mut did_resize = false;
                {
                    let mut pending = pending_resize_clone.lock().unwrap();
                    if let Some((rows, cols, when)) = *pending {
                        if when.elapsed() >= std::time::Duration::from_millis(200) {
                            // Final grid resize with proper reflow + bookkeeping
                            let dims = TermDimensions {
                                cols: cols as usize,
                                rows: rows as usize,
                            };
                            term_clone.lock().resize(dims);

                            // Send SIGWINCH so the shell redraws for the new size
                            let window_size = WindowSize {
                                num_lines: rows,
                                num_cols: cols,
                                cell_width: CELL_WIDTH,
                                cell_height: CELL_HEIGHT,
                            };
                            let _ = sender_clone.send(Msg::Resize(window_size));
                            *pending = None;
                            did_resize = true;
                        }
                    }
                }

                if did_resize {
                    let entity_clone = entity.clone();
                    let _ = cx.update(|cx| {
                        if let Some(e) = entity_clone.upgrade() {
                            e.update(cx, |_this, cx| cx.notify());
                        }
                    });
                }

                // Drain events from terminal listener to keep buffer clear
                let _ = listener_clone.take_events();

                // Compute a hash of visible terminal content to detect changes
                let content_changed;
                {
                    let term = term_clone.lock();
                    let grid = term.grid();
                    let screen_lines = grid.screen_lines();
                    let cols = grid.columns();

                    use std::hash::{Hash, Hasher};
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    let cursor = term.grid().cursor.point;
                    cursor.line.0.hash(&mut hasher);
                    cursor.column.0.hash(&mut hasher);

                    // Hash ALL visible lines for accurate change detection
                    let mut bottom_lines: Vec<String> = Vec::new();
                    for line_idx in 0..screen_lines {
                        let row = &grid[alacritty_terminal::index::Line(line_idx as i32)];
                        let mut line_text_full = String::new();
                        for col_idx in 0..cols {
                            let cell = &row[alacritty_terminal::index::Column(col_idx)];
                            cell.c.hash(&mut hasher);
                            if cell.c != '\0' {
                                line_text_full.push(cell.c);
                            }
                        }
                        // Collect bottom lines (with spaces) for Claude status detection
                        if line_idx >= screen_lines.saturating_sub(8) {
                            let trimmed = line_text_full.trim();
                            if !trimmed.is_empty() {
                                bottom_lines.push(trimmed.to_string());
                            }
                        }
                    }
                    detect_claude_state(&bottom_lines, &claude_status_clone);

                    // Track state transitions for unread response tracking
                    {
                        let new_status = claude_status_clone.lock().unwrap().clone();
                        if prev_claude_status == DetectedClaudeStatus::Working
                            && new_status == DetectedClaudeStatus::WaitingForInput
                        {
                            *unread_clone.lock().unwrap() = true;
                        }
                        if new_status == DetectedClaudeStatus::Working {
                            *unread_clone.lock().unwrap() = false;
                        }
                        prev_claude_status = new_status;
                    }

                    let new_hash = hasher.finish();
                    content_changed = new_hash != last_content_hash;
                    last_content_hash = new_hash;
                }

                // Only notify the UI when content actually changed
                if content_changed {
                    let entity_clone = entity.clone();
                    let res = cx.update(|cx| {
                        if let Some(e) = entity_clone.upgrade() {
                            e.update(cx, |_this, cx| cx.notify());
                        }
                    });
                    if res.is_err() {
                        break;
                    }
                }
            }
        })
        .detach();

        Self {
            term,
            event_loop_sender,
            _listener: listener,
            claude_status,
            has_unread_response,
            pending_pty_resize,
        }
    }

    pub fn write_to_pty(&self, data: &[u8]) {
        let _ = self
            .event_loop_sender
            .send(Msg::Input(Cow::Owned(data.to_vec())));
    }

    pub fn write_str_to_pty(&self, text: &str) {
        self.write_to_pty(text.as_bytes());
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        // Debounce both grid and PTY resize. The grid is NOT touched during the
        // drag — Term::resize() (with full bookkeeping) runs once after 200ms of
        // stability, followed by the PTY SIGWINCH so the shell redraws.
        *self.pending_pty_resize.lock().unwrap() = Some((rows, cols, std::time::Instant::now()));
    }

    /// Returns the display-ready status, factoring in whether the user has read the response.
    pub fn get_claude_status(&self) -> DetectedClaudeStatus {
        let raw = self.claude_status.lock().unwrap().clone();
        match raw {
            DetectedClaudeStatus::Working => DetectedClaudeStatus::Working,
            DetectedClaudeStatus::WaitingForInput => {
                if *self.has_unread_response.lock().unwrap() {
                    DetectedClaudeStatus::WaitingForInput
                } else {
                    DetectedClaudeStatus::NotRunning
                }
            }
            DetectedClaudeStatus::NotRunning => DetectedClaudeStatus::NotRunning,
        }
    }

    /// Clear the unread flag — called when the user focuses on this terminal's tab.
    pub fn mark_claude_as_read(&self) {
        *self.has_unread_response.lock().unwrap() = false;
    }
}

fn detect_claude_state(bottom_lines: &[String], status: &Arc<std::sync::Mutex<DetectedClaudeStatus>>) {
    if bottom_lines.is_empty() {
        return;
    }

    let mut found_shell_prompt = false;
    let mut found_claude_running = false; // Claude Code status bar is visible
    let mut found_working = false;

    for line in bottom_lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // ── Working indicators ──

        // Claude Code thinking label: "Infusing…", "Tomfoolering…", etc.
        // Always a single word ending in "ing" + ellipsis.
        if trimmed.ends_with("ing\u{2026}") || trimmed.ends_with("ing...") {
            found_working = true;
            continue;
        }

        // "esc to interrupt" only appears in the status bar during active work
        if trimmed.contains("esc to interrupt") {
            found_working = true;
            found_claude_running = true;
            continue;
        }

        // Tool call patterns visible on screen
        if trimmed.contains("Read(")
            || trimmed.contains("Edit(")
            || trimmed.contains("Write(")
            || trimmed.contains("Bash(")
            || trimmed.contains("Agent(")
            || trimmed.contains("Grep(")
            || trimmed.contains("Glob(")
            || trimmed.contains("Skill(")
            || trimmed.contains("Cooked for")
            || trimmed.contains("Compiling")
        {
            found_working = true;
            continue;
        }

        // ── Claude running (but idle) indicators ──

        // Status bar mode text — present whenever Claude Code is running
        if trimmed.contains("auto mode on")
            || trimmed.contains("plan mode")
            || trimmed.contains("shift+tab to cycle")
        {
            found_claude_running = true;
            continue;
        }

        // ── Shell prompt detection ──
        if trimmed.ends_with('%') || trimmed.ends_with("$ ")
            || trimmed.ends_with('$') || trimmed.ends_with("% ")
        {
            found_shell_prompt = true;
        }
    }

    let mut s = status.lock().unwrap();

    if found_working {
        *s = DetectedClaudeStatus::Working;
    } else if found_claude_running {
        *s = DetectedClaudeStatus::WaitingForInput;
    } else if found_shell_prompt {
        *s = DetectedClaudeStatus::NotRunning;
    }
    // Otherwise keep current state
}

// ── Color conversion ──

pub fn alac_color_to_gpui(color: &AlacColor, colors: &alacritty_terminal::term::color::Colors) -> Rgba {
    match color {
        AlacColor::Named(named) => named_color_to_rgba(*named),
        AlacColor::Spec(AlacRgb { r, g, b }) => {
            let hex = (*r as u32) << 16 | (*g as u32) << 8 | (*b as u32);
            rgb(hex)
        }
        AlacColor::Indexed(idx) => {
            if let Some(c) = colors[*idx as usize] {
                let hex = (c.r as u32) << 16 | (c.g as u32) << 8 | (c.b as u32);
                rgb(hex)
            } else {
                // Standard 256-color palette fallback
                indexed_color_to_rgba(*idx)
            }
        }
    }
}

fn named_color_to_rgba(c: NamedColor) -> Rgba {
    match c {
        NamedColor::Black => rgb(0x45475a),
        NamedColor::Red => rgb(0xf38ba8),
        NamedColor::Green => rgb(0xa6e3a1),
        NamedColor::Yellow => rgb(0xf9e2af),
        NamedColor::Blue => rgb(0x89b4fa),
        NamedColor::Magenta => rgb(0xf5c2e7),
        NamedColor::Cyan => rgb(0x94e2d5),
        NamedColor::White => rgb(0xbac2de),
        NamedColor::BrightBlack => rgb(0x585b70),
        NamedColor::BrightRed => rgb(0xf38ba8),
        NamedColor::BrightGreen => rgb(0xa6e3a1),
        NamedColor::BrightYellow => rgb(0xf9e2af),
        NamedColor::BrightBlue => rgb(0x89b4fa),
        NamedColor::BrightMagenta => rgb(0xf5c2e7),
        NamedColor::BrightCyan => rgb(0x94e2d5),
        NamedColor::BrightWhite => rgb(0xcdd6f4),
        NamedColor::Foreground => colors::text(),
        NamedColor::Background => colors::bg(),
        NamedColor::Cursor => colors::accent(),
        NamedColor::DimForeground => colors::text_muted(),
        _ => colors::text(),
    }
}

fn indexed_color_to_rgba(idx: u8) -> Rgba {
    if idx < 16 {
        // Use named colors for first 16
        let named = match idx {
            0 => NamedColor::Black,
            1 => NamedColor::Red,
            2 => NamedColor::Green,
            3 => NamedColor::Yellow,
            4 => NamedColor::Blue,
            5 => NamedColor::Magenta,
            6 => NamedColor::Cyan,
            7 => NamedColor::White,
            8 => NamedColor::BrightBlack,
            9 => NamedColor::BrightRed,
            10 => NamedColor::BrightGreen,
            11 => NamedColor::BrightYellow,
            12 => NamedColor::BrightBlue,
            13 => NamedColor::BrightMagenta,
            14 => NamedColor::BrightCyan,
            15 => NamedColor::BrightWhite,
            _ => unreachable!(),
        };
        named_color_to_rgba(named)
    } else if idx < 232 {
        // 216-color cube: 16-231
        let idx = idx - 16;
        let r = (idx / 36) * 51;
        let g = ((idx % 36) / 6) * 51;
        let b = (idx % 6) * 51;
        let hex = (r as u32) << 16 | (g as u32) << 8 | (b as u32);
        rgb(hex)
    } else {
        // Grayscale: 232-255
        let v = ((idx - 232) as u32) * 10 + 8;
        rgb(v << 16 | v << 8 | v)
    }
}
