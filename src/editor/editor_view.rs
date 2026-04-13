use gpui::*;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::actions::NewTab;
use crate::editor::file_tree::FileTree;
use crate::editor::FileViewer;
use crate::theme::colors;
use crate::util;

pub struct OpenFile {
    pub path: PathBuf,
    pub title: SharedString,
    pub viewer: Entity<FileViewer>,
}

pub struct EditorView {
    pub root_dir: PathBuf,
    pub file_tree: FileTree,
    pub open_files: Vec<OpenFile>,
    pub active_file_ix: usize,
    pub tree_collapsed: bool,
    pub tree_width: f32,
    resizing_tree: bool,
    view_origin_x: Rc<Cell<f32>>,
    focus_handle: FocusHandle,
}

impl EditorView {
    pub fn new(root_dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let file_tree = FileTree::new(root_dir.clone());
        Self {
            root_dir,
            file_tree,
            open_files: Vec::new(),
            active_file_ix: 0,
            tree_collapsed: false,
            tree_width: 220.0,
            resizing_tree: false,
            view_origin_x: Rc::new(Cell::new(0.0)),
            focus_handle: cx.focus_handle(),
        }
    }

    fn handle_new_tab(&mut self, _: &NewTab, _window: &mut Window, cx: &mut Context<Self>) {
        // Cmd+T inside an editor creates a new unsaved file sub-tab
        self.open_new_file(cx);
    }

    pub fn open_new_file(&mut self, cx: &mut Context<Self>) {
        // Generate a unique "Untitled" name
        let mut n = 1;
        loop {
            let name = if n == 1 {
                "Untitled".to_string()
            } else {
                format!("Untitled-{n}")
            };
            if !self.open_files.iter().any(|f| f.title.as_ref() == name) {
                let path = self.root_dir.join(&name);
                let viewer = cx.new(|cx| FileViewer::new_empty(path.clone(), cx));
                self.open_files.push(OpenFile {
                    path,
                    title: SharedString::from(name),
                    viewer,
                });
                self.active_file_ix = self.open_files.len() - 1;
                cx.notify();
                return;
            }
            n += 1;
        }
    }

    pub fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {

        // If already open, switch to it
        if let Some(ix) = self.open_files.iter().position(|f| f.path == path) {
            self.active_file_ix = ix;
            self.file_tree.selected_path = Some(path);
            cx.notify();
            return;
        }

        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());

        let viewer = cx.new(|cx| FileViewer::open(path.clone(), cx));

