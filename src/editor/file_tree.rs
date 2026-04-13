use gpui::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::theme::colors;
use crate::util;

pub struct FileTreeEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
}

pub struct FileTree {
    pub root: PathBuf,
    pub entries: Vec<FileTreeEntry>,
    pub expanded_dirs: HashSet<PathBuf>,
    pub selected_path: Option<PathBuf>,
}

const IGNORED_NAMES: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    ".DS_Store",
    "__pycache__",
    ".build",
];

impl FileTree {
    pub fn new(root: PathBuf) -> Self {
        let mut tree = Self {
            root,
            entries: Vec::new(),
            expanded_dirs: HashSet::new(),
            selected_path: None,
        };
        tree.rebuild_entries();
        tree
    }

    pub fn toggle_dir(&mut self, path: &Path) {
        if self.expanded_dirs.contains(path) {
            self.expanded_dirs.remove(path);
        } else {
            self.expanded_dirs.insert(path.to_path_buf());
        }
        self.rebuild_entries();
    }

    pub fn rebuild_entries(&mut self) {
        self.entries.clear();
        self.walk_dir(&self.root.clone(), 0);
    }

    fn walk_dir(&mut self, dir: &Path, depth: usize) {
        let mut children: Vec<(String, PathBuf, bool)> = Vec::new();

        if let Ok(read_dir) = std::fs::read_dir(dir) {
            for entry in read_dir.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();

                // Skip ignored directories/files
                if IGNORED_NAMES.contains(&name.as_str()) {
                    continue;
                }

                let path = entry.path();
                let is_dir = path.is_dir();
                children.push((name, path, is_dir));
            }
        }

        // Sort: directories first, then alphabetical case-insensitive
        children.sort_by(|a, b| {
            match (a.2, b.2) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.0.to_lowercase().cmp(&b.0.to_lowercase()),
            }
        });

        for (name, path, is_dir) in children {
            self.entries.push(FileTreeEntry {
                path: path.clone(),
                name,
                is_dir,
                depth,
            });

            if is_dir && self.expanded_dirs.contains(&path) {
                self.walk_dir(&path, depth + 1);
            }
        }
    }

    pub fn render_with_width(
        &self,
        on_file_click: Rc<dyn Fn(PathBuf, &mut Window, &mut App)>,
        on_dir_toggle: Rc<dyn Fn(PathBuf, &mut Window, &mut App)>,
        width: f32,
    ) -> Stateful<Div> {
        let mut container = div()
            .id("file-tree")
            .flex()
            .flex_col()
            .w(px(width))
            .min_w(px(100.0))
            .h_full()
            .flex_shrink_0()
            .bg(colors::surface())
            .border_r_1()
            .border_color(colors::border())
            .overflow_y_scroll()
            .font_family("Menlo")
            .text_xs();

        // Header
        let root_name = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Files".to_string());

        container = container.child(
            div()
                .flex()
                .items_center()
                .px_3()
                .py_2()
                .text_color(colors::text_muted())
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .child(root_name.to_uppercase()),
        );

        for (ix, entry) in self.entries.iter().enumerate() {
            let is_selected = self
                .selected_path
                .as_ref()
                .map(|p| p == &entry.path)
                .unwrap_or(false);

            let indent = entry.depth as f32 * 16.0 + 8.0;

            let mut row = div()
                .id(ElementId::Name(format!("file-tree-{ix}").into()))
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .pl(px(indent))
                .pr_2()
                .h(px(24.0))
                .w_full()
                .cursor_pointer()
                .text_color(colors::text())
                .hover(|s| s.bg(colors::surface_hover()));

            if is_selected {
                row = row
                    .bg(colors::surface_hover())
                    .border_l_2()
                    .border_color(colors::accent());
            }

            if entry.is_dir {
                let is_expanded = self.expanded_dirs.contains(&entry.path);
                let chevron = if is_expanded { "\u{25BC}" } else { "\u{25B6}" };
                let dir_icon = if is_expanded { util::dir_icon_open() } else { util::dir_icon() };

                row = row
                    .child(
                        div()
                            .text_color(colors::text_muted())
                            .text_size(px(8.0))
                            .w(px(10.0))
                            .child(chevron),
                    )
                    .child(
                        div()
                            .font_family(util::ICON_FONT)
                            .text_size(px(14.0))
                            .text_color(colors::text_muted())
                            .mr(px(4.0))
                            .child(dir_icon),
                    )
                    .child(entry.name.clone());

                let path = entry.path.clone();
                let on_dir_toggle = on_dir_toggle.clone();
                row = row.on_click(move |_, window, cx| {
                    on_dir_toggle(path.clone(), window, cx);
                });
            } else {
                let icon = util::icon_for_file(&entry.name);
                let icon_color = util::file_icon_color(&entry.name);
                row = row
                    .child(div().w(px(12.0)))
                    .child(
                        div()
                            .font_family(util::ICON_FONT)
                            .text_size(px(14.0))
                            .text_color(icon_color)
                            .w(px(16.0))
                            .flex_shrink_0()
                            .mr(px(4.0))
                            .child(icon),
                    )
                    .child(entry.name.clone());

                let path = entry.path.clone();
                let on_file_click = on_file_click.clone();
                row = row.on_click(move |_, window, cx| {
                    on_file_click(path.clone(), window, cx);
                });
            }

            container = container.child(row);
        }

        container
    }
}