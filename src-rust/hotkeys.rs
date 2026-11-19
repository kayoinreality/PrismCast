use crate::protocol::{HotkeyAction, HotkeyBindings};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ParsedAccelerator {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    pub vk: u32,
    pub canonical: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyConflict {
    pub accelerator: String,
    pub actions: Vec<HotkeyAction>,
}

/// Faz parse de uma string de acelerador (suporta formato Electron `CommandOrControl+Shift+M`,
/// `Ctrl+Alt+F1` ou `e.code` do DOM como `KeyM`, `Digit1`, `Numpad0`, `Space`, `ArrowUp`).
pub fn parse_accelerator(raw: &str) -> Result<ParsedAccelerator, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Atalho vazio".to_string());
    }

    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut meta = false;
    let mut key_token: Option<&str> = None;

    for part in trimmed.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "commandorcontrol" | "cmdorctrl" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            "meta" | "super" | "cmd" | "command" | "win" => meta = true,
            _ => {
                if key_token.is_some() {
                    return Err(format!("Mais de uma tecla principal em '{trimmed}'"));
                }
                key_token = Some(part);
            }
        }
    }

    let key_str = key_token.ok_or_else(|| format!("Nenhuma tecla principal em '{trimmed}'"))?;
    if !(ctrl || alt || shift || meta) {
        return Err(format!(
            "Atalho '{trimmed}' precisa de pelo menos um modificador (Ctrl, Alt ou Shift)"
        ));
    }

    let (vk, norm_key) = parse_key_vk(key_str)
        .ok_or_else(|| format!("Tecla desconhecida '{key_str}' em '{trimmed}'"))?;

    let mut parts = Vec::with_capacity(5);
    if ctrl {
        parts.push("CommandOrControl");
    }
    if alt {
        parts.push("Alt");
    }
    if shift {
        parts.push("Shift");
    }
    if meta {
        parts.push("Super");
    }
    parts.push(norm_key.as_str());

    Ok(ParsedAccelerator {
        ctrl,
        alt,
        shift,
        meta,
        vk,
        canonical: parts.join("+"),
    })
}

