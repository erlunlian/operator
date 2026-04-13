use gpui::{AppContext, BorrowAppContext};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::pane::pane_group::{SplitAxis, SplitNode, TabGroup};
use crate::pane::PaneGroup;
use crate::tab::tab::{Tab, TabContent};

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
    pub sidebar_collapsed: bool,
    pub sidebar_width: f32,
    pub diff_panel_collapsed: bool,
    pub window_x: Option<f32>,
    pub window_y: Option<f32>,
    pub window_width: Option<f32>,
    pub window_height: Option<f32>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WorkspaceState {
    pub name: String,
    pub directory: Option<PathBuf>,
    pub layout: Option<SplitNodeState>,
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
        let workspaces = app
            .workspaces
            .iter()
            .map(|ws_entity| {
                let ws = ws_entity.read(cx);
                let layout = ws.layout.as_ref().map(|pg_entity| {
                    let pg = pg_entity.read(cx);
                    Self::capture_split_node(&pg.root, cx)
                });
                WorkspaceState {
                    name: ws.name.to_string(),
                    directory: ws.directory.clone(),
                    layout,
                }
            })
            .collect();

        SessionState {
            workspaces,
            active_workspace_ix: app.active_workspace_ix,
            settings: SettingsState {
                vim_mode: settings.vim_mode,
                sidebar_collapsed: app.sidebar_collapsed,
                sidebar_width: app.sidebar_width,
                diff_panel_collapsed: app.diff_panel_collapsed,
                window_x: app.window_bounds.map(|b| b.0),
                window_y: app.window_bounds.map(|b| b.1),
                window_width: app.window_bounds.map(|b| b.2),
                window_height: app.window_bounds.map(|b| b.3),
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
                match &tab.content {
                    TabContent::Terminal(_) => TabState::Terminal {
                        title: tab.title.to_string(),
                    },
                    TabContent::Editor(editor_entity) => {
                        let editor = editor_entity.read(cx);
                        TabState::Editor(EditorState {
                            title: tab.title.to_string(),
                            root_dir: editor.root_dir.clone(),
                            open_files: editor
                                .open_files
                                .iter()
                                .map(|f| f.path.clone())
                                .collect(),
                            active_file_ix: editor.active_file_ix,
                        })
                    }
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
        cx.update_global::<crate::settings::AppSettings, _>(|settings, _cx| {
            settings.vim_mode = self.settings.vim_mode;
        });

        let sidebar_collapsed = self.settings.sidebar_collapsed;
        let sidebar_width = self.settings.sidebar_width;
        let diff_panel_collapsed = self.settings.diff_panel_collapsed;
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
                    cx.new(|cx: &mut gpui::Context<crate::workspace::Workspace>| {
                        let mut ws = crate::workspace::Workspace::new(
                            &name,
                            dir,
                            cx,
                        );
                        // Replace the default layout with the restored one
                        if let Some(ref ls) = layout_state {
                            let root = Self::restore_split_node(ls, &mut *cx);
                            let layout = cx.new(|_cx| {
                                PaneGroup {
                                    root,
                                    drop_target: None,
                                    focused_group_id: None,
                                }
                            });
                            cx.observe(&layout, |_this, _layout, cx| cx.notify())
                                .detach();
                            ws.layout = Some(layout);
                        }
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

        let work_dir = std::env::current_dir().unwrap_or_default();
        let diff_panel = cx.new(|_cx| crate::git::GitDiffPanel::new(work_dir));
        let settings_panel = cx.new(|cx| crate::settings::settings_panel::SettingsPanel::new(cx));

        crate::app::OperatorApp::from_restored(
            workspaces,
            active_workspace_ix,
            sidebar_collapsed,
            sidebar_width,
            diff_panel_collapsed,
            window_bounds,
            diff_panel,
            settings_panel,
            cx,
        )
    }

    fn restore_split_node(state: &SplitNodeState, cx: &mut gpui::App) -> SplitNode {
        match state {
            SplitNodeState::Leaf(group_state) => {
                SplitNode::Leaf(Self::restore_tab_group(group_state, cx))
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
                        .map(|c| Self::restore_split_node(c, cx))
                        .collect(),
                    ratios: ratios.clone(),
                }
            }
        }
    }

    fn restore_tab_group(state: &TabGroupState, cx: &mut gpui::App) -> TabGroup {
        let tabs: Vec<gpui::Entity<Tab>> = state
            .tabs
            .iter()
            .map(|tab_state| match tab_state {
                TabState::Terminal { title } => cx.new(|cx| Tab::new(title, cx)),
                TabState::Editor(editor_state) => {
                    cx.new(|cx| {
                        let tab =
                            Tab::new_editor(&editor_state.title, editor_state.root_dir.clone(), cx);
                        // Open the previously open files
                        if let TabContent::Editor(editor_entity) = &tab.content {
                            let editor_entity = editor_entity.clone();
                            editor_entity.update(cx, |editor: &mut crate::editor::EditorView, cx| {
                                for file_path in &editor_state.open_files {
                                    if file_path.exists() {
                                        editor.open_file(file_path.clone(), cx);
                                    }
                                }
                                if editor_state.active_file_ix < editor.open_files.len() {
                                    editor.active_file_ix = editor_state.active_file_ix;
                                }
                            });
                        }
                        tab
                    })
                }
            })
            .collect();

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
