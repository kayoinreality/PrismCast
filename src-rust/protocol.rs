#![allow(non_snake_case)]
//! Fonte única da verdade para contratos de rede, IPC e Schema v3 do PrismCast Studio.
//!
//! Todos os structs de configuração e estado usam `#[serde(default)]` e
//! `#[serde(rename_all = "camelCase")]` para compatibilidade retroativa com
//! JSONs existentes do frontend e persistência segura entre versões.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const PROTOCOL_VERSION: u32 = 1;
pub const SCHEMA_VERSION: u32 = 3;

pub const DISCOVERY_PORT: u16 = 41234;
pub const SIGNALING_PORT: u16 = 41235;
pub const RUST_RPC_PORT: u16 = 41236;

pub const ANNOUNCE_INTERVAL_MS: u64 = 2000;
pub const HOST_TTL_MS: u64 = 6000;
pub const MAX_VIEWERS: usize = 4;

pub const PIN_MAX_ATTEMPTS: u32 = 5;
pub const PIN_BLOCK_MS: u64 = 60_000;

pub const MIN_DB: f32 = -60.0;
pub const MAX_DB: f32 = 12.0;

pub const PRISMCAST_TWITCH_CLIENT_ID: &str = "xsohreda00eufw5ifqsxtakmgwyys6";