fn parse_key_vk(raw: &str) -> Option<(u32, String)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // e.code: KeyA..KeyZ
    if let Some(rest) = trimmed
        .strip_prefix("Key")
        .or_else(|| trimmed.strip_prefix("key"))
    {
        if rest.len() == 1 {
            let ch = rest.chars().next()?.to_ascii_uppercase();
            if ch.is_ascii_alphabetic() {
                return Some((ch as u32, ch.to_string()));
            }
        }
    }

    // e.code: Digit0..Digit9
    if let Some(rest) = trimmed
        .strip_prefix("Digit")
        .or_else(|| trimmed.strip_prefix("digit"))
    {
        if rest.len() == 1 {
            let ch = rest.chars().next()?;
            if ch.is_ascii_digit() {
                return Some((ch as u32, ch.to_string()));
            }
        }
    }

    // e.code: Numpad0..Numpad9
    if let Some(rest) = trimmed
        .strip_prefix("Numpad")
        .or_else(|| trimmed.strip_prefix("numpad"))
    {
        if rest.len() == 1 {
            let ch = rest.chars().next()?;
            if ch.is_ascii_digit() {
                let n = ch as u32 - '0' as u32;
                return Some((0x60 + n, format!("Numpad{n}")));
            }
        }
    }

    // Unico caractere A-Z ou 0-9
    if trimmed.len() == 1 {
        let ch = trimmed.chars().next()?.to_ascii_uppercase();
        if ch.is_ascii_alphanumeric() {
            return Some((ch as u32, ch.to_string()));
        }
    }

    let upper = trimmed.to_ascii_uppercase();
    // F1..F24
    if let Some(num_str) = upper.strip_prefix('F') {
        if let Ok(n) = num_str.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some((0x70 + (n - 1), format!("F{n}")));
            }
        }
    }

    let mapped = match upper.as_str() {
        "SPACE" | " " => (0x20, "Space"),
        "ENTER" | "RETURN" => (0x0D, "Return"),
        "TAB" => (0x09, "Tab"),
        "BACKSPACE" => (0x08, "Backspace"),
        "DELETE" | "DEL" => (0x2E, "Delete"),
        "INSERT" | "INS" => (0x2D, "Insert"),
        "HOME" => (0x24, "Home"),
        "END" => (0x23, "End"),
        "PAGEUP" => (0x21, "PageUp"),
        "PAGEDOWN" => (0x22, "PageDown"),
        "UP" | "ARROWUP" => (0x26, "Up"),
        "DOWN" | "ARROWDOWN" => (0x28, "Down"),
        "LEFT" | "ARROWLEFT" => (0x25, "Left"),
        "RIGHT" | "ARROWRIGHT" => (0x27, "Right"),
        "COMMA" | "," => (0xBC, "Comma"),
        "PERIOD" | "." => (0xBE, "Period"),
        "SLASH" | "/" => (0xBF, "Slash"),
        "MINUS" | "-" => (0xBD, "Minus"),
        "EQUAL" | "=" | "PLUS" => (0xBB, "Equal"),
        "BRACKETLEFT" | "[" => (0xDB, "BracketLeft"),
        "BRACKETRIGHT" | "]" => (0xDD, "BracketRight"),
        "BACKSLASH" | "\\" => (0xDC, "Backslash"),
        "SEMICOLON" | ";" => (0xBA, "Semicolon"),
        "QUOTE" | "'" => (0xDE, "Quote"),
        "BACKQUOTE" | "`" => (0xC0, "Backquote"),
        "PAUSE" => (0x13, "Pause"),
        "PRINTSCREEN" => (0x2C, "PrintScreen"),
        "NUMPADADD" => (0x6B, "NumpadAdd"),
        "NUMPADSUBTRACT" => (0x6D, "NumpadSubtract"),
        "NUMPADMULTIPLY" => (0x6A, "NumpadMultiply"),
        "NUMPADDIVIDE" => (0x6F, "NumpadDivide"),
        "NUMPADDECIMAL" => (0x6E, "NumpadDecimal"),
        _ => return None,
    };
    Some((mapped.0, mapped.1.to_string()))
}

pub fn bindings_entries(bindings: &HotkeyBindings) -> [(HotkeyAction, &str); 6] {
    [
        (HotkeyAction::ToggleMicMute, bindings.toggle_mic_mute.as_str()),
        (HotkeyAction::ToggleSystemMute, bindings.toggle_system_mute.as_str()),
        (
            HotkeyAction::TogglePrivacyShield,
            bindings.toggle_privacy_shield.as_str(),
        ),
        (HotkeyAction::SaveReplayClip, bindings.save_replay_clip.as_str()),
        (
            HotkeyAction::ToggleVerticalPreview,
            bindings.toggle_vertical_preview.as_str(),
        ),
        (HotkeyAction::ToggleCamera, bindings.toggle_camera.as_str()),
    ]
}

/// Detecta atalhos duplicados entre as acoes configuradas em `HotkeyBindings`.
pub fn detect_conflicts(bindings: &HotkeyBindings) -> Vec<HotkeyConflict> {
    let mut by_canonical: HashMap<String, Vec<HotkeyAction>> = HashMap::new();
    for (action, raw) in bindings_entries(bindings) {
        if raw.trim().is_empty() {
            continue;
        }
        if let Ok(parsed) = parse_accelerator(raw) {
            by_canonical
                .entry(parsed.canonical)
                .or_default()
                .push(action);
        }
    }
    let mut conflicts: Vec<HotkeyConflict> = by_canonical
        .into_iter()
        .filter(|(_, actions)| actions.len() > 1)
        .map(|(accelerator, actions)| HotkeyConflict {
            accelerator,
            actions,
        })
        .collect();
    conflicts.sort_by(|a, b| a.accelerator.cmp(&b.accelerator));
    conflicts
}

