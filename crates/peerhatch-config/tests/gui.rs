use std::{fs, sync::Barrier, thread};

use peerhatch_config::{
    file::{read_gui, save_gateway, set_gui, set_startup},
    gui::{Appearance, CursorShape, GuiChange, GuiSettings, TabPlacement, TerminalScheme},
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
    assert_eq!(first.tab_placement, TabPlacement::Top);
    for change in [
        GuiChange::TabPlacement(TabPlacement::Left),
        GuiChange::Scheme(TerminalScheme::Midnight),
        GuiChange::CursorShape(CursorShape::Bar),
        GuiChange::FontFamily("Cascadia Mono".to_owned()),
        GuiChange::FontSize(Some(12.5)),
        GuiChange::LineHeight(Some(24)),
        GuiChange::LetterSpacing(-1),
        GuiChange::BlinkCursor(true),
    ] {
        set_gui(&file, change, false).unwrap();
    }
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.starts_with(common) && text.contains("# theme"));
    let gui = parse(&text).unwrap().gui;
    assert_eq!(read_gui(&file).unwrap(), gui);
    assert_eq!(gui.appearance, Appearance::Dark);
    assert_eq!(gui.tab_placement, TabPlacement::Left);
    assert_eq!(gui.terminal.scheme, TerminalScheme::Midnight);
    assert_eq!(gui.terminal.cursor_shape, CursorShape::Bar);
    assert!(gui.terminal.blink_cursor);
    assert_eq!(gui.terminal.font_family, "Cascadia Mono");
    assert_eq!(gui.terminal.font_size, Some(12.5));
    assert_eq!(gui.terminal.line_height, Some(24));
    assert_eq!(gui.terminal.letter_spacing, -1);
    for invalid in [GuiChange::FontSize(Some(31.)), GuiChange::FontSize(Some(f32::NAN)), GuiChange::LineHeight(Some(0)), GuiChange::LetterSpacing(5)] {
        assert!(set_gui(&file, invalid, false).is_err());
    }
    assert_eq!(fs::read_to_string(&file).unwrap(), text);
    let automatic = set_gui(&file, GuiChange::LineHeight(None), false).unwrap();
    assert!((automatic.terminal.row_height(12.5) - 15.).abs() < 0.001);
    let reset = set_gui(&file, GuiChange::FontSize(None), false).unwrap();
    assert_eq!(reset.terminal.size(), 15.);
    assert_eq!(reset.terminal.letter_spacing, -1);
    assert!(!fs::read_to_string(&file).unwrap().contains("font_size"));
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

#[test]
fn terminal_key_rows_keep_order_and_validate_before_saving() {
    use peerhatch_config::keybar::KeybarSettings;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gui.toml");
    let original = "version = 1\n[gui]\nappearance = 'dark' # keep\n";
    fs::write(&path, original).unwrap();
    assert!(read_gui(&path).unwrap().keybar.is_none());
    let keys = KeybarSettings {
        row_height: 37,
        haptics: peerhatch_config::keybar::KeyHaptics::Light,
        rows: vec![vec!["Shift".into(), "F2".into()], vec!["|".into(), "a".into(), "Shift".into()]],
    };
    set_gui(&path, GuiChange::Keybar(keys.clone()), false).unwrap();
    assert_eq!(read_gui(&path).unwrap().keybar.as_deref(), Some(&keys));
    let saved = fs::read_to_string(&path).unwrap();
    assert!(saved.starts_with(original));
    for rows in [vec![vec![]], vec![vec!["not-a-key".into()]]] {
        let invalid = KeybarSettings { rows, ..keys.clone() };
        assert!(set_gui(&path, GuiChange::Keybar(invalid), false).is_err());
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), saved);
}
