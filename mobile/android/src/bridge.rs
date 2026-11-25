//! JNI callbacks, Activity actions, and shared Android platform state.
use super::*;
use jni::{
    EnvUnowned, jni_sig, jni_str,
    objects::{Global, JClass, JObject, JString},
};
use paxri_config::shortcuts::{ALT, CTRL, META, SHIFT};
use std::sync::{
    LazyLock, Mutex,
    atomic::{AtomicBool, Ordering},
};

pub(crate) static ACTIVITY_VISIBLE: LazyLock<tokio::sync::watch::Sender<bool>> =
    LazyLock::new(|| tokio::sync::watch::channel(false).0);

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_activityVisibilityChanged(
    _env: EnvUnowned,
    _class: JClass,
    visible: bool,
) {
    let changed = ACTIVITY_VISIBLE.send_if_modified(|old| {
        let changed = *old != visible;
        *old = visible;
        changed
    });
    if changed && visible {
        post(Update::Resumed);
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct DeviceName {
    pub(crate) custom: String,
    pub(crate) system: String,
}

impl DeviceName {
    pub(crate) fn effective(&self) -> String {
        if self.custom.is_empty() {
            &self.system
        } else {
            &self.custom
        }
        .to_owned()
    }
}

pub(crate) static DEVICE_NAME: LazyLock<tokio::sync::watch::Sender<DeviceName>> =
    LazyLock::new(|| {
        tokio::sync::watch::channel(DeviceName {
            custom: String::new(),
            system: "Android".into(),
        })
        .0
    });

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_deviceNameChanged(
    mut env: EnvUnowned,
    _class: JClass,
    name: JString,
    model: JString,
) {
    env.with_env(|_| {
        let clean = |value: String| {
            let mut text: String = value.chars().filter(|c| !c.is_control()).collect();
            text.truncate(text.floor_char_boundary(256));
            text.trim().to_owned()
        };
        let system = clean(model.to_string());
        let name = DeviceName {
            custom: clean(name.to_string()),
            system: if system.is_empty() {
                "Android".into()
            } else {
                system
            },
        };
        if DEVICE_NAME.send_if_modified(|current| {
            if *current == name {
                return false;
            }
            *current = name;
            true
        }) {
            post(Update::DeviceNameChanged);
        }
        Ok::<_, jni::errors::Error>(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

// Android dispatches hardware keys on the Activity thread, before GPUI or the IME.
pub(crate) static HARDWARE_CONTROLS: LazyLock<Mutex<TerminalControls>> =
    LazyLock::new(Mutex::default);
pub(crate) static BRIDGE: Mutex<Option<Sender<Update>>> = Mutex::new(None);
pub(crate) static SYSTEM_UI_REM: Mutex<f32> = Mutex::new(16.);
pub(crate) static EDGES: Mutex<[f32; 4]> = Mutex::new([0.; 4]);
pub(crate) static CAN_GO_BACK: AtomicBool = AtomicBool::new(false);

pub(crate) fn post(event: Update) {
    if let Some(tx) = BRIDGE.lock().unwrap().as_ref() {
        let _ = tx.try_send(event);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_initVerifier(
    mut env: EnvUnowned,
    _class: JClass,
    app: JObject,
) {
    env.with_env(|env| rustls_platform_verifier::android::init_with_env(env, app))
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PairingScannerActivity_isInvitation(
    mut env: EnvUnowned,
    _class: JClass,
    code: JString,
) -> bool {
    env.with_env(|_| {
        let valid = paxri_native::gateway::pairing::Invitation::parse(&code.to_string()).is_ok();
        Ok::<_, jni::errors::Error>(valid)
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_renameSession(
    mut env: EnvUnowned,
    _class: JClass,
    peer: JString,
    number: i64,
    nonce: i64,
    name: JString,
) {
    env.with_env(|_| {
        post(Update::RenameSession(
            peer.to_string(),
            SessionKey {
                number: number as usize,
                nonce: nonce as u64,
            },
            name.to_string(),
        ));
        Ok::<_, jni::errors::Error>(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_unpairDevice(
    mut env: EnvUnowned,
    _class: JClass,
    id: JString,
    generation: JString,
) {
    env.with_env(|_| {
        let generation = generation.to_string();
        post(Update::Unpair {
            id: id.to_string(),
            generation: (!generation.is_empty()).then_some(generation),
        });
        Ok::<_, jni::errors::Error>(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_removeForward(
    mut env: EnvUnowned,
    _class: JClass,
    owner: JString,
    name: JString,
) {
    env.with_env(|_| {
        post(Update::RemoveForward {
            owner: owner.to_string(),
            name: name.to_string(),
        });
        Ok::<_, jni::errors::Error>(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_submitCode(
    mut env: EnvUnowned,
    _class: JClass,
    code: JString,
) {
    env.with_env(|_| {
        post(Update::PairCode(code.to_string()));
        Ok::<_, jni::errors::Error>(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_composingTextChanged(
    mut env: EnvUnowned,
    _class: JClass,
    id: i64,
    text: JString,
    start: i32,
    end: i32,
) {
    env.with_env(|_| {
        post(Update::Composing {
            id: id as u64,
            text: text.to_string(),
            selection: start.max(0) as usize..end.max(0) as usize,
        });
        Ok::<_, jni::errors::Error>(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_systemTextSizeChanged(
    _env: EnvUnowned,
    _class: JClass,
    size: f32,
) {
    if size.is_finite() && size > 0. {
        *SYSTEM_UI_REM.lock().unwrap() = size;
        post(Update::DisplayChanged);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_insetsChanged(
    _env: EnvUnowned,
    _class: JClass,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
) {
    *EDGES.lock().unwrap() = [left, top, right, bottom];
    post(Update::DisplayChanged);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_terminalShortcut(
    mut env: EnvUnowned,
    activity: JObject,
    session: i64,
    modifiers: i32,
    key: JString,
    has_selection: bool,
) -> bool {
    env.with_env(|env| {
        let name = key.to_string();
        let bits = modifiers as u8;
        let key = Keystroke {
            modifiers: Modifiers {
                control: bits & CTRL != 0,
                alt: bits & ALT != 0,
                shift: bits & SHIFT != 0,
                platform: bits & META != 0,
                ..Default::default()
            },
            key: name.to_ascii_lowercase(),
            key_char: (name.len() == 1).then_some(name),
        };
        let action = keys::local_action(&key, &HARDWARE_CONTROLS.lock().unwrap());
        let local = match action {
            Some(TerminalAction::Copy) => has_selection,
            Some(_) => true,
            None => false,
        };
        // Route PTY control bytes before the focused Android editor can consume them.
        if !local
            && ((action.is_none() && !key.modifiers.control) || keys::sequence(&key, false).is_none())
        {
            return Ok(false);
        }
        if action == Some(TerminalAction::Paste) {
            env.call_method(
                &activity,
                jni_str!("finishTerminalComposition"),
                jni_sig!("()V"),
                &[],
            )?;
        }
        post(Update::Shortcut(session as u64, key, action));
        Ok::<_, jni::errors::Error>(true)
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_goBack(
    _env: EnvUnowned,
    _class: JClass,
) -> bool {
    let open = CAN_GO_BACK.load(Ordering::Acquire);
    if open {
        post(Update::Close);
    }
    open
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_terminalKey(
    mut env: EnvUnowned,
    _class: JClass,
    session: i64,
    text: JString,
    named: bool,
    first: i32,
    held: i32,
) {
    let modifiers = |bits: i32| Modifiers {
        control: bits & 1 != 0,
        alt: bits & 2 != 0,
        shift: bits & 4 != 0,
        ..Default::default()
    };
    env.with_env(|_| {
        post(Update::Key {
            id: session as u64,
            value: text.to_string(),
            named,
            first: modifiers(first),
            held: modifiers(held),
        });
        Ok::<_, jni::errors::Error>(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

#[derive(Clone, Copy)]
pub(crate) enum ActivityAction<'a> {
    Toast(&'a str),
    RenameSession(&'a str, SessionKey, &'a str),
    ConfirmRemoveForward {
        owner: &'a str,
        name: &'a str,
        route: &'a str,
    },
    ConfirmUnpair {
        id: &'a str,
        generation: &'a str,
        name: &'a str,
    },
    SaveDeviceName(&'a str),
    SaveZoom(f32),
    Pair,
    Scan,
    PairingReady,
    DeviceConnections {
        paired: bool,
        connected: bool,
    },
    HardwareText(u64, &'a str),
    BackEnabled(bool),
    Appearance(bool, &'a theme::Palette),
    TouchArea(u64, [i32; 5], &'a KeybarSettings, bool),
    SelectionHandles(u64, [i32; 5], bool),
    StopFling(u64),
    ShowTerminalKeyboard(u64),
}

pub(crate) fn load_terminal_zoom(app: &AndroidApp) -> f32 {
    gpui_mobile::android::jni::with_env(|env| {
        let raw = app.activity_as_ptr().cast();
        let activity =
            unsafe { env.as_cast_raw::<Global<JObject>>(&raw) }.map_err(|e| e.to_string())?;
        env.call_method(activity, jni_str!("terminalZoom"), jni_sig!("()F"), &[])
            .and_then(|value| value.f())
            .map_err(|e| e.to_string())
    })
    .unwrap_or(1.)
}

pub(crate) fn activity_action(app: &AndroidApp, action: ActivityAction<'_>) -> Result<(), String> {
    gpui_mobile::android::jni::with_env(|env| {
        let raw = app.activity_as_ptr().cast();
        // NativeActivity owns this global reference for the lifetime of AndroidApp.
        let activity =
            unsafe { env.as_cast_raw::<Global<JObject>>(&raw) }.map_err(|e| e.to_string())?;
        match action {
            ActivityAction::DeviceConnections { paired, connected } => env.call_method(
                activity,
                jni_str!("setDeviceConnections"),
                jni_sig!("(ZZ)V"),
                &[paired.into(), connected.into()],
            ),
            ActivityAction::Toast(message) => {
                let text = JString::from_str(env, message).map_err(|e| e.to_string())?;
                env.call_method(
                    activity,
                    jni_str!("showToast"),
                    jni_sig!("(Ljava/lang/String;)V"),
                    &[(&text).into()],
                )
            }
            ActivityAction::RenameSession(peer, key, title) => {
                let peer = JString::from_str(env, peer).map_err(|e| e.to_string())?;
                let title = JString::from_str(env, title).map_err(|e| e.to_string())?;
                env.call_method(
                    activity,
                    jni_str!("requestSessionName"),
                    jni_sig!("(Ljava/lang/String;JJLjava/lang/String;)V"),
                    &[
                        (&peer).into(),
                        (key.number as i64).into(),
                        (key.nonce as i64).into(),
                        (&title).into(),
                    ],
                )
            }
            ActivityAction::ConfirmRemoveForward { owner, name, route } => {
                let owner = JString::from_str(env, owner).map_err(|e| e.to_string())?;
                let name = JString::from_str(env, name).map_err(|e| e.to_string())?;
                let route = JString::from_str(env, route).map_err(|e| e.to_string())?;
                env.call_method(
                    activity,
                    jni_str!("confirmRemoveForward"),
                    jni_sig!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V"),
                    &[(&owner).into(), (&name).into(), (&route).into()],
                )
            }
            ActivityAction::ConfirmUnpair {
                id,
                generation,
                name,
            } => {
                let id = JString::from_str(env, id).map_err(|e| e.to_string())?;
                let generation = JString::from_str(env, generation).map_err(|e| e.to_string())?;
                let name = JString::from_str(env, name).map_err(|e| e.to_string())?;
                env.call_method(
                    activity,
                    jni_str!("confirmUnpair"),
                    jni_sig!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V"),
                    &[(&id).into(), (&generation).into(), (&name).into()],
                )
            }
            ActivityAction::SaveZoom(scale) => env.call_method(
                activity,
                jni_str!("saveTerminalZoom"),
                jni_sig!("(F)V"),
                &[scale.into()],
            ),
            ActivityAction::SaveDeviceName(name) => {
                let value = JString::from_str(env, name).map_err(|e| e.to_string())?;
                env.call_method(
                    activity,
                    jni_str!("saveDeviceName"),
                    jni_sig!("(Ljava/lang/String;)V"),
                    &[(&value).into()],
                )
            }
            ActivityAction::HardwareText(id, key) => {
                let text = JString::from_str(env, key).map_err(|e| e.to_string())?;
                env.call_method(
                    activity,
                    jni_str!("terminalHardwareText"),
                    jni_sig!("(JLjava/lang/String;)V"),
                    &[(id as i64).into(), (&text).into()],
                )
            }
            ActivityAction::BackEnabled(enabled) => env.call_method(
                activity,
                jni_str!("setBackEnabled"),
                jni_sig!("(Z)V"),
                &[enabled.into()],
            ),
            ActivityAction::Appearance(dark_icons, colors) => {
                let argb = |rgb: u32| ((0xff000000 | rgb) as i32).into();
                env.call_method(
                    activity,
                    jni_str!("setChromeAppearance"),
                    jni_sig!("(ZIIIII)V"),
                    &[
                        dark_icons.into(),
                        argb(colors.border),
                        argb(colors.secondary.background),
                        argb(colors.input_border),
                        argb(colors.secondary.foreground),
                        argb(colors.terminal.ansi[4]),
                    ],
                )
            }
            ActivityAction::SelectionHandles(id, [x1, y1, x2, y2, h], connected) => env
                .call_method(
                    activity,
                    jni_str!("setSelectionHandles"),
                    jni_sig!("(JIIIIIZ)V"),
                    &[
                        (id as i64).into(),
                        x1.into(),
                        y1.into(),
                        x2.into(),
                        y2.into(),
                        h.into(),
                        connected.into(),
                    ],
                ),
            ActivityAction::StopFling(id) => env.call_method(
                activity,
                jni_str!("stopTerminalFling"),
                jni_sig!("(J)V"),
                &[(id as i64).into()],
            ),
            ActivityAction::ShowTerminalKeyboard(id) => env.call_method(
                activity,
                jni_str!("showTerminalKeyboard"),
                jni_sig!("(J)V"),
                &[(id as i64).into()],
            ),
            ActivityAction::TouchArea(id, [x, y, width, height, toolbar], keys, touchable) => {
                let rows: Vec<Vec<_>> = keys
                    .rows
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|key| (key.as_str(), paxri_config::keybar::label(key)))
                            .collect()
                    })
                    .collect();
                let layout = serde_json::json!({
                    "rows": rows,
                    "row_height": keys.row_height,
                    "haptics": keys.haptics,
                })
                .to_string();
                let layout = JString::from_str(env, layout).map_err(|e| e.to_string())?;
                env.call_method(
                    activity,
                    jni_str!("setTerminalTouchArea"),
                    jni_sig!("(JIIIIILjava/lang/String;Z)V"),
                    &[
                        (id as i64).into(),
                        x.into(),
                        y.into(),
                        width.into(),
                        height.into(),
                        toolbar.into(),
                        (&layout).into(),
                        touchable.into(),
                    ],
                )
            }
            ActivityAction::Pair => env.call_method(
                activity,
                jni_str!("requestPairingCode"),
                jni_sig!("()V"),
                &[],
            ),
            ActivityAction::Scan => {
                env.call_method(activity, jni_str!("scanPairingCode"), jni_sig!("()V"), &[])
            }
            ActivityAction::PairingReady => env.call_method(
                activity,
                jni_str!("pairingClientReady"),
                jni_sig!("()V"),
                &[],
            ),
        }
        .map(|_| ())
        .map_err(|e| e.to_string())
    })
}
