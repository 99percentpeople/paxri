//! Edit one terminal keybar row per text field and preview the draft as it is typed.
use super::*;
use paxri_config::{
    gui::GuiChange,
    keybar::{KEYS, KeyHaptics, KeybarSettings, label, valid_key},
};
use paxri_ui::{
    components::{
        ButtonVariant::{Ghost, Outline},
        Icon, Select, heading,
        input::{EditingFinished, Input, NumberFormat, NumberStep},
        row,
    },
    settings::{
        SettingsGroup, SettingsSection, SettingsTable,
        controls::{number_field, setting_field},
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

fn row_text(keys: &[String]) -> String {
    keys.iter()
        .map(|key| match key.as_str() {
            "," => "Comma",
            "，" => "FullwidthComma",
            key => key,
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn bind_field(
    input: &Entity<Input>,
    keyboard: gpui_mobile::KeyboardType,
    cx: &mut Context<Mobile>,
) {
    Mobile::bind_input(input, keyboard, cx);
    cx.subscribe(input, |view, field, _: &EditingFinished, cx| {
        view.finish_keybar_fields(Some(&field), cx);
    })
    .detach();
    let mut previous = input.read(cx).text().to_owned();
    cx.observe(input, move |view, input, cx| {
        let text = input.read(cx).text();
        if view.key_editor.is_some() && text != previous {
            text.clone_into(&mut previous);
            cx.notify();
        }
    })
    .detach();
}

fn keys(text: &str) -> impl Iterator<Item = &str> {
    text.split(SEPARATORS).map(|token| key_name(token.trim()))
}

struct KeyRow {
    input: Entity<Input>,
    keys: Vec<String>,
}

impl KeyRow {
    fn new(keys: &[String], cx: &mut Context<Mobile>) -> Self {
        let input = Input::setting("Esc, Tab, Ctrl", cx);
        input.update(cx, |field, cx| field.set_text(&row_text(keys), cx));
        bind_field(&input, gpui_mobile::KeyboardType::Default, cx);
        Self {
            input,
            keys: keys.to_vec(),
        }
    }

    fn finish(&mut self, index: usize, cx: &mut App) -> Result<(), String> {
        let mut values = Vec::new();
        for (j, key) in keys(self.input.read(cx).text()).enumerate() {
            if key.is_empty() {
                continue;
            }
            if !valid_key(key) {
                return Err(format!(
                    "Row {}, key {}: unknown key “{key}”.",
                    index + 1,
                    j + 1
                ));
            }
            values.push(key.to_owned());
        }
        self.keys = values;
        Ok(())
    }
}

pub(super) struct Editor {
    rows: Vec<KeyRow>,
    height: Entity<Input>,
    row_height: u8,
    haptics: KeyHaptics,
}

impl Editor {
    fn load(layout: &KeybarSettings, cx: &mut Context<Mobile>) -> Self {
        let height = Input::number(NumberFormat::Integer, "dp", cx);
        height.update(cx, |field, cx| {
            field.set_placeholder(&KeybarSettings::DEFAULT_ROW_HEIGHT.to_string(), cx);
            field.set_text(&layout.row_height.to_string(), cx);
        });
        bind_field(&height, gpui_mobile::KeyboardType::NumberPad, cx);
        cx.subscribe(&height, |view, input, step: &NumberStep, cx| {
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
            view.finish_keybar_fields(Some(&input), cx);
        })
        .detach();
        Self {
            rows: layout
                .rows
                .iter()
                .map(|keys| KeyRow::new(keys, cx))
                .collect(),
            height,
            row_height: layout.row_height,
            haptics: layout.haptics,
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

    fn layout(&self) -> KeybarSettings {
        KeybarSettings {
            rows: self
                .rows
                .iter()
                .filter(|r| !r.keys.is_empty())
                .map(|r| r.keys.clone())
                .collect(),
            row_height: self.row_height,
            haptics: self.haptics,
        }
    }
}

impl Mobile {
    pub(super) fn open_keybar(&mut self, cx: &mut Context<Self>) {
        self.key_editor = Some(Editor::load(&self.keybar, cx));
        self.navigate(Page::SettingsCategory(SECTION), cx);
        cx.notify();
    }

    fn apply_keybar(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = &self.key_editor {
            let layout = editor.layout();
            if layout != *self.keybar {
                self.change_gui(GuiChange::Keybar(layout), cx);
            }
        }
    }

    pub(super) fn finish_keybar_fields(
        &mut self,
        field: Option<&Entity<Input>>,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &mut self.key_editor else {
            return;
        };
        let finished = |input: &Entity<Input>, cx: &App| {
            field.is_none_or(|field| field == input) && !input.read(cx).is_composing()
        };
        if finished(&editor.height, cx) {
            match editor.row_height(cx) {
                Ok(height) => editor.row_height = height,
                Err(error) => {
                    editor.height.update(cx, |input, cx| {
                        input.set_text(&editor.row_height.to_string(), cx)
                    });
                    Toast::show(error, self.window, cx);
                }
            }
        }
        for (i, row) in editor.rows.iter_mut().enumerate() {
            if finished(&row.input, cx)
                && let Err(error) = row.finish(i, cx)
            {
                row.input
                    .update(cx, |input, cx| input.set_text(&row_text(&row.keys), cx));
                Toast::show(error, self.window, cx);
            }
        }
        self.apply_keybar(cx);
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
                    keys(row.input.read(cx).text())
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
                        let visible_height = paxri_ui::components::DropdownState::active_field_popup_top(window, cx)
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
                        view.set_touch_area(0, area, draft.clone(), true);
                    }
                })
            },
            |_, _, _, _| (),
        )
        .w_full()
        .h(px(preview_height))
        .max_h(relative(0.35))
        .flex_none();
        let mut rows = SettingsGroup::new(SECTION);
        rows.notice = Some(ui.muted("Separate keys with commas.").into_any_element());
        for (i, row) in editor.rows.iter().enumerate() {
            let id = row.input.entity_id();
            let delete = ui
                .icon_button(Ghost, ("delete-row", id), Icon::Trash, "Delete row")
                .role(Role::Button)
                .aria_label(format!("Delete row {}", i + 1))
                .on_click(cx.listener(move |view, _, window, cx| {
                    window.blur(cx);
                    if let Some(editor) = &mut view.key_editor {
                        editor.rows.retain(|row| row.input.entity_id() != id);
                        view.apply_keybar(cx);
                        cx.notify();
                    }
                }));
            rows.field(
                format!("Row {}", i + 1),
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(row.input.clone()))
                    .child(delete),
            );
        }
        rows.footer = Some(
            row()
                .child(
                    ui.button(Outline, "add-row", "Add row")
                        .icon(Icon::Plus)
                        .on_click(cx.listener(|view, _, window, cx| {
                            let row = KeyRow::new(&[], cx);
                            let input = row.input.clone();
                            if let Some(editor) = &mut view.key_editor {
                                editor.rows.push(row);
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
                            view.apply_keybar(cx);
                            cx.notify();
                        })),
                )
                .into_any_element(),
        );
        let mut feedback = Select::new(
            "key-vibration",
            editor.haptics,
            cx.listener(|view, mode: &KeyHaptics, _, cx| {
                if let Some(editor) = &mut view.key_editor {
                    editor.haptics = *mode;
                    view.apply_keybar(cx);
                    cx.notify();
                }
            }),
        );
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
        options.add("haptics", "Key vibration", "", "", setting_field(feedback));
        let form = self.settings_body(
            "keybar-fields",
            Self::settings_column()
                .child(
                    div()
                        .id("keybar-rows")
                        .w_full()
                        .flex()
                        .flex_col()
                        .child(rows.render_rows(ui)),
                )
                .child(options.render_rows(ui)),
        );
        div()
            .id("terminal-key-editor")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().px_4().py_2().child(heading("Preview")))
            .child(preview)
            .child(form)
    }
}
