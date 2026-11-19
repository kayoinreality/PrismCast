//! Persistência de configurações (`AppSettings` Schema v3) + Cofre de Segredos nativo.
//!
//! - Deep-merge recursivo em `update()` para patches parciais não apagarem sub-objetos.
//! - Escrita atômica (`.tmp` + `sync_all` + `rename`).
//! - Migração v2 -> v3 sem re-injetar `audio_input` após remoção intencional.
//! - `SecretVault`: DPAPI (`CryptProtectData`/`CryptUnprotectData`) no Windows e
//!   `keyring` (com fallback arquivo `0o600`) no Linux/macOS.

use crate::protocol::{
    AppSettings, AudioChannel, AudioChannelKind, AudioFilters, AudioMixerSettings,
    CanvasOrientation, Destination, MultistreamTarget, PlatformId, Scene, SceneItem,
    SourceSettings, SourceTransform, SourceType, StudioCompositorSettings, StudioScenesState,
    PRISMCAST_TWITCH_CLIENT_ID, SCHEMA_VERSION,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const SETTINGS_FILE: &str = "prismcast-settings.json";
const LEGACY_SETTINGS_FILE: &str = "settings.json";
const SECRETS_FILE: &str = "prismcast-secrets.json";

const DEMO_SCENE_IDS: &[&str] = &[
    "scene-gameplay-cam",
    "scene-just-chatting",
    "scene-starting",
    "scene-brb",
    "scene-ending",
];

pub fn stream_key_secret(destination_id: &str) -> String {
    format!("streamKey:{destination_id}")
}

pub fn twitch_refresh_secret(destination_id: &str) -> String {
    format!("twitchRefresh:{destination_id}")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("data.json");
    let tmp = path.with_file_name(format!("{file_name}.tmp"));

    let mut file = File::create(&tmp).map_err(|e| format!("create tmp: {e}"))?;
    file.write_all(bytes).map_err(|e| format!("write tmp: {e}"))?;
    file.sync_all().map_err(|e| format!("sync tmp: {e}"))?;
    drop(file);

    #[cfg(unix)]
    if file_name == SECRETS_FILE {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }

    fs::rename(&tmp, path).map_err(|e| format!("rename tmp: {e}"))
}

// ------------------------------------------------------------------ SecretVault

pub struct SecretVault {
    secrets_path: PathBuf,
    cache: Mutex<HashMap<String, String>>,
}

impl SecretVault {
    pub fn new(base_dir: &Path) -> Self {
        let secrets_path = base_dir.join(SECRETS_FILE);
        let initial = fs::read_to_string(&secrets_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<HashMap<String, String>>(&raw).ok())
            .unwrap_or_default();
        Self {
            secrets_path,
            cache: Mutex::new(initial),
        }
    }

    pub fn get(&self, name: &str) -> Option<String> {
        #[cfg(not(windows))]
        {
            if let Ok(entry) = keyring::Entry::new("prismcast-studio", name) {
                if let Ok(secret) = entry.get_password() {
                    if !secret.is_empty() {
                        return Some(secret);
                    }
                }
            }
        }

        let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let raw = guard.get(name)?;
        if let Some(b64) = raw.strip_prefix("dpapi:") {
            let bytes = BASE64.decode(b64).ok()?;
            return dpapi_unprotect(&bytes);
        }
        raw.strip_prefix("raw:").map(str::to_string)
    }

    pub fn set(&self, name: &str, value: Option<&str>) -> Result<(), String> {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match value.filter(|v| !v.is_empty()) {
            None => {
                guard.remove(name);
                #[cfg(not(windows))]
                if let Ok(entry) = keyring::Entry::new("prismcast-studio", name) {
                    let _ = entry.delete_credential();
                }
            }
            Some(plain) => {
                #[cfg(windows)]
                {
                    let encrypted = dpapi_protect(plain.as_bytes())?;
                    guard.insert(name.to_string(), format!("dpapi:{}", BASE64.encode(encrypted)));
                }
                #[cfg(not(windows))]
                {
                    let saved_in_keyring = keyring::Entry::new("prismcast-studio", name)
                        .and_then(|entry| entry.set_password(plain))
                        .is_ok();
                    if saved_in_keyring {
                        guard.remove(name);
                    } else {
                        guard.insert(name.to_string(), format!("raw:{plain}"));
                    }
                }
            }
        }

        let serialized =
            serde_json::to_vec_pretty(&*guard).map_err(|e| format!("serialize secrets: {e}"))?;
        atomic_write(&self.secrets_path, &serialized)
    }
}

#[cfg(windows)]
fn dpapi_protect(plain: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut input = CRYPT_INTEGER_BLOB {
        cbData: plain.len() as u32,
        pbData: plain.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &mut input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|e| format!("CryptProtectData: {e}"))?;
        let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let vec = slice.to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData as *mut core::ffi::c_void)));
        Ok(vec)
    }
}

