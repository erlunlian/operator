use gpui::*;
use std::path::PathBuf;
use std::rc::Rc;

use crate::actions::*;
use crate::command_center::{CommandAction, CommandCenter};
use crate::git::GitDiffPanel;
use crate::recent_projects::RecentProjects;
use crate::settings::AppSettings;
use crate::settings::settings_panel::SettingsPanel;
use crate::theme::colors;
use crate::workspace::sidebar::WorkspaceCardData;
use crate::workspace::{Workspace, WorkspaceSidebar};

pub struct OperatorApp {
    pub workspaces: Vec<Entity<Workspace>>,
    pub active_workspace_ix: usize,
    pub sidebar_collapsed: bool,
    pub sidebar_width: f32,
    resizing_sidebar: bool,
    pub diff_panel_collapsed: bool,
    pub diff_panel: Entity<GitDiffPanel>,
    resizing_diff_panel: bool,
    pub settings_panel_open: bool,
    pub settings_panel: Entity<SettingsPanel>,
    pub command_center: Entity<CommandCenter>,
    picker_open: bool,
    focus_handle: FocusHandle,
    /// Cached window bounds (x, y, w, h) for session persistence.
    pub window_bounds: Option<(f32, f32, f32, f32)>,
    /// Cached recent projects (avoid disk reads in render path).
    recent_projects: RecentProjects,
}

