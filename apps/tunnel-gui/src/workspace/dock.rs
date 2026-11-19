//! Hit-test measured tab strips and transfer a page between workspace windows.
use super::*;

#[derive(Default)]
pub(super) struct TabLayout {
    pub(super) zone: Bounds<Pixels>,
    pub(super) tabs: Vec<(Page, Bounds<Pixels>)>,
}

impl TabLayout {
    pub(super) fn bounds(&self, page: &Page) -> Option<Bounds<Pixels>> {
        self.tabs.iter().find(|(p, _)| p == page).map(|(_, b)| *b)
    }

    pub(super) fn insertion(&self, cursor: Point<Pixels>, axis: Axis) -> Option<usize> {
        if self.tabs.is_empty()
            || cursor.x < self.zone.left()
            || cursor.x >= self.zone.right()
            || cursor.y < self.zone.top()
            || cursor.y >= self.zone.bottom()
        {
            return None;
        }
        Some(
            self.tabs
                .iter()
                .position(|(_, b)| cursor.along(axis) < b.center().along(axis))
                .unwrap_or(self.tabs.len()),
        )
    }
}

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
                    .update(cx, |v, w, _| {
                        if v.dashboard != self.dashboard || v.tabs.contains(&self.active) {
                            return None;
                        }
                        let cursor = local_position(v, w)?;
                        let index = v.tab_layout.insertion(cursor, v.tab_axis())?;
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

#[cfg(test)]
mod tests {
    use super::{Page, TabLayout};
    use gpui::{Axis, Bounds, point, px, size};

    #[test]
    fn positions_before_after_and_in_the_trailing_space() {
        let rect =
            |x, y, width, height| Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
        let mut layout = TabLayout {
            zone: rect(0., 0., 370., 40.),
            tabs: vec![(Page::Home, rect(4., 4., 96., 31.))],
        };
        let at = |x, y| point(px(x), px(y));
        assert_eq!(layout.insertion(at(10., 20.), Axis::Horizontal), Some(0));
        assert_eq!(layout.insertion(at(90., 20.), Axis::Horizontal), Some(1));
        assert_eq!(layout.insertion(at(340., 20.), Axis::Horizontal), Some(1));
        assert_eq!(layout.insertion(at(370., 20.), Axis::Horizontal), None);
        assert_eq!(layout.insertion(at(10., 45.), Axis::Horizontal), None);
        layout.zone = rect(0., 40., 142., 400.);
        layout.tabs = vec![(Page::Home, rect(4., 44., 134., 31.))];
        assert_eq!(layout.insertion(at(30., 50.), Axis::Vertical), Some(0));
        assert_eq!(layout.insertion(at(30., 400.), Axis::Vertical), Some(1));
        assert_eq!(layout.insertion(at(150., 120.), Axis::Vertical), None);
    }
}
