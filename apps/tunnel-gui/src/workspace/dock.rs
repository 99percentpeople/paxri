//! Transfer pages between workspace windows with a common gateway.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct DockTarget {
    window: WindowHandle<Workspace>,
    index: usize,
    cursor: Point<Pixels>,
}

impl Workspace {
    pub(super) fn dock_target_at(
        &self,
        source: &Window,
        cx: &mut App,
        local_position: impl Fn(&Workspace, &Window) -> Option<Point<Pixels>>,
    ) -> Option<DockTarget> {
        let source = source.window_handle();
        cx.windows()
            .into_iter()
            .filter(|h| *h != source)
            .filter_map(|h| h.downcast::<Self>())
            .find_map(|window| {
                window
                    .update(cx, |v, w, cx| {
                        if v.dashboard != self.dashboard || v.tabs.contains(&self.active) {
                            return None;
                        }
                        let cursor = local_position(v, w)?;
                        let index = v.dock.read(cx).insertion(cursor)?;
                        Some(DockTarget {
                            window,
                            index,
                            cursor,
                        })
                    })
                    .ok()
                    .flatten()
            })
    }

    pub(super) fn dock_at_pointer(
        &mut self,
        target: DockTarget,
        held_grab: Option<Point<Pixels>>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let page = self.active.clone();
        let accepted = target
            .window
            .update(cx, |v, w, cx| {
                if v.dashboard != self.dashboard || v.tabs.contains(&page) {
                    return false;
                }
                v.tabs.insert(target.index.min(v.tabs.len()), page.clone());
                v.dragged_tab = held_grab.map(|offset| TabDrag {
                    page: page.clone(),
                    previous_active: page.clone(),
                    offset,
                    cursor: target.cursor,
                });
                v.refresh_title(w);
                v.focus_tab(page.clone(), w, cx);
                true
            })
            .unwrap_or(false);
        if accepted {
            self.take_tab(&page, w, cx);
            if held_grab.is_some() {
                cx.defer(move |cx| {
                    let _ = target.window.update(cx, |v, w, cx| {
                        if v.dragged_tab.as_ref().is_some_and(|tab| tab.page == page) {
                            v.continue_tab_drag(page, w, cx);
                        }
                    });
                });
            }
        }
        accepted
    }
}
