use std::time::{Duration, Instant};

const HOLD: Duration = Duration::from_millis(200);
const FADE: Duration = Duration::from_millis(160);

#[derive(Default)]
struct Motion {
    offset: Option<f32>,
    last_move: Option<Instant>,
}

impl Motion {
    fn elapsed(&mut self, offset: Option<f32>, now: Instant) -> Option<Duration> {
        match (self.offset, offset) {
            (Some(previous), Some(current)) if previous != current => self.last_move = Some(now),
            (_, None) => self.last_move = None,
            _ => {}
        }
        self.offset = offset;
        self.last_move.map(|at| now.duration_since(at))
    }

    fn opacity(elapsed: Duration, reduce_motion: bool) -> f32 {
        if elapsed < HOLD {
            1.
        } else if reduce_motion {
            0.
        } else {
            (1. - (elapsed - HOLD).div_duration_f32(FADE)).max(0.)
        }
    }
}

#[cfg(target_os = "android")]
mod indicator {
    use super::super::{ScrollSource, Scrollbar, Ui};
    use super::{HOLD, Motion};
    use gpui::{prelude::*, *};
    use std::time::Instant;

    #[derive(Default)]
    struct Indicator {
        motion: Motion,
        wake: Option<Task<()>>,
    }

    impl<S: ScrollSource> RenderOnce for Scrollbar<S> {
        fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
            let state = window.use_keyed_state(self.id, cx, |_, _| Indicator::default());
            let rgb_color = self
                .color
                .unwrap_or_else(|| Ui::new(window, cx).colors.muted_foreground);
            let color = Hsla::from(rgb(rgb_color));
            canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    let metrics = self.source.metrics(cx);
                    let thumb = metrics.thumb(bounds.size.height);
                    let reduce_motion = cx.reduce_motion();
                    let alpha = state.update(cx, |indicator, cx| {
                        let position = thumb.map(|_| metrics.position());
                        let Some(elapsed) = indicator.motion.elapsed(position, Instant::now()) else {
                            indicator.wake = None;
                            return 0.;
                        };
                        if elapsed < HOLD && indicator.wake.is_none() {
                            let timer = cx.background_executor().timer(HOLD - elapsed);
                            indicator.wake = Some(cx.spawn(async move |state, cx| {
                                timer.await;
                                let _ = state.update(cx, |indicator, cx| {
                                    indicator.wake = None;
                                    cx.notify();
                                });
                            }));
                        }
                        let alpha = Motion::opacity(elapsed, reduce_motion);
                        if elapsed >= HOLD && alpha > 0. {
                            window.request_animation_frame();
                        }
                        alpha
                    });
                    if let Some(thumb) = thumb.filter(|_| alpha > 0.) {
                        let width = bounds.size.width;
                        let origin = bounds.origin + point(px(0.), thumb.start);
                        window.paint_quad(
                            fill(
                                Bounds::new(origin, size(width, thumb.length)),
                                color.opacity(0.65 * alpha),
                            )
                            .corner_radii(width / 2.),
                        );
                    }
                },
            )
            .absolute()
            .right(px(1.))
            .top(px(2.))
            .bottom(px(2.))
            .w(px(2.))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indicator_only_appears_for_motion_and_fades_after_the_last_scroll() {
        let mut motion = Motion::default();
        let start = Instant::now();
        assert_eq!(motion.elapsed(Some(20.), start), None);
        assert_eq!(motion.elapsed(Some(20.), start + HOLD), None);
        let drag = start + HOLD;
        assert_eq!(motion.elapsed(Some(25.), drag), Some(Duration::ZERO));
        let fling = drag + HOLD;
        assert_eq!(motion.elapsed(Some(40.), fling), Some(Duration::ZERO));
        assert_eq!(
            Motion::opacity(motion.elapsed(Some(40.), fling + HOLD / 2).unwrap(), false),
            1.
        );
        assert_eq!(Motion::opacity(HOLD + FADE / 2, false), 0.5);
        assert_eq!(Motion::opacity(HOLD + FADE, false), 0.);
        assert_eq!(Motion::opacity(HOLD, true), 0.);
        assert_eq!(motion.elapsed(None, fling + HOLD + FADE), None);
        assert_eq!(motion.elapsed(Some(0.), fling + HOLD + FADE), None);
    }

    #[test]
    fn terminal_output_at_bottom_does_not_wake_indicator_until_user_scrolls() {
        use super::super::ScrollMetrics;
        let mut motion = Motion::default();
        let t0 = Instant::now();
        // Growing scrollback while pinned at the bottom keeps normalized position at 1.0.
        for (i, history) in [20., 50., 200.].into_iter().enumerate() {
            let pos = ScrollMetrics::new(24., history, history).position();
            assert_eq!(
                motion.elapsed(Some(pos), t0 + Duration::from_millis(i as u64 * 50)),
                None
            );
        }
        // Scrolling up changes normalized position and wakes the indicator.
        let scrolled = ScrollMetrics::new(24., 200., 190.).position();
        assert_eq!(
            motion.elapsed(Some(scrolled), t0 + Duration::from_millis(200)),
            Some(Duration::ZERO)
        );
    }
}