enum HotkeyCmd {
    Update(HotkeyBindings),
    Stop,
}

/// Gerenciador dinamico de atalhos globais nativos ligados a `HotkeyBindings`.
/// No Windows usa `RegisterHotKey`/`UnregisterHotKey` + `GetMessageW` (`WM_HOTKEY`).
pub struct HotkeyManager {
    cmd_tx: Option<mpsc::UnboundedSender<HotkeyCmd>>,
    #[cfg(target_os = "windows")]
    thread_id: Arc<Mutex<Option<u32>>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl HotkeyManager {
    pub fn spawn<F>(initial: HotkeyBindings, on_hotkey: F) -> Self
    where
        F: Fn(HotkeyAction) + Send + Sync + 'static,
    {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        #[cfg(target_os = "windows")]
        let thread_id = Arc::new(Mutex::new(None));
        #[cfg(target_os = "windows")]
        let tid_ref = thread_id.clone();

        let worker = std::thread::Builder::new()
            .name("prismcast-hotkeys".to_string())
            .spawn(move || {
                #[cfg(target_os = "windows")]
                run_windows_hotkey_loop(initial, cmd_rx, tid_ref, on_hotkey);
                #[cfg(not(target_os = "windows"))]
                run_noop_loop(initial, cmd_rx, on_hotkey);
            })
            .ok();

        Self {
            cmd_tx: Some(cmd_tx),
            #[cfg(target_os = "windows")]
            thread_id,
            worker,
        }
    }

    pub fn update_bindings(&self, bindings: HotkeyBindings) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(HotkeyCmd::Update(bindings));
            self.wake_loop();
        }
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(HotkeyCmd::Stop);
            self.wake_loop();
        }
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }

    #[cfg(target_os = "windows")]
    fn wake_loop(&self) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};

        if let Some(tid) = *self.thread_id.lock().unwrap() {
            unsafe {
                let _ = PostThreadMessageW(tid, WM_APP + 1, WPARAM(0), LPARAM(0));
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn wake_loop(&self) {}
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Compatibilidade com o chamador legado em `main.rs` antes da integracao da Fase 5.
pub fn spawn_global_hotkey_listener<F>(on_hotkey: F) -> HotkeyManager
where
    F: Fn(HotkeyAction) + Send + Sync + 'static,
{
    HotkeyManager::spawn(HotkeyBindings::default(), on_hotkey)
}

#[cfg(target_os = "windows")]
fn run_windows_hotkey_loop<F>(
    initial: HotkeyBindings,
    mut cmd_rx: mpsc::UnboundedReceiver<HotkeyCmd>,
    tid_ref: Arc<Mutex<Option<u32>>>,
    on_hotkey: F,
) where
    F: Fn(HotkeyAction) + Send + Sync + 'static,
{
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
        MOD_SHIFT, MOD_WIN,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMessageW, PeekMessageW, MSG, PM_NOREMOVE, WM_APP, WM_HOTKEY, WM_QUIT,
    };

    unsafe {
        // Garante criacao da fila de mensagens da thread antes de expor o tid.
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, Some(HWND::default()), 0, 0, PM_NOREMOVE);
        *tid_ref.lock().unwrap() = Some(GetCurrentThreadId());
    }

    let mut registered: HashMap<i32, HotkeyAction> = HashMap::new();

    let unregister_all = |reg: &mut HashMap<i32, HotkeyAction>| {
        for id in reg.keys() {
            unsafe {
                let _ = UnregisterHotKey(None, *id);
            }
        }
        reg.clear();
    };

    let apply_bindings = |bindings: &HotkeyBindings, reg: &mut HashMap<i32, HotkeyAction>| {
        unregister_all(reg);
        let mut next_id = 1i32;
        for (action, raw) in bindings_entries(bindings) {
            if raw.trim().is_empty() {
                continue;
            }
            let Ok(parsed) = parse_accelerator(raw) else {
                continue;
            };
            let mut mods = MOD_NOREPEAT;
            if parsed.ctrl {
                mods |= MOD_CONTROL;
            }
            if parsed.alt {
                mods |= MOD_ALT;
            }
            if parsed.shift {
                mods |= MOD_SHIFT;
            }
            if parsed.meta {
                mods |= MOD_WIN;
            }
            let id = next_id;
            next_id += 1;
            let ok = unsafe {
                RegisterHotKey(None, id, HOT_KEY_MODIFIERS(mods.0), parsed.vk).is_ok()
            };
            if ok {
                reg.insert(id, action);
            }
        }
    };

    apply_bindings(&initial, &mut registered);

    let mut msg = MSG::default();
    loop {
        let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if ret.0 <= 0 {
            break;
        }
        if msg.message == WM_HOTKEY {
            let id = msg.wParam.0 as i32;
            if let Some(action) = registered.get(&id).copied() {
                on_hotkey(action);
            }
        } else if msg.message == WM_APP + 1 {
            let mut should_stop = false;
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    HotkeyCmd::Update(bindings) => apply_bindings(&bindings, &mut registered),
                    HotkeyCmd::Stop => should_stop = true,
                }
            }
            if should_stop {
                break;
            }
        } else if msg.message == WM_QUIT {
            break;
        }
    }

    unregister_all(&mut registered);
    *tid_ref.lock().unwrap() = None;
}

