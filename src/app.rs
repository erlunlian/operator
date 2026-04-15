use gpui::*;
use std::path::PathBuf;
use std::rc::Rc;

use crate::actions::*;
use crate::command_center::{CommandAction, CommandCenter};
use crate::debug::DebugPanel;
use crate::debug::metrics::SubsystemMetrics;
use crate::git::{GitDiffPanel, PrDiffPanel};
use crate::recent_projects::RecentProjects;
use crate::right_panel::{RightPanel, RightPanelTab};
use crate::settings::AppSettings;
use crate::settings::settings_panel::SettingsPanel;
use crate::theme::colors;
use crate::util;
use crate::workspace::sidebar::WorkspaceCardData;
use crate::workspace::{Workspace, WorkspaceSidebar};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FocusRegion {
    Center,
    RightPanel,
}

// Layout constraints
const MIN_SIDEBAR_WIDTH: f32 = 120.0;
const MAX_SIDEBAR_WIDTH: f32 = 500.0;
const MIN_RIGHT_PANEL_WIDTH: f32 = 200.0;
const MIN_CENTER_WIDTH: f32 = 100.0;

pub struct OperatorApp {
    pub workspaces: Vec<Entity<Workspace>>,
    pub active_workspace_ix: usize,
    pub sidebar_collapsed: bool,
    pub sidebar_width: f32,
    resizing_sidebar: bool,
    pub right_panel_collapsed: bool,
    pub right_panel: Entity<RightPanel>,
    resizing_right_panel: bool,
    pub settings_panel_open: bool,
    pub _settings_panel: Entity<SettingsPanel>,
    pub command_center: Entity<CommandCenter>,
    picker_open: bool,
    focus_handle: FocusHandle,
    /// Which region of the app has focus (center terminals or right panel).
    pub focus_region: FocusRegion,
    /// Cached window bounds (x, y, w, h) for session persistence.
    pub window_bounds: Option<(f32, f32, f32, f32)>,
    /// Cached recent projects (avoid disk reads in render path).
    recent_projects: RecentProjects,
    /// When true, shows a quit confirmation dialog.
    quit_requested: bool,
    /// Drop indicator index for workspace sidebar drag reorder.
    ws_drop_index: Option<usize>,
    /// Available update info (checked on startup).
    pub update_info: Option<crate::updater::UpdateInfo>,
    /// Handle to the current diff file-watcher task. Dropped (cancelled) before
    /// starting a new watcher so old watchers don't accumulate.
    diff_watcher_task: Option<Task<()>>,
    /// Debug overlay showing live process metrics.
    debug_panel: Entity<DebugPanel>,
    /// Background task that periodically logs metrics to stderr.
    _metrics_log_task: Task<()>,
}

