use gpui::*;
use std::path::PathBuf;
use std::rc::Rc;

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
    ToggleSettings,
}

/// A single search result from workspace grep.
#[derive(Clone)]
pub struct SearchResult {
    pub path: PathBuf,
    pub line_num: usize,
    pub line_text: String,
}

pub struct CommandCenter {
    pub visible: bool,
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
    /// Scroll handle for the results list.
    scroll_handle: ScrollHandle,
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
            inp.set_on_cancel(Rc::new(move |_window, cx| {
                entity_change.update(cx, |cc, cx| {
                    cc.dismiss(cx);
                });
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
                }
                cx.notify();
            }
        })
        .detach();

        Self {
            visible: false,
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
            scroll_handle: ScrollHandle::new(),
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

    /// Number of visible items in the current results list.
    fn result_count(&self) -> usize {
        match &self.mode {
            CommandCenterMode::Commands => self.filtered_commands().len(),
            CommandCenterMode::SearchWorkspace => self.search_results.len(),
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
        // Cancel any in-flight search
        self.search_task = None;
        self.searching = true;
        self.search_results.clear();
        self.status_message = Some("Searching...".into());
        cx.notify();

        let task = cx.spawn(async |entity, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Self::exec_search(&query, &root)
                })
                .await;

            let _ = cx.update(|cx| {
                let _ = entity.update(cx, |cc, cx| {
                    cc.searching = false;
                    cc.search_task = None;
                    match result {
                        Ok(results) => {
                            let count = results.len();
                            cc.search_results = results;
                            cc.selected_ix = 0;
                            cc.status_message = if count == 0 {
                                Some("No results found".into())
                            } else {
                                Some(format!("{} result{}", count, if count == 1 { "" } else { "s" }))
                            };
                        }
                        Err(msg) => {
                            cc.status_message = Some(format!("Error: {msg}"));
                        }
                    }
                    cx.notify();
                });
            });
        });
        self.search_task = Some(task);
    }

    /// Run the search using ripgrep (fast) with a grep fallback.
    fn exec_search(query: &str, root: &PathBuf) -> Result<Vec<SearchResult>, String> {
        // Try ripgrep first — it respects .gitignore, skips binary files,
        // and is an order of magnitude faster than grep.
        let rg_result = std::process::Command::new("rg")
            .args([
                "--line-number",
                "--max-count", "5",     // at most 5 matches per file
                "--max-columns", "200", // skip extremely long lines
                "--max-columns-preview",
                "--color", "never",
                "--smart-case",
                "--max-filesize", "1M",
                "-g", "!.git",
                "--",
                query,
            ])
            .current_dir(root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();

        if let Ok(out) = rg_result {
            if out.status.success() || out.status.code() == Some(1) {
                // rg exit 1 = no matches
                return Ok(Self::parse_search_output(
                    &String::from_utf8_lossy(&out.stdout),
                    Some(root),
                ));
            }
        }

        // Fallback to grep
        let output = std::process::Command::new("grep")
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
                "--",
                query,
                &root.to_string_lossy(),
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .map_err(|e| e.to_string())?;

        Ok(Self::parse_search_output(
            &String::from_utf8_lossy(&output.stdout),
            None,
        ))
    }

    /// Parse grep/rg output lines of the form `file:line:content`.
    fn parse_search_output(stdout: &str, relative_root: Option<&PathBuf>) -> Vec<SearchResult> {
        let mut results = Vec::new();
        for line in stdout.lines() {
            if results.len() >= 100 {
                break;
            }
            let parts: Vec<&str> = line.splitn(3, ':').collect();
            if parts.len() >= 3 {
                let file_path = parts[0];
                if let Ok(line_num) = parts[1].parse::<usize>() {
                    let content = parts[2].trim().to_string();
                    let path = if let Some(root) = relative_root {
                        root.join(file_path)
                    } else {
                        PathBuf::from(file_path)
                    };
                    results.push(SearchResult {
                        path,
                        line_num,
                        line_text: content,
                    });
                }
            }
        }
        results
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

        let entity = cx.entity().clone();
        let entity_up = cx.entity().clone();
        let entity_down = cx.entity().clone();

        // Backdrop
        let backdrop = div()
            .id("command-center-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(rgba(0x00000088))
            .on_click(move |_, _window, cx| {
                entity.update(cx, |cc, cx| cc.dismiss(cx));
            });

        // Build modal content
        let mut modal = div()
            .id("command-center-modal")
            .absolute()
            .top(px(120.0))
            .left_auto()
            .right_auto()
            .w(px(560.0))
            .max_h(px(420.0))
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
                            cc.scroll_handle.scroll_to_item(cc.selected_ix);
                            cx.notify();
                        });
                    }
                    "down" => {
                        entity_down.update(cx, |cc, cx| {
                            let max_ix = cc.result_count().saturating_sub(1);
                            if cc.selected_ix < max_ix {
                                cc.selected_ix += 1;
                            }
                            cc.scroll_handle.scroll_to_item(cc.selected_ix);
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
                if self.search_results.is_empty() {
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
                    let count = self.search_results.len();
                    let selected = self.selected_ix.min(count.saturating_sub(1));

                    let mut list = div()
                        .id("search-results-list")
                        .flex()
                        .flex_col()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll_handle)
                        .max_h(px(340.0));

                    // Group results by file — results are already ordered by file
                    // from grep/rg output so consecutive entries share a path.
                    let mut child_ix: usize = 0;
                    let mut scroll_child_ix: Option<usize> = None;
                    let mut last_path: Option<PathBuf> = None;

                    for (result_ix, result) in self.search_results.iter().enumerate() {
                        // Emit file header when the path changes
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

                            list = list.child(
                                div()
                                    .id(ElementId::Name(
                                        format!("search-file-{child_ix}").into(),
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
                                            .child(display_path),
                                    ),
                            );
                            child_ix += 1;
                        }

                        let is_selected = result_ix == selected;
                        if is_selected {
                            scroll_child_ix = Some(child_ix);
                        }
                        let bg = if is_selected {
                            colors::surface_hover()
                        } else {
                            colors::surface()
                        };

                        list = list.child(
                            div()
                                .id(ElementId::Name(
                                    format!("search-result-{child_ix}").into(),
                                ))
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
                                ),
                        );
                        child_ix += 1;
                    }

                    // Scroll to the selected item's visual position
                    if let Some(ci) = scroll_child_ix {
                        self.scroll_handle.scroll_to_item(ci);
                    }

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
