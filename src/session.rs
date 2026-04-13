use gpui::{AppContext, BorrowAppContext};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::pane::pane_group::{SplitAxis, SplitNode, TabGroup};
use crate::pane::PaneGroup;
use crate::right_panel::{RightPanel, RightPanelTab};
use crate::tab::tab::Tab;

/// Serializable snapshot of the entire app state.
#[derive(Serialize, Deserialize)]
pub struct SessionState {
    pub workspaces: Vec<WorkspaceState>,
    pub active_workspace_ix: usize,
    pub settings: SettingsState,
}

#[derive(Serialize, Deserialize)]
pub struct SettingsState {
    pub vim_mode: bool,
    #[serde(default = "default_theme")]
    pub theme: String,
    pub sidebar_collapsed: bool,
    pub sidebar_width: f32,
    #[serde(default)]
    pub right_panel_collapsed: bool,
    #[serde(default = "default_right_panel_width")]
    pub right_panel_width: f32,
    #[serde(default = "default_right_panel_tab")]
    pub right_panel_tab: String,
    // Legacy fields for backwards compat
    #[serde(default)]
    pub diff_panel_collapsed: bool,
    #[serde(default = "default_diff_panel_width")]
    pub diff_panel_width: f32,
    pub window_x: Option<f32>,
    pub window_y: Option<f32>,
    pub window_width: Option<f32>,
    pub window_height: Option<f32>,
    /// Editor state for the right panel
    #[serde(default)]
    pub editor_open_files: Vec<PathBuf>,
    #[serde(default)]
    pub editor_active_file_ix: usize,
}

fn default_theme() -> String {
    crate::theme::colors::DEFAULT_THEME.name.to_string()
}

fn default_right_panel_width() -> f32 {
    400.0
}

fn default_diff_panel_width() -> f32 {
    360.0
}

