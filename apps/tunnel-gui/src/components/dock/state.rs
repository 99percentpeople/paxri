//! Measured geometry shared by the tab strip, drag preview and docking hit tests.
use super::{DockSettings, sidebar::SidebarResize};
use gpui::{prelude::*, *};

const DETACH_DISTANCE: Pixels = px(24.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockPlacement {
    Top,
    Left,
}

impl DockPlacement {
    pub fn other(self) -> Self {
        match self {
            Self::Top => Self::Left,
            Self::Left => Self::Top,
        }
    }

    fn axis(self) -> Axis {
        match self {
            Self::Top => Axis::Horizontal,
            Self::Left => Axis::Vertical,
        }
    }
}

pub struct DockState<K> {
    pub(super) settings: DockSettings,
    pub(super) resizing: Option<SidebarResize>,
    pub(super) zone: Bounds<Pixels>,
    pub(super) tabs: Vec<(K, Bounds<Pixels>)>,
}

impl<K: PartialEq + 'static> DockState<K> {
    pub fn new<V: 'static>(
        settings: DockSettings,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Entity<Self> {
        let state = cx.new(|cx: &mut Context<Self>| {
            cx.observe_window_activation(window, |dock, w, cx| {
                if !w.is_window_active() && dock.resizing.take().is_some() {
                    cx.notify();
                }
            })
            .detach();
            Self {
                settings,
                resizing: None,
                zone: Bounds::default(),
                tabs: Vec::new(),
            }
        });
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        state
    }

    pub fn settings(&self) -> DockSettings {
        self.settings
    }

    pub fn placement(&self) -> DockPlacement {
        self.settings.placement
    }

    pub fn set_placement(&mut self, placement: DockPlacement, cx: &mut Context<Self>) {
        if self.settings.placement != placement {
            self.settings = self.settings.with_placement(placement);
            self.resizing = None;
            self.zone = Bounds::default();
            self.tabs.clear();
            cx.notify();
        }
    }

    pub fn forget(&mut self, key: &K) {
        self.tabs.retain(|(id, _)| id != key);
    }

    pub fn indent(&self, key: &K) -> Pixels {
        let Some((_, first)) = self.tabs.first() else {
            return px(0.);
        };
        self.bounds(key).map_or(px(0.), |slot| {
            (slot.origin - first.origin)
                .along(self.settings.placement.axis())
                .max(px(0.))
        })
    }

    pub fn bounds(&self, page: &K) -> Option<Bounds<Pixels>> {
        self.tabs.iter().find(|(p, _)| p == page).map(|(_, b)| *b)
    }

    pub fn insertion(&self, cursor: Point<Pixels>) -> Option<usize> {
        let axis = self.settings.placement.axis();
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

    pub fn leaves_strip(&self, cursor: Point<Pixels>, viewport: Size<Pixels>) -> bool {
        // Crossing the titlebar or the empty part of a sidebar along its length
        // keeps the drag in the same window.
        let area = if self.settings.placement == DockPlacement::Left {
            Bounds::new(
                point(self.zone.left(), px(0.)),
                size(self.zone.size.width, viewport.height),
            )
        } else {
            Bounds::new(
                point(px(0.), self.zone.top()),
                size(viewport.width, self.zone.size.height),
            )
        };
        !area.dilate(DETACH_DISTANCE).contains(&cursor)
    }
}

#[derive(Clone)]
pub struct DockDrag<K> {
    pub page: K,
    pub previous_active: K,
    pub offset: Point<Pixels>,
    pub cursor: Point<Pixels>,
}

impl<K: Clone + PartialEq + 'static> DockDrag<K> {
    pub fn projected(&self, mut bounds: Bounds<Pixels>, layout: &DockState<K>) -> Bounds<Pixels> {
        let axis = layout.settings.placement.axis();
        if let Some((_, first)) = layout.tabs.first() {
            let desired = (self.cursor - self.grab_in(bounds)).along(axis);
            let start = first.origin.along(axis).max(layout.zone.origin.along(axis));
            let end = layout.zone.bottom_right().along(axis) - bounds.size.along(axis);
            bounds.origin = bounds
                .origin
                .apply_along(axis, |_| desired.clamp(start, end.max(start)));
        }
        bounds
    }

    pub fn grab_in(&self, bounds: Bounds<Pixels>) -> Point<Pixels> {
        point(
            self.offset.x.clamp(px(0.), bounds.size.width),
            self.offset.y.clamp(px(0.), bounds.size.height),
        )
    }

    pub fn reorder<T>(&self, tabs: &mut Vec<T>, layout: &DockState<K>, key: impl Fn(&T) -> &K) {
        let Some(from) = tabs.iter().position(|tab| key(tab) == &self.page) else {
            return;
        };
        let Some(to) = self.destination(layout) else {
            return;
        };
        let to = to.min(tabs.len() - 1);
        if from != to {
            let tab = tabs.remove(from);
            tabs.insert(to, tab);
        }
    }

    fn destination(&self, layout: &DockState<K>) -> Option<usize> {
        // A move or release may arrive before the receiver measures its incoming tab.
        let Some(slot) = layout.bounds(&self.page) else {
            return layout.insertion(self.cursor);
        };
        let axis = layout.settings.placement.axis();
        let projected = self.projected(slot, layout);
        // Cross a neighbour's midpoint with the leading edge of the moving tab,
        // including when the two tabs have different widths.
        let index = layout
            .tabs
            .iter()
            .filter(|(page, bounds)| {
                if *page == self.page {
                    return false;
                }
                let edge = if bounds.origin.along(axis) < slot.origin.along(axis) {
                    projected.origin
                } else {
                    projected.bottom_right()
                };
                // The pointer can enter trailing space while the grabbed tab
                // still overlaps the last neighbour.
                self.cursor.along(axis) >= bounds.bottom_right().along(axis)
                    || edge.along(axis) > bounds.center().along(axis)
            })
            .count();
        Some(index)
    }
}

