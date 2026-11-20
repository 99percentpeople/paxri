//! Native text inputs share the clipboard, selection, IME and cursor implementation.
mod buffer;
mod layout;
use buffer::{TextBuffer, byte_at_utf16};
use layout::{CARET_SPACE, InputLayout};

use super::Ui;
use crate::theme;
use gpui::{prelude::*, *};
use std::{
    ops::Range,
    time::{Duration, Instant},
};
use unicode_segmentation::UnicodeSegmentation;

pub type Input = TextInput<false>;
pub type Textarea = TextInput<true>;

struct SelectionDrag {
    pointer: Point<Pixels>,
    last_frame: Option<Instant>,
}

pub struct TextInput<const MULTILINE: bool> {
    buffer: TextBuffer<MULTILINE>,
    focus: FocusHandle,
    blur_subscription: Option<(WindowId, Subscription)>,
    placeholder: String,
    inline: bool,
    layout: Option<InputLayout>,
    bounds: Option<Bounds<Pixels>>,
    scroll: Point<Pixels>,
    reveal_cursor: bool,
    drag: Option<SelectionDrag>,
    row_hint: Option<usize>,
    preferred_x: Option<Pixels>,
}

impl<const MULTILINE: bool> Focusable for TextInput<MULTILINE> {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextInput<false> {
    pub fn inline(text: &str, cx: &mut App) -> Entity<Self> {
        let mut buffer = TextBuffer::new(text);
        buffer.anchor = 0;
        Self::with_buffer(buffer, "", true, cx)
    }

    pub fn is_composing(&self) -> bool {
        self.buffer.marked.is_some()
    }
}

impl<const MULTILINE: bool> TextInput<MULTILINE> {
    pub fn new(text: &str, placeholder: &str, cx: &mut App) -> Entity<Self> {
        Self::with_buffer(TextBuffer::new(text), placeholder, false, cx)
    }

    pub fn read_only(text: &str, cx: &mut App) -> Entity<Self> {
        Self::with_buffer(TextBuffer::read_only(text), "", false, cx)
    }

    fn with_buffer(
        buffer: TextBuffer<MULTILINE>,
        placeholder: &str,
        inline: bool,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| Self {
            placeholder: placeholder.into(),
            inline,
            focus: cx.focus_handle().tab_stop(true),
            blur_subscription: None,
            buffer,
            layout: None,
            bounds: None,
            scroll: Point::default(),
            reveal_cursor: true,
            drag: None,
            row_hint: None,
            preferred_x: None,
        })
    }

    pub fn text(&self) -> &str {
        &self.buffer.text
    }

    fn is_focused(&self, window: &Window) -> bool {
        window.is_window_active() && self.focus.is_focused(window)
    }

    fn observe_blur(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = window.window_handle().window_id();
        if self
            .blur_subscription
            .as_ref()
            .is_some_and(|(old, _)| *old == id)
        {
            return;
        }
        let subscription = cx.on_blur(&self.focus, window, |field, _, cx| {
            field.drag = None;
            // The visible IME preedit is already in the buffer.
            if field.buffer.marked.take().is_some() {
                cx.notify();
            }
        });
        self.blur_subscription = Some((id, subscription));
    }

