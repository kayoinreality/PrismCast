use std::time::Duration;
use tokio::task::JoinHandle;

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(vkey: i32) -> i16;
}

#[cfg(target_os = "windows")]
fn is_pressed(vkey: i32) -> bool {
    unsafe { (GetAsyncKeyState(vkey) as u16 & 0x8000) != 0 }
}

#[cfg(not(target_os = "windows"))]
fn is_pressed(_vkey: i32) -> bool {
    false
}

const VK_CONTROL: i32 = 0x11;
const VK_SHIFT: i32 = 0x10;
const VK_M: i32 = 0x4D;
const VK_D: i32 = 0x44;
const VK_P: i32 = 0x50;
const VK_S: i32 = 0x53;
const VK_V: i32 = 0x56;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    ToggleMicMute,
    ToggleSystemMute,
    TogglePrivacyShield,
    SaveReplayClip,
    ToggleVerticalPreview,
}

impl HotkeyAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            HotkeyAction::ToggleMicMute => "toggle-mic-mute",
            HotkeyAction::ToggleSystemMute => "toggle-system-mute",
            HotkeyAction::TogglePrivacyShield => "toggle-privacy-shield",
            HotkeyAction::SaveReplayClip => "save-replay-clip",
            HotkeyAction::ToggleVerticalPreview => "toggle-vertical-preview",
        }
    }
}

/// Listener nativo de atalhos globais (inspirado no `HotkeysService` + `node-libuiohook` do Streamlabs).
/// Funciona mesmo quando um jogo DirectX/Vulkan esta em foco.
pub fn spawn_global_hotkey_listener<F>(on_hotkey: F) -> JoinHandle<()>
where
    F: Fn(HotkeyAction) + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let mut prev_m = false;
        let mut prev_d = false;
        let mut prev_p = false;
        let mut prev_s = false;
        let mut prev_v = false;

        let mut interval = tokio::time::interval(Duration::from_millis(45));
        loop {
            interval.tick().await;
            let mods = is_pressed(VK_CONTROL) && is_pressed(VK_SHIFT);
            let m = mods && is_pressed(VK_M);
            let d = mods && is_pressed(VK_D);
            let p = mods && is_pressed(VK_P);
            let s = mods && is_pressed(VK_S);
            let v = mods && is_pressed(VK_V);

            if m && !prev_m {
                on_hotkey(HotkeyAction::ToggleMicMute);
            }
            if d && !prev_d {
                on_hotkey(HotkeyAction::ToggleSystemMute);
            }
            if p && !prev_p {
                on_hotkey(HotkeyAction::TogglePrivacyShield);
            }
            if s && !prev_s {
                on_hotkey(HotkeyAction::SaveReplayClip);
            }
            if v && !prev_v {
                on_hotkey(HotkeyAction::ToggleVerticalPreview);
            }

            prev_m = m;
            prev_d = d;
            prev_p = p;
            prev_s = s;
            prev_v = v;
        }
    })
}
