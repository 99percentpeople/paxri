//! Edit one terminal keybar row per text field and preview the draft as it is typed.
use super::*;
use paxri_config::{
    gui::GuiChange,
    keybar::{KEYS, KeyHaptics, KeybarSettings, label, valid_key},
};
use paxri_ui::{
    components::{
        ButtonVariant::{Ghost, Outline, Primary},
        Icon, Select, heading,
        input::{Input, NumberFormat, NumberStep},
        row,
    },
    settings::{
        SettingsGroup, SettingsLayout, SettingsPage, SettingsSection, SettingsTable,
        controls::number_field,
    },
};

pub(super) const SECTION: SettingsSection = SettingsSection::new(
    SettingsTable::Named("gui.keybar"),
    "Terminal keys",
    Icon::Terminal,
);

const SEPARATORS: [char; 2] = [',', '，'];
const HAPTICS: [(KeyHaptics, &str); 4] = [
    (KeyHaptics::Off, "Off"),
    (KeyHaptics::Light, "Light"),
    (KeyHaptics::Standard, "Standard"),
    (KeyHaptics::Strong, "Strong"),
];

fn key_name(token: &str) -> &str {
    if token.eq_ignore_ascii_case("Comma") {
        return ",";
    }
    if token.eq_ignore_ascii_case("FullwidthComma") {
        return "，";
    }
    KEYS.iter()
        .copied()
        .find(|&key| {
            token.eq_ignore_ascii_case(key)
                || (label(key).len() > 1
                    && label(key).is_ascii()
                    && token.eq_ignore_ascii_case(label(key)))
        })
        .unwrap_or(token)
}

fn row_input(keys: &[String], cx: &mut Context<Mobile>) -> Entity<Input> {
    let text = keys
        .iter()
        .map(|key| match key.as_str() {
            "," => "Comma",
            "，" => "FullwidthComma",
            key => key,
        })
        .collect::<Vec<_>>()
        .join(", ");
    let input = Input::setting("Esc, Tab, Ctrl", cx);
    input.update(cx, |field, cx| field.set_text(&text, cx));
    bind_field(&input, gpui_mobile::KeyboardType::Default, cx);
    input
}

fn bind_field(
    input: &Entity<Input>,
    keyboard: gpui_mobile::KeyboardType,
    cx: &mut Context<Mobile>,
) {
    Mobile::bind_input(input, keyboard, cx);
    let mut previous = input.read(cx).text().to_owned();
    cx.observe(input, move |view, input, cx| {
        let text = input.read(cx).text();
        if let Some(editor) = &mut view.key_editor
            && text != previous
        {
            text.clone_into(&mut previous);
            editor.error.clear();
            cx.notify();
        }
    })
    .detach();
}

fn keys(text: &str) -> impl Iterator<Item = &str> {
    text.split(SEPARATORS).map(|token| key_name(token.trim()))
}

pub(super) struct Editor {
    rows: Vec<Entity<Input>>,
    height: Entity<Input>,
    haptics: KeyHaptics,
    error: String,
}

impl Editor {
    fn load(layout: &KeybarSettings, cx: &mut Context<Mobile>) -> Self {
        let height = Input::number(NumberFormat::Integer, "dp", cx);
        height.update(cx, |field, cx| {
            field.set_placeholder(&KeybarSettings::DEFAULT_ROW_HEIGHT.to_string(), cx);
            field.set_text(&layout.row_height.to_string(), cx);
        });
        bind_field(&height, gpui_mobile::KeyboardType::NumberPad, cx);
        cx.subscribe(&height, |_, input, step: &NumberStep, cx| {
            let range = KeybarSettings::HEIGHTS;
            let current = input
                .read(cx)
                .text()
                .parse::<i32>()
                .unwrap_or(KeybarSettings::DEFAULT_ROW_HEIGHT.into());
            let next = current
                .saturating_add(step.0)
                .clamp((*range.start()).into(), (*range.end()).into());
            input.update(cx, |field, cx| field.set_text(&next.to_string(), cx));
        })
        .detach();
        Self {
            rows: layout.rows.iter().map(|keys| row_input(keys, cx)).collect(),
            height,
            haptics: layout.haptics,
            error: String::new(),
        }
    }

    fn row_height(&self, cx: &App) -> Result<u8, String> {
        let text = self.height.read(cx).text();
        if text.is_empty() {
            return Ok(KeybarSettings::DEFAULT_ROW_HEIGHT);
        }
        text.parse()
            .ok()
            .filter(|n| KeybarSettings::HEIGHTS.contains(n))
            .ok_or_else(|| {
                format!(
                    "Enter a key height from {} to {} dp.",
                    KeybarSettings::HEIGHTS.start(),
                    KeybarSettings::HEIGHTS.end()
                )
            })
    }

    fn layout(&self, cx: &App) -> Result<KeybarSettings, String> {
        let mut rows = Vec::new();
        for (i, row) in self.rows.iter().enumerate() {
            let mut values = Vec::new();
            for (j, key) in keys(row.read(cx).text()).enumerate() {
                if key.is_empty() {
                    continue;
                }
                if !valid_key(key) {
                    return Err(format!(
                        "Row {}, key {}: unknown key “{key}”.",
                        i + 1,
                        j + 1
                    ));
                }
                values.push(key.to_owned());
            }
            if !values.is_empty() {
                rows.push(values);
            }
        }
        Ok(KeybarSettings {
            rows,
            row_height: self.row_height(cx)?,
            haptics: self.haptics,
        })
    }
}