    /// Keep focus, selection and scrolling when a refreshed model still contains the same value.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if self.text() == text {
            return false;
        }
        self.buffer = if self.buffer.read_only {
            TextBuffer::read_only(text)
        } else {
            TextBuffer::new(text)
        };
        self.layout = None;
        self.drag = None;
        self.scroll = Point::default();
        self.row_hint = None;
        self.preferred_x = None;
        self.reveal_cursor = true;
        cx.notify();
        true
    }

    fn replace(&mut self, range: Option<Range<usize>>, value: &str) -> Range<usize> {
        let added = self.buffer.replace(range, value);
        self.drag = None;
        self.layout = None;
        self.row_hint = None;
        self.preferred_x = None;
        self.reveal_cursor = true;
        added
    }

    fn key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_focused(window) || self.buffer.marked.is_some() {
            return;
        }
        let key = &ev.keystroke;
        let shortcut = if cfg!(target_os = "macos") {
            key.modifiers.platform
        } else {
            key.modifiers.control
        };
        let sel = self.buffer.selection();
        let vertical = MULTILINE && matches!(key.key.as_str(), "up" | "down");
        let row_key = MULTILINE && matches!(key.key.as_str(), "up" | "down" | "home" | "end");
        if row_key
            && self.layout.is_none()
            && let Some(bounds) = self.bounds
        {
            self.layout = self.shape(bounds, window, cx);
        }
        if !vertical {
            self.preferred_x = None;
        }
        match key.key.as_str() {
            "backspace" | "delete" | "enter" if self.buffer.read_only => {}
            "x" | "v" if shortcut && self.buffer.read_only => {}
            "enter" if MULTILINE => {
                self.replace(None, "\n");
            }
            "up" | "down" | "home" | "end" if row_key => {
                if let Some(layout) = &self.layout {
                    let row = layout.row_at(self.buffer.cursor, self.row_hint);
                    let (byte, target) = match key.key.as_str() {
                        "home" if shortcut => (0, 0),
                        "end" if shortcut => (self.buffer.text.len(), layout.row_count() - 1),
                        "home" => (layout.range(row).start, row),
                        "end" => (layout.range(row).end, row),
                        _ => {
                            let x = *self
                                .preferred_x
                                .get_or_insert_with(|| layout.x(row, self.buffer.cursor));
                            let target = if key.key == "up" {
                                row.saturating_sub(1)
                            } else {
                                (row + 1).min(layout.row_count() - 1)
                            };
                            (layout.byte_at(target, x), target)
                        }
                    };
                    self.buffer.cursor = byte.min(self.buffer.text.len());
                    self.row_hint = Some(target);
                    if !key.modifiers.shift {
                        self.buffer.anchor = self.buffer.cursor;
                    }
                }
            }
            "a" if shortcut => {
                self.buffer.anchor = 0;
                self.buffer.cursor = self.buffer.text.len();
            }
            "c" | "x" if shortcut => {
                if !sel.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        self.buffer.text[sel].to_owned(),
                    ));
                    if key.key == "x" {
                        self.replace(None, "");
                    }
                }
            }
            "v" if shortcut => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.replace(None, &t);
                }
            }
            "left" | "right" | "home" | "end" | "backspace" | "delete" => {
                let prev = self.buffer.text[..self.buffer.cursor]
                    .grapheme_indices(true)
                    .next_back()
                    .map_or(0, |(i, _)| i);
                let next = self.buffer.cursor
                    + self.buffer.text[self.buffer.cursor..]
                        .graphemes(true)
                        .next()
                        .map_or(0, str::len);
                match key.key.as_str() {
                    "backspace" | "delete" => {
                        if sel.is_empty() {
                            self.buffer.anchor = if key.key == "backspace" { prev } else { next };
                        }
                        self.replace(None, "");
                    }
                    k => {
                        self.buffer.cursor = match k {
                            "home" => 0,
                            "end" => self.buffer.text.len(),
                            "left" if !key.modifiers.shift && !sel.is_empty() => sel.start,
                            "right" if !key.modifiers.shift && !sel.is_empty() => sel.end,
                            "left" => prev,
                            _ => next,
                        };
                        if !key.modifiers.shift {
                            self.buffer.anchor = self.buffer.cursor;
                        }
                    }
                }
            }
            _ => return,
        }
        if !row_key {
            self.row_hint = None;
        }
        self.drag = None;
        self.reveal_cursor = true;
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    fn hit(&self, pt: Point<Pixels>) -> (usize, Option<usize>) {
        self.layout
            .as_ref()
            .map_or((self.buffer.cursor, self.row_hint), |layout| {
                let (byte, row) = layout.hit(pt);
                (byte.min(self.buffer.text.len()), Some(row))
            })
    }

    fn point_cursor(&mut self, pt: Point<Pixels>) {
        (self.buffer.cursor, self.row_hint) = self.hit(pt);
        self.reveal_cursor = true;
        self.preferred_x = None;
    }

    fn move_selection(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.pressed_button != Some(MouseButton::Left) || !self.is_focused(window) {
            self.drag = None;
            return;
        }
        let Some(drag) = &mut self.drag else { return };
        if drag.pointer != event.position {
            drag.pointer = event.position;
            drag.last_frame.get_or_insert_with(Instant::now);
            self.reveal_cursor = false;
            cx.notify();
        }
    }

    fn end_selection(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left {
            return;
        }
        if let Some(drag) = self.drag.take()
            && (drag.last_frame.is_some() || drag.pointer != event.position)
            && self.is_focused(window)
            && let Some(layout) = &mut self.layout
        {
            let (byte, row, _) = layout.drag_hit::<MULTILINE>(event.position, Duration::ZERO);
            self.buffer.cursor = byte.min(self.buffer.text.len());
            self.row_hint = Some(row);
            cx.notify();
        }
    }

    fn shape(&self, bounds: Bounds<Pixels>, window: &Window, cx: &App) -> Option<InputLayout> {
        let colors = theme::palette(window, cx);
        let empty = self.buffer.text.is_empty();
        let text: SharedString = (if empty {
            &self.placeholder
        } else {
            &self.buffer.text
        })
        .clone()
        .into();
        let run = TextRun {
            len: text.len(),
            font: window.text_style().font(),
            color: rgb(if empty {
                colors.muted_foreground
            } else {
                colors.canvas.foreground
            })
            .into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let width = MULTILINE.then(|| (bounds.size.width - px(2.)).max(px(1.)));
        let lines = window
            .text_system()
            .shape_text(text, px(14.), &[run], width, None)
            .ok()?;
        let height = if MULTILINE {
            px(20.)
        } else {
            bounds.size.height.max(px(1.))
        };
        Some(InputLayout::new(lines.into_vec(), bounds, height))
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let Some(mut layout) = self.shape(bounds, window, cx) else {
            return;
        };
        let colors = theme::palette(window, cx);
        let focused = self.is_focused(window);
        layout.scroll = self.scroll;
        if self.reveal_cursor || self.bounds.is_some_and(|old| old.size != bounds.size) {
            let row = layout.row_at(self.buffer.cursor, self.row_hint);
            if MULTILINE {
                let y = layout.height * row;
                layout.scroll.y = layout
                    .scroll
                    .y
                    .min(y)
                    .max(y + layout.height - bounds.size.height);
            } else {
                let x = layout.x(row, self.buffer.cursor);
                layout.scroll.x = layout
                    .scroll
                    .x
                    .min(x)
                    .max(x + CARET_SPACE - bounds.size.width);
            }
        }
        if MULTILINE {
            layout.scroll.y = layout.scroll.y.clamp(px(0.), layout.max_scroll_y());
        } else {
            layout.scroll.x = layout.scroll.x.clamp(px(0.), layout.max_scroll_x());
        }
        if focused
            && let Some(drag) = &mut self.drag
            && let Some(last) = &mut drag.last_frame
        {
            let now = Instant::now();
            let (byte, row, moving) = layout.drag_hit::<MULTILINE>(drag.pointer, now - *last);
            *last = now;
            self.buffer.cursor = byte.min(self.buffer.text.len());
            self.row_hint = Some(row);
            if moving {
                window.request_animation_frame();
            }
        }
        self.scroll = layout.scroll;
        self.reveal_cursor = false;
        self.bounds = Some(bounds);
        let selection = self.buffer.selection();
        if focused {
            layout.paint_range(selection.clone(), false, colors.selection, window);
        }
        layout.paint_text(window, cx);
        if focused && selection.is_empty() {
            let row = layout.row_at(self.buffer.cursor, self.row_hint);
            let x = layout.x(row, self.buffer.cursor);
            window.paint_quad(fill(
                Bounds::new(
                    layout.origin(row) + point(x, px(2.)),
                    size(px(1.5), layout.height - px(4.)),
                ),
                rgb(colors.ring),
            ));
        }
        if focused && let Some(r) = &self.buffer.marked {
            layout.paint_range(r.clone(), true, colors.ring, window);
        }
        self.layout = Some(layout);
    }
}

