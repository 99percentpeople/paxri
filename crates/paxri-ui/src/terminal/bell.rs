use gpui::{Context, Task};
use std::time::{Duration, Instant};

const SOUND_COOLDOWN: Duration = Duration::from_millis(350);
const INDICATOR_DURATION: Duration = Duration::from_millis(1800);

/// Tracks terminal BEL (`\x07`) sound throttling and brief tab/header bell-icon visibility.
#[derive(Default)]
pub struct BellState {
    ringing_until: Option<Instant>,
    last_sound: Option<Instant>,
    clear_task: Option<Task<()>>,
}

impl BellState {
    /// Record a bell event when owned as a GPUI `Entity<BellState>`.
    /// Returns `true` when the system alert sound should play.
    pub fn ring(&mut self, cx: &mut Context<Self>) -> bool {
        let play = self.trigger_at(Instant::now());
        self.clear_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(INDICATOR_DURATION).await;
            let _ = this.update(cx, |state, cx| {
                if state
                    .ringing_until
                    .is_some_and(|until| Instant::now() >= until)
                {
                    state.ringing_until = None;
                    state.clear_task = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
        play
    }

    /// Record a bell event when owned directly inside a parent view `V`.
    /// Calls `cx.notify()` on `V` when the indicator expires.
    pub fn ring_in<V: 'static>(
        &mut self,
        access: impl Fn(&mut V) -> Option<&mut Self> + 'static,
        cx: &mut Context<V>,
    ) -> bool {
        let play = self.trigger_at(Instant::now());
        self.clear_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(INDICATOR_DURATION).await;
            let _ = this.update(cx, |view, cx| {
                if let Some(state) = access(view)
                    && state
                        .ringing_until
                        .is_some_and(|until| Instant::now() >= until)
                {
                    state.ringing_until = None;
                    state.clear_task = None;
                    cx.notify();
                }
            });
        }));
        play
    }

    pub fn is_ringing(&self) -> bool {
        self.is_ringing_at(Instant::now())
    }

    fn trigger_at(&mut self, now: Instant) -> bool {
        self.ringing_until = Some(now + INDICATOR_DURATION);
        let play = self
            .last_sound
            .is_none_or(|prev| now.saturating_duration_since(prev) >= SOUND_COOLDOWN);
        if play {
            self.last_sound = Some(now);
        }
        play
    }

    fn is_ringing_at(&self, now: Instant) -> bool {
        self.ringing_until.is_some_and(|until| now < until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bell_throttles_burst_sounds_and_expires_indicator() {
        let mut bell = BellState::default();
        let t0 = Instant::now();
        assert!(!bell.is_ringing_at(t0));
        assert!(bell.trigger_at(t0));
        assert!(bell.is_ringing_at(t0 + Duration::from_millis(100)));
        assert!(!bell.trigger_at(t0 + Duration::from_millis(100)));
        assert!(bell.trigger_at(t0 + SOUND_COOLDOWN));
        let retriggered = t0 + SOUND_COOLDOWN;
        assert!(bell.is_ringing_at(retriggered + INDICATOR_DURATION - Duration::from_millis(1)));
        assert!(!bell.is_ringing_at(retriggered + INDICATOR_DURATION));
    }
}
