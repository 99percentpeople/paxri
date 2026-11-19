//! Single-line native text field shared by the rule and device-name forms.
use gpui::{prelude::*, *};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub struct Field {
    pub text: String,
    pub focus: FocusHandle,
    placeholder: String,
    anchor: usize,
    cursor: usize,
    marked: Option<Range<usize>>,
    layout: Option<(ShapedLine, Point<Pixels>)>,
    scroll: Pixels,
}

fn byte_at_utf16(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (byte, c) in text.char_indices() {
        if units + c.len_utf16() > offset {
            return byte;
        }
        units += c.len_utf16();
    }
    text.len()
}

impl Field {
    pub fn new(text: &str, placeholder: &str, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self {
            text: text.into(),
            placeholder: placeholder.into(),
            focus: cx.focus_handle().tab_stop(true),
            anchor: text.len(),
            cursor: text.len(),
            marked: None,
            layout: None,
            scroll: px(0.),
        })
    }

    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    fn utf16(&self, byte: usize) -> usize {
        self.text[..byte].encode_utf16().count()
    }

    fn replace(&mut self, range: Option<Range<usize>>, value: &str) -> Range<usize> {
        let range = range
            .map(|r| byte_at_utf16(&self.text, r.start)..byte_at_utf16(&self.text, r.end))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection());
        let value = value.replace(['\r', '\n'], "");
        self.text.replace_range(range.clone(), &value);
        self.cursor = range.start + value.len();
        self.anchor = self.cursor;
        self.marked = None;
        range.start..self.cursor
    }

    fn key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return;
        }
        let key = &ev.keystroke;
        let shortcut = if cfg!(target_os = "macos") {
            key.modifiers.platform
        } else {
            key.modifiers.control
        };
        let sel = self.selection();
        match key.key.as_str() {
            "a" if shortcut => {
                self.anchor = 0;
                self.cursor = self.text.len();
            }
            "c" | "x" if shortcut => {
                if !sel.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(self.text[sel].to_owned()));
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
                let prev = self.text[..self.cursor]
                    .grapheme_indices(true)
                    .next_back()
                    .map_or(0, |(i, _)| i);
                let next = self.cursor
                    + self.text[self.cursor..]
                        .graphemes(true)
                        .next()
                        .map_or(0, str::len);
                match key.key.as_str() {
                    "backspace" | "delete" => {
                        if sel.is_empty() {
                            self.anchor = if key.key == "backspace" { prev } else { next };
                        }
                        self.replace(None, "");
                    }
                    k => {
                        self.cursor = match k {
                            "home" => 0,
                            "end" => self.text.len(),
                            "left" if !key.modifiers.shift && !sel.is_empty() => sel.start,
                            "right" if !key.modifiers.shift && !sel.is_empty() => sel.end,
                            "left" => prev,
                            _ => next,
                        };
                        if !key.modifiers.shift {
                            self.anchor = self.cursor;
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
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    fn position(&self, x: Pixels) -> usize {
        self.layout.as_ref().map_or(0, |(line, origin)| {
            line.closest_index_for_x(x - origin.x).min(self.text.len())
        })
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let empty = self.text.is_empty();
        let text: SharedString = if empty { &self.placeholder } else { &self.text }
            .clone()
            .into();
        let run = TextRun {
            len: text.len(),
            font: window.text_style().font(),
            color: rgb(if empty { 0x75818b } else { 0x162633 }).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(text, px(14.), &[run], None);
        let cursor = line.x_for_index(self.cursor);
        self.scroll = self
            .scroll
            .min(cursor)
            .max(cursor - bounds.size.width + px(2.))
            .max(px(0.));
        let origin = point(bounds.left() - self.scroll, bounds.top());
        let focused = self.focus.is_focused(window);
        let sel = self.selection();
        if focused && !sel.is_empty() {
            let start = line.x_for_index(sel.start);
            window.paint_quad(fill(
                Bounds::new(
                    origin + point(start, px(0.)),
                    size(line.x_for_index(sel.end) - start, bounds.size.height),
                ),
                rgb(0xbde7dd),
            ));
        }
        let _ = line.paint(origin, bounds.size.height, window, cx);
        if focused && sel.is_empty() {
            window.paint_quad(fill(
                Bounds::new(
                    origin + point(cursor, px(2.)),
                    size(px(1.5), bounds.size.height - px(4.)),
                ),
                rgb(0x107c69),
            ));
        }
        if let Some(r) = &self.marked {
            let a = line.x_for_index(r.start);
            window.paint_quad(fill(
                Bounds::new(
                    point(origin.x + a, bounds.bottom() - px(1.)),
                    size(line.x_for_index(r.end) - a, px(1.)),
                ),
                rgb(0x107c69),
            ));
        }
        self.layout = Some((line, origin));
    }
}

impl Render for Field {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let field = cx.entity();
        let focus = self.focus.clone();
        div()
            .id("input")
            .track_focus(&focus)
            .w_full()
            .h(px(36.))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(rgb(0xc9d3d8))
            .bg(rgb(0xffffff))
            .cursor(CursorStyle::IBeam)
            .focus(|s| s.border_color(rgb(0x16826c)))
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, e: &MouseDownEvent, w, cx| {
                    v.focus.focus(w);
                    v.cursor = v.position(e.position.x);
                    if !e.modifiers.shift {
                        v.anchor = v.cursor;
                    }
                    if e.click_count >= 2 {
                        v.anchor = 0;
                        v.cursor = v.text.len();
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|v, e: &MouseMoveEvent, w, cx| {
                if e.pressed_button == Some(MouseButton::Left) && v.focus.is_focused(w) {
                    v.cursor = v.position(e.position.x);
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

impl EntityInputHandler for Field {
    fn text_for_range(
        &mut self,
        r: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let bytes = byte_at_utf16(&self.text, r.start)..byte_at_utf16(&self.text, r.end);
        *actual = Some(self.utf16(bytes.start)..self.utf16(bytes.end));
        Some(self.text[bytes].into())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let r = self.selection();
        Some(UTF16Selection {
            range: self.utf16(r.start)..self.utf16(r.end),
            reversed: self.cursor < self.anchor,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|r| self.utf16(r.start)..self.utf16(r.end))
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
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
        if !added.is_empty() {
            if let Some(sel) = selection {
                let marked = &self.text[added.clone()];
                self.anchor = added.start + byte_at_utf16(marked, sel.start);
                self.cursor = added.start + byte_at_utf16(marked, sel.end);
            }
            self.marked = Some(added);
        }
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        r: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (line, origin) = self.layout.as_ref()?;
        let x = line.x_for_index(byte_at_utf16(&self.text, r.start));
        let end = line.x_for_index(byte_at_utf16(&self.text, r.end));
        Some(Bounds::new(
            *origin + point(x, px(0.)),
            size((end - x).max(px(1.)), bounds.size.height),
        ))
    }
    fn character_index_for_point(
        &mut self,
        pt: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.utf16(self.position(pt.x)))
    }
}