impl OperatorApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let ws = cx.new(|cx| Workspace::new_empty(cx));
        cx.observe(&ws, |_this, _ws, cx| cx.notify()).detach();

        let diff_panel = cx.new(|_cx| GitDiffPanel::empty());
        let pr_diff_panel = cx.new(|_cx| PrDiffPanel::empty());
        let right_panel = cx.new(|_cx| RightPanel::new(diff_panel, pr_diff_panel));

        let settings_panel = cx.new(|cx| SettingsPanel::new(cx));
        let command_center = cx.new(|cx| CommandCenter::new(cx));
        cx.observe(&command_center, |this, cc, cx| {
            this.handle_command_center_update(&cc, cx);
        })
        .detach();

        Self::register_quit_handler(cx);
        let diff_watcher_task = Self::start_diff_watcher_from_right_panel(right_panel.clone(), cx);
        let debug_panel = cx.new(|cx| DebugPanel::new(cx));
        let metrics_log_task = Self::start_metrics_logging(cx);
        cx.observe_global::<AppSettings>(|_this, cx| cx.notify()).detach();

        let mut app = Self {
            workspaces: vec![ws],
            active_workspace_ix: 0,
            sidebar_collapsed: false,
            sidebar_width: 260.0,
            resizing_sidebar: false,
            right_panel_collapsed: false,
            right_panel,
            resizing_right_panel: false,
            settings_panel_open: false,
            _settings_panel: settings_panel,
            command_center,
            picker_open: false,
            focus_handle: cx.focus_handle(),
            focus_region: FocusRegion::Center,
            window_bounds: None,
            recent_projects: RecentProjects::load(),
            quit_requested: false,
            ws_drop_index: None,
            update_info: None,
            diff_watcher_task,
            debug_panel,
            _metrics_log_task: metrics_log_task,
        };
        app.check_for_updates(cx, false);
        app
    }

    pub fn from_restored(
        workspaces: Vec<Entity<Workspace>>,
        active_workspace_ix: usize,
        sidebar_collapsed: bool,
        sidebar_width: f32,
        right_panel_collapsed: bool,
        window_bounds: Option<(f32, f32, f32, f32)>,
        right_panel: Entity<RightPanel>,
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
        let diff_watcher_task = Self::start_diff_watcher_from_right_panel(right_panel.clone(), cx);
        let debug_panel = cx.new(|cx| DebugPanel::new(cx));
        let metrics_log_task = Self::start_metrics_logging(cx);
        cx.observe_global::<AppSettings>(|_this, cx| cx.notify()).detach();

        let mut app = Self {
            workspaces,
            active_workspace_ix,
            sidebar_collapsed,
            sidebar_width,
            resizing_sidebar: false,
            right_panel_collapsed,
            right_panel,
            resizing_right_panel: false,
            settings_panel_open: false,
            _settings_panel: settings_panel,
            command_center,
            picker_open: false,
            focus_handle: cx.focus_handle(),
            focus_region: FocusRegion::Center,
            window_bounds,
            recent_projects: RecentProjects::load(),
            quit_requested: false,
            ws_drop_index: None,
            update_info: None,
            diff_watcher_task,
            debug_panel,
            _metrics_log_task: metrics_log_task,
        };
        app.check_for_updates(cx, false);
        app
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

    /// Count total terminal tabs across all workspaces.
    fn count_terminals(&self, cx: &App) -> usize {
        self.workspaces
            .iter()
            .map(|ws| {
                let ws = ws.read(cx);
                ws.layout
                    .as_ref()
                    .map(|pg| {
                        crate::pane::pane_group::count_total_tabs(&pg.read(cx).root)
                    })
                    .unwrap_or(0)
            })
            .sum()
    }

    /// Collect per-subsystem memory metrics from the app state.
    fn collect_subsystem_metrics(&self, cx: &App) -> SubsystemMetrics {
        use crate::pane::pane_group::collect_all_tabs;
        use crate::tab::tab::TabContent;

        let mut sub = SubsystemMetrics::default();

        // Terminal grid memory across all workspaces
        for ws in &self.workspaces {
            let ws = ws.read(cx);
            if let Some(pg) = &ws.layout {
                let tabs = collect_all_tabs(&pg.read(cx).root);
                for tab in tabs {
                    let tab = tab.read(cx);
                    if let TabContent::Terminal(tv) = &tab.content {
                        let tm = tv.read(cx).terminal.read(cx);
                        let (bytes, lines, cols) = tm.estimated_grid_bytes();
                        sub.terminal_grid_bytes += bytes;
                        sub.terminal_details.push((lines, cols, bytes));
                    }
                }
            }
        }

        // Git diff panel
        let rp = self.right_panel.read(cx);
        let dp = rp.diff_panel.read(cx);
        sub.git_diff_bytes = dp.estimated_bytes();
        sub.git_diff_files = dp.file_count();

        // PR diff panel
        let pr = rp.pr_diff_panel.read(cx);
        sub.pr_diff_bytes = pr.estimated_bytes();
        sub.pr_diff_files = pr.file_count();

        sub
    }

    /// Spawn a background task that logs metrics to stderr every 30 seconds.
    fn start_metrics_logging(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async {
                    smol::Timer::after(std::time::Duration::from_secs(30)).await;
                })
                .await;

                let data = this
                    .read_with(cx, |app, cx| {
                        let tc = app.count_terminals(cx);
                        let wc = app.workspaces.len();
                        let sub = app.collect_subsystem_metrics(cx);
                        (tc, wc, sub)
                    })
                    .ok();

                if let Some((tc, wc, sub)) = data {
                    let m = crate::debug::ProcessMetrics::collect(tc, wc, sub);
                    log::info!(
                        "[perf] rss={} threads={} terminals={} grid_mem={} git_diff={} ({} files) pr_diff={} ({} files) tracked={} untracked={} workspaces={}",
                        m.resident_display(),
                        m.thread_count,
                        m.terminal_count,
                        crate::debug::metrics::format_bytes(m.subsystems.terminal_grid_bytes as u64),
                        crate::debug::metrics::format_bytes(m.subsystems.git_diff_bytes as u64),
                        m.subsystems.git_diff_files,
                        crate::debug::metrics::format_bytes(m.subsystems.pr_diff_bytes as u64),
                        m.subsystems.pr_diff_files,
                        crate::debug::metrics::format_bytes(m.tracked_total() as u64),
                        crate::debug::metrics::format_bytes(m.untracked_bytes()),
                        m.workspace_count,
                    );
                    // Log per-terminal details at debug level
                    for (i, (lines, cols, bytes)) in m.subsystems.terminal_details.iter().enumerate() {
                        log::debug!(
                            "[perf]   terminal[{i}]: {lines} lines x {cols} cols = {}",
                            crate::debug::metrics::format_bytes(*bytes as u64),
                        );
                    }
                }
            }
        })
    }

    fn start_diff_watcher_from_right_panel(right_panel: Entity<RightPanel>, cx: &mut Context<Self>) -> Option<Task<()>> {
        let diff_panel = right_panel.read(cx).diff_panel.clone();
        Self::start_diff_watcher(diff_panel, cx)
    }

    fn start_diff_watcher(diff_panel: Entity<GitDiffPanel>, cx: &mut Context<Self>) -> Option<Task<()>> {
        // Get the .git directory to watch
        let git_dir = diff_panel.read(cx).git_dir();
        let Some(git_dir) = git_dir else { return None };

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
            Err(_) => return None,
        };

        // Watch the .git directory (index, HEAD, refs changes)
        use notify::Watcher;
        let _ = watcher.watch(&git_dir, notify::RecursiveMode::Recursive);

        // Spawn a background thread that blocks on the watcher channel,
        // then pokes the UI when something changes.
        let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
        let task = cx.spawn(async move |_app, cx| {
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
        });
        Some(task)
    }

    fn check_for_updates_manual(&mut self, _: &CheckForUpdates, _window: &mut Window, cx: &mut Context<Self>) {
        self.check_for_updates(cx, true);
    }

    fn check_for_updates(&mut self, cx: &mut Context<Self>, force: bool) {
        let current = env!("CARGO_PKG_VERSION").to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::updater::check_for_update(&current, force) })
                .await;
            if let Some(info) = result {
                let _ = cx.update(|cx| {
                    let _ = this.update(cx, |app, cx| {
                        app.update_info = Some(info);
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    pub fn active_workspace(&self) -> &Entity<Workspace> {
        &self.workspaces[self.active_workspace_ix]
    }

    /// Remove the active workspace, adjusting the index or quitting if none remain.
    fn remove_active_workspace(&mut self, cx: &mut Context<Self>) {
        self.cache_editor_files(cx);
        self.workspaces.remove(self.active_workspace_ix);
        if self.workspaces.is_empty() {
            // Stop the file watcher — no workspace means no .git dir to watch
            self.diff_watcher_task = None;

            // Instead of quitting, create a fresh uninitialized workspace
            let ws = cx.new(|cx| Workspace::new_empty(cx));
            cx.observe(&ws, |_this, _ws, cx| cx.notify()).detach();
            self.workspaces.push(ws);
            self.active_workspace_ix = 0;
            self.right_panel.update(cx, |rp, cx| {
                rp.diff_panel.update(cx, |panel, cx| {
                    *panel = GitDiffPanel::empty();
                    cx.notify();
                });
                rp.pr_diff_panel.update(cx, |panel, cx| {
                    *panel = PrDiffPanel::empty();
                    cx.notify();
                });
                rp.editor = None;
                cx.notify();
            });
            cx.notify();
            return;
        }
        if self.active_workspace_ix >= self.workspaces.len() {
            self.active_workspace_ix = self.workspaces.len() - 1;
        }
        // Restore editor files for the new active workspace
        let new_ix = self.active_workspace_ix;
        let dir = self.workspaces[new_ix].read(cx).directory.clone();
        if let Some(dir) = dir {
            self.update_right_panel_dir(dir, cx);
            self.restore_editor_files_for_workspace(new_ix, cx);
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

    /// Cache the current editor's open files into the active workspace,
    /// so they survive workspace switches and session saves.
    fn cache_editor_files(&self, cx: &mut Context<Self>) {
        let files = self.right_panel.read(cx)
            .editor.as_ref()
            .map(|e| e.read(cx).all_open_files(cx))
            .unwrap_or_default();
        if let Some(ws) = self.workspaces.get(self.active_workspace_ix) {
            ws.update(cx, |ws, _cx| {
                ws.cached_editor_files = files;
            });
        }
    }

    /// Cache the right panel's active tab and width into the active workspace.
    /// Cache the right panel and sidebar state into the active workspace.
    fn cache_right_panel_state(&self, cx: &mut Context<Self>) {
        let rp = self.right_panel.read(cx);
        let tab = match rp.active_tab {
            RightPanelTab::Files => "files",
            RightPanelTab::Git => "git",
            RightPanelTab::Pr => "pr",
        };
        let width = rp.width;
        let sb_collapsed = self.sidebar_collapsed;
        let rp_collapsed = self.right_panel_collapsed;
        if let Some(ws) = self.workspaces.get(self.active_workspace_ix) {
            ws.update(cx, |ws, _cx| {
                ws.cached_right_panel_tab = Some(tab.to_string());
                ws.cached_right_panel_width = Some(width);
                ws.cached_sidebar_collapsed = Some(sb_collapsed);
                ws.cached_right_panel_collapsed = Some(rp_collapsed);
            });
        }
    }

    /// Restore the right panel and sidebar state from the given workspace.
    fn restore_right_panel_state(&mut self, ws_ix: usize, cx: &mut Context<Self>) {
        let (tab, width, sb_collapsed, rp_collapsed) = {
            let Some(ws) = self.workspaces.get(ws_ix) else { return };
            let ws = ws.read(cx);
            (
                ws.cached_right_panel_tab.clone(),
                ws.cached_right_panel_width,
                ws.cached_sidebar_collapsed,
                ws.cached_right_panel_collapsed,
            )
        };
        self.right_panel.update(cx, |rp, cx| {
            if let Some(ref tab_str) = tab {
                rp.active_tab = match tab_str.as_str() {
                    "files" => RightPanelTab::Files,
                    "pr" => RightPanelTab::Pr,
                    _ => RightPanelTab::Git,
                };
            }
            if let Some(w) = width {
                rp.width = w;
            }
            cx.notify();
        });
        if let Some(sc) = sb_collapsed {
            self.sidebar_collapsed = sc;
        }
        if let Some(rc) = rp_collapsed {
            self.right_panel_collapsed = rc;
        }
    }

    /// Restore cached editor files for the given workspace into the right panel editor.
    fn restore_editor_files_for_workspace(&self, ws_ix: usize, cx: &mut Context<Self>) {
        let (dir, files) = {
            let Some(ws) = self.workspaces.get(ws_ix) else { return };
            let ws = ws.read(cx);
            match &ws.directory {
                Some(dir) => (dir.clone(), ws.cached_editor_files.clone()),
                None => return,
            }
        };
        if files.is_empty() {
            return;
        }
        self.right_panel.update(cx, |rp, cx| {
            rp.ensure_editor(dir, cx);
            if let Some(editor) = &rp.editor {
                editor.update(cx, |editor, cx| {
                    for file_path in &files {
                        if file_path.exists() {
                            editor.open_file(file_path.clone(), cx);
                        }
                    }
                });
            }
        });
    }

    /// Whether the file editor in the right panel is currently focused.
    fn editor_focused(&self) -> bool {
        self.focus_region == FocusRegion::RightPanel && !self.right_panel_collapsed
    }

    /// Run a closure on the editor's pane group if it exists.
    fn with_editor_pane_group(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut crate::pane::PaneGroup, &mut App),
    ) {
        self.right_panel.update(cx, |rp, cx| {
            if let Some(editor) = &rp.editor {
                editor.update(cx, |ev, cx| {
                    ev.pane_group.update(cx, |pg, cx| {
                        f(pg, cx);
                        cx.notify();
                    });
                });
            }
        });
    }

    fn new_tab(&mut self, _: &NewTab, _window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_focused() {
            self.with_editor_pane_group(cx, |pg, cx| {
                pg.add_tab_to_focused(cx);
            });
        } else {
            let ws = self.active_workspace().clone();
            ws.update(cx, |ws, cx| ws.add_tab(cx));
        }
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_focused() {
            self.with_editor_pane_group(cx, |pg, cx| {
                pg.close_focused_tab(cx);
            });
            // The closed tab's focus handle was destroyed — restore focus to the
            // app root so subsequent Cmd+W keystrokes still dispatch.
            self.focus_handle.focus(window);
            return;
        }

        let ws = self.active_workspace().clone();

        // If workspace has no directory yet (welcome screen), close it —
        // unless it's the last one, then do nothing.
        if !ws.read(cx).has_directory() {
            if self.workspaces.len() > 1 {
                self.remove_active_workspace(cx);
                self.focus_handle.focus(window);
            }
            return;
        }

        // If workspace already has zero tabs, Cmd+W means "close workspace"
        if ws.read(cx).total_tab_count(cx) == 0 {
            self.remove_active_workspace(cx);
            self.focus_handle.focus(window);
            return;
        }

        // Try smart close (handles sub-tabs first, then outer tabs)
        ws.update(cx, |ws, cx| ws.close_tab(cx));
        self.focus_handle.focus(window);
        // Leave the workspace open even if tabs reach 0
        cx.notify();
    }

    fn next_tab(&mut self, _: &NextTab, _window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_focused() {
            self.with_editor_pane_group(cx, |pg, _cx| {
                pg.next_tab_in_focused();
            });
        } else {
            let ws = self.active_workspace().clone();
            ws.update(cx, |ws, cx| ws.next_tab(cx));
        }
    }

    fn prev_tab(&mut self, _: &PrevTab, _window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_focused() {
            self.with_editor_pane_group(cx, |pg, _cx| {
                pg.prev_tab_in_focused();
            });
        } else {
            let ws = self.active_workspace().clone();
            ws.update(cx, |ws, cx| ws.prev_tab(cx));
        }
    }

    fn split_pane(&mut self, _: &SplitPane, _window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_focused() {
            self.with_editor_pane_group(cx, |pg, cx| {
                pg.split(crate::pane::pane_group::SplitAxis::Horizontal, cx);
            });
        } else {
            let ws = self.active_workspace().clone();
            ws.update(cx, |ws, cx| ws.split_active_pane(cx));
        }
    }

    fn split_pane_vertical(
        &mut self,
        _: &SplitPaneVertical,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor_focused() {
            self.with_editor_pane_group(cx, |pg, cx| {
                pg.split(crate::pane::pane_group::SplitAxis::Vertical, cx);
            });
        } else {
            let ws = self.active_workspace().clone();
            ws.update(cx, |ws, cx| ws.split_active_pane_vertical(cx));
        }
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
        if self.right_panel_collapsed {
            self.show_right_panel_tab(RightPanelTab::Git, cx);
        } else {
            let current = self.right_panel.read(cx).active_tab;
            match current {
                RightPanelTab::Git => self.show_right_panel_tab(RightPanelTab::Pr, cx),
                RightPanelTab::Pr => {
                    self.right_panel_collapsed = true;
                    self.focus_region = FocusRegion::Center;
                    cx.notify();
                }
                _ => self.show_right_panel_tab(RightPanelTab::Git, cx),
            }
        }
    }

    fn toggle_files_panel(
        &mut self,
        _: &ToggleFilesPanel,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_right_panel_tab(RightPanelTab::Files, cx);
    }

    fn toggle_pr_panel(
        &mut self,
        _: &TogglePrPanel,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_right_panel_tab(RightPanelTab::Pr, cx);
    }

    fn show_right_panel_tab(&mut self, tab: RightPanelTab, cx: &mut Context<Self>) {
        let is_same_tab = self.right_panel.read(cx).active_tab == tab;
        if !self.right_panel_collapsed && is_same_tab {
            // Toggle off if already showing this tab
            self.right_panel_collapsed = true;
            self.focus_region = FocusRegion::Center;
        } else {
            self.right_panel_collapsed = false;
            self.focus_region = FocusRegion::RightPanel;
            self.right_panel.update(cx, |rp, cx| {
                rp.active_tab = tab;
                if tab == RightPanelTab::Git {
                    rp.diff_panel.update(cx, |panel, cx| {
                        panel.refresh();
                        cx.notify();
                    });
                }
                if tab == RightPanelTab::Pr {
                    rp.pr_diff_panel.update(cx, |panel, cx| {
                        panel.refresh(cx);
                    });
                }
                cx.notify();
            });
            // Ensure editor has the right directory
            if tab == RightPanelTab::Files {
                if let Some(dir) = self.active_workspace().read(cx).directory.clone() {
                    self.right_panel.update(cx, |rp, cx| {
                        rp.ensure_editor(dir, cx);
                    });
                }
            }
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_visible = self.command_center.read(cx).visible;
        if was_visible {
            // Closing — restore previous focus
            let prev = self.command_center.read(cx).previous_focus.clone();
            self.command_center.update(cx, |cc, cx| {
                cc.previous_focus = None;
                cc.toggle(cx);
            });
            if let Some(handle) = prev {
                handle.focus(window);
            }
        } else {
            // Opening — save current focus
            let prev = window.focused(cx);
            self.command_center.update(cx, |cc, cx| {
                cc.previous_focus = prev;
                cc.toggle(cx);
            });
        }
    }

    fn toggle_debug_panel(
        &mut self,
        _: &ToggleDebugPanel,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.debug_panel.update(cx, |panel, cx| {
            panel.toggle();
            cx.notify();
        });
        cx.notify();
    }

    fn request_quit(
        &mut self,
        _: &Quit,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.quit_requested = true;
        cx.notify();
    }

    fn search_workspace(
        &mut self,
        _: &SearchWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dir = self
            .active_workspace()
            .read(cx)
            .directory
            .clone();
        let prev = window.focused(cx);
        self.command_center.update(cx, |cc, cx| {
            cc.previous_focus = prev;
            cc.search_root = dir;
            cc.show_workspace_search_mode(cx);
        });
    }

    fn find_file(
        &mut self,
        _: &FindFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dir = self
            .active_workspace()
            .read(cx)
            .directory
            .clone();
        let prev = window.focused(cx);
        self.command_center.update(cx, |cc, cx| {
            cc.previous_focus = prev;
            cc.search_root = dir;
            cc.show_file_search_mode(cx);
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

        // Check for pending file search result (Cmd+P)
        let pending_file = cc.read(cx).pending_file_path.clone();
        if let Some(path) = pending_file {
            cc.update(cx, |cc, cx| {
                cc.pending_file_path = None;
                cc.dismiss(cx);
            });
            self.open_file_result(path, cx);
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
                CommandAction::ToggleFilesPanel => {
                    self.show_right_panel_tab(RightPanelTab::Files, cx);
                }
                CommandAction::ToggleSidebar => {
                    self.sidebar_collapsed = !self.sidebar_collapsed;
                    cx.notify();
                }
                CommandAction::ToggleDiffPanel => {
                    self.show_right_panel_tab(RightPanelTab::Git, cx);
                }
                CommandAction::TogglePrPanel => {
                    self.show_right_panel_tab(RightPanelTab::Pr, cx);
                }
                CommandAction::ToggleSettings => {
                    self.settings_panel_open = !self.settings_panel_open;
                    cx.notify();
                }
                CommandAction::SearchWorkspace => {
                    let dir = self.active_workspace().read(cx).directory.clone();
                    self.command_center.update(cx, |cc, cx| {
                        cc.search_root = dir;
                        cc.show_workspace_search_mode(cx);
                    });
                }
                CommandAction::FindFile => {
                    let dir = self.active_workspace().read(cx).directory.clone();
                    self.command_center.update(cx, |cc, cx| {
                        cc.search_root = dir;
                        cc.show_file_search_mode(cx);
                    });
                }
            }
        }

        // Always re-render the app when the command center state changes,
        // so focus restoration in render() fires after dismiss-via-Escape.
        cx.notify();
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

        if let Some(dir) = ws.read(cx).directory.clone() {
            self.right_panel_collapsed = false;
            self.focus_region = FocusRegion::RightPanel;
            self.right_panel.update(cx, |rp, cx| {
                rp.open_file(result.path.clone(), dir, Some(result.line_num), cx);
            });
            cx.notify();
        }
    }

    /// Open a file from a file name search result (Cmd+P) in the editor.
    fn open_file_result(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let ws = self.active_workspace().clone();
        if !ws.read(cx).has_directory() {
            return;
        }

        if let Some(dir) = ws.read(cx).directory.clone() {
            self.right_panel_collapsed = false;
            self.focus_region = FocusRegion::RightPanel;
            self.right_panel.update(cx, |rp, cx| {
                rp.open_file(path, dir, None, cx);
            });
            cx.notify();
        }
    }

    /// Open a directory in the active workspace (or set it if empty).
    pub fn open_directory(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        // Track in recent projects (update cached copy and persist)
        self.recent_projects.add(dir.clone());

        // Cache current editor files before changing directories
        self.cache_editor_files(cx);

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
        self.update_right_panel_dir(dir, cx);
        // Invalidate the file search index so it rebuilds for the new directory
        self.command_center.update(cx, |cc, _cx| {
            cc.invalidate_file_index();
        });
        cx.notify();
    }

    /// Switch to a different workspace, caching editor state and restoring the target's files.
    fn switch_to_workspace(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix == self.active_workspace_ix {
            return;
        }
        self.cache_editor_files(cx);
        self.cache_right_panel_state(cx);
        self.active_workspace_ix = ix;
        let dir = self.workspaces[ix].read(cx).directory.clone();
        // Always restore sidebar/panel collapsed states
        self.restore_right_panel_state(ix, cx);

        if let Some(dir) = dir {
            self.update_right_panel_dir(dir, cx);
            self.restore_editor_files_for_workspace(ix, cx);
        } else {
            self.right_panel.update(cx, |rp, cx| {
                rp.diff_panel.update(cx, |panel, cx| {
                    *panel = GitDiffPanel::empty();
                    cx.notify();
                });
                rp.pr_diff_panel.update(cx, |panel, cx| {
                    *panel = PrDiffPanel::empty();
                    cx.notify();
                });
                rp.editor = None;
                cx.notify();
            });
        }
        cx.notify();
    }

    /// Switch to workspace N (0-indexed) via Cmd+1..9 shortcut.
    fn activate_workspace(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.workspaces.len() {
            self.switch_to_workspace(ix, cx);
            self.focus_handle.focus(window);
        }
    }

    /// Point the right panel at a new working directory and restart the file watcher.
    fn update_right_panel_dir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        // Drop the old watcher task so it stops watching the previous .git dir
        self.diff_watcher_task = None;

        self.right_panel.update(cx, |rp, cx| {
            rp.diff_panel.update(cx, |panel, cx| {
                let w = panel.width;
                *panel = GitDiffPanel::new(dir.clone());
                panel.width = w;
                cx.notify();
            });
            rp.pr_diff_panel.update(cx, |panel, cx| {
                let w = panel.width;
                *panel = PrDiffPanel::new(dir.clone());
                panel.width = w;
                cx.notify();
            });
            rp.set_directory(dir, cx);
        });
        let diff_panel = self.right_panel.read(cx).diff_panel.clone();
        self.diff_watcher_task = Self::start_diff_watcher(diff_panel, cx);
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

    /// Cmd+O: open directory picker, always create a new workspace with the chosen directory.
    fn open_directory_in_new_workspace(&mut self, _: &OpenDirectory, _window: &mut Window, cx: &mut Context<Self>) {
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
                            app.cache_editor_files(cx);
                            app.recent_projects.add(dir.clone());

                            let dir_name = dir
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| "Workspace".to_string());
                            let new_ws = cx.new(|cx| Workspace::new(&dir_name, dir.clone(), cx));
                            cx.observe(&new_ws, |_this, _ws, cx| cx.notify()).detach();
                            app.workspaces.push(new_ws);
                            app.active_workspace_ix = app.workspaces.len() - 1;
                            app.update_right_panel_dir(dir, cx);
                            app.command_center.update(cx, |cc, _cx| {
                                cc.invalidate_file_index();
                            });
                            cx.notify();
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

    fn render_center(&self, center_focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let ws_entity = self.active_workspace().clone();
        let ws = ws_entity.read(cx);

        // When the sidebar is collapsed, the tab bar is the leftmost element
        // and needs to clear the macOS traffic light buttons (~70px).
        // 70px for traffic lights + 28px for the sidebar toggle button
        let tab_bar_left_inset = if self.sidebar_collapsed { px(98.0) } else { px(0.0) };

        if let Some(layout_entity) = &ws.layout {
            let layout = layout_entity.read(cx);
            layout.render_tree(layout_entity, center_focused, tab_bar_left_inset, cx)
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
                                .font_family(util::ICON_FONT)
                                .text_color(colors::text_muted())
                                .text_base()
                                .child("\u{f07c}"), // nf-fa-folder_open
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
                                .font_family(util::ICON_FONT)
                                .text_color(colors::text_muted())
                                .text_base()
                                .child("\u{f1d3}"), // nf-fa-git_square
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
                    .mb_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors::text_muted())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Recent projects"),
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
        let active_ws_ix = self.active_workspace_ix;
        for (i, ws_entity) in self.workspaces.iter().enumerate() {
            let is_active = i == active_ws_ix;
            ws_entity.update(cx, |ws, cx| {
                ws.refresh_claude_status(is_active, cx);
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
                    pane_statuses: ws.pane_statuses.clone(),
                }
            })
            .collect();

        let app_entity = cx.entity().clone();
        let app_entity2 = app_entity.clone();
        let app_entity3 = app_entity.clone();
        let app_entity4 = app_entity.clone();
        let app_entity5 = app_entity.clone();
        let app_entity6 = app_entity.clone();

        let sidebar_width = self.sidebar_width;
        let ws_drop_index = self.ws_drop_index;
        let sidebar = if !self.sidebar_collapsed {
            Some(WorkspaceSidebar::render_with_width(
                &ws_cards,
                self.active_workspace_ix,
                Rc::new(move |ix, _window, cx| {
                    app_entity.update(cx, |app, cx| {
                        app.switch_to_workspace(ix, cx);
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
                Some(Rc::new(move |ix, _window, cx| {
                    app_entity3.update(cx, |app, cx| {
                        if app.workspaces.len() <= 1 {
                            return; // Don't close the last workspace
                        }
                        // Cache files before removing if closing the active workspace
                        if ix == app.active_workspace_ix {
                            app.cache_editor_files(cx);
                        }
                        app.workspaces.remove(ix);
                        if app.active_workspace_ix >= app.workspaces.len() {
                            app.active_workspace_ix = app.workspaces.len() - 1;
                        } else if app.active_workspace_ix > ix {
                            app.active_workspace_ix -= 1;
                        } else if app.active_workspace_ix == ix {
                            app.active_workspace_ix = app.active_workspace_ix.min(app.workspaces.len() - 1);
                        }
                        let new_ix = app.active_workspace_ix;
                        let dir = app.workspaces[new_ix].read(cx).directory.clone();
                        if let Some(dir) = dir {
                            app.update_right_panel_dir(dir, cx);
                            app.restore_editor_files_for_workspace(new_ix, cx);
                        }
                        cx.notify();
                    });
                })),
                Some(Rc::new(move |from_ix, to_slot, _window, cx| {
                    app_entity4.update(cx, |app, cx| {
                        if from_ix >= app.workspaces.len() || to_slot > app.workspaces.len() {
                            return;
                        }
                        // to_slot is a gap index (0..=len). After removing from_ix,
                        // slots above from_ix shift down by 1.
                        let insert_ix = if from_ix < to_slot {
                            to_slot - 1
                        } else {
                            to_slot
                        };
                        if insert_ix == from_ix {
                            app.ws_drop_index = None;
                            cx.notify();
                            return;
                        }
                        let ws = app.workspaces.remove(from_ix);
                        app.workspaces.insert(insert_ix, ws);
                        // Update active index to follow the active workspace
                        if app.active_workspace_ix == from_ix {
                            app.active_workspace_ix = insert_ix;
                        } else if from_ix < app.active_workspace_ix && insert_ix >= app.active_workspace_ix {
                            app.active_workspace_ix -= 1;
                        } else if from_ix > app.active_workspace_ix && insert_ix <= app.active_workspace_ix {
                            app.active_workspace_ix += 1;
                        }
                        app.ws_drop_index = None;
                        cx.notify();
                    });
                })),
                ws_drop_index,
                Rc::new(move |ix: Option<usize>, _window: &mut Window, cx: &mut App| {
                    app_entity5.update(cx, |app, cx| {
                        if app.ws_drop_index != ix {
                            app.ws_drop_index = ix;
                            cx.notify();
                        }
                    });
                }),
                Rc::new(move |_window: &mut Window, cx: &mut App| {
                    app_entity6.update(cx, |app, cx| {
                        if app.ws_drop_index.is_some() {
                            app.ws_drop_index = None;
                            cx.notify();
                        }
                    });
                }),
                sidebar_width,
                self.update_info.as_ref(),
            ))
        } else {
            None
        };

        let center_focused = self.focus_region == FocusRegion::Center
            || self.right_panel_collapsed;
        let center = self.render_center(center_focused, cx);
        let right_panel = if !self.right_panel_collapsed {
            Some(self.right_panel.clone())
        } else {
            None
        };

        let app_resize_move = cx.entity().clone();
        let app_resize_up = cx.entity().clone();

        let mut root = div()
            .id("operator-app-root")
            .relative()
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
            .on_action(cx.listener(Self::toggle_files_panel))
            .on_action(cx.listener(Self::toggle_pr_panel))
            .on_action(cx.listener(Self::toggle_settings))
            .on_action(cx.listener(Self::toggle_command_center))
            .on_action(cx.listener(Self::find_file))
            .on_action(cx.listener(Self::search_workspace))
            .on_action(cx.listener(Self::toggle_debug_panel))
            .on_action(cx.listener(Self::request_quit))
            .on_action(cx.listener(Self::check_for_updates_manual))
            .on_action(cx.listener(Self::open_directory_in_new_workspace))
            .on_action(cx.listener(|this: &mut Self, _: &ActivateWorkspace1, window, cx| this.activate_workspace(0, window, cx)))
            .on_action(cx.listener(|this: &mut Self, _: &ActivateWorkspace2, window, cx| this.activate_workspace(1, window, cx)))
            .on_action(cx.listener(|this: &mut Self, _: &ActivateWorkspace3, window, cx| this.activate_workspace(2, window, cx)))
            .on_action(cx.listener(|this: &mut Self, _: &ActivateWorkspace4, window, cx| this.activate_workspace(3, window, cx)))
            .on_action(cx.listener(|this: &mut Self, _: &ActivateWorkspace5, window, cx| this.activate_workspace(4, window, cx)))
            .on_action(cx.listener(|this: &mut Self, _: &ActivateWorkspace6, window, cx| this.activate_workspace(5, window, cx)))
            .on_action(cx.listener(|this: &mut Self, _: &ActivateWorkspace7, window, cx| this.activate_workspace(6, window, cx)))
            .on_action(cx.listener(|this: &mut Self, _: &ActivateWorkspace8, window, cx| this.activate_workspace(7, window, cx)))
            .on_action(cx.listener(|this: &mut Self, _: &ActivateWorkspace9, window, cx| this.activate_workspace(8, window, cx)))
            .on_action(cx.listener(|_: &mut Self, _: &Minimize, window, _cx| window.minimize_window()))
            .on_action(cx.listener(|_: &mut Self, _: &Zoom, window, _cx| window.zoom_window()))
            .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
                app_resize_move.update(cx, |app, cx| {
                    let x = f32::from(event.position.x);
                    if app.resizing_sidebar {
                        app.sidebar_width = x.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
                        cx.notify();
                    }
                    if app.resizing_right_panel {
                        let window_width = f32::from(window.bounds().size.width);
                        let left_edge = if app.sidebar_collapsed { 0.0 } else { app.sidebar_width };
                        let max_right = window_width - left_edge - MIN_CENTER_WIDTH;
                        let new_width = (window_width - x).clamp(MIN_RIGHT_PANEL_WIDTH, max_right);
                        app.right_panel.update(cx, |rp, cx| {
                            rp.width = new_width;
                            cx.notify();
                        });
                        cx.notify();
                    }
                });
            })
            .on_mouse_up(MouseButton::Left, move |_event: &MouseUpEvent, _window, cx| {
                app_resize_up.update(cx, |app, cx| {
                    if app.resizing_sidebar || app.resizing_right_panel {
                        app.resizing_sidebar = false;
                        app.resizing_right_panel = false;
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
                    .mx(px(-6.0))
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
        let right_focused = self.focus_region == FocusRegion::RightPanel
            && !self.right_panel_collapsed;

        let app_focus_center = cx.entity().clone();
        let sidebar_collapsed = self.sidebar_collapsed;
        let app_toggle_sidebar = cx.entity().clone();
        let app_toggle_right = cx.entity().clone();

        root = root.child(
            div()
                .id("center-region")
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(100.0))
                .h_full()
                .overflow_hidden()
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    app_focus_center.update(cx, |app, cx| {
                        if app.focus_region != FocusRegion::Center {
                            app.focus_region = FocusRegion::Center;
                            cx.notify();
                        }
                    });
                })
                .child(center),
        );

        if let Some(rp) = right_panel {
            // Resize handle for right panel (on its left edge)
            let app_resize_right = cx.entity().clone();
            root = root.child(
                div()
                    .id("right-panel-resize-handle")
                    .w(px(12.0))
                    .mx(px(-6.0))
                    .h_full()
                    .flex_shrink_0()
                    .cursor_col_resize()
                    .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                        app_resize_right.update(cx, |app, cx| {
                            app.resizing_right_panel = true;
                            cx.notify();
                        });
                        cx.stop_propagation();
                    }),
            );

            let app_focus_right = cx.entity().clone();
            let mut right_wrapper = div()
                .id("right-panel-region")
                .flex()
                .flex_col()
                .h_full()
                .flex_shrink_0()
                .overflow_hidden()
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    app_focus_right.update(cx, |app, cx| {
                        if app.focus_region != FocusRegion::RightPanel {
                            app.focus_region = FocusRegion::RightPanel;
                            cx.notify();
                        }
                    });
                })
                .child(rp);
            if !right_focused {
                right_wrapper = right_wrapper.opacity(0.85);
            }
            root = root.child(right_wrapper);
        }

        // Command center overlay (always rendered on top)
        let cc = self.command_center.clone();
        root = root.child(cc);

        // Sidebar toggle button (floating, always on top)
        {
            let left_pos = if sidebar_collapsed {
                // Clear macOS traffic lights (~70px)
                px(70.0)
            } else {
                // Position at the right edge of the sidebar header
                px(sidebar_width - 32.0)
            };
            root = root.child(
                div()
                    .id("toggle-sidebar-btn")
                    .absolute()
                    .top(px(8.0))
                    .left(left_pos)
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(24.0))
                    .h(px(24.0))
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(colors::text_muted())
                    .hover(|s| s.bg(colors::surface_hover()).text_color(colors::text()))
                    .tooltip(|_window, cx| util::render_tooltip("Toggle Sidebar (Cmd+B)", cx))
                    .on_click(move |_, _window, cx| {
                        app_toggle_sidebar.update(cx, |app, cx| {
                            app.sidebar_collapsed = !app.sidebar_collapsed;
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .font_family(crate::util::ICON_FONT)
                            .text_size(px(14.0))
                            .child("\u{F06FD}"), // nf-cod-layout_sidebar_left
                    ),
            );
        }

        // Right panel toggle (always visible, top-right)
        root = root.child(
            div()
                .id("toggle-right-panel-btn")
                .absolute()
                .top(px(8.0))
                .right(px(8.0))
                .flex()
                .items_center()
                .justify_center()
                .w(px(24.0))
                .h(px(24.0))
                .rounded_md()
                .cursor_pointer()
                .text_color(colors::text_muted())
                .hover(|s| s.bg(colors::surface_hover()).text_color(colors::text()))
                .tooltip(|_window, cx| util::render_tooltip("Toggle Right Panel (Cmd+E)", cx))
                .on_click(move |_, _window, cx| {
                    app_toggle_right.update(cx, |app, cx| {
                        app.right_panel_collapsed = !app.right_panel_collapsed;
                        cx.notify();
                    });
                })
                .child(
                    div()
                        .font_family(crate::util::ICON_FONT)
                        .text_size(px(14.0))
                        .child("\u{F06FE}"), // nf-cod-layout_sidebar_right
                ),
        );

        // Debug overlay (update counts and subsystem data only when visible)
        {
            let visible = self.debug_panel.read(cx).visible;
            if visible {
                let tc = self.count_terminals(cx);
                let wc = self.workspaces.len();
                let sub = self.collect_subsystem_metrics(cx);
                self.debug_panel.update(cx, |panel, _cx| {
                    panel.update_subsystems(tc, wc, sub);
                });
            }
            root = root.child(self.debug_panel.clone());
        }

        // Quit confirmation dialog
        if self.quit_requested {
            let entity_confirm = cx.entity().clone();
            let entity_cancel = cx.entity().clone();
            let entity_backdrop = cx.entity().clone();

            root = root.child(
                div()
                    .id("quit-confirm-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(rgba(0x00000088u32))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_click(move |_, _window, cx| {
                        entity_backdrop.update(cx, |app, cx| {
                            app.quit_requested = false;
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .id("quit-confirm-dialog")
                            .w(px(320.0))
                            .bg(colors::surface())
                            .border_1()
                            .border_color(colors::border())
                            .rounded(px(8.0))
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .on_click(|_, _window, cx| {
                                cx.stop_propagation();
                            })
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors::text())
                                    .child("Quit Operator?"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors::text_muted())
                                    .child("Are you sure you want to quit? Your session will be saved."),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .justify_end()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("quit-cancel-btn")
                                            .px_3()
                                            .py(px(6.0))
                                            .rounded(px(4.0))
                                            .text_xs()
                                            .text_color(colors::text_muted())
                                            .bg(colors::surface_hover())
                                            .cursor_pointer()
                                            .hover(|s| s.text_color(colors::text()))
                                            .child("Cancel")
                                            .on_click(move |_, _window, cx| {
                                                entity_cancel.update(cx, |app, cx| {
                                                    app.quit_requested = false;
                                                    cx.notify();
                                                });
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id("quit-confirm-btn")
                                            .px_3()
                                            .py(px(6.0))
                                            .rounded(px(4.0))
                                            .text_xs()
                                            .text_color(rgb(0xffffff))
                                            .bg(colors::diff_removed())
                                            .cursor_pointer()
                                            .hover(|s| s.opacity(0.8))
                                            .child("Quit")
                                            .on_click(move |_, _window, cx| {
                                                entity_confirm.update(cx, |_app, cx| {
                                                    cx.quit();
                                                });
                                            }),
                                    ),
                            ),
                    ),
            );
        }

        root
    }
}

fn short_path(path: &PathBuf) -> String {
    crate::util::short_path(path)
}