#[cfg(windows)]
fn dpapi_unprotect(cipher: &[u8]) -> Option<String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut input = CRYPT_INTEGER_BLOB {
        cbData: cipher.len() as u32,
        pbData: cipher.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &mut input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .ok()?;
        let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let text = String::from_utf8(slice.to_vec()).ok();
        let _ = LocalFree(Some(HLOCAL(output.pbData as *mut core::ffi::c_void)));
        text
    }
}

#[cfg(not(windows))]
fn dpapi_unprotect(_cipher: &[u8]) -> Option<String> {
    None
}

// ------------------------------------------------------------------ Defaults & Migração

pub fn make_destination(platform: PlatformId, id: Option<&str>) -> Destination {
    let (default_id, name, server, orientation, bitrate_kbps) = match platform {
        PlatformId::Twitch => (
            "twitch-main",
            "Twitch",
            "rtmp://live.twitch.tv/app",
            CanvasOrientation::Horizontal,
            6000,
        ),
        PlatformId::Youtube => (
            "youtube-main",
            "YouTube",
            "rtmp://a.rtmp.youtube.com/live2",
            CanvasOrientation::Horizontal,
            9000,
        ),
        PlatformId::Kick => (
            "kick-main",
            "Kick",
            "rtmps://fa723fc1b171.global-contribute.live-video.net/app",
            CanvasOrientation::Horizontal,
            8000,
        ),
        PlatformId::Tiktok => (
            "tiktok-vertical",
            "TikTok",
            "",
            CanvasOrientation::Vertical,
            4500,
        ),
        PlatformId::Custom => (
            "custom-main",
            "RTMP personalizado",
            "",
            CanvasOrientation::Horizontal,
            6000,
        ),
    };
    Destination {
        id: id.unwrap_or(default_id).to_string(),
        platform,
        name: name.to_string(),
        enabled: false,
        server: server.to_string(),
        has_key: false,
        orientation,
        bitrate_kbps,
        account: None,
    }
}

pub fn default_destinations() -> Vec<Destination> {
    vec![
        make_destination(PlatformId::Twitch, Some("twitch-main")),
        make_destination(PlatformId::Youtube, Some("youtube-main")),
        make_destination(PlatformId::Kick, Some("kick-main")),
        make_destination(PlatformId::Tiktok, Some("tiktok-main")),
    ]
}

fn default_main_scene(include_mic: bool) -> Scene {
    let mut items = vec![SceneItem {
        id: "item-display".to_string(),
        source_id: "display".to_string(),
        name: "Tela principal".to_string(),
        source_type: SourceType::ScreenCapture,
        visible: true,
        locked: false,
        transform: SourceTransform {
            z_index: 1,
            ..Default::default()
        },
        vertical_transform: SourceTransform {
            z_index: 1,
            ..Default::default()
        },
        show_on_horizontal: true,
        show_on_vertical: true,
        settings: SourceSettings::default(),
    }];

    if include_mic {
        items.push(SceneItem {
            id: "item-mic".to_string(),
            source_id: "mic".to_string(),
            name: "Microfone".to_string(),
            source_type: SourceType::AudioInput,
            visible: true,
            locked: true,
            transform: SourceTransform {
                z_index: 2,
                ..Default::default()
            },
            vertical_transform: SourceTransform {
                z_index: 2,
                ..Default::default()
            },
            show_on_horizontal: true,
            show_on_vertical: true,
            settings: SourceSettings {
                audio_channel_id: Some("mic".to_string()),
                ..Default::default()
            },
        });
    }

    Scene {
        id: "scene-main".to_string(),
        name: "Cena principal".to_string(),
        items,
        created_at: now_ms(),
    }
}

