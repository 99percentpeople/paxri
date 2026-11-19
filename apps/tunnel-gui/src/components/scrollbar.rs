//! A vertical overlay for pixel-based scroll views and line-based terminal history.
use super::Ui;
use gpui::{prelude::*, *};
use std::cell::Cell;

const TRACK_WIDTH: Pixels = px(12.);
const MIN_THUMB: Pixels = px(24.);

/// Visible extent, scrollable distance (content minus viewport), and distance from the top.
/// All three measurements use the same units, such as pixels or terminal rows.
#[derive(Clone, Copy)]
pub struct ScrollMetrics {
    viewport: f32,
    max_offset: f32,
    offset: f32,
}

impl ScrollMetrics {
    pub fn new(viewport: f32, max_offset: f32, offset: f32) -> Self {
        let max_offset = max_offset.max(0.);
        Self {
            viewport,
            max_offset,
            offset: offset.clamp(0., max_offset),
        }
    }

    fn thumb(self, track: Pixels) -> Option<Thumb> {
        if self.viewport <= 0. || self.max_offset <= 0. || track <= px(0.) {
            return None;
        }
        let visible = self.viewport / (self.viewport + self.max_offset);
        let length = (track * visible).max(MIN_THUMB.min(track / 2.));
        let travel = track - length;
        (travel > px(0.)).then_some(Thumb {
            start: travel * (self.offset / self.max_offset),
            length,
            travel,
        })
    }

    fn page(self, forward: bool) -> f32 {
        let delta = if forward {
            self.viewport
        } else {
            -self.viewport
        };
        (self.offset + delta).clamp(0., self.max_offset)
    }
}

/// The view remains the owner of its scrolling position, including wheel or keyboard changes.
pub trait ScrollSource: Clone + 'static {
    fn metrics(&self, cx: &App) -> ScrollMetrics;
    fn scroll_to(&self, offset: f32, cx: &mut App);
}

impl ScrollSource for ScrollHandle {
    fn metrics(&self, _: &App) -> ScrollMetrics {
        ScrollMetrics::new(
            self.bounds().size.height.into(),
            self.max_offset().height.into(),
            (-self.offset().y).into(),
        )
    }

    fn scroll_to(&self, offset: f32, _: &mut App) {
        self.set_offset(point(self.offset().x, px(-offset)));
    }
}

#[derive(Clone, Copy)]
struct Thumb {
    start: Pixels,
    length: Pixels,
    travel: Pixels,
}

impl Thumb {
    fn grab_at(self, y: Pixels) -> Option<f32> {
        (self.start..=self.start + self.length)
            .contains(&y)
            .then(|| (y - self.start) / self.length)
    }

    fn offset_at(self, y: Pixels, grab: f32, max_offset: f32) -> f32 {
        ((y - self.length * grab) / self.travel).clamp(0., 1.) * max_offset
    }
}

/// Place after the scrollable content in a relative container. It only receives hits on overflow.
/// For GPUI containers, pair `.track_scroll(&handle)` with `Scrollbar::vertical(id, handle.clone())`.
#[derive(IntoElement)]
pub struct Scrollbar<S: ScrollSource> {
    id: ElementId,
    source: S,
}

impl<S: ScrollSource> Scrollbar<S> {
    pub fn vertical(id: impl Into<ElementId>, source: S) -> Self {
        Self {
            id: id.into(),
            source,
        }
    }
}

