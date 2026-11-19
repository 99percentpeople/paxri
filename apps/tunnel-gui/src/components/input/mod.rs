//! Native text inputs share the clipboard, selection, IME and cursor implementation.
mod buffer;
mod layout;
use buffer::{TextBuffer, byte_at_utf16};
use layout::InputLayout;

use super::Ui;
use crate::theme;
use gpui::{prelude::*, *};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub type Input = TextInput<false>;
pub type Textarea = TextInput<true>;

pub struct TextInput<const MULTILINE: bool> {
    buffer: TextBuffer<MULTILINE>,
    pub focus: FocusHandle,
    placeholder: String,
    layout: Option<InputLayout>,
    bounds: Option<Bounds<Pixels>>,
    scroll: Point<Pixels>,
    reveal_cursor: bool,
    row_hint: Option<usize>,
    preferred_x: Option<Pixels>,
}

impl<const MULTILINE: bool> TextInput<MULTILINE> {
    pub fn new(text: &str, placeholder: &str, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self {
            placeholder: placeholder.into(),
            focus: cx.focus_handle().tab_stop(true),
            buffer: TextBuffer::new(text),
            layout: None,
            bounds: None,
            scroll: Point::default(),
            reveal_cursor: true,
            row_hint: None,
            preferred_x: None,
        })
    }

    pub fn text(&self) -> &str {
        &self.buffer.text
    }

    fn replace(&mut self, range: Option<Range<usize>>, value: &str) -> Range<usize> {
        let added = self.buffer.replace(range, value);
        self.layout = None;
        self.row_hint = None;
        self.preferred_x = None;
        self.reveal_cursor = true;
        added
    }

    fn key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.marked.is_some() {
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
            "tab" => {
                if key.modifiers.shift {
                    window.focus_prev()
                } else {
                    window.focus_next()
                }
            }
            _ => return,
        }
        if !row_key {
            self.row_hint = None;
        }
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
        let row = layout.row_at(self.buffer.cursor, self.row_hint);
        let x = layout.x(row, self.buffer.cursor);
        let y = layout.height * row;
        if MULTILINE {
            if self.reveal_cursor || self.bounds.is_some_and(|old| old.size != bounds.size) {
                self.scroll.y = self
                    .scroll
                    .y
                    .min(y)
                    .max(y + layout.height - bounds.size.height);
            }
            self.scroll.y = self.scroll.y.clamp(px(0.), layout.max_scroll_y());
        } else {
            self.scroll.x = self
                .scroll
                .x
                .min(x)
                .max(x - bounds.size.width + px(2.))
                .max(px(0.));
        }
        layout.scroll = self.scroll;
        self.reveal_cursor = false;
        self.bounds = Some(bounds);
        let focused = self.focus.is_focused(window);
        let selection = self.buffer.selection();
        if focused {
            layout.paint_range(selection.clone(), false, colors.selection, window);
        }
        layout.paint_text(window, cx);
        if focused && selection.is_empty() {
            window.paint_quad(fill(
                Bounds::new(
                    layout.origin(row) + point(x, px(2.)),
                    size(px(1.5), layout.height - px(4.)),
                ),
                rgb(colors.ring),
            ));
        }
        if let Some(r) = &self.buffer.marked {
            layout.paint_range(r.clone(), true, colors.ring, window);
        }
        self.layout = Some(layout);
    }
}

impl<const MULTILINE: bool> Render for TextInput<MULTILINE> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let field = cx.entity();
        let focus = self.focus.clone();
        Ui::new(window, cx)
            .input_frame("input", &focus)
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
                cx.listener(|v, e: &MouseDownEvent, w, cx| {
                    v.focus.focus(w);
                    v.point_cursor(e.position);
                    if !e.modifiers.shift {
                        v.buffer.anchor = v.buffer.cursor;
                    }
                    if e.click_count >= 2 {
                        v.buffer.anchor = 0;
                        v.buffer.cursor = v.buffer.text.len();
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|v, e: &MouseMoveEvent, w, cx| {
                if e.pressed_button == Some(MouseButton::Left) && v.focus.is_focused(w) {
                    v.point_cursor(e.position);
                    cx.notify();
                }
            }))
            .child(
                div().size_full().overflow_hidden().child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, w, cx| {
                            w.handle_input(
                                &focus,
                                ElementInputHandler::new(bounds, field.clone()),
                                cx,
                            );
                            field.update(cx, |v, cx| v.paint(bounds, w, cx));
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
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.marked = None;
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace(r, text);
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let added = self.replace(r, text);
        self.buffer.mark(added, text, selection);
        cx.notify();
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
