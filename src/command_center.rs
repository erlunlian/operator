use gpui::prelude::FluentBuilder;
use gpui::*;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use crate::text_input::TextInput;
use crate::theme::colors;

/// The mode the command center is currently in.
#[derive(Clone, PartialEq)]
pub enum CommandCenterMode {
    /// Default: show all commands, filter by typed query.
    Commands,
    /// Clone repo: user types a git URL, then Enter clones it.
    CloneRepo,
    /// Workspace grep search: user types a query, results show file:line matches.
    SearchWorkspace,
    /// File name search (Cmd+P): user types a name, results show matching file paths.
    FileSearch,
}

/// A single command entry in the palette.
#[derive(Clone)]
pub struct CommandEntry {
    pub label: String,
    pub description: String,
    pub action: CommandAction,
}

#[derive(Clone)]
pub enum CommandAction {
    OpenProject,
    CloneRepo,
    NewTerminalTab,
    ToggleFilesPanel,
    ToggleSidebar,
    ToggleDiffPanel,
    TogglePrPanel,
    ToggleSettings,
}

/// A single search result from workspace grep.
#[derive(Clone)]
pub struct SearchResult {
    pub path: PathBuf,
    pub line_num: usize,
    pub line_text: String,
}

/// A single row in the flattened search display list.
#[derive(Clone)]
enum SearchDisplayRow {
    /// File path header — groups results under a filename.
    FileHeader(String),
    /// A search result line with its index into `search_results`.
    Result(usize),
}

pub struct CommandCenter {
    pub visible: bool,
    /// Focus handle to restore when the command center closes.
    pub previous_focus: Option<FocusHandle>,
    pub mode: CommandCenterMode,
    pub query: String,
    pub selected_ix: usize,
    pub commands: Vec<CommandEntry>,
    pub input: Entity<TextInput>,
    /// Set after a clone completes — the app reads this.
    pub cloned_dir: Option<PathBuf>,
    /// Set when the user presses Enter on a command — the app reads and clears this.
    pub pending_action: Option<CommandAction>,
    /// Status message shown during clone.
    pub status_message: Option<String>,
    pub cloning: bool,
    /// Workspace search results.
    pub search_results: Vec<SearchResult>,
    /// The search result the user selected (file + line to open).
    pub pending_search_result: Option<SearchResult>,
    /// Root directory for workspace search.
    pub search_root: Option<PathBuf>,
    /// Whether a search is currently running.
    searching: bool,
    /// In-flight search task (dropped = cancelled).
    search_task: Option<Task<()>>,
    /// Debounce task for search-as-you-type.
    search_debounce: Option<Task<()>>,
    /// Receiver for streaming search results from the worker thread.
    search_rx: Option<std::sync::mpsc::Receiver<Vec<SearchResult>>>,
    /// Scroll handle for the commands list.
    scroll_handle: ScrollHandle,
    /// Virtualized scroll handle for search results.
    search_scroll_handle: UniformListScrollHandle,
    /// Flattened display rows for the search result list (file headers + result lines).
    /// Rebuilt whenever search_results changes.
    search_display_rows: Vec<SearchDisplayRow>,
    /// File search results (Cmd+P).
    pub file_results: Vec<PathBuf>,
    /// The file the user selected from file search.
    pub pending_file_path: Option<PathBuf>,
    /// Virtualized scroll handle for file search results.
    file_scroll_handle: UniformListScrollHandle,
    /// Pre-built in-memory file index for instant Cmd+P search.
    /// Relative paths from search_root, populated once when the project opens.
    file_index: Arc<Vec<String>>,
    /// The root that file_index was built for (to detect when we need to rebuild).
    file_index_root: Option<PathBuf>,
    /// Whether the file index is currently being built.
    file_index_building: bool,
}

