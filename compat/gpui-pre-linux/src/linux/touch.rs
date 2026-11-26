use gpui::{
    ContactPhase, DirectTouchEvent, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
    PinchEvent, Pixels, PlatformInput, Point, ScrollDelta, ScrollWheelEvent, TouchPhase,
};
use std::collections::BTreeMap;

struct Contact<W> {
    window: W,
    origin: Point<Pixels>,
    position: Point<Pixels>,
    claimed: bool,
    moved: bool,
    multi: bool,
}

/// Direct touchscreens report individual contacts. The window and device that received the
/// initial down own the contact until its release, even if the fingers cross window boundaries.
pub(super) struct TouchContacts<W>(BTreeMap<u32, Contact<W>>);

impl<W> Default for TouchContacts<W> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}

impl<W: Eq + Clone> TouchContacts<W> {
    pub(super) fn down(&mut self, id: u32, window: W, position: Point<Pixels>, claimed: bool) {
        let mut multi = false;
        for contact in self
            .0
            .values_mut()
            .filter(|contact| contact.window == window)
        {
            contact.multi = true;
            multi = true;
        }
        self.0.insert(
            id,
            Contact {
                window,
                origin: position,
                position,
                claimed,
                moved: false,
                multi,
            },
        );
    }

    pub(super) fn motion(
        &mut self,
        id: u32,
        position: Point<Pixels>,
        modifiers: Modifiers,
    ) -> Option<(W, PlatformInput)> {
        let contact = self.0.get_mut(&id)?;
        let previous = std::mem::replace(&mut contact.position, position);
        let window = contact.window.clone();
        let event = if contact.claimed {
            PlatformInput::DirectTouch(DirectTouchEvent {
                id,
                position,
                phase: ContactPhase::Move,
            })
        } else if contact.multi {
            let mut others = self
                .0
                .iter()
                .filter(|(other, contact)| **other != id && contact.window == window);
            let other = others.next()?.1.position;
            if others.next().is_some() {
                return None;
            }
            let from = (previous - other).magnitude() as f32;
            let to = (position - other).magnitude() as f32;
            if from < 1. || to < 1. || from == to {
                return None;
            }
            PlatformInput::Pinch(PinchEvent {
                position: (position + other) * 0.5,
                delta: to / from - 1.,
                modifiers,
                phase: TouchPhase::Moved,
            })
        } else {
            // Recognize a drag before scrolling so a slightly unsteady tap still clicks.
            if !contact.moved && (position - contact.origin).magnitude() < 6. {
                return None;
            }
            let delta = position
                - if contact.moved {
                    previous
                } else {
                    contact.origin
                };
            contact.moved = true;
            PlatformInput::ScrollWheel(ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(delta),
                modifiers,
                touch_phase: TouchPhase::Moved,
            })
        };
        Some((window, event))
    }

    pub(super) fn up(
        &mut self,
        id: u32,
        position: Option<Point<Pixels>>,
        modifiers: Modifiers,
    ) -> Option<(W, Vec<PlatformInput>)> {
        let contact = self.0.remove(&id)?;
        let position = position.unwrap_or(contact.position);
        if contact.claimed {
            return Some((
                contact.window,
                vec![PlatformInput::DirectTouch(DirectTouchEvent {
                    id,
                    position,
                    phase: ContactPhase::Up,
                })],
            ));
        }
        if contact.multi || contact.moved {
            return None;
        }
        Some((
            contact.window,
            vec![
                PlatformInput::MouseDown(MouseDownEvent {
                    position,
                    modifiers,
                    button: MouseButton::Left,
                    click_count: 1,
                    first_mouse: false,
                }),
                PlatformInput::MouseUp(MouseUpEvent {
                    position,
                    modifiers,
                    button: MouseButton::Left,
                    click_count: 1,
                }),
            ],
        ))
    }

    pub(super) fn cancel(&mut self) -> impl Iterator<Item = (W, PlatformInput)> {
        std::mem::take(&mut self.0)
            .into_iter()
            .filter_map(|(id, c)| {
                c.claimed.then_some((
                    c.window,
                    PlatformInput::DirectTouch(DirectTouchEvent {
                        id,
                        position: c.position,
                        phase: ContactPhase::Cancel,
                    }),
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px};

    #[test]
    fn claimed_contacts_deliver_final_coordinates_and_cancel_without_mouse_events() {
        let mut contacts = TouchContacts::default();
        let at = |x| point(px(x), px(20.));
        contacts.down(4, 1, at(10.), true);
        contacts.down(5, 1, at(75.), true);
        assert!(matches!(contacts.motion(4, at(13.), Modifiers::none()),
            Some((1, PlatformInput::DirectTouch(e))) if e.id == 4 && e.phase == ContactPhase::Move && e.position == at(13.)));
        let (_, events) = contacts.up(4, Some(at(18.)), Modifiers::none()).unwrap();
        assert!(matches!(events.as_slice(), [PlatformInput::DirectTouch(e)]
            if e.id == 4 && e.phase == ContactPhase::Up && e.position == at(18.)));
        let cancelled: Vec<_> = contacts.cancel().collect();
        assert!(
            matches!(cancelled.as_slice(), [(1, PlatformInput::DirectTouch(e))]
            if e.id == 5 && e.phase == ContactPhase::Cancel && e.position == at(75.))
        );
        assert!(contacts.up(5, None, Modifiers::none()).is_none());
    }

    #[test]
    fn two_contacts_zoom_without_clicking_or_scrolling_on_release() {
        let mut contacts = TouchContacts::default();
        let at = |x| point(px(x), px(30.));
        contacts.down(0, 1, at(10.), false);
        contacts.down(1, 1, at(50.), false);
        let Some((1, PlatformInput::Pinch(pinch))) = contacts.motion(1, at(70.), Modifiers::none())
        else {
            panic!()
        };
        assert_eq!(pinch.delta, 0.5);
        assert_eq!(pinch.position, at(40.));
        assert!(contacts.up(0, None, Modifiers::none()).is_none());
        assert!(contacts.motion(1, at(71.), Modifiers::none()).is_none());
        assert!(contacts.up(1, None, Modifiers::none()).is_none());
        contacts.down(2, 1, at(10.), false);
        contacts.down(3, 2, at(10.), false);
        assert!(matches!(
            contacts.up(2, None, Modifiers::none()),
            Some((1, _))
        ));
        assert!(matches!(
            contacts.motion(3, at(50.), Modifiers::none()),
            Some((2, PlatformInput::ScrollWheel(_)))
        ));
    }
}