#[cfg(test)]
mod tests {
    use super::{DockDrag, DockPlacement, DockSettings, DockState};
    use gpui::{Bounds, point, px, size};

    fn order(
        drag: &DockDrag<&'static str>,
        pages: &[&'static str],
        layout: &DockState<&'static str>,
    ) -> Vec<&'static str> {
        let mut pages = pages.to_vec();
        drag.reorder(&mut pages, layout, |key| key);
        pages
    }

    #[test]
    fn positions_before_after_and_in_the_trailing_space() {
        let rect =
            |x, y, width, height| Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
        let mut layout = DockState {
            settings: DockSettings::default(),
            resizing: None,
            zone: rect(0., 0., 370., 40.),
            tabs: vec![("one", rect(4., 4., 96., 31.))],
        };
        let at = |x, y| point(px(x), px(y));
        assert_eq!(layout.insertion(at(10., 20.)), Some(0));
        assert_eq!(layout.insertion(at(90., 20.)), Some(1));
        assert_eq!(layout.insertion(at(340., 20.)), Some(1));
        assert_eq!(layout.insertion(at(370., 20.)), None);
        assert_eq!(layout.insertion(at(10., 45.)), None);
        layout.settings.placement = DockPlacement::Left;
        layout.zone = rect(0., 40., 142., 400.);
        layout.tabs = vec![("one", rect(4., 44., 134., 31.))];
        assert_eq!(layout.insertion(at(30., 50.)), Some(0));
        assert_eq!(layout.insertion(at(30., 400.)), Some(1));
        assert_eq!(layout.insertion(at(150., 120.)), None);
    }

    #[test]
    fn tearing_off_requires_clearance() {
        let viewport = size(px(900.), px(650.));
        let top = Bounds::new(point(px(0.), px(0.)), size(px(280.), px(40.)));
        let mut dock = DockState::<&str> {
            settings: DockSettings::default(),
            resizing: None,
            zone: top,
            tabs: vec![],
        };
        assert!(!dock.leaves_strip(point(px(850.), px(60.)), viewport));
        assert!(!dock.leaves_strip(point(px(-15.), px(17.)), viewport));
        assert!(dock.leaves_strip(point(px(95.), px(65.)), viewport));
        assert!(dock.leaves_strip(point(px(-25.), px(17.)), viewport));
        dock.settings.placement = DockPlacement::Left;
        dock.zone = Bounds::new(point(px(0.), px(40.)), size(px(142.), px(610.)));
        assert!(!dock.leaves_strip(point(px(164.), px(570.)), viewport));
        assert!(dock.leaves_strip(point(px(167.), px(210.)), viewport));
    }