#[cfg(not(target_os = "windows"))]
fn run_noop_loop<F>(
    _initial: HotkeyBindings,
    mut cmd_rx: mpsc::UnboundedReceiver<HotkeyCmd>,
    _on_hotkey: F,
) where
    F: Fn(HotkeyAction) + Send + Sync + 'static,
{
    while let Some(cmd) = cmd_rx.blocking_recv() {
        if matches!(cmd, HotkeyCmd::Stop) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_accelerator_standard_and_dom_code() {
        let p1 = parse_accelerator("CommandOrControl+Shift+M").unwrap();
        assert!(p1.ctrl && p1.shift && !p1.alt && !p1.meta);
        assert_eq!(p1.vk, 0x4D);
        assert_eq!(p1.canonical, "CommandOrControl+Shift+M");

        // e.code em teclados ABNT2 (KeyM, Digit1, F9, ArrowUp)
        let p2 = parse_accelerator("Ctrl+Alt+KeyM").unwrap();
        assert_eq!(p2.vk, 0x4D);
        assert_eq!(p2.canonical, "CommandOrControl+Alt+M");

        let p3 = parse_accelerator("Ctrl+Shift+Digit1").unwrap();
        assert_eq!(p3.vk, 0x31);
        assert_eq!(p3.canonical, "CommandOrControl+Shift+1");

        let p4 = parse_accelerator("Alt+F10").unwrap();
        assert_eq!(p4.vk, 0x79);
        assert_eq!(p4.canonical, "Alt+F10");

        let p5 = parse_accelerator("Shift+ArrowUp").unwrap();
        assert_eq!(p5.vk, 0x26);
        assert_eq!(p5.canonical, "Shift+Up");
    }

    #[test]
    fn test_parse_accelerator_rejects_missing_modifier_or_key() {
        assert!(parse_accelerator("M").is_err());
        assert!(parse_accelerator("Ctrl+Shift").is_err());
        assert!(parse_accelerator("").is_err());
        assert!(parse_accelerator("Ctrl+UnknownKeyXYZ").is_err());
    }

    #[test]
    fn test_detect_conflicts_finds_normalized_duplicates() {
        let mut bindings = HotkeyBindings::default();
        assert!(detect_conflicts(&bindings).is_empty());

        bindings.toggle_camera = "Ctrl+Shift+KeyM".to_string();
        let conflicts = detect_conflicts(&bindings);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].accelerator, "CommandOrControl+Shift+M");
        assert!(conflicts[0].actions.contains(&HotkeyAction::ToggleMicMute));
        assert!(conflicts[0].actions.contains(&HotkeyAction::ToggleCamera));
    }
}
