//! Touch input for a shortcut when no hardware keyboard is attached.
use super::*;
use crate::components::Select;
use paxri_config::shortcuts::{ALT, CTRL, META, SHIFT};

#[derive(Default, Clone, Copy)]
pub(super) struct Choice {
    modifiers: u8,
    group: usize,
}

const GROUPS: [(&str, &str); 5] = [
    (
        "Letters",
        "A B C D E F G H I J K L M N O P Q R S T U V W X Y Z",
    ),
    ("Numbers", "0 1 2 3 4 5 6 7 8 9"),
    (
        "Symbols",
        r##"+ - ! " # $ % & ' ( ) * , . / : ; < > ? @ [ \ ] ^ _ ` { | } ~"##,
    ),
    (
        "Special keys",
        "Backspace Delete Down End Enter Escape Home Insert Left PageDown PageUp Right Space Tab Up",
    ),
    ("Function keys", "F1 F2 F3 F4 F5 F6 F7 F8 F9 F10 F11 F12"),
];

pub(super) fn render(
    id: &ElementId,
    state: &Entity<Recording>,
    choice: Choice,
    change: Change,
    ui: Ui,
) -> Div {
    let mut modifiers = div().flex().flex_wrap().gap_1();
    for (name, bit) in [
        ("Ctrl", CTRL),
        ("Alt", ALT),
        ("Shift", SHIFT),
        ("Meta", META),
    ] {
        let state = state.clone();
        modifiers = modifiers.child(
            ui.button(ButtonVariant::Secondary, name, name)
                .pressed(choice.modifiers & bit != 0)
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| {
                        if let Some(choice) = &mut s.touch {
                            choice.modifiers ^= bit;
                        }
                        s.error = None;
                        cx.notify();
                    })
                }),
        );
    }
    let target = state.clone();
    let mut groups = Select::new(
        (id.clone(), "key-group"),
        choice.group,
        move |index, _, cx| {
            target.update(cx, |s, cx| {
                if let Some(choice) = &mut s.touch {
                    choice.group = *index;
                }
                cx.notify();
            });
        },
    );
    for (i, (name, _)) in GROUPS.iter().enumerate() {
        groups = groups.option(i, *name);
    }
    let target = state.clone();
    let mut keys = Select::new((id.clone(), "key"), None, move |key: &&str, w, cx| {
        let result =
            Shortcut::from_keys(choice.modifiers, key).and_then(|chord| change(Some(chord), w, cx));
        target.update(cx, |s, cx| {
            if result.is_ok() {
                s.touch = None;
            }
            s.error = result.err();
            cx.notify();
        });
    })
    .placeholder("Choose key…");
    for key in GROUPS[choice.group].1.split_whitespace() {
        keys = keys.option(key, key);
    }
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(modifiers)
        .child(groups)
        .child(keys)
}