    #[test]
    fn dragging_into_blank_space_tracks_the_pointer_and_appends_on_release() {
        let mut layout = DockState {
            settings: DockSettings::default(),
            resizing: None,
            zone: Bounds::new(point(px(0.), px(0.)), size(px(500.), px(40.))),
            tabs: vec![
                (
                    "one",
                    Bounds::new(point(px(4.), px(4.)), size(px(96.), px(31.))),
                ),
                (
                    "two",
                    Bounds::new(point(px(104.), px(4.)), size(px(120.), px(31.))),
                ),
            ],
        };
        let mut drag = DockDrag {
            page: "one",
            previous_active: "one",
            offset: point(px(40.), px(12.)),
            cursor: point(px(355.), px(37.)),
        };
        let pages = ["one", "two"];
        let home = layout.bounds(&"one").unwrap();
        let preview = drag.projected(home, &layout);
        assert_eq!(preview.origin, point(px(315.), px(4.)));
        assert!(preview.left() > layout.bounds(&"two").unwrap().right());
        assert!(order(&drag, &pages, &layout) == ["two", "one"]);
        drag.cursor.x = px(650.);
        assert_eq!(drag.projected(home, &layout).right(), layout.zone.right());
        drag.cursor.x = px(-8.);
        assert_eq!(drag.projected(home, &layout), home);
        assert!(order(&drag, &pages, &layout) == pages);

        // Home is the only other tab when Settings arrives at its right edge.
        drag.page = "two";
        drag.offset.x = px(110.);
        drag.cursor.x = home.right() + px(1.);
        assert!(order(&drag, &pages, &layout) == pages);

        // The button can be released at a new location before Settings has a slot.
        layout.tabs.pop();
        assert!(order(&drag, &pages, &layout) == pages);
        drag.cursor.x = home.left();
        assert!(order(&drag, &pages, &layout) == ["two", "one"]);
    }

    #[test]
    fn sidebar_preview_can_reach_the_bottom_and_drop_last() {
        let slot = |y| Bounds::new(point(px(4.), px(y)), size(px(134.), px(31.)));
        let home = slot(44.);
        let layout = DockState {
            settings: DockSettings::default().with_placement(DockPlacement::Left),
            resizing: None,
            zone: Bounds::new(point(px(0.), px(40.)), size(px(142.), px(480.))),
            tabs: vec![("one", home), ("two", slot(79.))],
        };
        let pages = ["one", "two"];
        let mut drag = DockDrag {
            page: "one",
            previous_active: "one",
            offset: point(px(40.), px(28.)),
            cursor: point(px(65.), px(390.)),
        };
        assert_eq!(drag.projected(home, &layout), slot(362.));
        assert!(order(&drag, &pages, &layout) == ["two", "one"]);
        drag.cursor.y = layout.zone.bottom() - px(1.);
        assert_eq!(drag.projected(home, &layout).bottom(), layout.zone.bottom());
        drag.cursor.y = layout.zone.top();
        assert_eq!(drag.projected(home, &layout), home);
        assert!(order(&drag, &pages, &layout) == pages);

        // An incoming tab grabbed near its bottom still docks after Home
        // when the pointer is just below Home's slot.
        drag.page = "two";
        drag.cursor.y = home.bottom() + px(1.);
        assert!(order(&drag, &pages, &layout) == pages);
    }

    #[test]
    fn floating_tab_keeps_the_pointer_over_its_label() {
        let grab = point(px(35.), px(15.));
        assert_eq!(
            DockSettings::default().window_grab_offset(grab, px(900.)),
            point(px(39.), px(19.))
        );
        assert_eq!(
            DockSettings::default()
                .with_placement(DockPlacement::Left)
                .window_grab_offset(grab, px(900.)),
            point(px(39.), px(59.))
        );
    }
}