pub fn default_scenes_state() -> StudioScenesState {
    let main = default_main_scene(true);
    StudioScenesState {
        active_scene_id: main.id.clone(),
        preview_scene_id: main.id.clone(),
        scenes: vec![main],
        ..Default::default()
    }
}

pub fn fresh_settings() -> AppSettings {
    let mut s = AppSettings {
        destinations: default_destinations(),
        scenes: default_scenes_state(),
        ..Default::default()
    };
    s.normalize();
    s
}

fn is_fake_v2_key(key: &str) -> bool {
    if key.starts_with("pk_live_local") {
        return true;
    }
    if let Some(rest) = key.strip_prefix("live_") {
        for prefix in ["twitch_", "youtube_", "kick_", "tiktok_", "prismcast_"] {
            if let Some(suffix) = rest.strip_prefix(prefix) {
                return (4..=10).contains(&suffix.len())
                    && suffix
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
            }
        }
    }
    false
}

fn linear_to_db(v: f32) -> f32 {
    if v <= 0.001 {
        -60.0
    } else {
        (20.0 * v.log10()).clamp(-60.0, 12.0)
    }
}

fn migrate_v2_json(raw: &mut Map<String, Value>, vault: &SecretVault) -> bool {
    let ver = raw
        .get("schemaVersion")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    if ver >= SCHEMA_VERSION {
        return false;
    }

    // 1. Destinos legados (`multistreamTargets` -> `destinations` + `SecretVault`)
    if !raw.contains_key("destinations") {
        let mut dests = default_destinations();
        if let Some(targets) = raw.get("multistreamTargets").and_then(|v| v.as_array()) {
            for t in targets {
                let provider = t.get("provider").and_then(|v| v.as_str()).unwrap_or("");
                let target_platform = match provider {
                    "twitch" => PlatformId::Twitch,
                    "youtube" => PlatformId::Youtube,
                    "kick" => PlatformId::Kick,
                    "tiktok" => PlatformId::Tiktok,
                    _ => continue,
                };
                if let Some(dest) = dests.iter_mut().find(|d| d.platform == target_platform) {
                    let key = t
                        .get("streamKey")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    if !key.is_empty() && !is_fake_v2_key(key) {
                        let _ = vault.set(&stream_key_secret(&dest.id), Some(key));
                        dest.has_key = true;
                        dest.enabled = t
                            .get("enabled")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                    }
                    if let Some(url) = t.get("rtmpUrl").and_then(|v| v.as_str()) {
                        if !url.trim().is_empty() {
                            dest.server = url.trim().to_string();
                        }
                    }
                }
            }
        }
        if let Ok(val) = serde_json::to_value(dests) {
            raw.insert("destinations".to_string(), val);
        }
    }

    // 2. Mixer legado da 2.2 (sem `channels`)
    if let Some(mixer_obj) = raw.get_mut("audioMixer").and_then(|v| v.as_object_mut()) {
        if !mixer_obj.contains_key("channels") {
            let mut channels = Vec::new();
            if mixer_obj
                .get("systemAudioEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true)
            {
                let vol = mixer_obj
                    .get("systemVolume")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0) as f32;
                let muted = mixer_obj
                    .get("systemMuted")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                channels.push(AudioChannel {
                    id: "desktop".to_string(),
                    kind: AudioChannelKind::Desktop,
                    name: "Áudio do PC".to_string(),
                    volume_db: linear_to_db(vol),
                    muted,
                    filters: AudioFilters::default_for(AudioChannelKind::Desktop),
                    ..Default::default()
                });
            }
            let mic_vol = mixer_obj
                .get("micVolume")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0) as f32;
            let mic_muted = mixer_obj
                .get("micMuted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || mixer_obj
                    .get("micEnabled")
                    .and_then(|v| v.as_bool())
                    .map(|b| !b)
                    .unwrap_or(false);
            let mut mic_filters = AudioFilters::default_for(AudioChannelKind::Mic);
            if let Some(ns) = mixer_obj.get("noiseSuppression").and_then(|v| v.as_bool()) {
                mic_filters.noise_suppression = ns;
            }
            if let Some(ec) = mixer_obj.get("echoCancellation").and_then(|v| v.as_bool()) {
                mic_filters.echo_cancellation = ec;
            }
            if let Some(comp) = mixer_obj.get("compressorEnabled").and_then(|v| v.as_bool()) {
                mic_filters.compressor.enabled = comp;
            }
            channels.push(AudioChannel {
                id: "mic".to_string(),
                kind: AudioChannelKind::Mic,
                name: "Microfone".to_string(),
                device_id: mixer_obj
                    .get("micDeviceId")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                volume_db: linear_to_db(mic_vol),
                muted: mic_muted,
                filters: mic_filters,
                ..Default::default()
            });

            let party = mixer_obj
                .get("partyVoiceEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if let Ok(val) = serde_json::to_value(AudioMixerSettings {
                channels,
                party_voice_enabled: party,
                monitor_device_id: None,
            }) {
                raw.insert("audioMixer".to_string(), val);
            }
        }
    }

    // 3. Cenas v2 -> v3: remove placeholders de demo e injeta audio_input uma única vez
    let demo_set: HashSet<&str> = DEMO_SCENE_IDS.iter().copied().collect();
    let mut scenes_state = default_scenes_state();
    if let Some(scenes_arr) = raw
        .get("scenes")
        .and_then(|s| s.get("scenes"))
        .and_then(|a| a.as_array())
    {
        for s_val in scenes_arr {
            if let Ok(mut scene) = serde_json::from_value::<Scene>(s_val.clone()) {
                if demo_set.contains(scene.id.as_str()) {
                    continue;
                }
                scene.items.retain(|it| {
                    it.source_type != SourceType::AudioInput
                        && it.source_type != SourceType::AudioOutput
                });
                if !scene.name.to_lowercase().contains("volto")
                    && !scene.name.to_lowercase().contains("brb")
                {
                    scene.items.push(SceneItem {
                        id: format!("item-mic-{}", scene.id),
                        source_id: "mic".to_string(),
                        name: "Microfone".to_string(),
                        source_type: SourceType::AudioInput,
                        visible: true,
                        locked: true,
                        transform: SourceTransform {
                            z_index: (scene.items.len() + 1) as i32,
                            ..Default::default()
                        },
                        vertical_transform: SourceTransform {
                            z_index: (scene.items.len() + 1) as i32,
                            ..Default::default()
                        },
                        show_on_horizontal: true,
                        show_on_vertical: true,
                        settings: SourceSettings {
                            audio_channel_id: Some("mic".to_string()),
                            ..Default::default()
                        },
                    });
                }
                scenes_state.scenes.push(scene);
            }
        }
    }
    if let Ok(val) = serde_json::to_value(scenes_state) {
        raw.insert("scenes".to_string(), val);
    }

    // 4. Studio legado
    if let Some(studio_val) = raw.get("studio").cloned() {
        let mut studio = StudioCompositorSettings::default();
        if let Some(b) = studio_val.get("dualOutputEnabled").and_then(|v| v.as_bool()) {
            studio.dual_output_enabled = b;
        }
        if let Some(sec) = studio_val.get("replayBufferSeconds").and_then(|v| v.as_u64()) {
            studio.replay_buffer_seconds = sec as u32;
        }
        if let Ok(val) = serde_json::to_value(studio) {
            raw.insert("studio".to_string(), val);
        }
    }

    raw.remove("multistreamTargets");
    raw.remove("widgets");
    raw.insert("schemaVersion".to_string(), Value::from(SCHEMA_VERSION));
    true
}