        self.open_files.push(OpenFile {
            path: path.clone(),
            title: SharedString::from(title),
            viewer,
        });
        self.active_file_ix = self.open_files.len() - 1;
        self.file_tree.selected_path = Some(path);
        cx.notify();
    }

    pub fn close_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.open_files.len() {
            self.open_files.remove(ix);
            if self.active_file_ix >= self.open_files.len() && !self.open_files.is_empty() {
                self.active_file_ix = self.open_files.len() - 1;
            } else if ix < self.active_file_ix {
                self.active_file_ix -= 1;
            }

            // Update selected path
            self.file_tree.selected_path = self
                .open_files
                .get(self.active_file_ix)
                .map(|f| f.path.clone());

            cx.notify();
        }
    }

    fn render_sub_tab_bar(&self, cx: &Context<Self>) -> Div {
        let entity = cx.entity().clone();

        let mut bar = div()
            .flex()
            .flex_row()
            .w_full()
            .h(px(32.0))
            .bg(colors::tab_inactive_bg())
            .border_b_1()
            .border_color(colors::border());

        for (ix, file) in self.open_files.iter().enumerate() {
            let is_active = ix == self.active_file_ix;
            let is_dirty = file.viewer.read(cx).dirty;
            let entity_select = entity.clone();
            let entity_close = entity.clone();

            let file_icon = util::icon_for_file(&file.title);
            let tab_title = if is_dirty {
                format!("{file_icon} {} *", file.title)
            } else {
                format!("{file_icon} {}", file.title)
            };

            let mut tab_el = div()
                .id(ElementId::Name(format!("sub-tab-{ix}").into()))
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_3()
                .h_full()
                .text_xs()
                .cursor_pointer();

            if is_active {
                tab_el = tab_el
                    .bg(colors::tab_active_bg())
                    .border_b_2()
                    .border_color(colors::accent())
                    .text_color(colors::text());
            } else {
                tab_el = tab_el
                    .bg(colors::tab_inactive_bg())
                    .text_color(colors::text_muted())
                    .hover(|s| s.bg(colors::surface_hover()));
            }

            tab_el = tab_el
                .on_click(move |_, _window, cx| {
                    entity_select.update(cx, |view, cx| {
                        view.active_file_ix = ix;
                        view.file_tree.selected_path = view
                            .open_files
                            .get(ix)
                            .map(|f| f.path.clone());
                        cx.notify();
                    });
                })
                .child(tab_title);

            // Close button
            let close_ix = ix;
            tab_el = tab_el.child(
                div()
                    .id(ElementId::Name(format!("sub-tab-close-{ix}").into()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(14.0))
                    .h(px(14.0))
                    .rounded_sm()
                    .text_xs()
                    .text_color(colors::text_muted())
                    .hover(|s| s.bg(colors::surface_hover()).text_color(colors::text()))
                    .cursor_pointer()
                    .on_click(move |_, _window, cx| {
                        entity_close.update(cx, |view, cx| {
                            view.close_file(close_ix, cx);
                        });
                    })
                    .child("×"),
            );

            bar = bar.child(tab_el);
        }

        bar
    }
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().clone();
        let entity2 = entity.clone();
        let entity_resize_move = cx.entity().clone();
        let entity_resize_up = cx.entity().clone();
        let view_origin_x = self.view_origin_x.clone();
        let view_origin_for_resize = self.view_origin_x.clone();

        let mut root = div()
            .id("editor-view")
            .flex()
            .flex_row()
            .size_full()
            .bg(colors::bg())
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::handle_new_tab))
            // Capture the left edge of this view in window coordinates
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        view_origin_x.set(f32::from(bounds.origin.x));
                    },
                    |_, _, _, _| {},
                )
                .size_0(),
            )
            .on_mouse_move(move |event: &MouseMoveEvent, _window, cx| {
                entity_resize_move.update(cx, |view, cx| {
                    if view.resizing_tree {
                        let origin_x = view_origin_for_resize.get();
                        let new_width = f32::from(event.position.x) - origin_x;
                        view.tree_width = new_width.clamp(100.0, 600.0);
                        cx.notify();
                    }
                });
            })
            .on_mouse_up(MouseButton::Left, move |_event: &MouseUpEvent, _window, cx| {
                entity_resize_up.update(cx, |view, cx| {
                    if view.resizing_tree {
                        view.resizing_tree = false;
                        cx.notify();
                    }
                });
            });

        // File tree (left panel)
        if !self.tree_collapsed {
            let file_tree_el = self.file_tree.render_with_width(
                // on_file_click
                Rc::new(move |path, _window, cx| {
                    entity.update(cx, |view, cx| {
                        view.open_file(path, cx);
                    });
                }),
                // on_dir_toggle
                Rc::new(move |path, _window, cx| {
                    entity2.update(cx, |view, cx| {
                        view.file_tree.toggle_dir(&path);
                        cx.notify();
                    });
                }),
                self.tree_width,
            );

            root = root.child(file_tree_el);

            // Resize handle between file tree and content
            let entity_resize_down = cx.entity().clone();
            root = root.child(
                div()
                    .id("tree-resize-handle")
                    .w(px(12.0))
                    .mx(px(-4.0))
                    .h_full()
                    .flex_shrink_0()
                    .cursor_col_resize()
                    .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                        entity_resize_down.update(cx, |view, cx| {
                            view.resizing_tree = true;
                            cx.notify();
                        });
                    }),
            );
        }

        // Right panel: sub-tab bar + file content
        let mut right_panel = div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden();

        if self.open_files.is_empty() {
            // Placeholder when no files are open
            right_panel = right_panel.child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_color(colors::text_muted())
                    .text_sm()
                    .child("Select a file from the tree"),
            );
        } else {
            // Sub-tab bar
            let sub_tab_bar = self.render_sub_tab_bar(cx);
            right_panel = right_panel.child(sub_tab_bar);

            // Active file content
            if self.active_file_ix < self.open_files.len() {
                let viewer = self.open_files[self.active_file_ix].viewer.clone();
                right_panel = right_panel.child(
                    div()
                        .flex()
                        .flex_1()
                        .size_full()
                        .overflow_hidden()
                        .child(viewer),
                );
            }
        }

        root = root.child(right_panel);
        root
    }
}