impl<const MULTILINE: bool> Render for TextInput<MULTILINE> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.observe_blur(window, cx);
        let field = cx.entity();
        let editable = !self.buffer.read_only;
        let focus = self.focus.clone();
        let move_selection = cx.listener(Self::move_selection);
        let end_selection = cx.listener(Self::end_selection);
        Ui::new(window, cx)
            .input_frame("input", &focus)
            .when(self.inline, |field| {
                field.h(px(24.)).px(theme::SPACE_1).py_0()
            })
            .when(MULTILINE, |field| {
                field
                    .h(px(90.))
                    .on_scroll_wheel(cx.listener(|v, e: &ScrollWheelEvent, _, cx| {
                        let Some(layout) = &v.layout else { return };
                        let next = (v.scroll.y - e.delta.pixel_delta(layout.height).y)
                            .clamp(px(0.), layout.max_scroll_y());
                        if next != v.scroll.y {
                            v.scroll.y = next;
                            v.reveal_cursor = false;
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
            })
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, e: &MouseDownEvent, _, cx| {
                    v.point_cursor(e.position);
                    if !e.modifiers.shift {
                        v.buffer.anchor = v.buffer.cursor;
                    }
                    if e.click_count >= 2 {
                        v.buffer.anchor = 0;
                        v.buffer.cursor = v.buffer.text.len();
                    }
                    v.drag = (!v.buffer.text.is_empty()).then_some(SelectionDrag {
                        pointer: e.position,
                        last_frame: None,
                    });
                    cx.notify();
                }),
            )
            .child(
                div().size_full().overflow_hidden().child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, w, cx| {
                            if editable && field.read(cx).is_focused(w) {
                                w.handle_input(
                                    &focus,
                                    ElementInputHandler::new(bounds, field.clone()),
                                    cx,
                                );
                            }
                            field.update(cx, |v, cx| v.paint(bounds, w, cx));
                            // Keep the drag that began in this input when the pointer leaves its bounds.
                            w.on_mouse_event(move |event, phase, w, cx| {
                                if phase == DispatchPhase::Capture {
                                    move_selection(event, w, cx);
                                }
                            });
                            w.on_mouse_event(move |event, phase, w, cx| {
                                if phase == DispatchPhase::Capture {
                                    end_selection(event, w, cx);
                                }
                            });
                        },
                    )
                    .size_full(),
                ),
            )
    }
}