// ------------------------------------------------------------------ Qualidade & Presets

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DegradationPreference {
    Balanced,
    MaintainFramerate,
    #[default]
    MaintainResolution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    #[default]
    Auto,
    H264,
    Vp9,
    Av1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PresetId {
    Leve,
    #[default]
    Filme,
    Jogo,
    Maxima,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct QualitySettings {
    pub max_height: Option<u32>,
    pub fps: u32,
    pub max_bitrate_kbps: u32,
    pub degradation: DegradationPreference,
    pub codec: VideoCodec,
    pub audio: bool,
}

impl Default for QualitySettings {
    fn default() -> Self {
        Self {
            max_height: Some(1080),
            fps: 30,
            max_bitrate_kbps: 8000,
            degradation: DegradationPreference::MaintainResolution,
            codec: VideoCodec::Auto,
            audio: true,
        }
    }
}

impl QualitySettings {
    pub fn normalize(&mut self) {
        self.fps = match self.fps {
            0..=20 => 15,
            21..=27 => 24,
            28..=39 => 30,
            40..=54 => 48,
            _ => 60,
        };
        self.max_bitrate_kbps = self.max_bitrate_kbps.clamp(1000, 40_000);
        if let Some(h) = self.max_height {
            self.max_height = Some(h.clamp(360, 4320));
        }
    }
}

// ------------------------------------------------------------------ Plataformas & Destinos

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    #[default]
    Screen,
    Window,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CanvasOrientation {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PlatformId {
    #[default]
    Twitch,
    Youtube,
    Kick,
    Tiktok,
    #[serde(alias = "prismcast")]
    Custom,
}

/// Alias mantido para compatibilidade com módulos legados (`store.rs` / `multistream.rs`).
pub type StreamProvider = PlatformId;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct PlatformAccount {
    pub login: String,
    pub display_name: String,
    pub avatar_url: String,
    pub id: Option<String>,
    pub scopes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct LiveInfo {
    pub title: String,
    pub category_id: String,
    pub category_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct TwitchCategory {
    pub id: String,
    pub name: String,
    pub box_art_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct TwitchCapabilities {
    pub linked: bool,
    pub login: Option<String>,
    pub can_edit_info: bool,
    pub can_chat: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TwitchActionResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_relogin: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "step", rename_all = "lowercase")]
pub enum TwitchLoginState {
    #[default]
    Idle,
    #[serde(rename_all = "camelCase")]
    Code {
        user_code: String,
        verification_uri: String,
        expires_at: u64,
    },
    Done {
        account: PlatformAccount,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Destination {
    pub id: String,
    pub platform: PlatformId,
    pub name: String,
    pub enabled: bool,
    pub server: String,
    pub has_key: bool,
    pub orientation: CanvasOrientation,
    pub bitrate_kbps: u32,
    pub account: Option<PlatformAccount>,
}

impl Destination {
    pub fn normalize(&mut self) {
        self.bitrate_kbps = self.bitrate_kbps.clamp(1000, 25_000);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct DestinationStatus {
    pub id: String,
    pub state: String, // "idle" | "connecting" | "live" | "reconnecting" | "error"
    pub uptime_sec: u64,
    pub bytes_sent: u64,
    pub bitrate_kbps: u32,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct MultistreamEngineStatus {
    pub active: bool,
    pub encoder: String,
    pub ffmpeg_found: bool,
    pub horizontal_live: bool,
    pub vertical_live: bool,
    pub destinations: Vec<DestinationStatus>,
}

/// Estrutura usada pelo `multistream.rs` atual (mantida até refatoração para `Destination`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct MultistreamTarget {
    pub id: String,
    pub provider: StreamProvider,
    pub name: String,
    pub enabled: bool,
    pub rtmpUrl: String,
    pub streamKey: String,
    pub orientation: CanvasOrientation,
    pub bitrateKbps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct MultistreamTargetStatus {
    pub id: String,
    pub provider: StreamProvider,
    pub name: String,
    pub orientation: CanvasOrientation,
    pub state: String, // "idle" | "connecting" | "live" | "error"
    pub uptimeSec: u64,
    pub bytesSent: u64,
    pub lastError: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct MultistreamStatus {
    pub active: bool,
    pub encoder: String,
    pub horizontalLive: bool,
    pub verticalLive: bool,
    pub targets: Vec<MultistreamTargetStatus>,
}

// ------------------------------------------------------------------ Widgets Legados (store.rs v2 compat)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum WidgetKind {
    #[default]
    StreamElements,
    AlertBox,
    ChatOverlay,
    GoalBar,
    ViewerCounter,
    LowerThird,
    CustomUrl,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct StreamWidget {
    pub id: String,
    pub name: String,
    pub kind: WidgetKind,
    pub enabled: bool,
    pub url: String,
    pub primaryText: String,
    pub secondaryText: String,
    pub accentColor: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub verticalX: f32,
    pub verticalY: f32,
    pub showOnHorizontal: bool,
    pub showOnVertical: bool,
    pub opacity: f32,
}

// ------------------------------------------------------------------ Cenas, Fontes & Transições

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    #[default]
    ScreenCapture,
    WindowCapture,
    CameraInput,
    BrowserSource,
    ImageSource,
    ColorSource,
    TextSource,
    AudioInput,
    AudioOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct SourceSettings {
    pub capture_id: Option<String>,
    pub capture_name: Option<String>,
    pub device_id: Option<String>,
    pub device_label: Option<String>,
    pub url: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<u32>,
    pub kit_widget: Option<String>,
    pub path: Option<String>,
    pub color: Option<String>,
    pub text: Option<String>,
    pub font_size: Option<u32>,
    pub rounded: Option<bool>,
    pub restart_on_show: Option<bool>,
    pub audio_channel_id: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SourceTransform {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub z_index: i32,
    pub crop_top: Option<f32>,
    pub crop_bottom: Option<f32>,
    pub crop_left: Option<f32>,
    pub crop_right: Option<f32>,
    pub flip_x: bool,
    pub opacity: f32,
}

impl Default for SourceTransform {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            z_index: 0,
            crop_top: None,
            crop_bottom: None,
            crop_left: None,
            crop_right: None,
            flip_x: false,
            opacity: 1.0,
        }
    }
}

impl SourceTransform {
    pub fn normalize(&mut self) {
        self.width = self.width.clamp(0.02, 4.0);
        self.height = self.height.clamp(0.02, 4.0);
        self.x = self.x.clamp(-2.0, 2.0);
        self.y = self.y.clamp(-2.0, 2.0);
        self.opacity = self.opacity.clamp(0.0, 1.0);
        self.crop_top = self.crop_top.map(|v| v.clamp(0.0, 0.9));
        self.crop_bottom = self.crop_bottom.map(|v| v.clamp(0.0, 0.9));
        self.crop_left = self.crop_left.map(|v| v.clamp(0.0, 0.9));
        self.crop_right = self.crop_right.map(|v| v.clamp(0.0, 0.9));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TransitionType {
    Cut,
    #[default]
    Fade,
    FadeToBlack,
    Slide,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct SceneItem {
    pub id: String,
    pub source_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub source_type: SourceType,
    pub visible: bool,
    pub locked: bool,
    pub transform: SourceTransform,
    pub vertical_transform: SourceTransform,
    pub show_on_horizontal: bool,
    pub show_on_vertical: bool,
    pub settings: SourceSettings,
}

impl SceneItem {
    pub fn normalize(&mut self) {
        self.transform.normalize();
        self.vertical_transform.normalize();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Scene {
    pub id: String,
    pub name: String,
    pub items: Vec<SceneItem>,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StudioScenesState {
    pub scenes: Vec<Scene>,
    pub active_scene_id: String,
    pub preview_scene_id: String,
    pub studio_mode: bool,
    pub transition_type: TransitionType,
    pub transition_duration_ms: u32,
}

impl Default for StudioScenesState {
    fn default() -> Self {
        Self {
            scenes: Vec::new(),
            active_scene_id: String::new(),
            preview_scene_id: String::new(),
            studio_mode: false,
            transition_type: TransitionType::Fade,
            transition_duration_ms: 300,
        }
    }
}

impl StudioScenesState {
    pub fn normalize(&mut self) {
        self.transition_duration_ms = self.transition_duration_ms.clamp(50, 3000);
        for scene in &mut self.scenes {
            for item in &mut scene.items {
                item.normalize();
            }
        }
    }
}

// ------------------------------------------------------------------ Browser Sources & Kit de Widgets

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct BrowserSourceSpec {
    pub id: String,
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum KitRole {
    Scene,
    #[default]
    Overlay,
    Frame,
    Chat,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct KitParamOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum KitParam {
    #[serde(rename_all = "camelCase")]
    Text {
        key: String,
        label: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
        placeholder: String,
    },
    #[serde(rename_all = "camelCase")]
    Number {
        key: String,
        label: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
        placeholder: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        min: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Select {
        key: String,
        label: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
        options: Vec<KitParamOption>,
    },
    #[serde(rename_all = "camelCase")]
    Toggle {
        key: String,
        label: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
        on: String,
        off: String,
        default_on: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KitCameraRule {
    Hide(String),
    Param { param: String, off: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct KitOverlay {
    pub file: String,
    pub name: String,
    pub description: String,
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub role: KitRole,
    pub params: Vec<KitParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_on_show: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<KitCameraRule>,
}

pub type KitWidget = KitOverlay;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct KitSocialLink {
    pub rede: String,
    pub handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KitConfig {
    pub canal: String,
    pub nome: String,
    pub jogo: String,
    pub contagem: u32,
    pub agenda: String,
    pub pix: String,
    pub redes: Vec<KitSocialLink>,
    pub recados: Vec<String>,
    pub extra: HashMap<String, serde_json::Value>,
}

impl Default for KitConfig {
    fn default() -> Self {
        Self {
            canal: String::new(),
            nome: String::new(),
            jogo: String::new(),
            contagem: 5,
            agenda: String::new(),
            pix: String::new(),
            redes: Vec::new(),
            recados: Vec::new(),
            extra: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct KitInfo {
    pub path: String,
    pub name: String,
    pub widgets: Vec<KitOverlay>,
    pub has_stream_elements: bool,
}

pub type WidgetKit = KitInfo;

// ------------------------------------------------------------------ Áudio, DSP, EQ 3 Bandas & Ducking

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioGateSettings {
    pub enabled: bool,
    pub threshold_db: f32,
    pub release_ms: u32,
}

impl Default for AudioGateSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_db: -45.0,
            release_ms: 150,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioCompressorSettings {
    pub enabled: bool,
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_db: f32,
}

impl Default for AudioCompressorSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 6.0,
            release_ms: 150.0,
            makeup_db: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioLimiterSettings {
    pub enabled: bool,
    pub ceiling_db: f32,
}

impl Default for AudioLimiterSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            ceiling_db: -1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioFilters {
    pub noise_suppression: bool,
    pub echo_cancellation: bool,
    pub auto_gain: bool,
    pub highpass_hz: f32,
    pub gate: AudioGateSettings,
    pub compressor: AudioCompressorSettings,
    pub gain_db: f32,
    pub limiter: AudioLimiterSettings,
    // Equalizador 3 bandas (Fase 5)
    pub eq_enabled: bool,
    pub eq_low_db: f32,
    pub eq_mid_db: f32,
    pub eq_high_db: f32,
    // Audio Ducking (Fase 5)
    pub ducking_enabled: bool,
    pub ducking_source_id: Option<String>,
    pub ducking_amount_db: f32,
    pub ducking_release_ms: u32,
}

impl Default for AudioFilters {
    fn default() -> Self {
        Self {
            noise_suppression: false,
            echo_cancellation: false,
            auto_gain: false,
            highpass_hz: 0.0,
            gate: AudioGateSettings::default(),
            compressor: AudioCompressorSettings::default(),
            gain_db: 0.0,
            limiter: AudioLimiterSettings::default(),
            eq_enabled: false,
            eq_low_db: 0.0,
            eq_mid_db: 0.0,
            eq_high_db: 0.0,
            ducking_enabled: false,
            ducking_source_id: None,
            ducking_amount_db: -12.0,
            ducking_release_ms: 350,
        }
    }
}

impl AudioFilters {
    pub fn default_for(kind: AudioChannelKind) -> Self {
        let mut f = Self::default();
        if kind == AudioChannelKind::Mic {
            f.noise_suppression = true;
            f.echo_cancellation = true;
            f.highpass_hz = 85.0;
            f.compressor.enabled = true;
            f.limiter.enabled = true;
        }
        f
    }

    pub fn normalize(&mut self) {
        self.highpass_hz = self.highpass_hz.clamp(0.0, 500.0);
        self.gate.threshold_db = self.gate.threshold_db.clamp(-80.0, 0.0);
        self.gate.release_ms = self.gate.release_ms.clamp(10, 2000);
        self.compressor.threshold_db = self.compressor.threshold_db.clamp(-60.0, 0.0);
        self.compressor.ratio = self.compressor.ratio.clamp(1.0, 20.0);
        self.compressor.attack_ms = self.compressor.attack_ms.clamp(0.5, 250.0);
        self.compressor.release_ms = self.compressor.release_ms.clamp(10.0, 2000.0);
        self.compressor.makeup_db = self.compressor.makeup_db.clamp(-12.0, 24.0);
        self.gain_db = self.gain_db.clamp(-24.0, 24.0);
        self.limiter.ceiling_db = self.limiter.ceiling_db.clamp(-12.0, 0.0);
        self.eq_low_db = self.eq_low_db.clamp(-12.0, 12.0);
        self.eq_mid_db = self.eq_mid_db.clamp(-12.0, 12.0);
        self.eq_high_db = self.eq_high_db.clamp(-12.0, 12.0);
        self.ducking_amount_db = self.ducking_amount_db.clamp(-40.0, 0.0);
        self.ducking_release_ms = self.ducking_release_ms.clamp(50, 5000);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AudioChannelKind {
    #[default]
    Desktop,
    Mic,
    App,
    PartyVoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AudioMonitorMode {
    #[default]
    Off,
    Monitor,
    MonitorOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioChannel {
    pub id: String,
    pub kind: AudioChannelKind,
    pub name: String,
    pub device_id: Option<String>,
    pub app_exe: Option<String>,
    pub volume_db: f32,
    pub muted: bool,
    pub monitor: AudioMonitorMode,
    pub sync_offset_ms: i32,
    pub filters: AudioFilters,
}

impl AudioChannel {
    pub fn normalize(&mut self) {
        self.volume_db = self.volume_db.clamp(MIN_DB, MAX_DB);
        self.sync_offset_ms = self.sync_offset_ms.clamp(-2000, 2000);
        self.filters.normalize();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioMixerSettings {
    pub channels: Vec<AudioChannel>,
    pub party_voice_enabled: bool,
    pub monitor_device_id: Option<String>,
}

impl Default for AudioMixerSettings {
    fn default() -> Self {
        Self {
            channels: vec![
                AudioChannel {
                    id: "desktop".to_string(),
                    kind: AudioChannelKind::Desktop,
                    name: "Áudio do PC".to_string(),
                    filters: AudioFilters::default_for(AudioChannelKind::Desktop),
                    ..Default::default()
                },
                AudioChannel {
                    id: "mic".to_string(),
                    kind: AudioChannelKind::Mic,
                    name: "Microfone".to_string(),
                    filters: AudioFilters::default_for(AudioChannelKind::Mic),
                    ..Default::default()
                },
            ],
            party_voice_enabled: true,
            monitor_device_id: None,
        }
    }
}

impl AudioMixerSettings {
    pub fn normalize(&mut self) {
        for ch in &mut self.channels {
            ch.normalize();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioChannelStatus {
    pub id: String,
    pub state: String, // "ok" | "waiting" | "error" | "off"
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioApp {
    pub hwnd: isize,
    pub pid: u32,
    pub exe: String,
    pub title: String,
    pub audio: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioLevelSample {
    pub id: String,
    pub peak_db: f32,
    pub input_db: f32,
    pub muted_speech: bool,
}

// ------------------------------------------------------------------ Estúdio, Hotkeys, Telemetria & Gravador

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StudioCompositorSettings {
    pub camera_enabled: bool,
    pub dual_output_enabled: bool,
    pub fps: u32,
    pub privacy_shield_active: bool,
    pub privacy_shield_title: String,
    pub privacy_shield_subtitle: String,
    pub replay_buffer_seconds: u32,
}

impl Default for StudioCompositorSettings {
    fn default() -> Self {
        Self {
            camera_enabled: true,
            dual_output_enabled: false,
            fps: 60,
            privacy_shield_active: false,
            privacy_shield_title: "VOLTO JÁ".to_string(),
            privacy_shield_subtitle: "Só um minutinho".to_string(),
            replay_buffer_seconds: 30,
        }
    }
}

impl StudioCompositorSettings {
    pub fn normalize(&mut self) {
        self.fps = if self.fps <= 35 { 30 } else { 60 };
        self.replay_buffer_seconds = self.replay_buffer_seconds.clamp(10, 180);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HotkeyBindings {
    pub toggle_mic_mute: String,
    pub toggle_system_mute: String,
    pub toggle_privacy_shield: String,
    pub save_replay_clip: String,
    pub toggle_vertical_preview: String,
    pub toggle_camera: String,
}

impl Default for HotkeyBindings {
    fn default() -> Self {
        Self {
            toggle_mic_mute: "CommandOrControl+Shift+M".to_string(),
            toggle_system_mute: "CommandOrControl+Shift+D".to_string(),
            toggle_privacy_shield: "CommandOrControl+Shift+P".to_string(),
            save_replay_clip: "CommandOrControl+Shift+S".to_string(),
            toggle_vertical_preview: "CommandOrControl+Shift+V".to_string(),
            toggle_camera: "CommandOrControl+Shift+C".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HotkeyAction {
    ToggleMicMute,
    ToggleSystemMute,
    TogglePrivacyShield,
    SaveReplayClip,
    ToggleVerticalPreview,
    ToggleCamera,
}

impl HotkeyAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            HotkeyAction::ToggleMicMute => "toggle-mic-mute",
            HotkeyAction::ToggleSystemMute => "toggle-system-mute",
            HotkeyAction::TogglePrivacyShield => "toggle-privacy-shield",
            HotkeyAction::SaveReplayClip => "save-replay-clip",
            HotkeyAction::ToggleVerticalPreview => "toggle-vertical-preview",
            HotkeyAction::ToggleCamera => "toggle-camera",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct TelemetryStats {
    pub compositor_fps: f32,
    pub target_fps: u32,
    pub render_ms_p95: f32,
    pub dropped_frames: u64,
    pub encode_bitrate_kbps: u32,
    pub encoder: String,
    pub cpu_fallback: bool,
    pub mic_muted_speaking: bool,
    pub replay_buffered_seconds: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct RecorderStatus {
    pub recording: bool,
    pub recordingPath: Option<String>,
    pub recordingDurationSec: u64,
    pub replayBufferActive: bool,
    pub replayBufferedSeconds: f32,
    pub replaySavedPath: Option<String>,
}

// ------------------------------------------------------------------ Chat & Multichat (Twitch, YouTube, Kick)

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct WindowBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChatSettings {
    pub channel: String,
    pub youtube: String,
    pub kick: String,
    pub font_size: u32,
    pub show_timestamps: bool,
    pub popout_on_top: bool,
    pub popout_bounds: Option<WindowBounds>,
}

impl Default for ChatSettings {
    fn default() -> Self {
        Self {
            channel: String::new(),
            youtube: String::new(),
            kick: String::new(),
            font_size: 14,
            show_timestamps: false,
            popout_on_top: true,
            popout_bounds: None,
        }
    }
}

impl ChatSettings {
    pub fn normalize(&mut self) {
        self.font_size = self.font_size.clamp(10, 28);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChatPlatform {
    #[default]
    Twitch,
    Youtube,
    Kick,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum ChatFragment {
    Text {
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    Emote {
        url: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src_set: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MultiChatMessageKind {
    #[default]
    Msg,
    Event,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct MultiChatMessage {
    pub id: String,
    pub platform: ChatPlatform,
    pub kind: MultiChatMessageKind,
    pub user: String,
    pub name: String,
    pub color: String,
    pub badges: Vec<String>,
    pub fragments: Vec<ChatFragment>,
    pub ts: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChatSourceState {
    #[default]
    Off,
    Connecting,
    Connected,
    Waiting,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ChatSourceStatus {
    pub platform: ChatPlatform,
    pub state: ChatSourceState,
    pub channel: String,
    pub message: Option<String>,
}

pub type MultiChatStatus = ChatSourceStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum MultiChatEvent {
    Messages {
        messages: Vec<MultiChatMessage>,
    },
    Delete {
        platform: ChatPlatform,
        id: String,
    },
    Status {
        status: Vec<ChatSourceStatus>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct WidgetLiveState {
    pub title: String,
    pub game: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WidgetEvent {
    Live { t: String, live: WidgetLiveState },
    Chat(MultiChatEvent),
}

// ------------------------------------------------------------------ Preferências Globais (AppSettings Schema v3)

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub schema_version: u32,
    pub device_name: String,
    pub host_name: String,
    pub viewer_name: String,
    pub quality: QualitySettings,
    pub preset_id: PresetId,
    pub require_pin: bool,
    pub preferred_interface: Option<String>,
    pub firewall_prompt_dismissed: bool,
    pub audio_mixer: AudioMixerSettings,
    pub studio: StudioCompositorSettings,
    pub hotkeys: HotkeyBindings,
    pub destinations: Vec<Destination>,
    pub scenes: StudioScenesState,
    #[serde(alias = "live")]
    pub live_info: LiveInfo,
    pub chat: ChatSettings,
    pub kit_path: Option<String>,
    pub twitch_client_id: String,
    pub recording_dir: Option<String>,
    pub ffmpeg_path: Option<String>,
    pub chat_window_bounds: Option<WindowBounds>,
}

impl Default for AppSettings {
    fn default() -> Self {
        let name = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "PrismCast PC".to_string());
        Self {
            schema_version: SCHEMA_VERSION,
            device_name: name.clone(),
            host_name: name.clone(),
            viewer_name: name,
            quality: QualitySettings::default(),
            preset_id: PresetId::Filme,
            require_pin: true,
            preferred_interface: None,
            firewall_prompt_dismissed: false,
            audio_mixer: AudioMixerSettings::default(),
            studio: StudioCompositorSettings::default(),
            hotkeys: HotkeyBindings::default(),
            destinations: Vec::new(),
            scenes: StudioScenesState::default(),
            live_info: LiveInfo::default(),
            chat: ChatSettings::default(),
            kit_path: None,
            twitch_client_id: String::new(),
            recording_dir: None,
            ffmpeg_path: None,
            chat_window_bounds: None,
        }
    }
}

impl AppSettings {
    pub fn normalize(&mut self) {
        self.schema_version = SCHEMA_VERSION;
        if self.device_name.trim().is_empty() && !self.host_name.trim().is_empty() {
            self.device_name = self.host_name.clone();
        } else if self.host_name.trim().is_empty() && !self.device_name.trim().is_empty() {
            self.host_name = self.device_name.clone();
        }
        self.quality.normalize();
        self.audio_mixer.normalize();
        self.studio.normalize();
        self.scenes.normalize();
        self.chat.normalize();
        for dest in &mut self.destinations {
            dest.normalize();
        }
    }
}

// ------------------------------------------------------------------ Chat LAN & Reações

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub senderId: String,
    pub senderName: String,
    pub role: String, // "host" | "viewer" | "system"
    pub text: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ReactionEvent {
    pub id: String,
    pub senderName: String,
    pub emoji: String,
    pub timestamp: u64,
}

// ------------------------------------------------------------------ Descoberta UDP

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum DiscoveryMessage {
    #[serde(rename_all = "camelCase")]
    Announce {
        v: u32,
        id: String,
        name: String,
        ip: String,
        port: u16,
        requires_pin: bool,
        source_name: String,
        source_kind: SourceKind,
        viewers: usize,
        max_viewers: usize,
        started_at: u64,
    },
    Query {
        v: u32,
    },
    Bye {
        v: u32,
        id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredHost {
    pub id: String,
    pub name: String,
    pub ip: String,
    pub port: u16,
    pub requires_pin: bool,
    pub source_name: String,
    pub source_kind: SourceKind,
    pub viewers: usize,
    pub max_viewers: usize,
    pub started_at: u64,
    pub last_seen: u64,
}

// ------------------------------------------------------------------ Sinalização WebRTC + Chat LAN

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDescriptionPayload {
    #[serde(rename = "type")]
    pub desc_type: String,
    pub sdp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IceCandidatePayload {
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_m_line_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username_fragment: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RejectReason {
    PinInvalido,
    SalaCheia,
    VersaoIncompativel,
    Bloqueado,
    SemTransmissao,
}

impl RejectReason {
    pub fn message(&self) -> &'static str {
        match self {
            RejectReason::PinInvalido => "Código incorreto.",
            RejectReason::SalaCheia => "A transmissão já está com o número máximo de espectadores.",
            RejectReason::VersaoIncompativel => "As duas máquinas estão com versões diferentes do app.",
            RejectReason::Bloqueado => "Muitas tentativas erradas. Espere um minuto e tente de novo.",
            RejectReason::SemTransmissao => "Este computador não está transmitindo no momento.",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum ViewerToHost {
    Hello {
        v: u32,
        name: String,
        pin: Option<String>,
    },
    Answer {
        desc: SessionDescriptionPayload,
    },
    Ice {
        candidate: IceCandidatePayload,
    },
    Ping {
        #[serde(rename = "sentAt")]
        sent_at: u64,
    },
    Chat {
        text: String,
    },
    Reaction {
        emoji: String,
    },
    VoiceOffer {
        desc: SessionDescriptionPayload,
    },
    VoiceIce {
        candidate: IceCandidatePayload,
    },
    Bye,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum HostToViewer {
    #[serde(rename_all = "camelCase")]
    Welcome {
        viewer_id: String,
        host_name: String,
        source_name: String,
        source_kind: SourceKind,
    },
    #[serde(rename_all = "camelCase")]
    Rejected {
        reason: RejectReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_after_ms: Option<u64>,
    },
    Offer {
        desc: SessionDescriptionPayload,
    },
    Ice {
        candidate: IceCandidatePayload,
    },
    Pong {
        #[serde(rename = "sentAt")]
        sent_at: u64,
    },
    Chat {
        message: ChatMessage,
    },
    Reaction {
        reaction: ReactionEvent,
    },
    #[serde(rename_all = "camelCase")]
    SourceChanged {
        source_name: String,
        source_kind: SourceKind,
    },
    VoiceAnswer {
        desc: SessionDescriptionPayload,
    },
    VoiceIce {
        candidate: IceCandidatePayload,
    },
    Kicked,
    Ended,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedViewer {
    pub id: String,
    pub name: String,
    pub address: String,
    pub joined_at: u64,
    pub ping_ms: Option<u64>,
}

// ------------------------------------------------------------------ Captura / Sistema

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct DesktopSource {
    pub id: String,
    pub name: String,
    pub kind: SourceKind,
    pub thumbnail: String,
    pub app_icon: Option<String>,
    pub display_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_settings_partial_json_and_normalize() {
        let json = r#"{
            "schemaVersion": 3,
            "hostName": "Meu PC",
            "live": { "title": "Ao Vivo", "categoryName": "Just Chatting" },
            "audioMixer": {
                "monitorDeviceId": "headset-1",
                "channels": [
                    {
                        "id": "party",
                        "kind": "party_voice",
                        "name": "Party Voice",
                        "volumeDb": 99.0,
                        "filters": {
                            "eqEnabled": true,
                            "eqLowDb": 30.0,
                            "duckingEnabled": true,
                            "duckingAmountDb": -90.0,
                            "compressor": { "ratio": 0.2 }
                        }
                    }
                ]
            },
            "scenes": {
                "transitionType": "fade-to-black",
                "scenes": [
                    {
                        "id": "s1",
                        "name": "Cena",
                        "items": [
                            {
                                "id": "i1",
                                "transform": { "flipX": true, "opacity": 5.0 }
                            }
                        ]
                    }
                ]
            }
        }"#;

        let mut settings: AppSettings = serde_json::from_str(json).expect("valid v3 json");
        settings.device_name.clear();
        settings.normalize();

        assert_eq!(settings.device_name, "Meu PC");
        assert_eq!(settings.live_info.title, "Ao Vivo");
        assert_eq!(settings.audio_mixer.monitor_device_id.as_deref(), Some("headset-1"));
        let ch = &settings.audio_mixer.channels[0];
        assert_eq!(ch.kind, AudioChannelKind::PartyVoice);
        assert_eq!(ch.volume_db, MAX_DB);
        assert_eq!(ch.filters.eq_low_db, 12.0);
        assert_eq!(ch.filters.ducking_amount_db, -40.0);
        assert_eq!(ch.filters.compressor.ratio, 1.0);
        assert_eq!(settings.scenes.transition_type, TransitionType::FadeToBlack);
        let t = &settings.scenes.scenes[0].items[0].transform;
        assert!(t.flip_x);
        assert_eq!(t.opacity, 1.0);
    }
}
