use gpui::*;
use std::rc::Rc;
use std::time::Duration;

use crate::theme::colors;

pub const WIDTH: Pixels = px(10.0);
pub const THUMB_WIDTH: Pixels = px(6.0);
const MIN_THUMB_HEIGHT: Pixels = px(24.0);
const IDLE_FADE_MS: u64 = 1200;

#[derive(Default)]
pub struct ScrollbarState {
    pub visible: bool,
    /// Cursor-Y offset within the thumb at drag start (window coords).
    /// `Some` means an active drag — the caller's root on_mouse_move
    /// should translate mouse Y into a new scroll offset via
    /// `drag_to_offset`.
    pub drag_cursor_within_thumb: Option<Pixels>,
    /// Track origin Y in window coords, captured at last paint.
    pub track_origin_y: Pixels,
    /// Track height in pixels (== viewport height), captured at last paint.
    pub track_height: Pixels,
    /// Total content height, captured at last paint.
    pub content_height: Pixels,
    pub hide_task: Option<Task<()>>,
}

/// Schedule a task that hides the scrollbar after the idle timeout unless
/// the user is currently dragging. Store the returned task in
/// `state.hide_task` so a new scroll event cancels the pending hide.
pub fn schedule_hide<T: 'static, F>(cx: &mut Context<T>, extract: F) -> Task<()>
where
    F: Fn(&mut T) -> &mut ScrollbarState + Copy + 'static,
{
    cx.spawn(async move |this, cx| {
        smol::Timer::after(Duration::from_millis(IDLE_FADE_MS)).await;
        let _ = this.update(cx, move |this, cx| {
            let state = extract(this);
            if state.drag_cursor_within_thumb.is_none() {
                state.visible = false;
                cx.notify();
            }
        });
    })
}

/// Record the start of a thumb drag. `cursor_y` is the window-coord Y of
/// the click, `thumb_top_within_track` is the thumb's top relative to
/// the track (already computed by the render layer).
pub fn start_drag(state: &mut ScrollbarState, cursor_y: Pixels, thumb_top_within_track: Pixels) {
    let thumb_abs_top = state.track_origin_y + thumb_top_within_track;
    state.drag_cursor_within_thumb = Some(cursor_y - thumb_abs_top);
}

/// Given the drag state + current mouse Y (window coords), return the
/// new scroll offset to apply. Returns `None` when not dragging or when
/// there's nothing to scroll.
pub fn drag_to_offset(state: &ScrollbarState, mouse_y: Pixels) -> Option<Pixels> {
    let cursor_within_thumb = state.drag_cursor_within_thumb?;
    let max_offset = (state.content_height - state.track_height).max(px(0.0));
    if max_offset <= px(0.0) || state.track_height <= px(0.0) {
        return None;
    }
    let thumb_h = thumb_height_px(state.track_height, state.content_height);
    let usable = (state.track_height - thumb_h).max(px(1.0));
    let raw_top = mouse_y - state.track_origin_y - cursor_within_thumb;
    let thumb_top = raw_top.clamp(px(0.0), usable);
    let ratio = f32::from(thumb_top) / f32::from(usable);
    Some(max_offset * ratio)
}

fn thumb_color(alpha: f32) -> Rgba {
    let base = colors::text_muted();
    Rgba { a: alpha, ..base }
}

fn thumb_height_px(track: Pixels, content: Pixels) -> Pixels {
    if content <= px(0.0) || content <= track {
        return track;
    }
    let ratio = f32::from(track) / f32::from(content);
    let raw = track * ratio;
    if raw < MIN_THUMB_HEIGHT {
        MIN_THUMB_HEIGHT.min(track)
    } else {
        raw
    }
}

pub struct Geometry {
    pub scroll_offset: Pixels,
    pub content_height: Pixels,
    pub viewport_height: Pixels,
}

/// Render a vertical auto-hiding scrollbar.
///
/// Returns `None` when there's nothing to scroll. The caller overlays
/// this on a `.relative()` container and is responsible for:
///   1. forwarding `on_mouse_move` at the root to call `drag_to_offset`
///      and apply the result to the scroll handle;
///   2. clearing `state.drag_cursor_within_thumb` on `on_mouse_up`;
///   3. calling `schedule_hide` whenever a scroll event occurs.
///
/// `bounds_sink` is called each paint with the track's bounds so drag
/// math can translate mouse Y into a scroll offset. `on_thumb_down` is
/// called when the user presses the thumb; arguments are the cursor Y
/// (window coords) and the thumb top within the track.
pub fn render_vertical(
    id: impl Into<ElementId>,
    geometry: Geometry,
    visible: bool,
    dragging: bool,
    bounds_sink: Rc<dyn Fn(Bounds<Pixels>, &mut App)>,
    on_thumb_down: Rc<dyn Fn(Pixels, Pixels, &mut Window, &mut App)>,
) -> Option<Stateful<Div>> {
    let Geometry { scroll_offset, content_height, viewport_height } = geometry;
    if content_height <= px(0.0) {
        return None;
    }
    // Don't render while viewport is unknown AND content clearly fits the
    // measured viewport — nothing to scroll.
    if viewport_height > px(0.0) && content_height <= viewport_height {
        return None;
    }

    let shown = visible || dragging;
    let opacity = if shown { 1.0 } else { 0.0 };

    let mut track = div()
        .id(id)
        .absolute()
        .top_0()
        .right_0()
        .h_full()
        .w(WIDTH)
        .opacity(opacity)
        .child(
            canvas(
                move |bounds, _window, cx| {
                    bounds_sink(bounds, cx);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );
    // Only intercept mouse events while visible so hidden state doesn't
    // block clicks on underlying elements.
    if shown {
        track = track.occlude();
    }

    // Thumb is only added once we have a measured viewport height —
    // before that we can't compute its size or position.
    if viewport_height > px(0.0) {
        let max_offset = (content_height - viewport_height).max(px(1.0));
        let thumb_h = thumb_height_px(viewport_height, content_height);
        let usable = (viewport_height - thumb_h).max(px(1.0));
        let thumb_top = {
            let ratio = (f32::from(scroll_offset) / f32::from(max_offset)).clamp(0.0, 1.0);
            usable * ratio
        };
        track = track.child(
            div()
                .id("scrollbar-thumb")
                .absolute()
                .top(thumb_top)
                .right(px(2.0))
                .w(THUMB_WIDTH)
                .h(thumb_h)
                .rounded_full()
                .bg(thumb_color(0.55))
                .hover(|s| s.bg(thumb_color(0.85)))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    on_thumb_down(event.position.y, thumb_top, window, cx);
                    cx.stop_propagation();
                }),
        );
    }

    Some(track)
}