fn validate_and_normalize(mut settings: AppSettings, vault: &SecretVault) -> AppSettings {
    if settings.destinations.is_empty() {
        settings.destinations = default_destinations();
    } else {
        for dest in &mut settings.destinations {
            dest.has_key = vault.get(&stream_key_secret(&dest.id)).is_some();
        }
    }

    if settings.scenes.scenes.is_empty() {
        settings.scenes = default_scenes_state();
    } else {
        // Garante unicidade de cenas válidas e refs consistentes sem re-injetar audio_input!
        let valid_ids: HashSet<String> = settings
            .scenes
            .scenes
            .iter()
            .map(|s| s.id.clone())
            .collect();
        let fallback_id = settings.scenes.scenes[0].id.clone();
        if !valid_ids.contains(&settings.scenes.active_scene_id) {
            settings.scenes.active_scene_id = fallback_id.clone();
        }
        if !valid_ids.contains(&settings.scenes.preview_scene_id) {
            settings.scenes.preview_scene_id = settings.scenes.active_scene_id.clone();
        }
    }

    if settings.twitch_client_id.trim() == PRISMCAST_TWITCH_CLIENT_ID {
        settings.twitch_client_id.clear();
    }

    settings.normalize();
    settings
}

pub fn deep_merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(t_map), Value::Object(p_map)) => {
            for (k, v) in p_map {
                if v.is_object() {
                    if let Some(existing) = t_map.get_mut(k) {
                        if existing.is_object() {
                            deep_merge(existing, v);
                            continue;
                        }
                    }
                }
                t_map.insert(k.clone(), v.clone());
            }
        }
        (t, p) => {
            *t = p.clone();
        }
    }
}