impl Mobile {
    pub(super) fn open_keybar(&mut self, cx: &mut Context<Self>) {
        self.key_editor = Some(Editor::load(&self.keybar, cx));
        self.navigate(Page::SettingsCategory(SECTION), cx);
        cx.notify();
    }

    pub(super) fn save_keybar_button(&self, ui: Ui, cx: &Context<Self>) -> AnyElement {
        ui.button(Primary, "save-keys", "Save")
            .on_click(cx.listener(|this, _, _, cx| {
                let Some(editor) = &mut this.key_editor else {
                    return;
                };
                match editor.layout(cx) {
                    Ok(layout) => {
                        this.change_gui(GuiChange::Keybar(layout), cx);
                        if this.settings_error.is_empty() {
                            this.back(cx);
                        }
                    }
                    Err(error) => {
                        editor.error = error;
                        cx.notify();
                    }
                }
            }))
            .into_any_element()
    }

    pub(super) fn keybar_editor(&self, ui: Ui, cx: &Context<Self>) -> Stateful<Div> {
        let editor = self
            .key_editor
            .as_ref()
            .expect("open terminal key settings");
        let draft = Arc::new(KeybarSettings {
            row_height: editor.row_height(cx).unwrap_or(self.keybar.row_height),
            haptics: editor.haptics,
            rows: editor
                .rows
                .iter()
                .map(|row| {
                    keys(row.read(cx).text())
                        .map(|key| if key.is_empty() { "…" } else { key }.to_owned())
                        .collect()
                })
                .collect(),
        });
        let preview_height = f32::from(draft.row_height) * draft.rows.len() as f32;
        let view = cx.entity();
        // Let the terminal's native keybar render and scroll the preview at its actual size.
        let preview = canvas(
            move |bounds, window, cx| {
                let scale = window.scale_factor();
                view.update(cx, |view, cx| {
                    if view.page == Page::SettingsCategory(SECTION) {
                        let visible_height = view
                            .keybar_menu
                            .read(cx)
                            .field_popup_top(HAPTICS.len(), window)
                            .map_or(bounds.size.height, |top| {
                                (top - bounds.origin.y).clamp(px(0.), bounds.size.height)
                            });
                        let area = [
                            bounds.origin.x,
                            bounds.origin.y,
                            bounds.size.width,
                            px(0.),
                            visible_height,
                        ]
                        .map(|v| (f32::from(v) * scale).round() as i32);
                        view.set_touch_area(0, area, draft.clone());
                    }
                })
            },
            |_, _, _, _| (),
        )
        .w_full()
        .h(px(preview_height))
        .max_h(relative(0.35))
        .flex_none();
        let mut form = div()
            .id("keybar-fields")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .restrict_scroll_to_axis()
            .track_scroll(&self.settings_scroll)
            .flex()
            .flex_col()
            .p_4()
            .gap_3()
            .child(ui.muted("Separate keys with commas.").text_sm());
        for error in [&editor.error, &self.settings_error] {
            if !error.is_empty() {
                form = form.child(ui.alert(error.clone()));
            }
        }
        for (i, row) in editor.rows.iter().enumerate() {
            let id = row.entity_id();
            let delete = ui
                .icon_button(Ghost, ("delete-row", id), Icon::Trash, "Delete row")
                .role(Role::Button)
                .aria_label(format!("Delete row {}", i + 1))
                .on_click(cx.listener(move |view, _, window, cx| {
                    window.blur(cx);
                    if let Some(editor) = &mut view.key_editor {
                        editor.rows.retain(|row| row.entity_id() != id);
                        editor.error.clear();
                        cx.notify();
                    }
                }));
            form = form.child(
                ui.labeled(
                    format!("Row {}", i + 1),
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().min_w_0().child(row.clone()))
                        .child(delete),
                )
                .flex_none()
                .min_w_0(),
            );
        }
        form = form.child(
            row()
                .child(
                    ui.button(Outline, "add-row", "Add row")
                        .icon(Icon::Plus)
                        .on_click(cx.listener(|view, _, window, cx| {
                            let input = row_input(&[], cx);
                            if let Some(editor) = &mut view.key_editor {
                                editor.rows.push(input.clone());
                            }
                            window.on_next_frame(move |w, cx| {
                                input.update(cx, |field, cx| field.edit_at_end(w, cx))
                            });
                            cx.notify();
                        })),
                )
                .child(
                    ui.button(Ghost, "reset-keys", "Restore defaults")
                        .on_click(cx.listener(|view, _, window, cx| {
                            window.blur(cx);
                            view.key_editor = Some(Editor::load(&KeybarSettings::default(), cx));
                            cx.notify();
                        })),
                ),
        );
        let mut feedback = Select::new(
            "key-vibration",
            editor.haptics,
            cx.listener(|view, mode: &KeyHaptics, _, cx| {
                if let Some(editor) = &mut view.key_editor {
                    editor.haptics = *mode;
                    cx.notify();
                }
            }),
        )
        .with_optional_state(Some(&self.keybar_menu));
        for (mode, label) in HAPTICS {
            feedback = feedback.option(mode, label);
        }
        let mut options = SettingsGroup::new(SECTION);
        options.add(
            "row_height",
            "Key height",
            "",
            "",
            number_field(editor.height.clone()),
        );
        options.add(
            "haptics",
            "Key vibration",
            "",
            "",
            div().w(px(140.)).max_w_full().child(feedback),
        );
        let settings = SettingsPage {
            section: SECTION,
            groups: vec![options],
        };
        div()
            .id("terminal-key-editor")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().px_4().py_2().child(heading("Preview")))
            .child(preview)
            .child(form.child(settings.render(ui, SettingsLayout::Mobile)))
    }
}
