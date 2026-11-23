use crate::{
    Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, PinchEvent, Pixels, PlatformInput, Point,
    ScrollDelta, ScrollWheelEvent, TouchPhase,
};
use std::collections::BTreeMap;

struct Contact<W> {
    window: W,
    origin: Point<Pixels>,
    position: Point<Pixels>,
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
    pub(super) fn down(&mut self, id: u32, window: W, position: Point<Pixels>) {
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
        let event = if contact.multi {
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
                scale: to / from,
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

    pub(super) fn up(&mut self, id: u32, modifiers: Modifiers) -> Option<(W, [PlatformInput; 2])> {
        let contact = self.0.remove(&id)?;
        if contact.multi || contact.moved {
            return None;
        }
        let position = contact.position;
        Some((
            contact.window,
            [
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{point, px};

    #[test]
    fn two_contacts_zoom_without_clicking_or_scrolling_on_release() {
        let mut contacts = TouchContacts::default();
        let at = |x| point(px(x), px(30.));
        contacts.down(0, 1, at(10.));
        contacts.down(1, 1, at(50.));
        let Some((1, PlatformInput::Pinch(pinch))) = contacts.motion(1, at(70.), Modifiers::none())
        else {
            panic!()
        };
        assert_eq!(pinch.scale, 1.5);
        assert_eq!(pinch.position, at(40.));
        assert!(contacts.up(0, Modifiers::none()).is_none());
        assert!(contacts.motion(1, at(71.), Modifiers::none()).is_none());
        assert!(contacts.up(1, Modifiers::none()).is_none());
        contacts.down(2, 1, at(10.));
        contacts.down(3, 2, at(10.));
        assert!(matches!(contacts.up(2, Modifiers::none()), Some((1, _))));
        assert!(matches!(
            contacts.motion(3, at(50.), Modifiers::none()),
            Some((2, PlatformInput::ScrollWheel(_)))
        ));
    }
}
