//! Desktop input simulation.
//!
//! - Windows: drives the real cursor and keyboard via `SendInput`.
//! - Other platforms: null backend that logs actions so the server and the
//!   phone protocol stay fully testable without a display.

#[cfg(windows)]
mod imp {
    use std::process::Command;
    use windows::Win32::System::Power::SetSuspendState;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    pub fn backend_name() -> &'static str {
        "sendinput"
    }

    fn send(inputs: &[INPUT]) {
        if inputs.is_empty() {
            return;
        }
        unsafe {
            SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }

    fn mouse(flags: MOUSE_EVENT_FLAGS, data: u32, dx: i32, dy: i32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn key(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS, scan: u16) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn tap_vk(vk: VIRTUAL_KEY, extended: bool) {
        let flags = if extended {
            KEYEVENTF_EXTENDEDKEY
        } else {
            KEYBD_EVENT_FLAGS(0)
        };
        send(&[
            key(vk, flags, 0),
            key(vk, flags | KEYEVENTF_KEYUP, 0),
        ]);
    }

    fn button_flags(button: &str) -> (MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS) {
        match button {
            "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
            _ => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        }
    }

    pub fn mouse_move(dx: f64, dy: f64) {
        send(&[mouse(
            MOUSEEVENTF_MOVE,
            0,
            dx.round() as i32,
            dy.round() as i32,
        )]);
    }

    pub fn mouse_click(button: &str, count: u32) {
        let (down, up) = button_flags(button);
        for _ in 0..count.max(1) {
            send(&[mouse(down, 0, 0, 0), mouse(up, 0, 0, 0)]);
        }
    }

    pub fn mouse_press(button: &str) {
        let (down, _) = button_flags(button);
        send(&[mouse(down, 0, 0, 0)]);
    }

    pub fn mouse_release(button: &str) {
        let (_, up) = button_flags(button);
        send(&[mouse(up, 0, 0, 0)]);
    }

    pub fn mouse_scroll(dx: f64, dy: f64) {
        const WHEEL_DELTA: f64 = 120.0;
        if dy != 0.0 {
            let delta = (dy * WHEEL_DELTA).round() as i32 as u32;
            send(&[mouse(MOUSEEVENTF_WHEEL, delta, 0, 0)]);
        }
        if dx != 0.0 {
            let delta = (dx * WHEEL_DELTA).round() as i32 as u32;
            send(&[mouse(MOUSEEVENTF_HWHEEL, delta, 0, 0)]);
        }
    }

    /// Type arbitrary text via KEYEVENTF_UNICODE (handles any Unicode,
    /// independent of the active keyboard layout).
    pub fn type_text(text: &str) {
        if text.is_empty() {
            return;
        }
        let mut inputs = Vec::with_capacity(text.len() * 2);
        for unit in text.encode_utf16() {
            inputs.push(key(VIRTUAL_KEY(0), KEYEVENTF_UNICODE, unit));
            inputs.push(key(
                VIRTUAL_KEY(0),
                KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                unit,
            ));
        }
        for chunk in inputs.chunks(512) {
            send(chunk);
        }
    }

    /// Map a friendly key name to a virtual-key code.
    /// Returns (vk, extended) — navigation keys are sent as extended keys.
    fn vk_for_name(name: &str) -> Option<(VIRTUAL_KEY, bool)> {
        let vk = match name.to_lowercase().as_str() {
            "enter" => (VK_RETURN, false),
            "backspace" => (VK_BACK, false),
            "tab" => (VK_TAB, false),
            "esc" | "escape" => (VK_ESCAPE, false),
            "space" => (VK_SPACE, false),
            "up" => (VK_UP, true),
            "down" => (VK_DOWN, true),
            "left" => (VK_LEFT, true),
            "right" => (VK_RIGHT, true),
            "delete" => (VK_DELETE, true),
            "home" => (VK_HOME, true),
            "end" => (VK_END, true),
            "pageup" => (VK_PRIOR, true),
            "pagedown" => (VK_NEXT, true),
            "insert" => (VK_INSERT, true),
            "capslock" => (VK_CAPITAL, false),
            "shift" => (VK_SHIFT, false),
            "ctrl" => (VK_CONTROL, false),
            "alt" => (VK_MENU, false),
            "super" => (VK_LWIN, true),
            "menu" => (VK_APPS, true),
            "f1" => (VK_F1, false),
            "f2" => (VK_F2, false),
            "f3" => (VK_F3, false),
            "f4" => (VK_F4, false),
            "f5" => (VK_F5, false),
            "f6" => (VK_F6, false),
            "f7" => (VK_F7, false),
            "f8" => (VK_F8, false),
            "f9" => (VK_F9, false),
            "f10" => (VK_F10, false),
            "f11" => (VK_F11, false),
            "f12" => (VK_F12, false),
            _ => return None,
        };
        Some(vk)
    }

    fn vk_for_media(action: &str) -> Option<VIRTUAL_KEY> {
        match action.to_lowercase().as_str() {
            "playpause" | "play" | "pause" => Some(VK_MEDIA_PLAY_PAUSE),
            "next" => Some(VK_MEDIA_NEXT_TRACK),
            "prev" | "previous" => Some(VK_MEDIA_PREV_TRACK),
            "stop" => Some(VK_MEDIA_STOP),
            "volup" | "volumeup" => Some(VK_VOLUME_UP),
            "voldn" | "volumedown" => Some(VK_VOLUME_DOWN),
            "mute" => Some(VK_VOLUME_MUTE),
            _ => None,
        }
    }

    pub fn press_key(name: &str) {
        if let Some((vk, extended)) = vk_for_name(name) {
            tap_vk(vk, extended);
        } else if name.chars().count() == 1 {
            type_text(name);
        } else {
            eprintln!("[openmouse] unknown key name: {name:?}");
        }
    }

    pub fn media(action: &str) {
        match vk_for_media(action) {
            Some(vk) => tap_vk(vk, true),
            None => eprintln!("[openmouse] unknown media action: {action:?}"),
        }
    }

    /// Press a chord, e.g. ["ctrl", "c"].
    pub fn hotkey(keys: &[String]) {
        let mut vks: Vec<(VIRTUAL_KEY, bool)> = Vec::new();
        for k in keys.iter().take(4) {
            if let Some(vk) = vk_for_name(k) {
                vks.push(vk);
            } else if k.chars().count() == 1 {
                if let Some(c) = k.chars().next() {
                    // Letters/digits map 1:1 onto their VK codes.
                    vks.push((VIRTUAL_KEY(c.to_ascii_uppercase() as u16), false));
                }
            }
        }
        if vks.is_empty() {
            return;
        }
        let mut inputs = Vec::with_capacity(vks.len() * 2);
        for (vk, extended) in &vks {
            let flags = if *extended {
                KEYEVENTF_EXTENDEDKEY
            } else {
                KEYBD_EVENT_FLAGS(0)
            };
            inputs.push(key(*vk, flags, 0));
        }
        for (vk, extended) in vks.iter().rev() {
            let flags = if *extended {
                KEYEVENTF_EXTENDEDKEY
            } else {
                KEYBD_EVENT_FLAGS(0)
            };
            inputs.push(key(*vk, flags | KEYEVENTF_KEYUP, 0));
        }
        send(&inputs);
    }

    pub fn power(action: &str) {
        match action.to_lowercase().as_str() {
            "sleep" => unsafe {
                SetSuspendState(false, false, false);
            },
            "shutdown" => {
                let _ = Command::new("shutdown")
                    .args(["/s", "/t", "5", "/c", "OpenMouse remote shutdown"])
                    .spawn();
            }
            "restart" => {
                let _ = Command::new("shutdown")
                    .args(["/r", "/t", "5", "/c", "OpenMouse remote restart"])
                    .spawn();
            }
            "logoff" => {
                let _ = Command::new("shutdown").arg("/l").spawn();
            }
            _ => eprintln!("[openmouse] unknown power action: {action:?}"),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn backend_name() -> &'static str {
        "null"
    }
    fn log(action: &str, detail: &str) {
        eprintln!("[openmouse] [null-input] {action} {detail}");
    }
    pub fn mouse_move(dx: f64, dy: f64) {
        log("move", &format!("{dx},{dy}"));
    }
    pub fn mouse_click(button: &str, count: u32) {
        log("click", &format!("{button} x{count}"));
    }
    pub fn mouse_press(button: &str) {
        log("press", button);
    }
    pub fn mouse_release(button: &str) {
        log("release", button);
    }
    pub fn mouse_scroll(dx: f64, dy: f64) {
        log("scroll", &format!("{dx},{dy}"));
    }
    pub fn type_text(text: &str) {
        log("type_text", &format!("{text:?}"));
    }
    pub fn press_key(name: &str) {
        log("press_key", name);
    }
    pub fn media(action: &str) {
        log("media", action);
    }
    pub fn hotkey(keys: &[String]) {
        log("hotkey", &keys.join("+"));
    }
    pub fn power(action: &str) {
        log("power", action);
    }
}

pub use imp::*;
