use gpui::*;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::tab::{Tab, TabBar, TabDragPayload};
use crate::theme::colors;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SplitAxis {
    Horizontal, // children laid out left-to-right
    Vertical,   // children laid out top-to-bottom
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DropZone {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DropTarget {
    pub group_id: usize,
    pub zone: DropZone,
}

static NEXT_GROUP_ID: AtomicUsize = AtomicUsize::new(1);
fn next_group_id() -> usize {
    NEXT_GROUP_ID.fetch_add(1, Ordering::Relaxed)
}
pub fn next_group_id_pub() -> usize {
    next_group_id()
}

static NEXT_SPLIT_ID: AtomicUsize = AtomicUsize::new(1);
fn next_split_id() -> usize {
    NEXT_SPLIT_ID.fetch_add(1, Ordering::Relaxed)
}
pub fn next_split_id_pub() -> usize {
    next_split_id()
}

// ── Drag payload for resize handles ──

#[derive(Clone)]
struct ResizeHandleDrag {
    split_id: usize,
    handle_idx: usize,
    axis: SplitAxis,
}

struct ResizeGhost;
impl Render for ResizeGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(1.0)).h(px(1.0))
    }
}

// ── Tab group: a leaf in the split tree with its own tabs ──

pub struct TabGroup {
    pub id: usize,
    pub tabs: Vec<Entity<Tab>>,
    pub active_tab_ix: usize,
}

impl TabGroup {
    pub fn new_terminal(cx: &mut App) -> Self {
        let tab = cx.new(|cx| Tab::new("Terminal", cx));
        Self {
            id: next_group_id(),
            tabs: vec![tab],
            active_tab_ix: 0,
        }
    }
}

// ── Split tree node ──

pub enum SplitNode {
    Leaf(TabGroup),
    Split {
        id: usize,
        axis: SplitAxis,
        children: Vec<SplitNode>,
        ratios: Vec<f32>,
    },
}

