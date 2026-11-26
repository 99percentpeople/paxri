use super::*;

pub(super) struct TabRename {
    title: Entity<terminal::TabTitle>,
    pub(super) input: Entity<Input>,
    initial: SharedString,
    _blur: Subscription,
}

impl TabRename {
    pub(super) fn matches(&self, page: &Page) -> bool {
        matches!(page, Page::Terminal { title, .. } if *title == self.title)
    }
}

pub(super) enum EndRename {
    Confirm,
    Cancel,
    Blur,
}

impl Workspace {
    pub(super) fn begin_rename(&mut self, page: &Page, w: &mut Window, cx: &mut Context<Self>) {
        let Page::Terminal { title, .. } = page else {
            return;
        };
        if self.active != *page {
            return;
        }
        self.end_rename(EndRename::Blur, cx);
        let initial = page.label(cx);
        let input = Input::inline(&initial, cx);
        let focus = input.focus_handle(cx);
        let id = input.entity_id();
        let blur = cx.on_blur(&focus, w, move |v, _, cx| {
            if v.rename
                .as_ref()
                .is_some_and(|edit| edit.input.entity_id() == id)
            {
                v.end_rename(EndRename::Blur, cx);
            }
        });
        self.rename = Some(TabRename {
            title: title.clone(),
            initial,
            input,
            _blur: blur,
        });
        focus.focus(w, cx);
        cx.notify();
    }

    pub(super) fn end_rename(&mut self, end: EndRename, cx: &mut Context<Self>) {
        let Some(edit) = self.rename.take() else {
            return;
        };
        let text = edit.input.read(cx).text();
        if matches!(end, EndRename::Confirm)
            || (matches!(end, EndRename::Blur) && text != edit.initial.as_ref())
        {
            let text = text.to_owned();
            edit.title.update(cx, |title, cx| {
                if title.rename(&text) {
                    cx.notify();
                }
            });
        }
        cx.notify();
    }

    pub(super) fn rename_key(
        &mut self,
        event: &KeyDownEvent,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let end = match event.keystroke.key.as_str() {
            "enter" => EndRename::Confirm,
            "escape" => EndRename::Cancel,
            _ => return,
        };
        let Some(edit) = &self.rename else { return };
        if edit.input.read(cx).is_composing() {
            return;
        }
        self.end_rename(end, cx);
        self.focus_content(w, cx);
        w.prevent_default();
        cx.stop_propagation();
    }
}