impl<const MULTILINE: bool> EntityInputHandler for TextInput<MULTILINE> {
    fn text_for_range(
        &mut self,
        r: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let bytes =
            byte_at_utf16(&self.buffer.text, r.start)..byte_at_utf16(&self.buffer.text, r.end);
        *actual = Some(self.buffer.utf16(bytes.start)..self.buffer.utf16(bytes.end));
        Some(self.buffer.text[bytes].into())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let r = self.buffer.selection();
        Some(UTF16Selection {
            range: self.buffer.utf16(r.start)..self.buffer.utf16(r.end),
            reversed: self.buffer.cursor < self.buffer.anchor,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.buffer
            .marked
            .as_ref()
            .map(|r| self.buffer.utf16(r.start)..self.buffer.utf16(r.end))
    }
    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_focused(window) && self.buffer.marked.take().is_some() {
            cx.notify();
        }
    }
    fn replace_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_focused(window) {
            self.replace(r, text);
            cx.notify();
        }
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_focused(window) {
            let added = self.replace(r, text);
            self.buffer.mark(added, text, selection);
            cx.notify();
        }
    }
    fn bounds_for_range(
        &mut self,
        r: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let start = byte_at_utf16(&self.buffer.text, r.start);
        let end = byte_at_utf16(&self.buffer.text, r.end);
        let row = layout.row_at(
            start,
            (start == self.buffer.cursor)
                .then_some(self.row_hint)
                .flatten(),
        );
        let x = layout.x(row, start);
        Some(Bounds::new(
            layout.origin(row) + point(x, px(0.)),
            size((layout.x(row, end) - x).max(px(1.)), layout.height),
        ))
    }
    fn character_index_for_point(
        &mut self,
        pt: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.buffer.utf16(self.hit(pt).0))
    }
}
