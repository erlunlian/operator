use gpui::*;
use std::rc::Rc;

use crate::theme::colors;

#[derive(Clone)]
pub struct TabDragPayload {
    pub tab_ix: usize,
    pub title: SharedString,
    pub source_group_id: usize,
}

pub struct DragGhost {
    title: SharedString,
}

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .bg(colors::surface_hover())
            .rounded_md()
            .text_color(colors::text())
            .text_sm()
            .child(self.title.clone())
    }
}

pub struct TabBar;

impl TabBar {
    pub fn render(
        tab_titles: &[SharedString],
        active_ix: usize,
        group_id: usize,
        on_select: Rc<dyn Fn(usize, &mut Window, &mut App)>,
        on_new: Rc<dyn Fn(&mut Window, &mut App)>,
        on_drop: Option<Rc<dyn Fn(usize, usize, &mut Window, &mut App)>>,
        on_close: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
        on_cross_drop: Option<Rc<dyn Fn(usize, usize, usize, &mut Window, &mut App)>>,
    ) -> Div {
        let mut bar = div()
            .flex()
            .flex_row()
            .w_full()
            .h(px(36.0))
            .bg(colors::tab_inactive_bg())
            .border_b_1()
            .border_color(colors::border());

        for (ix, title) in tab_titles.iter().enumerate() {
            let is_active = ix == active_ix;

            let mut tab_el = div()
                .id(ElementId::Name(format!("tab-{ix}").into()))
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_3()
                .h_full()
                .text_sm()
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

            // Drag support
            let payload = TabDragPayload {
                tab_ix: ix,
                title: title.clone(),
                source_group_id: group_id,
            };
            tab_el = tab_el.on_drag(payload, move |payload, _offset, _window, cx| {
                cx.new(|_cx| DragGhost {
                    title: payload.title.clone(),
                })
            });

            // Drop support (reorder within same group, or cross-group move)
            {
                let on_drop = on_drop.clone();
                let on_cross_drop = on_cross_drop.clone();
                let target_ix = ix;
                tab_el = tab_el.on_drop(move |payload: &TabDragPayload, window, cx| {
                    if payload.source_group_id == group_id {
                        // Same group: reorder
                        if let Some(ref on_drop) = on_drop {
                            on_drop(payload.tab_ix, target_ix, window, cx);
                        }
                    } else {
                        // Different group: cross-group move
                        if let Some(ref on_cross_drop) = on_cross_drop {
                            on_cross_drop(
                                payload.source_group_id,
                                payload.tab_ix,
                                target_ix,
                                window,
                                cx,
                            );
                        }
                    }
                });
            }

            // Click to select
            let on_select = on_select.clone();
            tab_el = tab_el
                .on_click(move |_, window, cx| {
                    on_select(ix, window, cx);
                })
                .child(title.clone());

            // Close button
            if let Some(on_close) = &on_close {
                let on_close = on_close.clone();
                let close_ix = ix;
                tab_el = tab_el.child(
                    div()
                        .id(ElementId::Name(format!("tab-close-{ix}").into()))
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(16.0))
                        .h(px(16.0))
                        .rounded_sm()
                        .text_xs()
                        .text_color(colors::text_muted())
                        .hover(|s| s.bg(colors::surface_hover()).text_color(colors::text()))
                        .cursor_pointer()
                        .on_click(move |_, window, cx| {
                            on_close(close_ix, window, cx);
                        })
                        .child("×"),
                );
            }

            bar = bar.child(tab_el);
        }

        let on_new = on_new.clone();
        bar = bar.child(
            div()
                .id("tab-add")
                .flex()
                .items_center()
                .justify_center()
                .w(px(32.0))
                .h_full()
                .text_color(colors::text_muted())
                .text_sm()
                .cursor_pointer()
                .hover(|s| s.text_color(colors::text()))
                .on_click(move |_, window, cx| {
                    on_new(window, cx);
                })
                .child("+"),
        );

        bar
    }
}
