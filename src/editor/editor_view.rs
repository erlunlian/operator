use gpui::*;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::editor::file_tree::FileTree;
use crate::pane::PaneGroup;
use crate::theme::colors;

pub struct EditorView {
    pub _root_dir: PathBuf,
    pub file_tree: FileTree,
    pub pane_group: Entity<PaneGroup>,
    pub tree_collapsed: bool,
    pub tree_width: f32,
    resizing_tree: bool,
    view_origin_x: Rc<Cell<f32>>,
    focus_handle: FocusHandle,
}

impl EditorView {
    pub fn new(root_dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let file_tree = FileTree::new(root_dir.clone());
        let pane_group = cx.new(|_cx| PaneGroup::new_file_editor(Some(root_dir.clone())));
        Self {
            _root_dir: root_dir,
            file_tree,
            pane_group,
            tree_collapsed: false,
            tree_width: 220.0,
            resizing_tree: false,
            view_origin_x: Rc::new(Cell::new(0.0)),
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.file_tree.selected_path = Some(path.clone());
        self.pane_group.update(cx, |pg, cx| {
            pg.open_file(path, cx);
        });
        cx.notify();
    }

    pub fn navigate_to_line(&mut self, line: usize, cx: &mut Context<Self>) {
        self.pane_group.update(cx, |pg, cx| {
            pg.navigate_to_line(line, cx);
        });
    }

    pub fn all_open_files(&self, cx: &App) -> Vec<PathBuf> {
        self.pane_group.read(cx).all_open_files(cx)
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

        let pane_group_entity = self.pane_group.clone();

        let mut root = div()
            .id("editor-view")
            .flex()
            .flex_row()
            .size_full()
            .bg(colors::bg())
            .track_focus(&self.focus_handle)
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

        // Right panel: PaneGroup with file editor tabs
        let pane_tree = pane_group_entity.read(cx).render_tree(&pane_group_entity, true, px(0.0), cx);
        let right_panel = div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .child(pane_tree);

        root = root.child(right_panel);
        root
    }
}
