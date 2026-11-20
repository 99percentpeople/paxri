use std::{fs, sync::Barrier, thread};

use tunnel_config::{
    file::{read_gui, save_gateway, set_gui, set_startup},
    gui::{Appearance, CursorShape, GuiChange, GuiSettings, TerminalScheme},
    parse,
    startup::Startup,
};

#[test]
fn editing_one_gui_key_preserves_siblings_comments_and_gateway_data() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("node.toml");
    let common = "version = 1\ndevice_name = 'Studio' # kept\n";
    fs::write(
        &file,
        format!("{common}\n[gui]\nappearance = 'system' # theme\n"),
    )
    .unwrap();
    let first = set_gui(&file, GuiChange::Appearance(Appearance::Dark), false).unwrap();
    assert_eq!(first.appearance, Appearance::Dark);
    for change in [
        GuiChange::Scheme(TerminalScheme::Midnight),
        GuiChange::CursorShape(CursorShape::Bar),
        GuiChange::BlinkCursor(true),
    ] {
        set_gui(&file, change, false).unwrap();
    }
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.starts_with(common) && text.contains("# theme"));
    let gui = parse(&text).unwrap().gui;
    assert_eq!(read_gui(&file).unwrap(), gui);
    assert_eq!(gui.appearance, Appearance::Dark);
    assert_eq!(gui.terminal.scheme, TerminalScheme::Midnight);
    assert_eq!(gui.terminal.cursor_shape, CursorShape::Bar);
    assert!(gui.terminal.blink_cursor);
    assert!(parse(&text.replace("midnight", "typo")).is_err());
    assert!(parse(&text.replace("blink_cursor", "blink_curser")).is_err());
}

#[test]
fn concurrent_gateway_and_desktop_writes_keep_all_edits() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("node.toml");
    let original = "version=1\n";
    fs::write(&file, original).unwrap();
    let ready = Barrier::new(4);
    thread::scope(|s| {
        for change in [
            GuiChange::BlinkCursor(true),
            GuiChange::Scheme(TerminalScheme::Light),
        ] {
            let (file, ready) = (&file, &ready);
            s.spawn(move || {
                ready.wait();
                set_gui(file, change, false).unwrap();
            });
        }
        s.spawn(|| {
            ready.wait();
            set_startup(&file, Startup::Login).unwrap();
        });
        ready.wait();
        save_gateway(&file, original, format!("{original}device_name='Lab'\n")).unwrap();
    });
    let config = parse(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(config.device_name.as_deref(), Some("Lab"));
    assert_eq!(config.gateway.startup, Startup::Login);
    assert!(config.gui.terminal.blink_cursor);
    assert_eq!(config.gui.terminal.scheme, TerminalScheme::Light);
}

#[test]
fn gateway_merges_local_preferences_but_detects_external_node_edits() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("node.toml");
    let initial = "# head\nversion = 1";
    fs::write(&file, initial).unwrap();
    set_gui(&file, GuiChange::Scheme(TerminalScheme::Midnight), false).unwrap();
    set_startup(&file, Startup::Login).unwrap();
    let merged = save_gateway(&file, initial, format!("{initial}\ndevice_name='Lab'\n")).unwrap();
    let config = parse(&merged).unwrap();
    assert_eq!(config.gui.terminal.scheme, TerminalScheme::Midnight);
    assert_eq!(config.device_name.as_deref(), Some("Lab"));
    assert_eq!(config.gateway.startup, Startup::Login);
    let disk_edit = merged.replace("'Lab'", "'Office'");
    fs::write(&file, &disk_edit).unwrap();
    set_gui(&file, GuiChange::BlinkCursor(true), false).unwrap();
    assert!(save_gateway(&file, &merged, merged.replace("'Lab'", "'Studio'")).is_err());
    assert_eq!(
        parse(&fs::read_to_string(file).unwrap())
            .unwrap()
            .device_name
            .as_deref(),
        Some("Office")
    );
}

#[test]
fn fresh_and_partial_nodes_use_defaults_and_offline_edits_target_the_selected_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("profile/node.toml");
    assert_eq!(read_gui(&file).unwrap(), GuiSettings::default());
    assert!(set_gui(&file, GuiChange::BlinkCursor(true), false).is_err());
    let gui = set_gui(&file, GuiChange::CursorShape(CursorShape::Block), true).unwrap();
    assert_eq!(gui.appearance, Appearance::System);
    assert_eq!(gui.terminal.scheme, TerminalScheme::FollowApp);
    assert!(!gui.terminal.blink_cursor);
    assert_eq!(gui.terminal.cursor_shape, CursorShape::Block);
    assert_eq!(read_gui(&file).unwrap(), gui);
}