impl CommandCenter {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new(cx));

        let entity = cx.entity().clone();
        let entity_change = entity.clone();

        // Wire up submit (Enter)
        input.update(cx, |inp, _cx| {
            inp.set_on_submit(Rc::new(move |_text, _window, cx| {
                entity.update(cx, |cc, cx| {
                    cc.handle_submit(cx);
                });
            }));
            inp.set_on_cancel(Rc::new(move |window, cx| {
                let prev = entity_change.read(cx).previous_focus.clone();
                entity_change.update(cx, |cc, cx| {
                    cc.previous_focus = None;
                    cc.dismiss(cx);
                });
                if let Some(handle) = prev {
                    handle.focus(window);
                }
            }));
        });

        // Observe the input for text changes — debounce workspace search
        cx.observe(&input, |cc, input, cx| {
            let new_text = input.read(cx).text.clone();
            if cc.query != new_text {
                cc.query = new_text.clone();
                cc.selected_ix = 0;
                if cc.mode == CommandCenterMode::SearchWorkspace {
                    cc.search_results.clear();
                    cc.search_display_rows.clear();
                    // Debounce: cancel previous pending search, schedule a new one
                    cc.search_debounce = None;
                    if new_text.trim().is_empty() {
                        cc.status_message = None;
                    } else {
                        cc.status_message = Some("Searching...".into());
                        let task = cx.spawn(async |entity, cx| {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(150))
                                .await;
                            let _ = cx.update(|cx| {
                                let _ = entity.update(cx, |cc, cx| {
                                    cc.search_debounce = None;
                                    let q = cc.query.clone();
                                    cc.run_workspace_search(q, cx);
                                });
                            });
                        });
                        cc.search_debounce = Some(task);
                    }
                } else if cc.mode == CommandCenterMode::FileSearch {
                    // In-memory filter — no debounce needed, it's instant.
                    cc.filter_file_index(&new_text);
                }
                cx.notify();
            }
        })
        .detach();

        Self {
            visible: false,
            previous_focus: None,
            mode: CommandCenterMode::Commands,
            query: String::new(),
            selected_ix: 0,
            commands: Self::default_commands(),
            input,
            cloned_dir: None,
            pending_action: None,
            status_message: None,
            cloning: false,
            search_results: Vec::new(),
            pending_search_result: None,
            search_root: None,
            searching: false,
            search_task: None,
            search_debounce: None,
            search_rx: None,
            scroll_handle: ScrollHandle::new(),
            search_scroll_handle: UniformListScrollHandle::new(),
            search_display_rows: Vec::new(),
            file_results: Vec::new(),
            pending_file_path: None,
            file_scroll_handle: UniformListScrollHandle::new(),
            file_index: Arc::new(Vec::new()),
            file_index_root: None,
            file_index_building: false,
        }
    }

    fn default_commands() -> Vec<CommandEntry> {
        vec![
            CommandEntry {
                label: "Open Project".into(),
                description: "Open a directory".into(),
                action: CommandAction::OpenProject,
            },
            CommandEntry {
                label: "Clone Repository".into(),
                description: "Clone a git repository".into(),
                action: CommandAction::CloneRepo,
            },
            CommandEntry {
                label: "New Terminal Tab".into(),
                description: "Open a new terminal".into(),
                action: CommandAction::NewTerminalTab,
            },
            CommandEntry {
                label: "Toggle Files Panel".into(),
                description: "Open file browser".into(),
                action: CommandAction::ToggleFilesPanel,
            },
            CommandEntry {
                label: "Toggle Sidebar".into(),
                description: "Show/hide workspace sidebar".into(),
                action: CommandAction::ToggleSidebar,
            },
            CommandEntry {
                label: "Toggle Diff Panel".into(),
                description: "Show/hide git diff panel".into(),
                action: CommandAction::ToggleDiffPanel,
            },
            CommandEntry {
                label: "Toggle PR Panel".into(),
                description: "Show pull request diff".into(),
                action: CommandAction::TogglePrPanel,
            },
            CommandEntry {
                label: "Settings".into(),
                description: "Open settings".into(),
                action: CommandAction::ToggleSettings,
            },
        ]
    }

    /// Reset shared state when switching modes or dismissing.
    fn reset_state(&mut self, cx: &mut Context<Self>) {
        self.selected_ix = 0;
        self.query.clear();
        self.status_message = None;
        self.search_results.clear();
        self.search_display_rows.clear();
        // Cancel any in-flight search (drops rx → worker thread exits)
        self.search_task = None;
        self.search_rx = None;
        self.file_results.clear();
        self.searching = false;
        // Defer the input clear to avoid a double-lease panic when
        // reset_state is called from within a TextInput callback
        // (e.g. on_cancel/on_submit → dismiss → reset_state).
        let input = self.input.clone();
        cx.defer(move |cx| {
            input.update(cx, |inp, _cx| inp.clear());
        });
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.visible = !self.visible;
        if self.visible {
            self.reset_state(cx);
            self.mode = CommandCenterMode::Commands;
            self.input.update(cx, |inp, _cx| {
                inp.set_placeholder("Type a command...");
            });
        }
        cx.notify();
    }

    pub fn show_clone_mode(&mut self, cx: &mut Context<Self>) {
        self.visible = true;
        self.reset_state(cx);
        self.mode = CommandCenterMode::CloneRepo;
        self.input.update(cx, |inp, _cx| {
            inp.set_placeholder("Enter repository URL (e.g. https://github.com/user/repo)...");
        });
        cx.notify();
    }

    pub fn show_workspace_search_mode(&mut self, cx: &mut Context<Self>) {
        self.visible = true;
        self.reset_state(cx);
        self.mode = CommandCenterMode::SearchWorkspace;
        self.input.update(cx, |inp, _cx| {
            inp.set_placeholder("Search across workspace files...");
        });
        cx.notify();
    }

    pub fn show_file_search_mode(&mut self, cx: &mut Context<Self>) {
        self.visible = true;
        self.reset_state(cx);
        self.mode = CommandCenterMode::FileSearch;
        self.input.update(cx, |inp, _cx| {
            inp.set_placeholder("Search files by name...");
        });
        // Build file index if needed (first open or directory changed)
        self.ensure_file_index(cx);
        cx.notify();
    }

    /// Number of visible items in the current results list.
    fn result_count(&self) -> usize {
        match &self.mode {
            CommandCenterMode::Commands => self.filtered_commands().len(),
            CommandCenterMode::SearchWorkspace => self.search_results.len(),
            CommandCenterMode::FileSearch => self.file_results.len(),
            CommandCenterMode::CloneRepo => 0,
        }
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        self.reset_state(cx);
        self.mode = CommandCenterMode::Commands;
        cx.notify();
    }

    pub fn filtered_commands(&self) -> Vec<(usize, &CommandEntry)> {
        if self.query.is_empty() {
            return self.commands.iter().enumerate().collect();
        }
        let q = self.query.to_lowercase();
        self.commands
            .iter()
            .enumerate()
            .filter(|(_, cmd)| {
                cmd.label.to_lowercase().contains(&q)
                    || cmd.description.to_lowercase().contains(&q)
            })
            .collect()
    }

    /// Returns the selected command action (if any) when the user presses Enter in Commands mode.
    pub fn selected_action(&self) -> Option<CommandAction> {
        let filtered = self.filtered_commands();
        filtered
            .get(self.selected_ix)
            .map(|(_, cmd)| cmd.action.clone())
    }

    /// Called when the user presses Enter in the input.
    fn handle_submit(&mut self, cx: &mut Context<Self>) {
        match &self.mode {
            CommandCenterMode::Commands => {
                if let Some(action) = self.selected_action() {
                    self.pending_action = Some(action);
                }
            }
            CommandCenterMode::CloneRepo => {
                let url = self.query.clone();
                self.clone_repo(url, cx);
            }
            CommandCenterMode::SearchWorkspace => {
                if !self.search_results.is_empty() {
                    let ix = self.selected_ix.min(
                        self.search_results.len().saturating_sub(1),
                    );
                    self.pending_search_result =
                        Some(self.search_results[ix].clone());
                } else {
                    // Cancel any pending debounce and search immediately
                    self.search_debounce = None;
                    let query = self.query.clone();
                    self.run_workspace_search(query, cx);
                }
            }
            CommandCenterMode::FileSearch => {
                if !self.file_results.is_empty() {
                    let ix = self.selected_ix.min(
                        self.file_results.len().saturating_sub(1),
                    );
                    self.pending_file_path =
                        Some(self.file_results[ix].clone());
                } else {
                    let query = self.query.clone();
                    self.filter_file_index(&query);
                }
            }
        }
        cx.notify();
    }

    pub fn run_workspace_search(&mut self, query: String, cx: &mut Context<Self>) {
        if query.trim().is_empty() {
            return;
        }
        let root = match &self.search_root {
            Some(r) => r.clone(),
            None => return,
        };
        // Cancel any in-flight search. Dropping search_rx causes the worker
        // thread's tx.send() to fail, which makes it exit cleanly.
        self.search_task = None;
        self.search_rx = None;
        self.searching = true;
        self.search_results.clear();
        self.search_display_rows.clear();
        self.status_message = Some("Searching...".into());
        cx.notify();

        let (tx, rx) = std::sync::mpsc::channel::<Vec<SearchResult>>();
        self.search_rx = Some(rx);

        // Worker thread: blocking I/O reading from rg/grep, sends batches
        std::thread::spawn(move || {
            search_worker(&query, &root, tx);
        });

        // GPUI task: polls the entity's receiver and streams batches into the UI
        let task = cx.spawn(async |entity, cx| {
            loop {
                // Brief sleep to batch up results and avoid UI thrashing
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(32))
                    .await;

                let should_stop = cx.update(|cx| {
                    entity.update(cx, |cc, cx| {
                        cc.drain_search_batches(cx)
                    }).unwrap_or(true)
                }).unwrap_or(true);

                if should_stop {
                    return;
                }
            }
        });
        self.search_task = Some(task);
    }

    /// Drain all available batches from the search receiver.
    /// Returns `true` when the search is done (worker disconnected and queue empty).
    /// Scroll the appropriate list to keep the selected item visible.
    fn scroll_to_selected(&self) {
        match &self.mode {
            CommandCenterMode::SearchWorkspace => {
                // Find the display row for the selected result index
                if let Some(row_ix) = self.search_display_rows.iter().position(|row| {
                    matches!(row, SearchDisplayRow::Result(i) if *i == self.selected_ix)
                }) {
                    self.search_scroll_handle.scroll_to_item(row_ix, ScrollStrategy::Center);
                }
            }
            CommandCenterMode::FileSearch => {
                self.file_scroll_handle.scroll_to_item(self.selected_ix, ScrollStrategy::Center);
            }
            _ => {
                self.scroll_handle.scroll_to_item(self.selected_ix);
            }
        }
    }

    /// Rebuild the flattened display rows from `search_results`.
    fn rebuild_search_display_rows(&mut self) {
        let mut rows = Vec::new();
        let mut last_path: Option<PathBuf> = None;
        for (i, result) in self.search_results.iter().enumerate() {
            if last_path.as_ref() != Some(&result.path) {
                last_path = Some(result.path.clone());
                let display_path = if let Some(root) = &self.search_root {
                    result
                        .path
                        .strip_prefix(root)
                        .unwrap_or(&result.path)
                        .to_string_lossy()
                        .to_string()
                } else {
                    result.path.to_string_lossy().to_string()
                };
                rows.push(SearchDisplayRow::FileHeader(display_path));
            }
            rows.push(SearchDisplayRow::Result(i));
        }
        self.search_display_rows = rows;
    }

    fn drain_search_batches(&mut self, cx: &mut Context<Self>) -> bool {
        let rx = match &self.search_rx {
            Some(rx) => rx,
            None => return true,
        };

        let mut got_batch = false;
        let mut disconnected = false;

        loop {
            match rx.try_recv() {
                Ok(batch) => {
                    self.search_results.extend(batch);
                    got_batch = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }

        if got_batch {
            self.rebuild_search_display_rows();
            let count = self.search_results.len();
            self.selected_ix = self.selected_ix.min(count.saturating_sub(1));
            self.status_message = Some(format!(
                "{} result{}{}",
                count,
                if count == 1 { "" } else { "s" },
                if disconnected { "" } else { "..." }
            ));
            cx.notify();
        }

        if disconnected {
            self.searching = false;
            self.search_rx = None;
            let count = self.search_results.len();
            self.status_message = if count == 0 {
                Some("No results found".into())
            } else {
                Some(format!("{} result{}", count, if count == 1 { "" } else { "s" }))
            };
            cx.notify();
            return true;
        }

        false
    }

    /// Build (or rebuild) the in-memory file index if needed.
    fn ensure_file_index(&mut self, cx: &mut Context<Self>) {
        let root = match &self.search_root {
            Some(r) => r.clone(),
            None => return,
        };
        // Already built for this root, or currently building
        if self.file_index_root.as_ref() == Some(&root) || self.file_index_building {
            return;
        }
        self.file_index_building = true;
        self.status_message = Some("Indexing files...".into());
        cx.notify();

        cx.spawn(async move |entity, cx| {
            let index = cx
                .background_executor()
                .spawn({
                    let root = root.clone();
                    async move { build_file_index(&root) }
                })
                .await;

            let _ = cx.update(|cx| {
                let _ = entity.update(cx, |cc, cx| {
                    cc.file_index = Arc::new(index);
                    cc.file_index_root = Some(root);
                    cc.file_index_building = false;
                    // If the user already typed a query while we were indexing,
                    // apply the filter now.
                    let q = cc.query.clone();
                    cc.filter_file_index(&q);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Invalidate the file index so it rebuilds on next Cmd+P.
    /// Call this when files are created, deleted, or renamed.
    pub fn invalidate_file_index(&mut self) {
        self.file_index_root = None;
    }

    /// Filter the pre-built file index in memory — instant, no I/O.
    fn filter_file_index(&mut self, query: &str) {
        self.file_results.clear();
        self.selected_ix = 0;

        if query.trim().is_empty() {
            self.status_message = None;
            return;
        }

        let root = match &self.search_root {
            Some(r) => r,
            None => return,
        };

        if self.file_index_building {
            self.status_message = Some("Indexing files...".into());
            return;
        }

        let query_lower = query.to_lowercase();
        let query_parts: Vec<&str> = query_lower.split_whitespace().collect();

        let mut scored: Vec<(i64, &str)> = self
            .file_index
            .iter()
            .filter_map(|path| {
                let path_lower = path.to_lowercase();
                // All query parts must match somewhere in the path
                if !query_parts.iter().all(|part| path_lower.contains(part)) {
                    return None;
                }
                // Score: prefer filename matches over deep path matches
                let filename = path.rsplit('/').next().unwrap_or(path);
                let filename_lower = filename.to_lowercase();
                let mut score: i64 = 0;
                // Bonus for filename containing the full query
                if filename_lower.contains(&query_lower) {
                    score += 100;
                    // Extra bonus for prefix match
                    if filename_lower.starts_with(&query_lower) {
                        score += 50;
                    }
                    // Bonus for exact filename match
                    if filename_lower == query_lower {
                        score += 200;
                    }
                }
                // Prefer shorter paths (less deeply nested)
                score -= (path.matches('/').count() as i64) * 2;
                // Prefer shorter filenames
                score -= filename.len() as i64;
                Some((score, path.as_str()))
            })
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0));

        let count = scored.len();
        self.file_results = scored
            .into_iter()
            .take(1000)
            .map(|(_, p)| root.join(p))
            .collect();

        self.status_message = Some(format!(
            "{} file{}",
            count,
            if count == 1 { "" } else { "s" }
        ));
    }

    pub fn clone_repo(&mut self, url: String, cx: &mut Context<Self>) {
        if url.trim().is_empty() || self.cloning {
            return;
        }
        self.cloning = true;
        self.status_message = Some("Cloning...".into());
        cx.notify();

        cx.spawn(async |entity, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let repo_name = extract_repo_name(&url);
                    let base_dir = dirs::home_dir()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .join("projects");
                    let _ = std::fs::create_dir_all(&base_dir);
                    let target = base_dir.join(&repo_name);

                    let output = std::process::Command::new("git")
                        .args(["clone", &url, &target.to_string_lossy()])
                        .output();

                    match output {
                        Ok(out) if out.status.success() => Ok(target),
                        Ok(out) => {
                            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                            Err(stderr)
                        }
                        Err(e) => Err(e.to_string()),
                    }
                })
                .await;

            let _ = cx.update(|cx| {
                let _ = entity.update(cx, |cc, cx| {
                    cc.cloning = false;
                    match result {
                        Ok(dir) => {
                            cc.cloned_dir = Some(dir);
                            cc.status_message = Some("Clone complete!".into());
                        }
                        Err(msg) => {
                            cc.status_message = Some(format!("Error: {msg}"));
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}

// ── Search worker (runs on a background std::thread) ──

/// Blocking I/O worker: spawns rg (or grep fallback), reads stdout line-by-line,
/// and sends batches of results through the channel. Exits when:
/// - rg/grep finishes naturally (EOF)
/// - the receiver is dropped (search cancelled by the UI)
fn search_worker(query: &str, root: &PathBuf, tx: std::sync::mpsc::Sender<Vec<SearchResult>>) {
    use std::io::BufRead;

    let (mut child, is_rg) = if let Some(c) = spawn_rg(query, root) {
        (c, true)
    } else if let Some(c) = spawn_grep(query, root) {
        (c, false)
    } else {
        return;
    };

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return,
    };

    let relative_root = if is_rg { Some(root) } else { None };
    let reader = std::io::BufReader::new(stdout);
    let mut batch = Vec::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if let Some(result) = parse_search_line(&line, relative_root) {
            batch.push(result);
        }
        if batch.len() >= 20 {
            if tx.send(std::mem::take(&mut batch)).is_err() {
                // Receiver dropped — search was cancelled
                break;
            }
        }
    }

    // Flush remaining results
    if !batch.is_empty() {
        let _ = tx.send(batch);
    }

    // Ensure the child process is fully reaped
    let _ = child.kill();
    let _ = child.wait();
}

fn spawn_rg(query: &str, root: &PathBuf) -> Option<std::process::Child> {
    std::process::Command::new("rg")
        .args([
            "--line-number",
            "--max-count", "5",     // at most 5 matches per file
            "--max-columns", "200", // skip extremely long lines
            "--max-columns-preview",
            "--color", "never",
            "--smart-case",
            "--max-filesize", "1M",
            "-g", "!.git",
            "-g", "!.venv",
            "-g", "!venv",
            "-g", "!__pycache__",
            "-g", "!.mypy_cache",
            "-g", "!.pytest_cache",
            "-g", "!.tox",
            "-g", "!.ruff_cache",
            "-g", "!.eggs",
            "-g", "!*.egg-info",
            "-g", "!node_modules",
            "-g", "!target",
            "-g", "!dist",
            "-g", "!build",
            "-g", "!.next",
            "-g", "!.turbo",
            "-g", "!.cache",
            "-g", "!vendor",
            "-g", "!coverage",
            "-g", "!.parcel-cache",
            "-g", "!.nyc_output",
            "-g", "!.svelte-kit",
            "-g", "!.nuxt",
            "-g", "!.output",
            "-g", "!.angular",
            "-g", "!storybook-static",
            "-g", "!bower_components",
            "--",
            query,
        ])
        .current_dir(root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
}

fn spawn_grep(query: &str, root: &PathBuf) -> Option<std::process::Child> {
    std::process::Command::new("grep")
        .args([
            "-rn",
            "-i",
            "-m", "5",
            "--include=*.rs",
            "--include=*.ts",
            "--include=*.tsx",
            "--include=*.js",
            "--include=*.jsx",
            "--include=*.py",
            "--include=*.go",
            "--include=*.toml",
            "--include=*.json",
            "--include=*.yaml",
            "--include=*.yml",
            "--include=*.md",
            "--include=*.css",
            "--include=*.html",
            "--include=*.sh",
            "--include=*.c",
            "--include=*.cpp",
            "--include=*.h",
            "--include=*.java",
            "--include=*.rb",
            "--include=*.swift",
            "--exclude-dir=.git",
            "--exclude-dir=node_modules",
            "--exclude-dir=target",
            "--exclude-dir=build",
            "--exclude-dir=dist",
            "--exclude-dir=.next",
            "--exclude-dir=vendor",
            "--exclude-dir=__pycache__",
            "--exclude-dir=.venv",
            "--exclude-dir=venv",
            "--exclude-dir=.mypy_cache",
            "--exclude-dir=.pytest_cache",
            "--exclude-dir=.tox",
            "--exclude-dir=.ruff_cache",
            "--exclude-dir=.eggs",
            "--exclude-dir=.turbo",
            "--exclude-dir=.cache",
            "--exclude-dir=coverage",
            "--exclude-dir=.parcel-cache",
            "--exclude-dir=.nyc_output",
            "--exclude-dir=.svelte-kit",
            "--exclude-dir=.nuxt",
            "--exclude-dir=.output",
            "--exclude-dir=.angular",
            "--exclude-dir=storybook-static",
            "--exclude-dir=bower_components",
            "--",
            query,
            &root.to_string_lossy(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
}

/// Parse a single grep/rg output line of the form `file:line:content`.
fn parse_search_line(line: &str, relative_root: Option<&PathBuf>) -> Option<SearchResult> {
    let parts: Vec<&str> = line.splitn(3, ':').collect();
    if parts.len() < 3 {
        return None;
    }
    let line_num = parts[1].parse::<usize>().ok()?;
    let content = parts[2].trim().to_string();
    let path = if let Some(root) = relative_root {
        root.join(parts[0])
    } else {
        PathBuf::from(parts[0])
    };
    Some(SearchResult {
        path,
        line_num,
        line_text: content,
    })
}

// ── File search worker (runs on a background std::thread) ──

// ── File index builder (uses the `ignore` crate — same engine as ripgrep) ──

/// Walk the directory tree once, respecting .gitignore, and return all
/// relative file paths as strings. This runs on a background thread.
fn build_file_index(root: &PathBuf) -> Vec<String> {
    use ignore::WalkBuilder;

    let mut files = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(true)         // skip hidden files by default
        .git_ignore(true)     // respect .gitignore
        .git_global(true)     // respect global gitignore
        .git_exclude(true)    // respect .git/info/exclude
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
        if let Ok(rel) = entry.path().strip_prefix(root) {
            files.push(rel.to_string_lossy().to_string());
        }
    }
    files
}

fn extract_repo_name(url: &str) -> String {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    url.rsplit('/')
        .next()
        .unwrap_or("repo")
        .to_string()
}

impl Render for CommandCenter {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }

        // Focus the text input so it receives key events
        self.input.update(cx, |inp, _cx| {
            inp.focus(window);
        });

        let entity_up = cx.entity().clone();
        let entity_down = cx.entity().clone();

        // Backdrop
        let backdrop_entity = cx.entity().clone();
        let backdrop = div()
            .id("command-center-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(rgba(0x00000088))
            .on_click(move |_, window, cx| {
                let prev = backdrop_entity.read(cx).previous_focus.clone();
                backdrop_entity.update(cx, |cc, cx| {
                    cc.previous_focus = None;
                    cc.dismiss(cx);
                });
                if let Some(handle) = prev {
                    handle.focus(window);
                }
            });

        // Build modal content
        // When search results exist, use a definite height so the uniform_list's
        // flex_1() resolves to real pixels (uniform_list needs bounded height to
        // know how many items to virtualize).
        let has_virtual_list = (matches!(self.mode, CommandCenterMode::SearchWorkspace)
            && !self.search_display_rows.is_empty())
            || (matches!(self.mode, CommandCenterMode::FileSearch)
                && !self.file_results.is_empty());

        let mut modal = div()
            .id("command-center-modal")
            .absolute()
            .top(px(120.0))
            .left_auto()
            .right_auto()
            .w(px(680.0))
            .max_h(px(600.0))
            .when(has_virtual_list, |m: Stateful<Div>| m.h(px(600.0)))
            .bg(colors::surface())
            .border_1()
            .border_color(colors::border())
            .rounded_lg()
            .shadow_lg()
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_key_down(move |event: &KeyDownEvent, _window, cx| {
                let key = event.keystroke.key.as_str();
                match key {
                    "up" => {
                        entity_up.update(cx, |cc, cx| {
                            if cc.selected_ix > 0 {
                                cc.selected_ix -= 1;
                            }
                            cc.scroll_to_selected();
                            cx.notify();
                        });
                    }
                    "down" => {
                        entity_down.update(cx, |cc, cx| {
                            let max_ix = cc.result_count().saturating_sub(1);
                            if cc.selected_ix < max_ix {
                                cc.selected_ix += 1;
                            }
                            cc.scroll_to_selected();
                            cx.notify();
                        });
                    }
                    _ => {}
                }
            });

        // Input field — uses the real TextInput entity
        modal = modal.child(
            div()
                .border_b_1()
                .border_color(colors::border())
                .px_2()
                .py_1()
                .child(self.input.clone()),
        );

        // Status message
        if let Some(msg) = &self.status_message {
            let color = if msg.starts_with("Error") {
                rgb(0xf38ba8)
            } else {
                colors::accent()
            };
            modal = modal.child(
                div()
                    .px_4()
                    .py_2()
                    .text_xs()
                    .text_color(color)
                    .child(msg.clone()),
            );
        }

        // Results list
        match &self.mode {
            CommandCenterMode::Commands => {
                let filtered = self.filtered_commands();
                let count = filtered.len();
                let selected = self.selected_ix.min(count.saturating_sub(1));

                let mut list = div()
                    .id("command-list")
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .max_h(px(340.0));

                for (display_ix, (_orig_ix, cmd)) in filtered.iter().enumerate() {
                    let is_selected = display_ix == selected;
                    let bg = if is_selected {
                        colors::surface_hover()
                    } else {
                        colors::surface()
                    };

                    list = list.child(
                        div()
                            .id(ElementId::Name(format!("cmd-{display_ix}").into()))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .px_4()
                            .py_2()
                            .bg(bg)
                            .cursor_pointer()
                            .hover(|s| s.bg(colors::surface_hover()))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(colors::text())
                                            .child(cmd.label.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(colors::text_muted())
                                            .child(cmd.description.clone()),
                                    ),
                            ),
                    );
                }

                modal = modal.child(list);
            }
            CommandCenterMode::CloneRepo => {
                if self.query.is_empty() && self.status_message.is_none() {
                    modal = modal.child(
                        div()
                            .px_4()
                            .py_6()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_sm()
                            .text_color(colors::text_muted())
                            .child("Paste a git URL and press Enter to clone"),
                    );
                }
            }
            CommandCenterMode::SearchWorkspace => {
                if self.search_display_rows.is_empty() {
                    if self.query.is_empty() && self.status_message.is_none() {
                        modal = modal.child(
                            div()
                                .px_4()
                                .py_6()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .text_color(colors::text_muted())
                                .child("Type a search query and press Enter"),
                        );
                    }
                } else {
                    let selected = self.selected_ix.min(self.search_results.len().saturating_sub(1));
                    let display_rows = self.search_display_rows.clone();
                    let results = self.search_results.clone();
                    let entity = cx.entity().clone();

                    let list = uniform_list(
                        "search-results-list",
                        display_rows.len(),
                        move |range, _window, _cx| {
                            range
                                .map(|row_ix| {
                                    match &display_rows[row_ix] {
                                        SearchDisplayRow::FileHeader(path) => {
                                            div()
                                                .id(ElementId::Name(
                                                    format!("search-file-{row_ix}").into(),
                                                ))
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .px_3()
                                                .pt(px(6.0))
                                                .pb(px(2.0))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(colors::accent())
                                                        .overflow_x_hidden()
                                                        .child(path.clone()),
                                                )
                                                .into_any_element()
                                        }
                                        SearchDisplayRow::Result(result_ix) => {
                                            let result_ix = *result_ix;
                                            let result = &results[result_ix];
                                            let is_selected = result_ix == selected;
                                            let bg = if is_selected {
                                                colors::surface_hover()
                                            } else {
                                                colors::surface()
                                            };
                                            let click_entity = entity.clone();
                                            let click_result = result.clone();
                                            div()
                                                .id(ElementId::Name(
                                                    format!("search-result-{row_ix}").into(),
                                                ))
                                                .w_full()
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .gap_2()
                                                .pl(px(20.0))
                                                .pr(px(12.0))
                                                .py_1()
                                                .bg(bg)
                                                .cursor_pointer()
                                                .hover(|s| s.bg(colors::surface_hover()))
                                                .on_click(move |_, _window, cx| {
                                                    click_entity.update(cx, |cc, cx| {
                                                        cc.pending_search_result =
                                                            Some(click_result.clone());
                                                        cx.notify();
                                                    });
                                                })
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(colors::text_muted())
                                                        .flex_shrink_0()
                                                        .w(px(32.0))
                                                        .child(format!("{}", result.line_num)),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(colors::text())
                                                        .flex_1()
                                                        .overflow_x_hidden()
                                                        .child(result.line_text.clone()),
                                                )
                                                .into_any_element()
                                        }
                                    }
                                })
                                .collect()
                        },
                    )
                    .flex_1()
                    .track_scroll(self.search_scroll_handle.clone());

                    modal = modal.child(list);
                }
            }
            CommandCenterMode::FileSearch => {
                if self.file_results.is_empty() {
                    if self.query.is_empty() && self.status_message.is_none() {
                        modal = modal.child(
                            div()
                                .px_4()
                                .py_6()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .text_color(colors::text_muted())
                                .child("Type to search for files by name"),
                        );
                    }
                } else {
                    let selected =
                        self.selected_ix.min(self.file_results.len().saturating_sub(1));
                    let results = self.file_results.clone();
                    let search_root = self.search_root.clone();
                    let entity = cx.entity().clone();

                    let list = uniform_list(
                        "file-results-list",
                        results.len(),
                        move |range, _window, _cx| {
                            range
                                .map(|ix| {
                                    let path = &results[ix];
                                    let display = if let Some(root) = &search_root {
                                        path.strip_prefix(root)
                                            .unwrap_or(path)
                                            .to_string_lossy()
                                            .to_string()
                                    } else {
                                        path.to_string_lossy().to_string()
                                    };

                                    let is_selected = ix == selected;
                                    let bg = if is_selected {
                                        colors::surface_hover()
                                    } else {
                                        colors::surface()
                                    };

                                    let click_entity = entity.clone();
                                    let click_path = path.clone();

                                    // Split into filename and directory parts
                                    let (dir_part, file_part) =
                                        if let Some(pos) = display.rfind('/') {
                                            (
                                                Some(display[..=pos].to_string()),
                                                display[pos + 1..].to_string(),
                                            )
                                        } else {
                                            (None, display.clone())
                                        };

                                    let mut row = div()
                                        .id(ElementId::Name(
                                            format!("file-result-{ix}").into(),
                                        ))
                                        .w_full()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        .px_4()
                                        .py(px(6.0))
                                        .bg(bg)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(colors::surface_hover()))
                                        .on_click(move |_, _window, cx| {
                                            click_entity.update(cx, |cc, cx| {
                                                cc.pending_file_path =
                                                    Some(click_path.clone());
                                                cx.notify();
                                            });
                                        });

                                    row = row.child(
                                        div()
                                            .text_sm()
                                            .text_color(colors::text())
                                            .child(file_part),
                                    );

                                    if let Some(dir) = dir_part {
                                        row = row.child(
                                            div()
                                                .ml_2()
                                                .text_xs()
                                                .text_color(colors::text_muted())
                                                .child(dir),
                                        );
                                    }

                                    row.into_any_element()
                                })
                                .collect()
                        },
                    )
                    .flex_1()
                    .track_scroll(self.file_scroll_handle.clone());

                    modal = modal.child(list);
                }
            }
        }

        // Position the modal centered horizontally
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(backdrop)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(modal),
            )
            .into_any_element()
    }
}