impl<S: ScrollSource> RenderOnce for Scrollbar<S> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let grab = window.use_keyed_state(self.id, cx, |_, _| Cell::new(None::<f32>));
        let colors = Ui::new(window, cx).colors;
        let source = self.source;
        let measure = source.clone();
        canvas(
            move |bounds, w, cx| {
                let thumb = measure.metrics(cx).thumb(bounds.size.height)?;
                let hit = w.insert_hitbox(bounds, HitboxBehavior::BlockMouseExceptScroll);
                Some((thumb, hit))
            },
            move |bounds, measured, w, cx| {
                if !w.is_window_active() || measured.is_none() {
                    grab.read(cx).set(None);
                }
                let Some((thumb, hit)) = measured else { return };
                let hovered = hit.is_hovered(w);
                let active = hovered || grab.read(cx).get().is_some();
                let width = if active { px(8.) } else { px(6.) };
                let origin = bounds.origin + point((bounds.size.width - width) / 2., thumb.start);
                let color = Hsla::from(rgb(colors.muted_foreground)).opacity(if active {
                    1.
                } else {
                    0.65
                });
                w.paint_quad(
                    fill(Bounds::new(origin, size(width, thumb.length)), color)
                        .corner_radii(width / 2.),
                );
                w.set_cursor_style(CursorStyle::Arrow, &hit);

                let owner = w.current_view();
                let down_hit = hit.clone();
                let down_source = source.clone();
                let down_grab = grab.clone();
                w.on_mouse_event(move |event: &MouseDownEvent, phase, w, cx| {
                    if phase != DispatchPhase::Bubble
                        || event.button != MouseButton::Left
                        || !down_hit.is_hovered(w)
                    {
                        return;
                    }
                    let y = event.position.y - bounds.top();
                    let anchor = thumb.grab_at(y);
                    down_grab.read(cx).set(anchor);
                    if anchor.is_none() {
                        let offset = down_source.metrics(cx).page(y > thumb.start);
                        down_source.scroll_to(offset, cx);
                    }
                    w.prevent_default();
                    cx.stop_propagation();
                    cx.notify(owner);
                });

                // Read the current range on each event so resizes and incoming text are reflected mid-drag.
                let seek = move |y: Pixels, anchor: f32, cx: &mut App| {
                    let metrics = source.metrics(cx);
                    if let Some(thumb) = metrics.thumb(bounds.size.height) {
                        source.scroll_to(
                            thumb.offset_at(y - bounds.top(), anchor, metrics.max_offset),
                            cx,
                        );
                    }
                };
                let move_grab = grab.clone();
                let move_seek = seek.clone();
                w.on_mouse_event(move |event: &MouseMoveEvent, phase, w, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    if let Some(anchor) = move_grab.read(cx).get() {
                        if event.pressed_button == Some(MouseButton::Left) && w.is_window_active() {
                            move_seek(event.position.y, anchor, cx);
                            cx.stop_propagation();
                        } else {
                            move_grab.read(cx).set(None);
                        }
                        cx.notify(owner);
                    } else if hovered != hit.is_hovered(w) {
                        cx.notify(owner);
                    }
                });
                w.on_mouse_event(move |event: &MouseUpEvent, phase, w, cx| {
                    if phase == DispatchPhase::Capture
                        && event.button == MouseButton::Left
                        && let Some(anchor) = grab.read(cx).take()
                    {
                        if w.is_window_active() {
                            seek(event.position.y, anchor, cx);
                        }
                        cx.stop_propagation();
                        cx.notify(owner);
                    }
                });
            },
        )
        .absolute()
        .right(px(1.))
        .top(px(2.))
        .bottom(px(2.))
        .w(TRACK_WIDTH)
    }
}

#[cfg(test)]
mod tests {
    use super::{MIN_THUMB, ScrollMetrics};
    use gpui::px;

    #[test]
    fn only_overflowing_views_have_a_thumb() {
        for (viewport, overflow, track) in [(100., 0., 100.), (0., 30., 100.), (20., 50., 0.)] {
            assert!(
                ScrollMetrics::new(viewport, overflow, 0.)
                    .thumb(px(track))
                    .is_none()
            );
        }
        let thumb = ScrollMetrics::new(25., 75., 50.).thumb(px(200.)).unwrap();
        assert_eq!(thumb.length, px(50.));
        assert_eq!(thumb.start, px(100.));
        assert_eq!(thumb.grab_at(px(110.)), Some(0.2));
        assert_eq!(thumb.grab_at(px(20.)), None);
        assert_eq!(thumb.offset_at(px(110.), 0.2, 75.), 50.);
    }

    #[test]
    fn minimum_thumb_still_reaches_both_ends_on_short_tracks() {
        let metrics = ScrollMetrics::new(20., 2000., 1600.);
        for track in [px(200.), px(20.)] {
            let thumb = metrics.thumb(track).unwrap();
            assert_eq!(thumb.length, MIN_THUMB.min(track / 2.));
            assert_eq!(thumb.offset_at(px(-50.), 0.5, 2000.), 0.);
            assert_eq!(thumb.offset_at(track + px(50.), 0.5, 2000.), 2000.);
        }
    }

    #[test]
    fn track_clicks_page_in_source_units_and_clamp_at_the_ends() {
        assert_eq!(ScrollMetrics::new(12., 100., 30.).page(false), 18.);
        assert_eq!(ScrollMetrics::new(12., 100., 95.).page(true), 100.);
        assert_eq!(ScrollMetrics::new(12., 100., 3.).page(false), 0.);
    }
}
