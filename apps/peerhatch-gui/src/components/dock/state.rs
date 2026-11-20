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
    pub(super) scroll: ScrollHandle,
    pub(super) reveal_after_placement: Option<usize>,
    // Resting slots in content coordinates, before the strip's own scroll offset.
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
                scroll: ScrollHandle::new(),
                reveal_after_placement: None,
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

    pub fn set_placement(&mut self, placement: DockPlacement, active: &K, cx: &mut Context<Self>) {
        if self.settings.placement != placement {
            self.reveal_after_placement = self.tabs.iter().position(|(key, _)| key == active);
            self.settings = self.settings.with_placement(placement);
            self.resizing = None;
            self.zone = Bounds::default();
            self.scroll = ScrollHandle::new();
            self.tabs.clear();
            cx.notify();
        }
    }

    pub fn forget(&mut self, key: &K) {
        self.tabs.retain(|(id, _)| id != key);
    }

    pub fn reveal_tab(&self, index: usize) {
        self.scroll.scroll_to_item(index);
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
        self.slots().find(|(p, _)| *p == page).map(|(_, b)| b)
    }

    fn slots(&self) -> impl Iterator<Item = (&K, Bounds<Pixels>)> {
        let offset = self.scroll.offset();
        self.tabs
            .iter()
            .map(move |(page, slot)| (page, *slot + offset))
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
            self.slots()
                .position(|(_, b)| cursor.along(axis) < b.center().along(axis))
                .unwrap_or(self.tabs.len()),
        )
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
    pub fn pressed(
        page: K,
        previous_active: K,
        cursor: Point<Pixels>,
        layout: &DockState<K>,
    ) -> Option<Self> {
        let slot = layout.bounds(&page)?;
        Some(Self {
            page,
            previous_active,
            offset: cursor - slot.origin,
            cursor,
        })
    }

    pub fn projected(&self, mut bounds: Bounds<Pixels>, layout: &DockState<K>) -> Bounds<Pixels> {
        let axis = layout.settings.placement.axis();
        let desired = (self.cursor - self.grab_in(bounds)).along(axis);
        let strip_start = layout.zone.origin.along(axis);
        let end =
            (layout.zone.bottom_right().along(axis) - bounds.size.along(axis)).max(strip_start);
        let start = layout.slots().next().map_or(strip_start, |(_, first)| {
            first.origin.along(axis).clamp(strip_start, end)
        });
        bounds.origin = bounds
            .origin
            .apply_along(axis, |_| desired.clamp(start, end));
        bounds
    }

    pub fn window_grab(&self, layout: &DockState<K>) -> Option<Point<Pixels>> {
        let slot = layout.bounds(&self.page)?;
        Some(slot.origin + self.grab_in(slot))
    }

    pub fn should_detach(&self, layout: &DockState<K>) -> bool {
        let Some(slot) = layout.bounds(&self.page) else {
            return false;
        };
        let visible = self.projected(slot, layout).intersect(&layout.zone);
        if visible.is_empty() {
            return false;
        }
        let nearest = self.cursor.clamp(&visible.origin, &visible.bottom_right());
        (self.cursor - nearest).magnitude() > f64::from(DETACH_DISTANCE)
    }

    fn grab_in(&self, bounds: Bounds<Pixels>) -> Point<Pixels> {
        // A different strip can be narrower than the original grab. Keep the
        // pointer inside that face without losing the offset for the next strip.
        let inside = |grab: Pixels, extent: Pixels| {
            if grab < extent {
                grab.max(px(0.))
            } else {
                (extent - px(1.)).max(px(0.))
            }
        };
        point(
            inside(self.offset.x, bounds.size.width),
            inside(self.offset.y, bounds.size.height),
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
            .slots()
            .filter(|(page, bounds)| {
                if *page == &self.page {
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
            scroll: Default::default(),
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
    fn tear_off_uses_the_distance_to_the_visible_moving_tab() {
        let rect =
            |x, y, width, height| Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
        let mut dock = DockState {
            settings: DockSettings::default(),
            resizing: None,
            scroll: Default::default(),
            zone: rect(0., 0., 400., 40.),
            tabs: vec![("one", rect(4., 4., 96., 32.))],
        };
        let drag = DockDrag::pressed("one", "one", point(px(84.), px(12.)), &dock).unwrap();
        let detaches = |dock: &DockState<_>, x, y| {
            DockDrag {
                cursor: point(px(x), px(y)),
                ..drag.clone()
            }
            .should_detach(dock)
        };
        assert!(!detaches(&dock, 340., 12.)); // Far from the press, still on the moving tab.
        assert!(!detaches(&dock, 340., 60.));
        assert!(detaches(&dock, 340., 61.));
        assert!(!detaches(&dock, 424., 12.));
        assert!(detaches(&dock, 425., 12.));
        assert!(!detaches(&dock, 414., 54.));
        assert!(detaches(&dock, 416., 55.)); // Diagonally past the bottom-right corner.

        dock.zone = rect(0., 0., 60., 40.); // Only part of this wide tab can be painted.
        assert!(detaches(&dock, 85., 12.));
        dock.settings.placement = DockPlacement::Left;
        dock.zone = rect(0., 40., 142., 480.);
        dock.tabs[0].1 = rect(4., 44., 134., 32.);
        assert!(!detaches(&dock, 84., 500.));
        assert!(!detaches(&dock, 162., 500.));
        assert!(detaches(&dock, 163., 500.));
        assert!(detaches(&dock, 84., 19.));
        dock.tabs.clear(); // The receiving window has yet to measure the incoming tab.
        assert!(!detaches(&dock, 163., 500.));
    }

    #[test]
    fn dragging_into_blank_space_tracks_the_pointer_and_appends_on_release() {
        let mut layout = DockState {
            settings: DockSettings::default(),
            resizing: None,
            scroll: Default::default(),
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
        drag.cursor.x = layout.zone.right() - px(1.);
        assert_eq!(drag.projected(home, &layout).right(), layout.zone.right());
        assert!(order(&drag, &pages, &layout) == ["two", "one"]);
        drag.cursor.x = layout.zone.left() + px(1.);
        assert_eq!(drag.projected(home, &layout), home);
        assert!(order(&drag, &pages, &layout) == pages);
        drag.cursor.x = px(117.);
        assert_eq!(
            drag.projected(home, &layout).left() + drag.offset.x,
            drag.cursor.x
        );

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
            scroll: Default::default(),
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
        assert!(order(&drag, &pages, &layout) == ["two", "one"]);
        drag.cursor.y = layout.zone.top();
        assert_eq!(drag.projected(home, &layout), home);
        assert!(order(&drag, &pages, &layout) == pages);
        drag.cursor.y = px(105.);
        assert_eq!(
            drag.projected(home, &layout).top() + drag.offset.y,
            drag.cursor.y
        );

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

    #[test]
    fn press_on_a_scrolled_tab_survives_relayout_and_a_narrower_strip() {
        let rect = |x, width| Bounds::new(point(px(x), px(4.)), size(px(width), px(31.)));
        let mut layout = DockState {
            settings: DockSettings::default(),
            resizing: None,
            scroll: Default::default(),
            zone: rect(0., 152.),
            tabs: vec![("one", rect(4., 96.)), ("two", rect(104., 140.))],
        };
        layout.scroll.set_offset(point(px(-80.), px(0.)));
        let mut drag = DockDrag::pressed("two", "one", point(px(132.), px(11.)), &layout).unwrap();
        assert_eq!(drag.offset, point(px(108.), px(7.)));
        assert_eq!(layout.indent(&"two"), px(20.));
        assert_eq!(layout.insertion(point(px(21.), px(18.))), Some(1));

        layout.scroll.set_offset(point(px(-28.), px(0.)));
        drag.cursor = point(px(110.), px(28.));
        for (width, expected_grab) in [(140., 108.), (64., 63.), (140., 108.)] {
            layout.tabs[1].1 = rect(104., width);
            let slot = layout.bounds(&"two").unwrap();
            let preview = drag.projected(slot, &layout);
            assert_eq!(preview.left() + px(expected_grab), drag.cursor.x);
            assert_eq!(preview.top(), slot.top());
            assert_eq!(
                drag.window_grab(&layout).unwrap().x,
                slot.left() + px(expected_grab)
            );
        }
        layout.zone = rect(0., 60.);
        let slot = layout.bounds(&"two").unwrap();
        assert_eq!(drag.projected(slot, &layout).left(), layout.zone.left());
    }
}