impl OperatorApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let ws = cx.new(|cx| Workspace::new_empty(cx));
        cx.observe(&ws, |_this, _ws, cx| cx.notify()).detach();

        let diff_panel = cx.new(|_cx| GitDiffPanel::empty());

        let settings_panel = cx.new(|cx| SettingsPanel::new(cx));
        let command_center = cx.new(|cx| CommandCenter::new(cx));
        cx.observe(&command_center, |this, cc, cx| {
            this.handle_command_center_update(&cc, cx);
        })
        .detach();

        Self::register_quit_handler(cx);
        Self::start_diff_watcher(diff_panel.clone(), cx);
        cx.observe_global::<AppSettings>(|_this, cx| cx.notify()).detach();

        Self {
            workspaces: vec![ws],
            active_workspace_ix: 0,
            sidebar_collapsed: false,
            sidebar_width: 260.0,
            resizing_sidebar: false,
            diff_panel_collapsed: false,
            diff_panel,
            resizing_diff_panel: false,
            settings_panel_open: false,
            settings_panel,
            command_center,
            picker_open: false,
            focus_handle: cx.focus_handle(),
            window_bounds: None,
            recent_projects: RecentProjects::load(),
        }
    }

    pub fn from_restored(
        workspaces: Vec<Entity<Workspace>>,
        active_workspace_ix: usize,
        sidebar_collapsed: bool,
        sidebar_width: f32,
        diff_panel_collapsed: bool,
        window_bounds: Option<(f32, f32, f32, f32)>,
        diff_panel: Entity<GitDiffPanel>,
        settings_panel: Entity<SettingsPanel>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Ensure we always have at least one workspace
        let workspaces = if workspaces.is_empty() {
            let ws = cx.new(|cx| Workspace::new_empty(cx));
            vec![ws]
        } else {
            workspaces
        };
        let active_workspace_ix = active_workspace_ix.min(workspaces.len().saturating_sub(1));

        for ws in &workspaces {
            cx.observe(ws, |_this, _ws, cx| cx.notify()).detach();
        }

        let command_center = cx.new(|cx| CommandCenter::new(cx));
        cx.observe(&command_center, |this, cc, cx| {
            this.handle_command_center_update(&cc, cx);
        })
        .detach();

        Self::register_quit_handler(cx);
        Self::start_diff_watcher(diff_panel.clone(), cx);
        cx.observe_global::<AppSettings>(|_this, cx| cx.notify()).detach();

        Self {
            workspaces,
            active_workspace_ix,
            sidebar_collapsed,
            sidebar_width,
            resizing_sidebar: false,
            diff_panel_collapsed,
            diff_panel,
            resizing_diff_panel: false,
            settings_panel_open: false,
            settings_panel,
            command_center,
            picker_open: false,
            focus_handle: cx.focus_handle(),
            window_bounds,
            recent_projects: RecentProjects::load(),
        }
    }

    fn register_quit_handler(cx: &mut Context<Self>) {
        cx.on_app_quit(|app, cx| {
            crate::session::save_session(app, &*cx);
            async {}
        })
        .detach();

        // Auto-save session every 5 seconds so cargo-watch restarts don't lose state
        cx.spawn(async |app, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
                let should_continue = cx
                    .update(|cx| {
                        app.update(cx, |app, cx| {
                            crate::session::save_session(app, &*cx);
                        })
                        .is_ok()
                    })
                    .unwrap_or(false);
                if !should_continue {
                    break;
                }
            }
        })
        .detach();
    }

    fn start_diff_watcher(diff_panel: Entity<GitDiffPanel>, cx: &mut Context<Self>) {
        // Get the .git directory to watch
        let git_dir = diff_panel.read(cx).git_dir();
        let Some(git_dir) = git_dir else { return };

        // Create a channel-based file watcher using the `notify` crate.
        // We watch the .git dir for changes to index, HEAD, refs, etc.
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = match notify::recommended_watcher(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    // Only care about writes/creates/removes
                    use notify::EventKind;
                    match event.kind {
                        EventKind::Create(_)
                        | EventKind::Modify(_)
                        | EventKind::Remove(_) => {
                            let _ = tx.send(());
                        }
                        _ => {}
                    }
                }
            },
        ) {
            Ok(w) => w,
            Err(_) => return,
        };

        // Watch the .git directory (index, HEAD, refs changes)
        use notify::Watcher;
        let _ = watcher.watch(&git_dir, notify::RecursiveMode::Recursive);

        // Spawn a background thread that blocks on the watcher channel,
        // then pokes the UI when something changes.
        let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
        cx.spawn(async move |_app, cx| {
            // Keep the watcher alive for the lifetime of this task
            let _watcher = watcher;
            loop {
                let rx = rx.clone();
                let got_event = cx
                    .background_executor()
                    .spawn(async move {
                        let rx = rx.lock().unwrap();
                        // Wait for first event (blocking)
                        if rx.recv().is_err() {
                            return false;
                        }
                        // Drain any queued events to coalesce
                        while rx.try_recv().is_ok() {}
                        true
                    })
                    .await;

                if !got_event {
                    break;
                }

                // Small debounce so rapid FS events don't cause excessive refreshes
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;

                let ok = cx.update(|cx| {
                    diff_panel.update(cx, |panel, cx| {
                        panel.refresh();
                        cx.notify();
                    })
                });
                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn active_workspace(&self) -> &Entity<Workspace> {
        &self.workspaces[self.active_workspace_ix]
    }

    /// Remove the active workspace, adjusting the index or quitting if none remain.
    fn remove_active_workspace(&mut self, cx: &mut Context<Self>) {
        self.workspaces.remove(self.active_workspace_ix);
        if self.workspaces.is_empty() {
            // Instead of quitting, create a fresh uninitialized workspace
            let ws = cx.new(|cx| Workspace::new_empty(cx));
            cx.observe(&ws, |_this, _ws, cx| cx.notify()).detach();
            self.workspaces.push(ws);
            self.active_workspace_ix = 0;
            self.diff_panel.update(cx, |panel, cx| {
                *panel = GitDiffPanel::empty();
                cx.notify();
            });
            cx.notify();
            return;
        }
        if self.active_workspace_ix >= self.workspaces.len() {
            self.active_workspace_ix = self.workspaces.len() - 1;
        }
        cx.notify();
    }

    fn new_workspace(&mut self, _: &NewWorkspace, _window: &mut Window, cx: &mut Context<Self>) {
        let ws = cx.new(|cx| Workspace::new_empty(cx));
        cx.observe(&ws, |_this, _ws, cx| cx.notify()).detach();
        self.workspaces.push(ws);
        self.active_workspace_ix = self.workspaces.len() - 1;
        cx.notify();
    }

    fn new_tab(&mut self, _: &NewTab, _window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.active_workspace().clone();
        ws.update(cx, |ws, cx| ws.add_tab(cx));
    }

    fn close_tab(&mut self, _: &CloseTab, _window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.active_workspace().clone();

        // If workspace has no directory yet (welcome screen), close it —
        // unless it's the last one, then do nothing.
        if !ws.read(cx).has_directory() {
            if self.workspaces.len() > 1 {
                self.remove_active_workspace(cx);
            }
            return;
        }

        // If workspace already has zero tabs, Cmd+W means "close workspace"
        if ws.read(cx).total_tab_count(cx) == 0 {
            self.remove_active_workspace(cx);
            return;
        }

        // Try smart close (handles sub-tabs first, then outer tabs)
        ws.update(cx, |ws, cx| ws.close_tab(cx));
        // Leave the workspace open even if tabs reach 0
        cx.notify();
    }

    fn next_tab(&mut self, _: &NextTab, _window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.active_workspace().clone();
        ws.update(cx, |ws, cx| ws.next_tab(cx));
    }

    fn prev_tab(&mut self, _: &PrevTab, _window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.active_workspace().clone();
        ws.update(cx, |ws, cx| ws.prev_tab(cx));
    }

    fn split_pane(&mut self, _: &SplitPane, _window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.active_workspace().clone();
        ws.update(cx, |ws, cx| ws.split_active_pane(cx));
    }

    fn split_pane_vertical(
        &mut self,
        _: &SplitPaneVertical,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ws = self.active_workspace().clone();
        ws.update(cx, |ws, cx| ws.split_active_pane_vertical(cx));
    }

    fn new_editor_tab(&mut self, _: &NewEditorTab, _window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.active_workspace().clone();
        ws.update(cx, |ws, cx| ws.add_editor_tab(cx));
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        cx.notify();
    }

    fn toggle_diff_panel(
        &mut self,
        _: &ToggleDiffPanel,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_diff_panel_inner(cx);
    }

    fn toggle_diff_panel_inner(&mut self, cx: &mut Context<Self>) {
        self.diff_panel_collapsed = !self.diff_panel_collapsed;
        if !self.diff_panel_collapsed {
            self.diff_panel.update(cx, |panel, cx| {
                panel.refresh();
                cx.notify();
            });
        }
        cx.notify();
    }

    fn toggle_settings(
        &mut self,
        _: &ToggleSettings,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = Bounds::centered(None, size(px(400.0), px(400.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Settings".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(9.0), px(9.0))),
                }),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| SettingsPanel::new(cx)),
        )
        .ok();
    }

    fn toggle_command_center(
        &mut self,
        _: &ToggleCommandCenter,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.command_center.update(cx, |cc, cx| cc.toggle(cx));
    }

    fn search_workspace(
        &mut self,
        _: &SearchWorkspace,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dir = self
            .active_workspace()
            .read(cx)
            .directory
            .clone();
        self.command_center.update(cx, |cc, cx| {
            cc.search_root = dir;
            cc.show_workspace_search_mode(cx);
        });
    }

    /// Called when the command center entity notifies (e.g. clone complete, action selected).
    fn handle_command_center_update(
        &mut self,
        cc: &Entity<CommandCenter>,
        cx: &mut Context<Self>,
    ) {
        // Check for completed clone
        let cloned_dir = cc.read(cx).cloned_dir.clone();
        if let Some(dir) = cloned_dir {
            cc.update(cx, |cc, cx| {
                cc.cloned_dir = None;
                cc.dismiss(cx);
            });
            self.open_directory(dir, cx);
            return;
        }

        // Check for pending search result (workspace grep)
        let pending_result = cc.read(cx).pending_search_result.clone();
        if let Some(result) = pending_result {
            cc.update(cx, |cc, cx| {
                cc.pending_search_result = None;
                cc.dismiss(cx);
            });
            self.open_search_result(result, cx);
            return;
        }

        // Check for pending command action
        let pending = cc.read(cx).pending_action.clone();
        if let Some(action) = pending {
            cc.update(cx, |cc, cx| {
                cc.pending_action = None;
                cc.dismiss(cx);
            });
            match action {
                CommandAction::OpenProject => {
                    self.open_project_picker(cx);
                }
                CommandAction::CloneRepo => {
                    self.command_center.update(cx, |cc, cx| {
                        cc.show_clone_mode(cx);
                    });
                }
                CommandAction::NewTerminalTab => {
                    let ws = self.active_workspace().clone();
                    ws.update(cx, |ws, cx| ws.add_tab(cx));
                }
                CommandAction::NewEditorTab => {
                    let ws = self.active_workspace().clone();
                    ws.update(cx, |ws, cx| ws.add_editor_tab(cx));
                }
                CommandAction::ToggleSidebar => {
                    self.sidebar_collapsed = !self.sidebar_collapsed;
                    cx.notify();
                }
                CommandAction::ToggleDiffPanel => {
                    self.toggle_diff_panel_inner(cx);
                }
                CommandAction::ToggleSettings => {
                    self.settings_panel_open = !self.settings_panel_open;
                    cx.notify();
                }
            }
        }
    }

    /// Open a file from a workspace search result in the editor.
    fn open_search_result(
        &mut self,
        result: crate::command_center::SearchResult,
        cx: &mut Context<Self>,
    ) {
        let ws = self.active_workspace().clone();
        let has_dir = ws.read(cx).has_directory();
        if !has_dir {
            return;
        }

        // Ensure there's an editor tab, open the file, and navigate to the line
        ws.update(cx, |ws, cx| {
            // If no editor tab exists, create one
            if let Some(layout) = &ws.layout {
                layout.update(cx, |pg, cx| {
                    // Try to find an existing editor tab, or add one
                    let has_editor = pg.root.has_editor_tab(cx);
                    if !has_editor {
                        if let Some(dir) = &ws.directory {
                            pg.root.add_editor_tab(dir.clone(), cx);
                        }
                    }
                    // Now open the file in the editor tab and navigate to line
                    pg.root.open_file_in_editor(result.path.clone(), result.line_num, cx);
                    cx.notify();
                });
                cx.notify();
            }
        });
    }

    /// Open a directory in the active workspace (or set it if empty).
    pub fn open_directory(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        // Track in recent projects (update cached copy and persist)
        self.recent_projects.add(dir.clone());

        let ws = self.active_workspace().clone();
        if !ws.read(cx).has_directory() {
            ws.update(cx, |ws, cx| ws.set_directory(dir.clone(), cx));
        } else {
            // Create a new workspace for this directory
            let dir_name = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Workspace".to_string());
            let new_ws = cx.new(|cx| Workspace::new(&dir_name, dir.clone(), cx));
            cx.observe(&new_ws, |_this, _ws, cx| cx.notify()).detach();
            self.workspaces.push(new_ws);
            self.active_workspace_ix = self.workspaces.len() - 1;
        }
        self.update_diff_panel_dir(dir, cx);
        cx.notify();
    }

    /// Point the diff panel at a new working directory and restart the file watcher.
    fn update_diff_panel_dir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        self.diff_panel.update(cx, |panel, cx| {
            let w = panel.width;
            *panel = GitDiffPanel::new(dir);
            panel.width = w;
            cx.notify();
        });
        Self::start_diff_watcher(self.diff_panel.clone(), cx);
    }

    /// Open the OS directory picker and set directory on the active workspace.
    fn open_project_picker(&mut self, cx: &mut Context<Self>) {
        if self.picker_open {
            return;
        }
        self.picker_open = true;

        let paths_rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async |this, cx| {
            if let Ok(Ok(Some(paths))) = paths_rx.await {
                if let Some(dir) = paths.into_iter().next() {
                    let _ = cx.update(|cx| {
                        let _ = this.update(cx, |app, cx| {
                            app.open_directory(dir, cx);
                        });
                    });
                }
            }
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |app, cx| {
                    app.picker_open = false;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn render_center(&self, cx: &mut Context<Self>) -> AnyElement {
        let ws_entity = self.active_workspace().clone();
        let ws = ws_entity.read(cx);

        if let Some(layout_entity) = &ws.layout {
            let layout = layout_entity.read(cx);
            layout.render_tree(layout_entity, cx)
        } else {
            // Welcome screen — no directory selected yet
            self.render_welcome_screen(cx)
        }
    }

    fn render_welcome_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let app_entity = cx.entity().clone();
        let app_open = cx.entity().clone();
        let app_clone = cx.entity().clone();

        let recent_paths = self.recent_projects.paths.clone();
        let recent_count = recent_paths.len();

        let mut content = div()
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .items_center()
            .bg(colors::bg())
            .pt(px(80.0));

        // Title
        content = content
            .child(
                div()
                    .text_color(colors::text())
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .child("Operator"),
            )
            .child(
                div()
                    .text_color(colors::text_muted())
                    .text_sm()
                    .mt_1()
                    .mb_8()
                    .child("Your development workspace"),
            );

        // Action cards row
        content = content.child(
            div()
                .flex()
                .flex_row()
                .gap_3()
                .mb_8()
                // Open Project card
                .child(
                    div()
                        .id("welcome-open-project")
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .w(px(180.0))
                        .h(px(90.0))
                        .rounded_lg()
                        .bg(colors::surface())
                        .border_1()
                        .border_color(colors::border())
                        .cursor_pointer()
                        .hover(|s| s.bg(colors::surface_hover()))
                        .gap_2()
                        .child(
                            div()
                                .text_color(colors::text_muted())
                                .text_base()
                                .child("\u{1F4C2}"), // folder icon
                        )
                        .child(
                            div()
                                .text_color(colors::text())
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child("Open Project"),
                        )
                        .on_click(move |_, _window, cx| {
                            app_open.update(cx, |app, cx| {
                                app.open_project_picker(cx);
                            });
                        }),
                )
                // Clone Repo card
                .child(
                    div()
                        .id("welcome-clone-repo")
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .w(px(180.0))
                        .h(px(90.0))
                        .rounded_lg()
                        .bg(colors::surface())
                        .border_1()
                        .border_color(colors::border())
                        .cursor_pointer()
                        .hover(|s| s.bg(colors::surface_hover()))
                        .gap_2()
                        .child(
                            div()
                                .text_color(colors::text_muted())
                                .text_base()
                                .child("\u{2B07}\u{FE0F}"), // download icon
                        )
                        .child(
                            div()
                                .text_color(colors::text())
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child("Clone Repo"),
                        )
                        .on_click(move |_, _window, cx| {
                            app_clone.update(cx, |app, cx| {
                                app.command_center.update(cx, |cc, cx| {
                                    cc.show_clone_mode(cx);
                                });
                            });
                        }),
                ),
        );

        // Recent projects section
        if !recent_paths.is_empty() {
            let display_count = recent_paths.len().min(8);
            let display_paths: Vec<PathBuf> = recent_paths[..display_count].to_vec();

            let mut section = div()
                .flex()
                .flex_col()
                .w(px(380.0));

            // Header row
            section = section.child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .items_center()
                    .mb_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors::text_muted())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Recent projects"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors::text_muted())
                            .child(format!("View all ({recent_count})")),
                    ),
            );

            for (ix, path) in display_paths.iter().enumerate() {
                let dir_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string_lossy().to_string());

                let short = short_path(path);
                let path_clone = path.clone();
                let app_for_recent = app_entity.clone();

                section = section.child(
                    div()
                        .id(ElementId::Name(format!("recent-{ix}").into()))
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(colors::surface_hover()))
                        .child(
                            div()
                                .text_sm()
                                .text_color(colors::text())
                                .child(dir_name),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(colors::text_muted())
                                .child(short),
                        )
                        .on_click(move |_, _window, cx| {
                            let dir = path_clone.clone();
                            app_for_recent.update(cx, |app, cx| {
                                app.open_directory(dir, cx);
                            });
                        }),
                );
            }

            content = content.child(section);
        }

        content.into_any_element()
    }
}

