//! English display names for the tables and keys presented by the settings editor.
use crate::components::{Icon, SettingSpec, SettingsSection, SettingsTable};

pub(super) const DEVICE: SettingsSection =
    SettingsSection::new(SettingsTable::Root, "This device", Icon::Monitor);
pub(super) const GATEWAY: SettingsSection =
    SettingsSection::new(SettingsTable::Named("gateway"), "Gateway", Icon::Server);
pub(super) const PORT_ACCESS: SettingsSection = SettingsSection::new(
    SettingsTable::Named("port_access"),
    "Port access",
    Icon::Server,
);
pub(super) const NETWORK: SettingsSection =
    SettingsSection::new(SettingsTable::Named("native"), "Network", Icon::Globe);
pub(super) const APPEARANCE: SettingsSection =
    SettingsSection::new(SettingsTable::Named("gui"), "Appearance", Icon::Sun);
pub(super) const TERMINAL: SettingsSection = SettingsSection::new(
    SettingsTable::Named("gui.terminal"),
    "Terminal",
    Icon::Terminal,
);
pub(super) const SERVICES: SettingsSection = SettingsSection::new(
    SettingsTable::Named("services"),
    "Published services",
    Icon::TabsOnTop,
);
pub(super) const RUNTIME: SettingsSection =
    SettingsSection::new(SettingsTable::Runtime, "Runtime", Icon::Settings);

pub(super) const INTERFACE: SettingsSection =
    SettingsSection::new(SettingsTable::Named("gui"), "Interface", Icon::Settings);

pub(super) fn page_for(section: SettingsSection) -> SettingsSection {
    let table = section.table.top_level();
    [
        DEVICE,
        GATEWAY,
        NETWORK,
        PORT_ACCESS,
        INTERFACE,
        SERVICES,
        RUNTIME,
    ]
    .into_iter()
    .find(|page| page.table == table)
    .unwrap_or(SettingsSection { table, ..section })
}

pub(super) const DEVICE_NAME: SettingSpec = SettingSpec::new(
    "device_name",
    "Device name",
    "",
    "computer name hostname identity",
);
pub(super) const STARTUP: SettingSpec = SettingSpec::new(
    "startup",
    "Startup behavior",
    "Choose when the gateway starts on this computer.",
    "manual gui login system boot sign-in",
);
pub(super) const UPNP: SettingSpec = SettingSpec::new(
    "upnp",
    "Allow UPnP",
    "Allow UPnP router discovery and UDP port requests. PCP and NAT-PMP remain available.",
    "",
);
pub(super) const LAN_MAPPING: SettingSpec = SettingSpec::new(
    "lan_port_mapping",
    "Physical LAN mapping",
    "Try the physical IPv4 router using NAT-PMP when a VPN controls the default route.",
    "",
);
pub(super) const RELAY: SettingSpec = SettingSpec::new(
    "relay_url",
    "Relay URL",
    "Leave blank to select a public relay automatically.",
    "custom server",
);
pub(super) const THEME: SettingSpec = SettingSpec::new(
    "appearance",
    "Theme",
    "Use the system appearance or choose a light or dark theme.",
    "",
);
pub(super) const TAB_PLACEMENT: SettingSpec = SettingSpec::new(
    "tab_placement",
    "Tab bar position",
    "Show workspace tabs across the top or along the left of each window.",
    "layout sidebar vertical horizontal",
);
pub(super) const SCHEME: SettingSpec = SettingSpec::new(
    "scheme",
    "Color scheme",
    "Choose the palette for terminal text and backgrounds.",
    "follow app light dark midnight",
);
pub(super) const FONT_FAMILY: SettingSpec = SettingSpec::new(
    "font_family",
    "Font family",
    "Choose an installed monospaced font.",
    "typeface monospace text",
);
pub(super) const LINE_HEIGHT: SettingSpec = SettingSpec::new(
    "line_height",
    "Line height",
    "Height of each terminal row in pixels.",
    "vertical spacing leading",
);
pub(super) const LETTER_SPACING: SettingSpec = SettingSpec::new(
    "letter_spacing",
    "Letter spacing",
    "Adjust the width of each character cell in pixels.",
    "horizontal spacing tracking",
);
pub(super) const CURSOR_SHAPE: SettingSpec = SettingSpec::new(
    "cursor_shape",
    "Cursor shape",
    "Choose the default shape of the terminal cursor.",
    "default block bar underline",
);
pub(super) const CURSOR_BLINK: SettingSpec = SettingSpec::new(
    "blink_cursor",
    "Cursor blinking",
    "Use a steady or blinking terminal cursor.",
    "default",
);
