use gpui::{App, Bounds, Font, Pixels, Task, TextSystem, Window, px};
use std::time::Duration;

/// Vertical extent of the text caret relative to the same baseline used to paint glyphs.
#[derive(Clone, Copy)]
pub(crate) struct GlyphSpan {
    pub top: Pixels,
    pub height: Pixels,
}

impl GlyphSpan {
    pub fn measure(
        text: &TextSystem,
        font: &Font,
        size: Pixels,
        row: Pixels,
        include_descenders: bool,
    ) -> Self {
        let face = text.resolve_font(font);
        // Some platform font backends return a signed descender. GPUI's shaped-line
        // painter centers ascent plus the positive descent in the available row.
        let ascent = text.ascent(face, size);
        let descent = text.descent(face, size).abs();
        let baseline = (row - ascent - descent) / 2. + ascent;
        let cap = text.cap_height(face, size);
        let cap = if cap > px(0.) { cap } else { size * 0.72 };
        let descent = if include_descenders {
            descent.min(size * 0.2)
        } else {
            px(0.)
        };
        let top = (baseline - cap).clamp(px(0.), (row - px(1.)).max(px(0.)));
        let bottom = (baseline + descent).clamp(top + px(1.), row.max(top + px(1.)));
        Self {
            top,
            height: bottom - top,
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