impl Render for OperatorApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Cache window bounds for session persistence
        let bounds = window.bounds();
        self.window_bounds = Some((
            f32::from(bounds.origin.x),
            f32::from(bounds.origin.y),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
        ));

        // Refresh claude status from terminal output for each workspace
        for ws_entity in &self.workspaces {
            ws_entity.update(cx, |ws, cx| {
                ws.refresh_claude_status(cx);
            });
        }

        // Collect workspace card data for sidebar
        let ws_cards: Vec<WorkspaceCardData> = self
            .workspaces
            .iter()
            .map(|ws| {
                let ws = ws.read(cx);
                WorkspaceCardData {
                    name: ws.name.clone(),
                    directory: ws.short_dir(),
                    git_branch: ws.git_branch.clone(),
                    claude_status: ws.claude_status.clone(),
                }
            })
            .collect();

        let app_entity = cx.entity().clone();
        let app_entity2 = app_entity.clone();

        let sidebar_width = self.sidebar_width;
        let sidebar = if !self.sidebar_collapsed {
            Some(WorkspaceSidebar::render_with_width(
                &ws_cards,
                self.active_workspace_ix,
                Rc::new(move |ix, _window, cx| {
                    app_entity.update(cx, |app, cx| {
                        app.active_workspace_ix = ix;
                        let dir = app.workspaces[ix].read(cx).directory.clone();
                        if let Some(dir) = dir {
                            app.update_diff_panel_dir(dir, cx);
                        } else {
                            app.diff_panel.update(cx, |panel, cx| {
                                *panel = GitDiffPanel::empty();
                                cx.notify();
                            });
                        }
                        cx.notify();
                    });
                }),
                Rc::new(move |_window, cx| {
                    app_entity2.update(cx, |app, cx| {
                        let ws = cx.new(|cx| Workspace::new_empty(cx));
                        cx.observe(&ws, |_this, _ws, cx| cx.notify()).detach();
                        app.workspaces.push(ws);
                        app.active_workspace_ix = app.workspaces.len() - 1;
                        cx.notify();
                    });
                }),
                sidebar_width,
            ))
        } else {
            None
        };

        let center = self.render_center(cx);
        let diff_panel = if !self.diff_panel_collapsed {
            Some(self.diff_panel.clone())
        } else {
            None
        };

        let app_resize_move = cx.entity().clone();
        let app_resize_up = cx.entity().clone();

        let mut root = div()
            .id("operator-app-root")
            .flex()
            .flex_row()
            .size_full()
            .bg(colors::bg())
            .text_color(colors::text())
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::new_workspace))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::split_pane))
            .on_action(cx.listener(Self::split_pane_vertical))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_diff_panel))
            .on_action(cx.listener(Self::toggle_settings))
            .on_action(cx.listener(Self::toggle_command_center))
            .on_action(cx.listener(Self::new_editor_tab))
            .on_action(cx.listener(Self::search_workspace))
            .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
                app_resize_move.update(cx, |app, cx| {
                    let x = f32::from(event.position.x);
                    if app.resizing_sidebar {
                        app.sidebar_width = x.clamp(120.0, 500.0);
                        cx.notify();
                    }
                    if app.resizing_diff_panel {
                        // Diff panel is on the right: width = window_width - mouse_x
                        let window_width = f32::from(window.bounds().size.width);
                        let new_width = (window_width - x).clamp(200.0, window_width - 100.0);
                        app.diff_panel.update(cx, |panel, cx| {
                            panel.width = new_width;
                            cx.notify();
                        });
                        cx.notify();
                    }
                });
            })
            .on_mouse_up(MouseButton::Left, move |_event: &MouseUpEvent, _window, cx| {
                app_resize_up.update(cx, |app, cx| {
                    if app.resizing_sidebar || app.resizing_diff_panel {
                        app.resizing_sidebar = false;
                        app.resizing_diff_panel = false;
                        cx.notify();
                    }
                });
            });

        if let Some(sb) = sidebar {
            root = root.child(sb);

            // Resize handle for sidebar
            let app_resize_down = cx.entity().clone();
            root = root.child(
                div()
                    .id("sidebar-resize-handle")
                    .w(px(12.0))
                    .mx(px(-4.0))
                    .h_full()
                    .flex_shrink_0()
                    .cursor_col_resize()
                    .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                        app_resize_down.update(cx, |app, cx| {
                            app.resizing_sidebar = true;
                            cx.notify();
                        });
                    }),
            );
        }
        root = root.child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(100.0))
                .h_full()
                .overflow_hidden()
                .pt(px(36.0))
                .child(center),
        );
        if let Some(dp) = diff_panel {
            // Resize handle for diff panel (on its left edge)
            let app_resize_diff = cx.entity().clone();
            root = root.child(
                div()
                    .id("diff-resize-handle")
                    .w(px(12.0))
                    .mx(px(-4.0))
                    .h_full()
                    .flex_shrink_0()
                    .cursor_col_resize()
                    .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                        app_resize_diff.update(cx, |app, cx| {
                            app.resizing_diff_panel = true;
                            cx.notify();
                        });
                    }),
            );
            root = root.child(dp);
        }

        // Command center overlay (always rendered on top)
        let cc = self.command_center.clone();
        root = root.child(cc);

        root
    }
}

fn short_path(path: &PathBuf) -> String {
    crate::util::short_path(path)
}