// ------------------------------------------------------------------ Snapshot compatível com main.rs legado

/// Visão serializável das preferências que preserva compatibilidade com o `main.rs` legado
/// enquanto o bridge RPC (`rpc.rs`) não é substituído.
#[derive(Debug, Clone)]
pub struct StoreSnapshot {
    pub settings: AppSettings,
    pub host_name: String,
    pub require_pin: bool,
    pub preferred_interface: Option<String>,
    pub multistream_targets: Vec<MultistreamTarget>,
}

impl Serialize for StoreSnapshot {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.settings.serialize(serializer)
    }
}

impl Deref for StoreSnapshot {
    type Target = AppSettings;
    fn deref(&self) -> &Self::Target {
        &self.settings
    }
}

// ------------------------------------------------------------------ SettingsStore

pub struct SettingsStore {
    path: PathBuf,
    vault: SecretVault,
    cached: Mutex<AppSettings>,
}

impl SettingsStore {
    pub fn new() -> Self {
        let base = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("PrismCast");
        Self::with_dir(base)
    }

    pub fn with_dir(base: PathBuf) -> Self {
        let path = base.join(SETTINGS_FILE);
        let legacy_path = base.join(LEGACY_SETTINGS_FILE);
        let vault = SecretVault::new(&base);

        let raw_text = fs::read_to_string(&path)
            .ok()
            .or_else(|| fs::read_to_string(&legacy_path).ok());

        let (initial, needs_persist) = match raw_text
            .as_deref()
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
        {
            Some(Value::Object(mut map)) => {
                let migrated = migrate_v2_json(&mut map, &vault);
                let parsed = serde_json::from_value::<AppSettings>(Value::Object(map))
                    .unwrap_or_else(|_| fresh_settings());
                (validate_and_normalize(parsed, &vault), migrated || !path.exists())
            }
            _ => (fresh_settings(), false),
        };

        if needs_persist {
            if let Ok(bytes) = serde_json::to_vec_pretty(&initial) {
                let _ = atomic_write(&path, &bytes);
            }
        }

        Self {
            path,
            vault,
            cached: Mutex::new(initial),
        }
    }

