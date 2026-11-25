//! Terminal shortcut and clipboard settings shared by both clients.
use super::{SettingSpec, SettingsGroup, catalog};
use crate::{
    components::{ShortcutRecorder, Ui},
    theme,
};
use gpui::{prelude::*, *};
use paxri_config::{gui::GuiChange, shortcuts::TerminalAction};
use std::rc::Rc;

pub fn group(
    ui: Ui,
    cx: &App,
    change: impl Fn(GuiChange, &mut Window, &mut App) + 'static,
) -> SettingsGroup {
    let mut group = SettingsGroup::new(catalog::SHORTCUTS);
    let hint = if cfg!(target_os = "android") {
        "Tap a shortcut to record it with a hardware keyboard, or choose keys on screen from its menu."
    } else {
        "Select a shortcut and press the keys together. Esc cancels recording."
    };
    group.notice = Some(ui.muted(hint).into_any_element());
    let change = Rc::new(change);
    for action in TerminalAction::ALL {
        let change = change.clone();
        let recorder = ShortcutRecorder::new(
            action.key(),
            action.title(),
            theme::terminal_controls(cx).shortcut(action),
            move |binding, window, cx| {
                let controls = theme::terminal_controls(cx);
                let binding = controls.rebind(action, binding)?;
                if binding.as_ref() != controls.shortcuts.get(&action) {
                    change(GuiChange::Shortcut(action, binding), window, cx);
                }
                Ok(())
            },
        );
        let description = match action {
            TerminalAction::Copy => "Copies selected text. With no selection, the terminal handles the shortcut.",
            _ => "",
        };
        let spec = SettingSpec::new(action.key(), action.title(), description, "keyboard shortcut");
        group.setting(spec, recorder);
    }
    group
}

pub fn features_group(
    ui: Ui,
    cx: &App,
    change: impl Fn(GuiChange, &mut Window, &mut App) + 'static,
) -> SettingsGroup {
    let controls = theme::terminal_controls(cx);
    let mut group = SettingsGroup::new(catalog::TERMINAL_CONTROLS);
    let change = Rc::new(change);

    let remote_clipboard = controls.remote_clipboard;
    let on_clipboard = change.clone();
    group.grouped_setting(
        "Features",
        SettingSpec::new(
            "remote_clipboard",
            "Remote clipboard writes",
            "Allow programs in the active terminal to copy text to this device.",
            "OSC 52 copy",
        ),
        ui.switch("terminal-remote-clipboard", remote_clipboard)
            .on_click(move |_, w, cx| {
                on_clipboard(GuiChange::RemoteClipboard(!remote_clipboard), w, cx)
            }),
    );

    let clickable_links = controls.clickable_links;
    let on_links = change.clone();
    let link_description = if cfg!(target_os = "android") {
        "Detect URLs and OSC 8 links in the terminal to open or copy on tap."
    } else if cfg!(target_os = "macos") {
        "Open URLs and OSC 8 links in the terminal with Cmd+Click."
    } else {
        "Open URLs and OSC 8 links in the terminal with Ctrl+Click."
    };
    group.grouped_setting(
        "Features",
        SettingSpec::new(
            "clickable_links",
            "Clickable links",
            link_description,
            "url hyperlink osc 8 open browser",
        ),
        ui.switch("terminal-clickable-links", clickable_links)
            .on_click(move |_, w, cx| {
                on_links(GuiChange::ClickableLinks(!clickable_links), w, cx)
            }),
    );

    let terminal_bell = controls.terminal_bell;
    let bell_description = if cfg!(target_os = "android") {
        "Play the system notification sound when a program rings the bell."
    } else {
        "Play the system alert sound and briefly show a bell icon on the tab."
    };
    group.grouped_setting(
        "Features",
        SettingSpec::new(
            "terminal_bell",
            "Terminal bell",
            bell_description,
            "bel alert sound beep icon",
        ),
        ui.switch("terminal-bell", terminal_bell)
            .on_click(move |_, w, cx| change(GuiChange::TerminalBell(!terminal_bell), w, cx)),
    );

    group
}