fn default_right_panel_tab() -> String {
    "git".to_string()
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WorkspaceState {
    pub name: String,
    pub directory: Option<PathBuf>,
    pub layout: Option<SplitNodeState>,
    /// Files open in the editor for this workspace.
    #[serde(default)]
    pub open_files: Vec<PathBuf>,
}

#[derive(Serialize, Deserialize, Clone)]
pub enum SplitNodeState {
    Leaf(TabGroupState),
    Split {
        axis: SplitAxisState,
        children: Vec<SplitNodeState>,
        ratios: Vec<f32>,
    },
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub enum SplitAxisState {
    Horizontal,
    Vertical,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TabGroupState {
    pub tabs: Vec<TabState>,
    pub active_tab_ix: usize,
}

#[derive(Serialize, Deserialize, Clone)]
pub enum TabState {
    Terminal { title: String },
    // Legacy: editor tabs are now in the right panel. Kept for migration.
    #[serde(alias = "Editor")]
    Editor(EditorState),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct EditorState {
    pub title: String,
    pub root_dir: PathBuf,
    pub open_files: Vec<PathBuf>,
    pub active_file_ix: usize,
}

// ── Snapshot capture ──

impl SessionState {
    pub fn capture(app: &crate::app::OperatorApp, cx: &gpui::App) -> Self {
        let settings = crate::settings::AppSettings::get(cx);

        let rp = app.right_panel.read(cx);
        let right_panel_tab = match rp.active_tab {
            RightPanelTab::Files => "files",
            RightPanelTab::Git => "git",
            RightPanelTab::Pr => "pr",
        };

        // Get live editor files for the active workspace
        let live_editor_files: Vec<PathBuf> = rp.editor.as_ref()
            .map(|editor| editor.read(cx).all_open_files(cx))
            .unwrap_or_default();

        let workspaces: Vec<WorkspaceState> = app
            .workspaces
            .iter()
            .enumerate()
            .map(|(i, ws_entity)| {
                let ws = ws_entity.read(cx);
                let layout = ws.layout.as_ref().map(|pg_entity| {
                    let pg = pg_entity.read(cx);
                    Self::capture_split_node(&pg.root, cx)
                });
                // Active workspace: use live editor files; others: use cached
                let open_files = if i == app.active_workspace_ix {
                    live_editor_files.clone()
                } else {
                    ws.cached_editor_files.clone()
                };
                WorkspaceState {
                    name: ws.name.to_string(),
                    directory: ws.directory.clone(),
                    layout,
                    open_files,
                }
            })
            .collect();

        SessionState {
            workspaces,
            active_workspace_ix: app.active_workspace_ix,
            settings: SettingsState {
                vim_mode: settings.vim_mode,
                theme: settings.theme.clone(),
                sidebar_collapsed: app.sidebar_collapsed,
                sidebar_width: app.sidebar_width,
                right_panel_collapsed: app.right_panel_collapsed,
                right_panel_width: rp.width,
                right_panel_tab: right_panel_tab.to_string(),
                diff_panel_collapsed: app.right_panel_collapsed,
                diff_panel_width: rp.width,
                window_x: app.window_bounds.map(|b| b.0),
                window_y: app.window_bounds.map(|b| b.1),
                window_width: app.window_bounds.map(|b| b.2),
                window_height: app.window_bounds.map(|b| b.3),
                // Legacy: keep for backwards compat, populated from active workspace
                editor_open_files: live_editor_files,
                editor_active_file_ix: 0,
            },
        }
    }

    fn capture_split_node(node: &SplitNode, cx: &gpui::App) -> SplitNodeState {
        match node {
            SplitNode::Leaf(group) => SplitNodeState::Leaf(Self::capture_tab_group(group, cx)),
            SplitNode::Split {
                axis,
                children,
                ratios,
                ..
            } => SplitNodeState::Split {
                axis: match axis {
                    SplitAxis::Horizontal => SplitAxisState::Horizontal,
                    SplitAxis::Vertical => SplitAxisState::Vertical,
                },
                children: children
                    .iter()
                    .map(|c| Self::capture_split_node(c, cx))
                    .collect(),
                ratios: ratios.clone(),
            },
        }
    }

    fn capture_tab_group(group: &TabGroup, cx: &gpui::App) -> TabGroupState {
        let tabs = group
            .tabs
            .iter()
            .map(|tab_entity| {
                let tab = tab_entity.read(cx);
                TabState::Terminal {
                    title: tab.title.to_string(),
                }
            })
            .collect();
        TabGroupState {
            tabs,
            active_tab_ix: group.active_tab_ix,
        }
    }
}

// ── Restore ──

impl SessionState {
    /// Restore app state into an OperatorApp. Must be called from within
    /// a `Context<OperatorApp>` (i.e., inside `cx.new(|cx| ...)`).
    pub fn restore(self, cx: &mut gpui::Context<crate::app::OperatorApp>) -> crate::app::OperatorApp {
        // Restore settings
        let vim_mode = self.settings.vim_mode;
        let theme_name = self.settings.theme.clone();
        crate::theme::colors::set_theme_by_name(&theme_name);
        cx.update_global::<crate::settings::AppSettings, _>(|settings, _cx| {
            settings.vim_mode = vim_mode;
            settings.theme = theme_name;
        });

        let sidebar_collapsed = self.settings.sidebar_collapsed;
        let sidebar_width = self.settings.sidebar_width;
        // Support both new and legacy field names
        let right_panel_collapsed = if self.settings.right_panel_collapsed {
            true
        } else {
            self.settings.diff_panel_collapsed
        };
        let right_panel_width = if self.settings.right_panel_width > 0.0 {
            self.settings.right_panel_width
        } else {
            self.settings.diff_panel_width.max(400.0)
        };
        let right_panel_tab = match self.settings.right_panel_tab.as_str() {
            "files" => RightPanelTab::Files,
            "pr" => RightPanelTab::Pr,
            _ => RightPanelTab::Git,
        };
        let window_bounds = match (
            self.settings.window_x,
            self.settings.window_y,
            self.settings.window_width,
            self.settings.window_height,
        ) {
            (Some(x), Some(y), Some(w), Some(h)) => Some((x, y, w, h)),
            _ => None,
        };

        let workspaces: Vec<gpui::Entity<crate::workspace::Workspace>> = self
            .workspaces
            .iter()
            .map(|ws_state| {
                if let Some(dir) = &ws_state.directory {
                    let name = ws_state.name.clone();
                    let dir = dir.clone();
                    let layout_state = ws_state.layout.clone();
                    let open_files = ws_state.open_files.clone();
                    cx.new(|cx: &mut gpui::Context<crate::workspace::Workspace>| {
                        let dir_for_layout = dir.clone();
                        let mut ws = crate::workspace::Workspace::new(
                            &name,
                            dir,
                            cx,
                        );
                        // Replace the default layout with the restored one
                        if let Some(ref ls) = layout_state {
                            let root = Self::restore_split_node(ls, Some(&dir_for_layout), &mut *cx);
                            let layout = cx.new(|_cx| {
                                PaneGroup {
                                    root,
                                    drop_target: None,
                                    focused_group_id: None,
                                    work_dir: Some(dir_for_layout),
                                    mode: crate::pane::pane_group::PaneGroupMode::Terminal,
                                }
                            });
                            cx.observe(&layout, |_this, _layout, cx| cx.notify())
                                .detach();
                            ws.layout = Some(layout);
                        }
                        ws.cached_editor_files = open_files;
                        ws
                    })
                } else {
                    cx.new(|cx| crate::workspace::Workspace::new_empty(cx))
                }
            })
            .collect();

        // Ensure we always have at least one workspace (prevents index-out-of-bounds panic)
        let workspaces = if workspaces.is_empty() {
            vec![cx.new(|cx| crate::workspace::Workspace::new_empty(cx))]
        } else {
            workspaces
        };

        let active_workspace_ix = self
            .active_workspace_ix
            .min(workspaces.len().saturating_sub(1));

        // Derive diff panel work_dir from the active workspace's directory
        let active_ws_dir = workspaces
            .get(active_workspace_ix)
            .and_then(|ws| ws.read(cx).directory.clone());

        // Get editor files for active workspace: prefer per-workspace, fall back to legacy global
        let active_ws_files: Vec<PathBuf> = workspaces
            .get(active_workspace_ix)
            .map(|ws| ws.read(cx).cached_editor_files.clone())
            .unwrap_or_default();
        let editor_open_files = if !active_ws_files.is_empty() {
            active_ws_files
        } else {
            self.settings.editor_open_files.clone()
        };
        let active_ws_dir_clone = active_ws_dir.clone();

        let right_panel = cx.new(|cx| {
            let diff_panel = cx.new(|_cx| {
                if let Some(dir) = &active_ws_dir {
                    crate::git::GitDiffPanel::new(dir.clone())
                } else {
                    crate::git::GitDiffPanel::empty()
                }
            });
            let pr_diff_panel = cx.new(|_cx| {
                if let Some(dir) = &active_ws_dir {
                    crate::git::PrDiffPanel::new(dir.clone())
                } else {
                    crate::git::PrDiffPanel::empty()
                }
            });
            let mut rp = RightPanel::new(diff_panel, pr_diff_panel);
            rp.width = right_panel_width;
            rp.active_tab = right_panel_tab;

            // Restore editor with open files for the active workspace
            if let Some(dir) = active_ws_dir_clone {
                rp.ensure_editor(dir, cx);
                if let Some(editor) = &rp.editor {
                    editor.update(cx, |editor, cx| {
                        for file_path in &editor_open_files {
                            if file_path.exists() {
                                editor.open_file(file_path.clone(), cx);
                            }
                        }
                    });
                }
            }

            rp
        });

        let settings_panel = cx.new(|cx| crate::settings::settings_panel::SettingsPanel::new(cx));

        crate::app::OperatorApp::from_restored(
            workspaces,
            active_workspace_ix,
            sidebar_collapsed,
            sidebar_width,
            right_panel_collapsed,
            window_bounds,
            right_panel,
            settings_panel,
            cx,
        )
    }

    fn restore_split_node(state: &SplitNodeState, work_dir: Option<&std::path::PathBuf>, cx: &mut gpui::App) -> SplitNode {
        match state {
            SplitNodeState::Leaf(group_state) => {
                SplitNode::Leaf(Self::restore_tab_group(group_state, work_dir, cx))
            }
            SplitNodeState::Split {
                axis,
                children,
                ratios,
            } => {
                let split_axis = match axis {
                    SplitAxisState::Horizontal => SplitAxis::Horizontal,
                    SplitAxisState::Vertical => SplitAxis::Vertical,
                };
                SplitNode::Split {
                    id: crate::pane::pane_group::next_split_id_pub(),
                    axis: split_axis,
                    children: children
                        .iter()
                        .map(|c| Self::restore_split_node(c, work_dir, cx))
                        .collect(),
                    ratios: ratios.clone(),
                }
            }
        }
    }

    fn restore_tab_group(state: &TabGroupState, work_dir: Option<&std::path::PathBuf>, cx: &mut gpui::App) -> TabGroup {
        let tabs: Vec<gpui::Entity<Tab>> = state
            .tabs
            .iter()
            .filter_map(|tab_state| match tab_state {
                TabState::Terminal { title } => {
                    Some(cx.new(|cx| Tab::new(title, work_dir.cloned(), cx)))
                }
                // Legacy editor tabs are skipped — editor is now in the right panel
                TabState::Editor(_) => None,
            })
            .collect();

        // If all tabs were editor tabs (now removed), add a terminal
        let tabs = if tabs.is_empty() {
            vec![cx.new(|cx| Tab::new("Terminal", work_dir.cloned(), cx))]
        } else {
            tabs
        };

        let active = state.active_tab_ix.min(tabs.len().saturating_sub(1));
        TabGroup {
            id: crate::pane::pane_group::next_group_id_pub(),
            tabs,
            active_tab_ix: active,
        }
    }
}

// ── File I/O ──

fn session_path() -> PathBuf {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("operator");
    config_dir.join("session.json")
}

pub fn save_session(app: &crate::app::OperatorApp, cx: &gpui::App) {
    let state = SessionState::capture(app, cx);
    let path = session_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(&state) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                log::error!("Failed to save session: {e}");
            }
        }
        Err(e) => log::error!("Failed to serialize session: {e}"),
    }
}

pub fn load_session() -> Option<SessionState> {
    let path = session_path();
    let data = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&data) {
        Ok(state) => Some(state),
        Err(e) => {
            log::warn!("Failed to parse session file: {e}");
            None
        }
    }
}
