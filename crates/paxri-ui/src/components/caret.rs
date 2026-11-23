use gpui::{App, Bounds, Font, Pixels, Task, TextSystem, Window};
use std::time::Duration;

/// Vertical extent of the text caret relative to the same baseline used to paint glyphs.
#[derive(Clone, Copy)]
pub(crate) struct GlyphSpan {
    top: Pixels,
    height: Pixels,
    pub(crate) baseline: Pixels,
}

impl GlyphSpan {
    pub fn measure(text: &TextSystem, font: &Font, size: Pixels, row: Pixels) -> Self {
        let face = text.resolve_font(font);
        Self::from_metrics(size, row, text.ascent(face, size), text.descent(face, size))
    }

    pub fn from_metrics(size: Pixels, row: Pixels, ascent: Pixels, descent: Pixels) -> Self {
        // Center the font's line box like the text painter. An empty line can lack metrics.
        let height = (ascent + descent.abs()).max(size).min(row);
        Self {
            top: (row - height) / 2.,
            height,
            baseline: (row + ascent - descent.abs()) / 2.,
        }
    }

    pub fn in_row(self, mut bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        bounds.origin.y += self.top;
        bounds.size.height = self.height;
        bounds
    }
}

struct Blink<K> {
    position: K,
    hidden: bool,
    next: Option<Task<()>>,
}

/// Keyed to the rendered view so a hidden or unfocused caret needs no timer.
pub fn blink_visible<K: Copy + PartialEq + 'static>(
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