    pub fn snapshot(&self) -> AppSettings {
        self.cached
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn apply_patch(&self, patch: Value) -> AppSettings {
        self.apply_patch_inner(patch, false)
    }

    fn apply_patch_inner(&self, patch: Value, from_internal: bool) -> AppSettings {
        let mut guard = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        let prev = guard.clone();

        if let Ok(mut current_val) = serde_json::to_value(&*guard) {
            deep_merge(&mut current_val, &patch);
            if let Ok(mut updated) = serde_json::from_value::<AppSettings>(current_val) {
                if !from_internal {
                    // `hasKey` e `account` são autoridade do motor Rust / cofre.
                    for d in &mut updated.destinations {
                        d.has_key = self.vault.get(&stream_key_secret(&d.id)).is_some();
                        if let Some(old) = prev.destinations.iter().find(|p| p.id == d.id) {
                            d.account = old.account.clone();
                        }
                    }
                }
                *guard = validate_and_normalize(updated, &self.vault);
            }
        }

        if let Ok(bytes) = serde_json::to_vec_pretty(&*guard) {
            let _ = atomic_write(&self.path, &bytes);
        }
        guard.clone()
    }

    pub fn update_destination<F>(&self, id: &str, mutator: F) -> AppSettings
    where
        F: FnOnce(&mut Destination),
    {
        let mut guard = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(dest) = guard.destinations.iter_mut().find(|d| d.id == id) {
            mutator(dest);
        }
        let next = validate_and_normalize(guard.clone(), &self.vault);
        *guard = next.clone();
        if let Ok(bytes) = serde_json::to_vec_pretty(&*guard) {
            let _ = atomic_write(&self.path, &bytes);
        }
        next
    }

    pub fn get_secret(&self, name: &str) -> Option<String> {
        self.vault.get(name)
    }

    pub fn set_secret(&self, name: &str, value: Option<&str>) -> Result<(), String> {
        self.vault.set(name, value)
    }

    /// Retorna targets RTMP com chaves resolvidas do cofre para o motor Multistream.
    pub fn resolved_targets(&self) -> Vec<MultistreamTarget> {
        let settings = self.snapshot();
        settings
            .destinations
            .into_iter()
            .filter_map(|d| {
                let key = self.vault.get(&stream_key_secret(&d.id))?;
                Some(MultistreamTarget {
                    id: d.id,
                    provider: d.platform,
                    name: d.name,
                    enabled: d.enabled,
                    rtmpUrl: d.server,
                    streamKey: key,
                    orientation: d.orientation,
                    bitrateKbps: d.bitrate_kbps,
                })
            })
            .collect()
    }

    fn to_snapshot(&self, settings: AppSettings) -> StoreSnapshot {
        let targets = settings
            .destinations
            .iter()
            .filter_map(|d| {
                let key = self.vault.get(&stream_key_secret(&d.id))?;
                Some(MultistreamTarget {
                    id: d.id.clone(),
                    provider: d.platform,
                    name: d.name.clone(),
                    enabled: d.enabled,
                    rtmpUrl: d.server.clone(),
                    streamKey: key,
                    orientation: d.orientation,
                    bitrateKbps: d.bitrate_kbps,
                })
            })
            .collect();
        StoreSnapshot {
            host_name: settings.host_name.clone(),
            require_pin: settings.require_pin,
            preferred_interface: settings.preferred_interface.clone(),
            multistream_targets: targets,
            settings,
        }
    }

    pub fn get(&self) -> StoreSnapshot {
        self.to_snapshot(self.snapshot())
    }

    pub fn update(&self, patch: Value) -> StoreSnapshot {
        let next = self.apply_patch(patch);
        self.to_snapshot(next)
    }
}

impl Default for SettingsStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_test_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "prismcast-store-test-{}-{}-{}",
            tag,
            std::process::id(),
            now_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_deep_merge_preserves_nested_fields() {
        let dir = temp_test_dir("deep-merge");
        let store = SettingsStore::with_dir(dir.clone());

        let initial = store.snapshot();
        assert_eq!(initial.quality.max_height, Some(1080));
        assert_eq!(initial.quality.fps, 30);

        let updated = store.apply_patch(json!({
            "quality": { "fps": 60 },
            "studio": { "dualOutputEnabled": true }
        }));

        assert_eq!(updated.quality.fps, 60);
        assert_eq!(updated.quality.max_height, Some(1080));
        assert_eq!(updated.quality.max_bitrate_kbps, 8000);
        assert!(updated.studio.dual_output_enabled);
        assert_eq!(updated.studio.replay_buffer_seconds, 30);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_v2_to_v3_migration_and_vault_roundtrip() {
        let dir = temp_test_dir("migration");
        let v2_json = json!({
            "hostName": "Studio-V2",
            "multistreamTargets": [
                {
                    "provider": "twitch",
                    "enabled": true,
                    "rtmpUrl": "rtmp://live.twitch.tv/app",
                    "streamKey": "live_123456_realkeyABCDEF123456"
                },
                {
                    "provider": "youtube",
                    "enabled": true,
                    "rtmpUrl": "rtmp://a.rtmp.youtube.com/live2",
                    "streamKey": "live_youtube_fake12"
                }
            ],
            "audioMixer": {
                "systemAudioEnabled": true,
                "systemVolume": 0.5,
                "micEnabled": true,
                "compressorEnabled": false
            },
            "scenes": {
                "scenes": [
                    { "id": "scene-gameplay-cam", "name": "Demo", "items": [] },
                    { "id": "user-scene-1", "name": "Minha Cena", "items": [] }
                ]
            }
        });
        fs::write(
            dir.join(SETTINGS_FILE),
            serde_json::to_string_pretty(&v2_json).unwrap(),
        )
        .unwrap();

        let store = SettingsStore::with_dir(dir.clone());
        let s = store.snapshot();
        assert_eq!(s.schema_version, SCHEMA_VERSION);

        let twitch = s
            .destinations
            .iter()
            .find(|d| d.platform == PlatformId::Twitch)
            .unwrap();
        assert!(twitch.has_key);
        assert!(twitch.enabled);
        assert_eq!(
            store.get_secret(&stream_key_secret(&twitch.id)).as_deref(),
            Some("live_123456_realkeyABCDEF123456")
        );

        let yt = s
            .destinations
            .iter()
            .find(|d| d.platform == PlatformId::Youtube)
            .unwrap();
        assert!(!yt.has_key);
        assert!(!yt.enabled);

        // Cena de demo removida, cena do usuário mantida
        assert!(s.scenes.scenes.iter().all(|sc| sc.id != "scene-gameplay-cam"));
        assert!(s.scenes.scenes.iter().any(|sc| sc.id == "user-scene-1"));

        // Garante que nenhuma streamKey foi gravada em texto plano no settings.json
        let disk_settings = fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(!disk_settings.contains("live_123456_realkeyABCDEF123456"));
        assert!(!dir.join("prismcast-settings.json.tmp").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_no_audio_input_reinjection_after_intentional_removal() {
        let dir = temp_test_dir("no-reinject");
        {
            let store = SettingsStore::with_dir(dir.clone());
            let mut scenes = store.snapshot().scenes;
            scenes.scenes[0]
                .items
                .retain(|it| it.source_type != SourceType::AudioInput);
            store.apply_patch(json!({ "scenes": scenes }));
        }

        // Recarrega do disco na versão 3: não deve re-injetar microfone
        let reloaded = SettingsStore::with_dir(dir.clone()).snapshot();
        assert!(reloaded.scenes.scenes[0]
            .items
            .iter()
            .all(|it| it.source_type != SourceType::AudioInput));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vault_roundtrip_and_delete() {
        let dir = temp_test_dir("vault");
        let store = SettingsStore::with_dir(dir.clone());
        store
            .set_secret("streamKey:twitch-main", Some("live_999_secretXYZ"))
            .unwrap();
        assert_eq!(
            store.get_secret("streamKey:twitch-main").as_deref(),
            Some("live_999_secretXYZ")
        );

        store.set_secret("streamKey:twitch-main", None).unwrap();
        assert_eq!(store.get_secret("streamKey:twitch-main"), None);

        let _ = fs::remove_dir_all(&dir);
    }
}