impl SplitNode {
    /// Split a specific leaf by group_id, inserting a new leaf with the given tab.
    /// If `before` is true, the new leaf goes before the existing one (for Left/Top).
    pub fn split_leaf_with_tab(
        &mut self,
        target_group_id: usize,
        axis: SplitAxis,
        new_tab: Entity<Tab>,
        before: bool,
    ) -> bool {
        match self {
            SplitNode::Leaf(group) => {
                if group.id != target_group_id {
                    return false;
                }
                let new_leaf = SplitNode::Leaf(TabGroup {
                    id: next_group_id(),
                    tabs: vec![new_tab],
                    active_tab_ix: 0,
                });
                let existing = std::mem::replace(
                    self,
                    SplitNode::Split {
                        id: next_split_id(),
                        axis,
                        children: vec![],
                        ratios: vec![],
                    },
                );
                let children = if before {
                    vec![new_leaf, existing]
                } else {
                    vec![existing, new_leaf]
                };
                *self = SplitNode::Split {
                    id: next_split_id(),
                    axis,
                    children,
                    ratios: vec![0.5, 0.5],
                };
                true
            }
            SplitNode::Split { children, .. } => {
                for child in children.iter_mut() {
                    if child.split_leaf_with_tab(target_group_id, axis, new_tab.clone(), before) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Split the active (first/only) leaf, creating a sibling with a new terminal.
    pub fn split_active(&mut self, axis: SplitAxis, cx: &mut App) -> bool {
        match self {
            SplitNode::Leaf(_existing) => {
                let existing = std::mem::replace(self, SplitNode::Leaf(TabGroup::new_terminal(cx)));
                let new_leaf = std::mem::replace(self, existing);
                let old = std::mem::replace(self, SplitNode::Split {
                    id: next_split_id(),
                    axis,
                    children: vec![],
                    ratios: vec![],
                });
                *self = SplitNode::Split {
                    id: next_split_id(),
                    axis,
                    children: vec![old, new_leaf],
                    ratios: vec![0.5, 0.5],
                };
                true
            }
            SplitNode::Split {
                children,
                axis: existing_axis,
                ratios,
                ..
            } => {
                if *existing_axis == axis {
                    let n = children.len() as f32 + 1.0;
                    let new_ratio = 1.0 / n;
                    let scale = 1.0 - new_ratio;
                    for r in ratios.iter_mut() {
                        *r *= scale;
                    }
                    ratios.push(new_ratio);
                    children.push(SplitNode::Leaf(TabGroup::new_terminal(cx)));
                    true
                } else {
                    if let Some(last) = children.last_mut() {
                        last.split_active(axis, cx)
                    } else {
                        false
                    }
                }
            }
        }
    }

    pub fn get_claude_status(&self, cx: &App) -> crate::workspace::workspace::ClaudeStatus {
        match self {
            SplitNode::Leaf(group) => {
                if let Some(tab) = group.tabs.get(group.active_tab_ix) {
                    tab.read(cx).get_claude_status(cx)
                } else {
                    crate::workspace::workspace::ClaudeStatus::Idle
                }
            }
            SplitNode::Split { children, .. } => {
                for child in children {
                    let status = child.get_claude_status(cx);
                    if status != crate::workspace::workspace::ClaudeStatus::Idle {
                        return status;
                    }
                }
                crate::workspace::workspace::ClaudeStatus::Idle
            }
        }
    }

    pub fn add_tab(&mut self, cx: &mut App) {
        match self {
            SplitNode::Leaf(group) => {
                let idx = group.tabs.len() + 1;
                let tab = cx.new(|cx| Tab::new(&format!("Terminal {}", idx), cx));
                group.tabs.push(tab);
                group.active_tab_ix = group.tabs.len() - 1;
            }
            SplitNode::Split { children, .. } => {
                if let Some(first) = children.first_mut() {
                    first.add_tab(cx);
                }
            }
        }
    }

    pub fn close_tab(&mut self) {
        match self {
            SplitNode::Leaf(group) => {
                if !group.tabs.is_empty() {
                    group.tabs.remove(group.active_tab_ix);
                    if group.active_tab_ix >= group.tabs.len() && !group.tabs.is_empty() {
                        group.active_tab_ix = group.tabs.len() - 1;
                    }
                }
                // Empty leaves get pruned by the caller
            }
            SplitNode::Split { children, .. } => {
                if let Some(first) = children.first_mut() {
                    first.close_tab();
                }
            }
        }
    }

    pub fn next_tab(&mut self) {
        match self {
            SplitNode::Leaf(group) => {
                if !group.tabs.is_empty() {
                    group.active_tab_ix = (group.active_tab_ix + 1) % group.tabs.len();
                }
            }
            SplitNode::Split { children, .. } => {
                if let Some(first) = children.first_mut() {
                    first.next_tab();
                }
            }
        }
    }

    pub fn prev_tab(&mut self) {
        match self {
            SplitNode::Leaf(group) => {
                if !group.tabs.is_empty() {
                    if group.active_tab_ix == 0 {
                        group.active_tab_ix = group.tabs.len() - 1;
                    } else {
                        group.active_tab_ix -= 1;
                    }
                }
            }
            SplitNode::Split { children, .. } => {
                if let Some(first) = children.first_mut() {
                    first.prev_tab();
                }
            }
        }
    }

    pub fn add_editor_tab(&mut self, root_dir: std::path::PathBuf, cx: &mut App) {
        match self {
            SplitNode::Leaf(group) => {
                let dir_name = root_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Editor".to_string());
                let tab = cx.new(|cx| Tab::new_editor(&dir_name, root_dir, cx));
                group.tabs.push(tab);
                group.active_tab_ix = group.tabs.len() - 1;
            }
            SplitNode::Split { children, .. } => {
                if let Some(first) = children.first_mut() {
                    first.add_editor_tab(root_dir, cx);
                }
            }
        }
    }

    /// Check if there's an editor tab anywhere in the tree.
    pub fn has_editor_tab(&self, cx: &App) -> bool {
        match self {
            SplitNode::Leaf(group) => group.tabs.iter().any(|t| t.read(cx).editor_entity().is_some()),
            SplitNode::Split { children, .. } => children.iter().any(|c| c.has_editor_tab(cx)),
        }
    }

    /// Open a file in the first editor tab found, then navigate to the given line.
    pub fn open_file_in_editor(
        &mut self,
        path: std::path::PathBuf,
        line_num: usize,
        cx: &mut App,
    ) {
        match self {
            SplitNode::Leaf(group) => {
                for (ix, tab) in group.tabs.iter().enumerate() {
                    if let Some(editor) = tab.read(cx).editor_entity().cloned() {
                        editor.update(cx, |view, cx| {
                            view.open_file(path.clone(), cx);
                            // Navigate the active file viewer to the line
                            if let Some(open_file) = view.open_files.get(view.active_file_ix) {
                                open_file.viewer.update(cx, |fv, _cx| {
                                    fv.navigate_to_line(line_num);
                                });
                            }
                        });
                        group.active_tab_ix = ix;
                        return;
                    }
                }
            }
            SplitNode::Split { children, .. } => {
                for child in children {
                    if child.has_editor_tab(cx) {
                        child.open_file_in_editor(path, line_num, cx);
                        return;
                    }
                }
            }
        }
    }

    pub fn reorder_tab(&mut self, from: usize, to: usize) {
        match self {
            SplitNode::Leaf(group) => {
                if from < group.tabs.len() && to < group.tabs.len() && from != to {
                    let tab = group.tabs.remove(from);
                    group.tabs.insert(to, tab);
                    group.active_tab_ix = to;
                }
            }
            SplitNode::Split { children, .. } => {
                if let Some(first) = children.first_mut() {
                    first.reorder_tab(from, to);
                }
            }
        }
    }

    pub fn close_tab_at(&mut self, ix: usize) {
        match self {
            SplitNode::Leaf(group) => {
                if ix < group.tabs.len() {
                    group.tabs.remove(ix);
                    if group.active_tab_ix >= group.tabs.len() && !group.tabs.is_empty() {
                        group.active_tab_ix = group.tabs.len() - 1;
                    } else if ix < group.active_tab_ix {
                        group.active_tab_ix -= 1;
                    }
                }
            }
            SplitNode::Split { children, .. } => {
                if let Some(first) = children.first_mut() {
                    first.close_tab_at(ix);
                }
            }
        }
    }

    pub fn set_active_tab(&mut self, ix: usize) {
        match self {
            SplitNode::Leaf(group) => {
                if ix < group.tabs.len() {
                    group.active_tab_ix = ix;
                }
            }
            SplitNode::Split { children, .. } => {
                if let Some(first) = children.first_mut() {
                    first.set_active_tab(ix);
                }
            }
        }
    }

    pub fn first_leaf_tabs(&self, cx: &App) -> (Vec<SharedString>, usize) {
        match self {
            SplitNode::Leaf(group) => {
                let titles = group
                    .tabs
                    .iter()
                    .map(|t| t.read(cx).title.clone())
                    .collect();
                (titles, group.active_tab_ix)
            }
            SplitNode::Split { children, .. } => {
                if let Some(first) = children.first() {
                    first.first_leaf_tabs(cx)
                } else {
                    (vec![], 0)
                }
            }
        }
    }
}

// ── PaneGroup: GPUI entity that owns and renders the split tree ──

pub struct PaneGroup {
    pub root: SplitNode,
    pub drop_target: Option<DropTarget>,
    pub focused_group_id: Option<usize>,
}

impl PaneGroup {
    pub fn new_terminal(cx: &mut App) -> Self {
        let group = TabGroup::new_terminal(cx);
        let id = group.id;
        Self {
            root: SplitNode::Leaf(group),
            drop_target: None,
            focused_group_id: Some(id),
        }
    }

    pub fn split(&mut self, axis: SplitAxis, cx: &mut App) {
        self.root.split_active(axis, cx);
    }

    pub fn split_right(&mut self, cx: &mut App) {
        self.root.split_active(SplitAxis::Horizontal, cx);
    }

    /// Smart close: closes the active sub-tab in the focused group's active tab (if editor),
    /// otherwise closes the outer tab in the focused group.
    /// Returns true if something was closed, false if nothing to close.
    pub fn close_focused_tab(&mut self, cx: &mut App) -> bool {
        let group_id = match self.focused_group_id {
            Some(id) => id,
            None => return false,
        };

        // First check if the focused group's active tab has sub-tabs to close
        let maybe_editor = find_group_mut(&mut self.root, group_id)
            .and_then(|group| group.tabs.get(group.active_tab_ix))
            .and_then(|tab| tab.read(cx).editor_entity().cloned());

        if let Some(editor) = maybe_editor {
            if !editor.read(cx).open_files.is_empty() {
                let ix = editor.read(cx).active_file_ix;
                editor.update(cx, |editor, cx| {
                    editor.close_file(ix, cx);
                });
                // If that was the last open file, close the editor tab too
                if editor.read(cx).open_files.is_empty() {
                    close_tab_in_focused_group(&mut self.root, group_id);
                    prune_empty_leaves(&mut self.root);
                    if find_group_mut(&mut self.root, group_id).is_none() {
                        self.focused_group_id = first_group_id(&self.root);
                    }
                }
                return true;
            }
        }

        // No sub-tabs — close the outer tab in the focused group
        close_tab_in_focused_group(&mut self.root, group_id);
        prune_empty_leaves(&mut self.root);

        // If the focused group was removed, pick a new one
        if find_group_mut(&mut self.root, group_id).is_none() {
            self.focused_group_id = first_group_id(&self.root);
        }

        true
    }

    pub fn get_claude_status(&self, cx: &App) -> crate::workspace::workspace::ClaudeStatus {
        self.root.get_claude_status(cx)
    }

    pub fn render_tree(&self, pane_group_entity: &Entity<PaneGroup>, cx: &App) -> AnyElement {
        Self::render_node(&self.root, pane_group_entity, self.drop_target, cx)
    }

    fn render_node(
        node: &SplitNode,
        pane_group_entity: &Entity<PaneGroup>,
        drop_target: Option<DropTarget>,
        cx: &App,
    ) -> AnyElement {
        match node {
            SplitNode::Leaf(group) => {
                Self::render_leaf(group, pane_group_entity, drop_target, cx)
            }
            SplitNode::Split {
                id,
                axis,
                children,
                ratios,
            } => {
                Self::render_split(*id, *axis, children, ratios, pane_group_entity, drop_target, cx)
            }
        }
    }

    fn render_leaf(
        group: &TabGroup,
        pane_group_entity: &Entity<PaneGroup>,
        drop_target: Option<DropTarget>,
        cx: &App,
    ) -> AnyElement {
        let group_id = group.id;
        let pg = pane_group_entity.clone();
        let pg2 = pane_group_entity.clone();
        let pg3 = pane_group_entity.clone();
        let pg4 = pane_group_entity.clone();
        let pg5 = pane_group_entity.clone();
        let pg6 = pane_group_entity.clone();
        let pg_drag = pane_group_entity.clone();
        let pg_drop = pane_group_entity.clone();

        let tab_titles: Vec<SharedString> = group
            .tabs
            .iter()
            .map(|t| t.read(cx).title.clone())
            .collect();
        let active_ix = group.active_tab_ix;

        let tab_bar = TabBar::render(
            &tab_titles,
            active_ix,
            group_id,
            Rc::new(move |ix, _window, cx| {
                pg.update(cx, |pg, cx| {
                    set_active_tab_in_group(&mut pg.root, group_id, ix);
                    cx.notify();
                });
            }),
            Rc::new(move |_window, cx| {
                pg2.update(cx, |pg, cx| {
                    add_tab_in_group(&mut pg.root, group_id, cx);
                    cx.notify();
                });
            }),
            Some(Rc::new(move |from, to, _window, cx| {
                pg3.update(cx, |pg, cx| {
                    reorder_tab_in_group(&mut pg.root, group_id, from, to);
                    cx.notify();
                });
            })),
            Some(Rc::new(move |ix, _window, cx| {
                pg4.update(cx, |pg, cx| {
                    close_tab_in_group(&mut pg.root, group_id, ix);
                    cx.notify();
                });
            })),
            Some(Rc::new(move |src_group, src_ix, target_ix, _window, cx| {
                pg6.update(cx, |pg, cx| {
                    move_tab_between_groups(
                        &mut pg.root,
                        src_group,
                        src_ix,
                        group_id,
                        target_ix,
                    );
                    cx.notify();
                });
            })),
        );

        // Render active tab content
        let content = if active_ix < group.tabs.len() {
            group.tabs[active_ix].read(cx).render_content()
        } else {
            div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(colors::text_muted())
                .child("No tab")
                .into_any_element()
        };

        // Build the content area with drop zone detection
        let content_area_id = format!("content-area-{group_id}");
        let content_area = div()
            .id(ElementId::Name(content_area_id.into()))
            .flex()
            .flex_1()
            .size_full()
            .overflow_hidden()
            .relative()
            // Track tab drags over this content area to determine drop zone
            .on_drag_move::<TabDragPayload>(
                move |event: &DragMoveEvent<TabDragPayload>, _window, cx| {
                    let bounds = event.bounds;
                    let pos = event.event.position;

                    // Only set drop target if cursor is actually inside this element's bounds
                    if bounds.contains(&pos) {
                        let zone = compute_drop_zone(bounds, pos);
                        pg_drag.update(cx, |pg, cx| {
                            let new_target = Some(DropTarget {
                                group_id,
                                zone,
                            });
                            if pg.drop_target != new_target {
                                pg.drop_target = new_target;
                                cx.notify();
                            }
                        });
                    } else {
                        // Cursor left this pane — clear if we were the active target
                        pg_drag.update(cx, |pg, cx| {
                            if pg.drop_target.map(|t| t.group_id) == Some(group_id) {
                                pg.drop_target = None;
                                cx.notify();
                            }
                        });
                    }
                },
            )
            .on_drop(move |payload: &TabDragPayload, _window, cx| {
                pg_drop.update(cx, |pg, cx| {
                    let zone = pg.drop_target.map(|t| t.zone).unwrap_or(DropZone::Center);
                    pg.drop_target = None;

                    handle_tab_drop(
                        &mut pg.root,
                        payload.source_group_id,
                        payload.tab_ix,
                        group_id,
                        zone,
                        cx,
                    );
                    cx.notify();
                });
            })
            .child(content)
            .children(render_drop_overlay(group_id, drop_target));

        // Clear drop target when drag leaves this group
        let pg_clear = pane_group_entity.clone();
        let pg_focus = pane_group_entity.clone();

        div()
            .id(ElementId::Name(format!("tab-group-{group_id}").into()))
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                pg_focus.update(cx, |pg, cx| {
                    if pg.focused_group_id != Some(group_id) {
                        pg.focused_group_id = Some(group_id);
                        cx.notify();
                    }
                });
            })
            .on_mouse_up(MouseButton::Left, move |_, _window, cx| {
                pg5.update(cx, |pg, cx| {
                    if pg.drop_target.is_some() {
                        pg.drop_target = None;
                        cx.notify();
                    }
                });
            })
            .on_mouse_move(move |event, _window, cx| {
                // Clear drop target if we're not dragging (no buttons pressed)
                if event.pressed_button.is_none() {
                    pg_clear.update(cx, |pg, cx| {
                        if pg.drop_target.is_some() {
                            pg.drop_target = None;
                            cx.notify();
                        }
                    });
                }
            })
            .child(tab_bar)
            .child(content_area)
            .into_any_element()
    }

    fn render_split(
        split_id: usize,
        axis: SplitAxis,
        children: &[SplitNode],
        ratios: &[f32],
        pane_group_entity: &Entity<PaneGroup>,
        drop_target: Option<DropTarget>,
        cx: &App,
    ) -> AnyElement {
        let split_axis = axis;

        let container_id = format!("split-container-{split_id}");
        let mut container = div()
            .id(ElementId::Name(container_id.into()))
            .flex()
            .size_full()
            .overflow_hidden();

        container = match axis {
            SplitAxis::Horizontal => container.flex_row(),
            SplitAxis::Vertical => container.flex_col(),
        };

        let pg = pane_group_entity.clone();
        container = container.on_drag_move::<ResizeHandleDrag>(
            move |event: &DragMoveEvent<ResizeHandleDrag>, _window, cx| {
                let drag = event.drag(cx);
                if drag.split_id != split_id {
                    return;
                }
                let bounds = event.bounds;
                let pos = event.event.position;

                let fraction = match drag.axis {
                    SplitAxis::Horizontal => {
                        let w: f32 = bounds.size.width.into();
                        if w > 0.0 {
                            (pos.x - bounds.origin.x) / bounds.size.width
                        } else {
                            0.5
                        }
                    }
                    SplitAxis::Vertical => {
                        let h: f32 = bounds.size.height.into();
                        if h > 0.0 {
                            (pos.y - bounds.origin.y) / bounds.size.height
                        } else {
                            0.5
                        }
                    }
                };

                let handle_idx = drag.handle_idx;

                pg.update(cx, |pg, cx| {
                    adjust_split_from_fraction(
                        &mut pg.root,
                        split_id,
                        handle_idx,
                        fraction,
                    );
                    cx.notify();
                });
            },
        );

        for (i, (child, ratio)) in children.iter().zip(ratios.iter()).enumerate() {
            if i > 0 {
                let handle_id = format!("rhandle-{split_id}-{i}");
                let drag_payload = ResizeHandleDrag {
                    split_id,
                    handle_idx: i - 1,
                    axis: split_axis,
                };

                let handle = match axis {
                    SplitAxis::Horizontal => div()
                        .id(ElementId::Name(handle_id.into()))
                        .w(px(1.0))
                        .h_full()
                        .flex_shrink_0()
                        .cursor_col_resize()
                        .bg(colors::border())
                        .hover(|s| s.bg(colors::accent()))
                        .on_drag(drag_payload, |_, _, _, cx| {
                            cx.new(|_| ResizeGhost)
                        }),
                    SplitAxis::Vertical => div()
                        .id(ElementId::Name(handle_id.into()))
                        .w_full()
                        .h(px(1.0))
                        .flex_shrink_0()
                        .cursor_row_resize()
                        .bg(colors::border())
                        .hover(|s| s.bg(colors::accent()))
                        .on_drag(drag_payload, |_, _, _, cx| {
                            cx.new(|_| ResizeGhost)
                        }),
                };

                container = container.child(handle);
            }

            let child_el = div()
                .flex()
                .size_full()
                .overflow_hidden()
                .flex_basis(relative(*ratio))
                .flex_grow()
                .flex_shrink()
                .child(Self::render_node(child, pane_group_entity, drop_target, cx));

            container = container.child(child_el);
        }

        container.into_any_element()
    }
}

// ── Drop zone computation ──

/// Given the bounds of a content area and the cursor position,
/// determine which drop zone the cursor is in.
/// The edges are the outer 25% on each side; center is everything else.
fn compute_drop_zone(bounds: Bounds<Pixels>, pos: Point<Pixels>) -> DropZone {
    let rel_x: f32 = (pos.x - bounds.origin.x).into();
    let rel_y: f32 = (pos.y - bounds.origin.y).into();
    let w: f32 = bounds.size.width.into();
    let h: f32 = bounds.size.height.into();

    if w <= 0.0 || h <= 0.0 {
        return DropZone::Center;
    }

    let frac_x = rel_x / w;
    let frac_y = rel_y / h;

    // Edge threshold: 25%
    let edge = 0.25;

    let in_left = frac_x < edge;
    let in_right = frac_x > (1.0 - edge);
    let in_top = frac_y < edge;
    let in_bottom = frac_y > (1.0 - edge);

    if !in_left && !in_right && !in_top && !in_bottom {
        return DropZone::Center;
    }

    // Distance from each edge (as fraction)
    let dist_left = frac_x;
    let dist_right = 1.0 - frac_x;
    let dist_top = frac_y;
    let dist_bottom = 1.0 - frac_y;

    // Pick the closest edge. Use ordered comparisons to avoid float equality issues.
    // Priority when tied: Left > Right > Top > Bottom
    let mut best = DropZone::Left;
    let mut best_dist = dist_left;

    if dist_right < best_dist {
        best = DropZone::Right;
        best_dist = dist_right;
    }
    if dist_top < best_dist {
        best = DropZone::Top;
        best_dist = dist_top;
    }
    if dist_bottom < best_dist {
        best = DropZone::Bottom;
    }

    best
}

/// Render a semi-transparent overlay showing where the drop will happen.
fn render_drop_overlay(group_id: usize, drop_target: Option<DropTarget>) -> Option<Div> {
    let target = drop_target?;
    if target.group_id != group_id {
        return None;
    }

    let overlay_color = rgba(0x89b4fa40); // accent with ~25% opacity

    let overlay = match target.zone {
        DropZone::Left => div()
            .absolute()
            .top_0()
            .left_0()
            .w(relative(0.5))
            .h_full()
            .bg(overlay_color)
            .border_2()
            .border_color(colors::accent()),
        DropZone::Right => div()
            .absolute()
            .top_0()
            .right_0()
            .w(relative(0.5))
            .h_full()
            .bg(overlay_color)
            .border_2()
            .border_color(colors::accent()),
        DropZone::Top => div()
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h(relative(0.5))
            .bg(overlay_color)
            .border_2()
            .border_color(colors::accent()),
        DropZone::Bottom => div()
            .absolute()
            .bottom_0()
            .left_0()
            .w_full()
            .h(relative(0.5))
            .bg(overlay_color)
            .border_2()
            .border_color(colors::accent()),
        DropZone::Center => div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(overlay_color)
            .border_2()
            .border_color(colors::accent()),
    };

    Some(overlay)
}

/// Handle a tab being dropped on a content area with a specific zone.
fn handle_tab_drop(
    root: &mut SplitNode,
    src_group_id: usize,
    src_tab_ix: usize,
    dst_group_id: usize,
    zone: DropZone,
    cx: &mut App,
) {
    match zone {
        DropZone::Center => {
            // Move tab into existing group (merge)
            if src_group_id != dst_group_id {
                move_tab_between_groups(root, src_group_id, src_tab_ix, dst_group_id, usize::MAX);
            }
        }
        DropZone::Left | DropZone::Right | DropZone::Top | DropZone::Bottom => {
            // Extract the tab from the source
            let tab = extract_tab_from_group(root, src_group_id, src_tab_ix);
            if let Some(tab) = tab {
                let (axis, before) = match zone {
                    DropZone::Left => (SplitAxis::Horizontal, true),
                    DropZone::Right => (SplitAxis::Horizontal, false),
                    DropZone::Top => (SplitAxis::Vertical, true),
                    DropZone::Bottom => (SplitAxis::Vertical, false),
                    DropZone::Center => unreachable!(),
                };

                // If we couldn't find the target (source was the only group and now empty),
                // just create a new leaf
                if !root.split_leaf_with_tab(dst_group_id, axis, tab.clone(), before) {
                    // Fallback: just add to first leaf
                    if let Some(group) = find_first_group_mut(root) {
                        group.tabs.push(tab);
                        group.active_tab_ix = group.tabs.len() - 1;
                    }
                }

                prune_empty_leaves(root);
            } else if src_group_id == dst_group_id {
                // Dragging within the same group to split: create a new terminal
                let new_tab = cx.new(|cx| Tab::new("Terminal", cx));
                let (axis, before) = match zone {
                    DropZone::Left => (SplitAxis::Horizontal, true),
                    DropZone::Right => (SplitAxis::Horizontal, false),
                    DropZone::Top => (SplitAxis::Vertical, true),
                    DropZone::Bottom => (SplitAxis::Vertical, false),
                    DropZone::Center => unreachable!(),
                };
                root.split_leaf_with_tab(dst_group_id, axis, new_tab, before);
            }
        }
    }
}

// ── Helper functions to operate on specific tab groups by ID ──

fn find_group_mut(node: &mut SplitNode, group_id: usize) -> Option<&mut TabGroup> {
    match node {
        SplitNode::Leaf(group) => {
            if group.id == group_id {
                Some(group)
            } else {
                None
            }
        }
        SplitNode::Split { children, .. } => {
            for child in children.iter_mut() {
                if let Some(g) = find_group_mut(child, group_id) {
                    return Some(g);
                }
            }
            None
        }
    }
}

fn find_first_group_mut(node: &mut SplitNode) -> Option<&mut TabGroup> {
    match node {
        SplitNode::Leaf(group) => Some(group),
        SplitNode::Split { children, .. } => {
            for child in children.iter_mut() {
                if let Some(g) = find_first_group_mut(child) {
                    return Some(g);
                }
            }
            None
        }
    }
}

/// Extract (remove) a tab from a group, returning the tab entity.
fn extract_tab_from_group(
    root: &mut SplitNode,
    group_id: usize,
    tab_ix: usize,
) -> Option<Entity<Tab>> {
    if let Some(group) = find_group_mut(root, group_id) {
        if tab_ix < group.tabs.len() {
            let tab = group.tabs.remove(tab_ix);
            if group.active_tab_ix >= group.tabs.len() && !group.tabs.is_empty() {
                group.active_tab_ix = group.tabs.len() - 1;
            }
            Some(tab)
        } else {
            None
        }
    } else {
        None
    }
}

fn set_active_tab_in_group(node: &mut SplitNode, group_id: usize, ix: usize) {
    if let Some(group) = find_group_mut(node, group_id) {
        if ix < group.tabs.len() {
            group.active_tab_ix = ix;
        }
    }
}

fn add_tab_in_group(node: &mut SplitNode, group_id: usize, cx: &mut App) {
    if let Some(group) = find_group_mut(node, group_id) {
        let idx = group.tabs.len() + 1;
        let tab = cx.new(|cx| Tab::new(&format!("Terminal {}", idx), cx));
        group.tabs.push(tab);
        group.active_tab_ix = group.tabs.len() - 1;
    }
}

fn reorder_tab_in_group(node: &mut SplitNode, group_id: usize, from: usize, to: usize) {
    if let Some(group) = find_group_mut(node, group_id) {
        if from < group.tabs.len() && to < group.tabs.len() && from != to {
            let tab = group.tabs.remove(from);
            group.tabs.insert(to, tab);
            group.active_tab_ix = to;
        }
    }
}

fn close_tab_in_group(node: &mut SplitNode, group_id: usize, ix: usize) {
    // Don't close the very last tab in the entire layout
    if count_total_tabs(node) <= 1 {
        return;
    }
    if let Some(group) = find_group_mut(node, group_id) {
        if ix < group.tabs.len() {
            group.tabs.remove(ix);
            if group.active_tab_ix >= group.tabs.len() && !group.tabs.is_empty() {
                group.active_tab_ix = group.tabs.len() - 1;
            } else if ix < group.active_tab_ix {
                group.active_tab_ix -= 1;
            }
        }
    }
    prune_empty_leaves(node);
}

/// Close the active tab in the given group (by group_id).
fn close_tab_in_focused_group(node: &mut SplitNode, group_id: usize) {
    if let Some(group) = find_group_mut(node, group_id) {
        if !group.tabs.is_empty() {
            group.tabs.remove(group.active_tab_ix);
            if group.active_tab_ix >= group.tabs.len() && !group.tabs.is_empty() {
                group.active_tab_ix = group.tabs.len() - 1;
            }
        }
    }
}

/// Get the id of the first leaf group in the tree.
fn first_group_id(node: &SplitNode) -> Option<usize> {
    match node {
        SplitNode::Leaf(group) => Some(group.id),
        SplitNode::Split { children, .. } => {
            for child in children {
                if let Some(id) = first_group_id(child) {
                    return Some(id);
                }
            }
            None
        }
    }
}

fn move_tab_between_groups(
    root: &mut SplitNode,
    src_group_id: usize,
    src_tab_ix: usize,
    dst_group_id: usize,
    dst_tab_ix: usize,
) {
    let tab = extract_tab_from_group(root, src_group_id, src_tab_ix);
    if let Some(tab) = tab {
        if let Some(dst) = find_group_mut(root, dst_group_id) {
            let insert_at = if dst_tab_ix > dst.tabs.len() {
                dst.tabs.len()
            } else {
                dst_tab_ix
            };
            dst.tabs.insert(insert_at, tab);
            dst.active_tab_ix = insert_at;
        }
        prune_empty_leaves(root);
    }
}

/// Public wrapper for pruning empty leaves, called from workspace.
/// Returns true if the root itself is now empty (shouldn't happen — caller should prevent).
pub fn prune_empty_leaves_pub(node: &mut SplitNode) {
    prune_empty_leaves(node);
}

/// Check if the tree has only a single tab left across all leaves.
pub fn count_total_tabs(node: &SplitNode) -> usize {
    match node {
        SplitNode::Leaf(group) => group.tabs.len(),
        SplitNode::Split { children, .. } => {
            children.iter().map(count_total_tabs).sum()
        }
    }
}

/// Remove empty leaf nodes from the tree and simplify single-child splits.
fn prune_empty_leaves(node: &mut SplitNode) {
    if let SplitNode::Split {
        children, ratios, ..
    } = node
    {
        for child in children.iter_mut() {
            prune_empty_leaves(child);
        }

        let mut i = 0;
        while i < children.len() {
            if let SplitNode::Leaf(group) = &children[i] {
                if group.tabs.is_empty() {
                    children.remove(i);
                    ratios.remove(i);
                    continue;
                }
            }
            i += 1;
        }

        if !ratios.is_empty() {
            let total: f32 = ratios.iter().sum();
            if total > 0.0 {
                for r in ratios.iter_mut() {
                    *r /= total;
                }
            }
        }

        if children.len() == 1 {
            let child = children.remove(0);
            *node = child;
        }
    }
}

fn set_ratio_from_fraction(ratios: &mut [f32], handle_idx: usize, fraction: f32) {
    if handle_idx + 1 >= ratios.len() {
        return;
    }
    let min_ratio = 0.05;
    let prefix_sum: f32 = ratios[..handle_idx].iter().sum();
    let a_plus_b: f32 = ratios[handle_idx] + ratios[handle_idx + 1];
    let desired_a = (fraction - prefix_sum).clamp(min_ratio, a_plus_b - min_ratio);
    ratios[handle_idx] = desired_a;
    ratios[handle_idx + 1] = a_plus_b - desired_a;
}

fn adjust_split_from_fraction(
    node: &mut SplitNode,
    split_id: usize,
    handle_idx: usize,
    fraction: f32,
) {
    match node {
        SplitNode::Leaf(_) => {}
        SplitNode::Split {
            id,
            ratios,
            children,
            ..
        } => {
            if *id == split_id {
                set_ratio_from_fraction(ratios, handle_idx, fraction);
            } else {
                for child in children.iter_mut() {
                    adjust_split_from_fraction(child, split_id, handle_idx, fraction);
                }
            }
        }
    }
}
