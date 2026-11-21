use gpui::{App, Task, Window};
use std::time::Duration;

struct Blink<K> {
    position: K,
    hidden: bool,
    next: Option<Task<()>>,
}

/// Keyed to the rendered view so a hidden or unfocused caret needs no timer.
pub(crate) fn blink_visible<K: Copy + PartialEq + 'static>(
    position: Option<K>,
    restart: bool,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(position) = position.filter(|_| window.is_window_active()) else {
        return true;
    };
    let blink = window.use_keyed_state("caret-blink", cx, |_, _| Blink {
        position,
        hidden: false,
        next: None,
    });
    blink.update(cx, |blink, cx| {
        if restart || blink.position != position {
            blink.position = position;
            blink.hidden = false;
            blink.next = None;
        }
        if blink.next.is_none() {
            blink.next = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(550))
                    .await;
                let _ = this.update(cx, |blink, cx| {
                    blink.next = None;
                    blink.hidden = !blink.hidden;
                    cx.notify();
                });
            }));
        }
        !blink.hidden
    })
}
