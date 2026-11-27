//! English display names for the tables and keys presented by the settings editor.
use super::{Icon, SettingSpec, SettingsSection, SettingsTable};

pub const DEVICE: SettingsSection =
    SettingsSection::new(SettingsTable::Root, "This device", Icon::Monitor);
pub const GATEWAY: SettingsSection =
    SettingsSection::new(SettingsTable::Named("gateway"), "Gateway", Icon::Server);
pub const NETWORK: SettingsSection =
    SettingsSection::new(SettingsTable::Named("native"), "Network", Icon::Globe);
pub const APPEARANCE: SettingsSection =
    SettingsSection::new(SettingsTable::Named("gui"), "Appearance", Icon::Sun);
pub const TERMINAL: SettingsSection = SettingsSection::new(
    SettingsTable::Named("gui.terminal"),
    "Terminal",
    Icon::Terminal,
);
pub const INTERFACE: SettingsSection =
    SettingsSection::new(SettingsTable::Named("gui"), "Interface", Icon::Settings);
pub const TERMINAL_CONTROLS: SettingsSection =
    SettingsSection::new(SettingsTable::Named("gui.controls"), "Terminal controls", Icon::Terminal);
pub const SHORTCUTS: SettingsSection =
    SettingsSection::new(SettingsTable::Named("gui.controls.shortcuts"), "Keyboard shortcuts", Icon::Terminal);

pub fn page_for(section: SettingsSection) -> SettingsSection {
    if section == TERMINAL_CONTROLS || section == SHORTCUTS {
        return TERMINAL_CONTROLS;
    }
    let table = section.table.top_level();
    [DEVICE, GATEWAY, NETWORK, INTERFACE]
        .into_iter()
        .find(|page| page.table == table)
        .unwrap_or(SettingsSection { table, ..section })
}

pub const DEVICE_NAME: SettingSpec = SettingSpec::new(
    "device_name",
    "Device name",
    "",
    "computer name hostname identity",
);
pub const STARTUP: SettingSpec = SettingSpec::new(
    "startup",
    "Startup behavior",
    "Choose when the gateway starts on this computer.",
    "manual gui login system boot sign-in",
);
pub const SESSION_KEEP_ALIVE: SettingSpec = SettingSpec::new(
    "session_keep_alive",
    "Session keep-alive",
    "How long this gateway keeps hosted terminal sessions running after a device disconnects.",
    "timeout expire ttl forever persistent idle disconnect",
);
pub const BACKGROUND_SERVICE: SettingSpec = SettingSpec::new(
    "background_service",
    "Background service",
    "Keep the gateway active in a foreground service when switching apps or closing the window.",
    "foreground daemon keep alive background persistent",
);
pub const SCREEN_OFF_KEEP_ALIVE: SettingSpec = SettingSpec::new(
    "screen_off_keep_alive",
    "Screen-off keep-alive",
    "Hold CPU and low-latency Wi-Fi locks while the gateway runs so turning off the screen does not drop connections.",
    "background sleep wake lock wifi screen off power",
);
pub const UPNP: SettingSpec = SettingSpec::new(
    "upnp",
    "Allow UPnP",
    "Allow UPnP router discovery and UDP port requests. PCP and NAT-PMP remain available.",
    "",
);
pub const LAN_MAPPING: SettingSpec = SettingSpec::new(
    "lan_port_mapping",
    "Physical LAN mapping",
    "Try the physical IPv4 router using NAT-PMP when a VPN controls the default route.",
    "",
);
pub const RELAY: SettingSpec = SettingSpec::new(
    "relay_url",
    "Relay URL",
    "Leave blank to select a public relay automatically.",
    "custom server",
);
pub const THEME: SettingSpec = SettingSpec::new(
    "appearance",
    "Theme",
    "Use the system appearance or choose a light or dark theme.",
    "",
);
pub const TAB_PLACEMENT: SettingSpec = SettingSpec::new(
    "tab_placement",
    "Tab bar position",
    "Show workspace tabs across the top or along the left of each window.",
    "layout sidebar vertical horizontal",
);
pub const SCHEME: SettingSpec = SettingSpec::new(
    "scheme",
    "Color scheme",
    "Choose the palette for terminal text and backgrounds.",
    "follow app light dark midnight",
);
pub const FONT_FAMILY: SettingSpec = SettingSpec::new(
    "font_family",
    "Font family",
    "Choose an installed monospaced font.",
    "typeface monospace text",
);
pub const LINE_HEIGHT: SettingSpec = SettingSpec::new(
    "line_height",
    "Line height",
    "Height of each terminal row in pixels.",
    "vertical spacing leading",
);
pub const LETTER_SPACING: SettingSpec = SettingSpec::new(
    "letter_spacing",
    "Letter spacing",
    "Adjust the width of each character cell in pixels.",
    "horizontal spacing tracking",
);
pub const CURSOR_SHAPE: SettingSpec = SettingSpec::new(
    "cursor_shape",
    "Cursor shape",
    "Use this cursor shape throughout the terminal, including during text composition.",
    "block bar underline",
);
pub const CURSOR_BLINK: SettingSpec = SettingSpec::new(
    "blink_cursor",
    "Cursor blinking",
    "Use a steady or blinking terminal cursor.",
    "default",
);

pub const FONT_SIZE: SettingSpec = SettingSpec::new("font_size", "Font size", "Size of terminal text in pixels.", "text zoom");
