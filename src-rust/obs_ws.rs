//! Spec-compliant `obs-websocket` v5 External Controller Server (`obs_ws`).
//!
//! Enables plug-and-play hardware and macro deck integration with:
//! - **Elgato Stream Deck** (official OBS Studio plugin)
//! - **Bitfocus Companion** (`obs-studio` v5 module)
//! - **Touch Portal**, **SAMMI**, **Streamer.bot**, and standard `obs-websocket-js` / `obws` clients
//!
//! Implements the obs-websocket 5.x wire protocol (default port `4455`):
//! - SHA-256 challenge/salt authentication ([`compute_obs_auth`]) with constant-time verification (`subtle::ConstantTimeEq`)
//! - Opcodes `0` (`Hello`), `1` (`Identify`), `2` (`Identified`), `3` (`Reidentify`), `5` (`Event`),
//!   `6` (`Request`), `7` (`RequestResponse`), `8` (`RequestBatch`), and `9` (`RequestBatchResponse`)
//! - Clean [`ObsControlHost`] trait abstraction so `AppEngine` in `rpc.rs` wires in cleanly while
//!   remaining 100% independently testable.

use crate::protocol::{
    AppSettings, AudioChannelKind, ExternalControlSettings, ExternalControlStatus, SourceType,
    TransitionType, MAX_DB, MIN_DB, OBS_WS_DEFAULT_PORT, RUST_RPC_PORT,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

/// Advertised OBS Studio compatibility version (checked by Stream Deck / Companion plugins).
pub const OBS_COMPAT_STUDIO_VERSION: &str = "30.2.3";
/// Advertised obs-websocket v5 protocol version.
pub const OBS_WS_PROTOCOL_VERSION: &str = "5.5.2";
/// Supported obs-websocket v5 RPC version.
pub const OBS_WS_RPC_VERSION: u32 = 1;

// ------------------------------------------------------------------ Opcodes

pub const OP_HELLO: u8 = 0;
pub const OP_IDENTIFY: u8 = 1;
pub const OP_IDENTIFIED: u8 = 2;
pub const OP_REIDENTIFY: u8 = 3;
pub const OP_EVENT: u8 = 5;
pub const OP_REQUEST: u8 = 6;
pub const OP_REQUEST_RESPONSE: u8 = 7;
pub const OP_REQUEST_BATCH: u8 = 8;
pub const OP_REQUEST_BATCH_RESPONSE: u8 = 9;

// ------------------------------------------------------------------ OBS WebSocket Close Codes

pub const CLOSE_MESSAGE_DECODE_ERROR: u16 = 4002;
pub const CLOSE_MISSING_DATA_FIELD: u16 = 4003;
pub const CLOSE_INVALID_DATA_FIELD_TYPE: u16 = 4004;
pub const CLOSE_UNKNOWN_OPCODE: u16 = 4006;
pub const CLOSE_NOT_IDENTIFIED: u16 = 4007;
pub const CLOSE_ALREADY_IDENTIFIED: u16 = 4008;
pub const CLOSE_AUTHENTICATION_FAILED: u16 = 4009;
pub const CLOSE_UNSUPPORTED_RPC_VERSION: u16 = 4010;

// ------------------------------------------------------------------ OBS RequestStatus Codes

pub const STATUS_SUCCESS: u16 = 100;
pub const STATUS_UNKNOWN_REQUEST_TYPE: u16 = 204;
pub const STATUS_NOT_READY: u16 = 207;
pub const STATUS_MISSING_REQUEST_FIELD: u16 = 300;
pub const STATUS_INVALID_REQUEST_FIELD: u16 = 400;
pub const STATUS_INVALID_REQUEST_FIELD_TYPE: u16 = 401;
pub const STATUS_REQUEST_FIELD_OUT_OF_RANGE: u16 = 402;
pub const STATUS_OUTPUT_RUNNING: u16 = 500;
pub const STATUS_OUTPUT_NOT_RUNNING: u16 = 501;
pub const STATUS_STUDIO_MODE_NOT_ACTIVE: u16 = 506;
pub const STATUS_RESOURCE_NOT_FOUND: u16 = 600;
pub const STATUS_ACTION_FAILED: u16 = 700;

// ------------------------------------------------------------------ EventSubscription Bitmasks

pub const EVENT_SUB_NONE: u32 = 0;
pub const EVENT_SUB_GENERAL: u32 = 1 << 0;
pub const EVENT_SUB_CONFIG: u32 = 1 << 1;
pub const EVENT_SUB_SCENES: u32 = 1 << 2;
pub const EVENT_SUB_INPUTS: u32 = 1 << 3;
pub const EVENT_SUB_TRANSITIONS: u32 = 1 << 4;
pub const EVENT_SUB_FILTERS: u32 = 1 << 5;
pub const EVENT_SUB_OUTPUTS: u32 = 1 << 6;
pub const EVENT_SUB_SCENE_ITEMS: u32 = 1 << 7;
pub const EVENT_SUB_MEDIA_INPUTS: u32 = 1 << 8;
pub const EVENT_SUB_VENDORS: u32 = 1 << 9;
pub const EVENT_SUB_UI: u32 = 1 << 10;
pub const EVENT_SUB_ALL: u32 = EVENT_SUB_GENERAL
    | EVENT_SUB_CONFIG
    | EVENT_SUB_SCENES
    | EVENT_SUB_INPUTS
    | EVENT_SUB_TRANSITIONS
    | EVENT_SUB_FILTERS
    | EVENT_SUB_OUTPUTS
    | EVENT_SUB_SCENE_ITEMS
    | EVENT_SUB_MEDIA_INPUTS
    | EVENT_SUB_VENDORS
    | EVENT_SUB_UI;

const MAX_OBS_WS_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
const OBS_WS_WRITE_QUEUE_CAP: usize = 256;
const IDENTIFY_TIMEOUT_SECS: u64 = 15;
const MAX_BATCH_SLEEP_MS: u64 = 10_000;

/// Full list of obs-websocket v5 request types handled by this server.
pub const AVAILABLE_REQUESTS: &[&str] = &[
    "GetVersion",
    "GetStats",
    "GetHotkeyList",
    "TriggerHotkeyByName",
    "Sleep",
    "GetProfileList",
    "GetSceneCollectionList",
    "GetVideoSettings",
    "GetSceneList",
    "GetCurrentProgramScene",
    "SetCurrentProgramScene",
    "GetCurrentPreviewScene",
    "SetCurrentPreviewScene",
    "GetStudioModeEnabled",
    "SetStudioModeEnabled",
    "TriggerStudioModeTransition",
    "GetSceneItemList",
    "GetSceneItemId",
    "GetSceneItemEnabled",
    "SetSceneItemEnabled",
    "GetSceneTransitionList",
    "GetCurrentSceneTransition",
    "SetCurrentSceneTransition",
    "SetCurrentSceneTransitionDuration",
    "GetInputList",
    "GetSpecialInputs",
    "GetInputMute",
    "SetInputMute",
    "ToggleInputMute",
    "GetInputVolume",
    "SetInputVolume",
    "GetStreamStatus",
    "StartStream",
    "StopStream",
    "ToggleStream",
    "GetRecordStatus",
    "StartRecord",
    "StopRecord",
    "ToggleRecord",
    "GetReplayBufferStatus",
    "StartReplayBuffer",
    "StopReplayBuffer",
    "ToggleReplayBuffer",
    "SaveReplayBuffer",
    "GetLastReplayBufferReplay",
    "GetVirtualCamStatus",
    "StartVirtualCam",
    "StopVirtualCam",
    "ToggleVirtualCam",
];

// ------------------------------------------------------------------ Authentication

/// Computes the official obs-websocket v5 SHA-256 authentication response:
/// 1. `secret = base64(sha256(password + salt))`
/// 2. `auth   = base64(sha256(secret + challenge))`
pub fn compute_obs_auth(password: &str, salt: &str, challenge: &str) -> String {
    let mut secret_input = String::with_capacity(password.len() + salt.len());
    secret_input.push_str(password);
    secret_input.push_str(salt);
    let secret_hash = Sha256::digest(secret_input.as_bytes());
    let base64_secret = BASE64.encode(secret_hash);

    let mut auth_input = String::with_capacity(base64_secret.len() + challenge.len());
    auth_input.push_str(&base64_secret);
    auth_input.push_str(challenge);
    let auth_hash = Sha256::digest(auth_input.as_bytes());
    BASE64.encode(auth_hash)
}

/// Constant-time comparison of the expected OBS authentication token against client input.
pub fn verify_obs_auth(expected: &str, provided: Option<&str>) -> bool {
    let Some(got) = provided else {
        return false;
    };
    let a = expected.as_bytes();
    let b = got.as_bytes();
    let len_eq = (a.len() as u64).ct_eq(&(b.len() as u64));
    let mut diff: u8 = if bool::from(len_eq) { 0 } else { 1 };
    let max_len = a.len().max(b.len());
    for i in 0..max_len {
        let ba = a.get(i).copied().unwrap_or(0);
        let bb = b.get(i).copied().unwrap_or(0);
        diff |= ba ^ bb;
    }
    bool::from(diff.ct_eq(&0) & len_eq)
}

fn generate_random_b64_32() -> String {
    let mut raw = [0u8; 32];
    rand::rng().fill_bytes(&mut raw);
    BASE64.encode(raw)
}

// ------------------------------------------------------------------ Helpers (Timecode, dB, Transitions)

/// Formats a duration in milliseconds as an OBS timecode (`"HH:MM:SS.mmm"`).
pub fn format_obs_timecode(duration_ms: u64) -> String {
    let ms = duration_ms % 1000;
    let total_secs = duration_ms / 1000;
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = total_secs / 3600;
    format!("{hours:02}:{mins:02}:{secs:02}.{ms:03}")
}

/// Converts decibels to OBS linear volume multiplier (`10^(dB/20)`).
pub fn db_to_mul(db: f64) -> f64 {
    if db <= -100.0 {
        0.0
    } else {
        10.0_f64.powf(db / 20.0)
    }
}

/// Converts OBS linear volume multiplier to decibels (`20 * log10(mul)`).
pub fn mul_to_db(mul: f64) -> f64 {
    if mul <= 0.0 {
        MIN_DB as f64
    } else {
        (20.0 * mul.log10()).clamp(MIN_DB as f64, MAX_DB as f64)
    }
}

/// Maps a PrismCast [`TransitionType`] to its OBS display name, kind, UUID, and fixed-duration flag.
pub fn transition_type_metadata(
    t: TransitionType,
) -> (&'static str, &'static str, &'static str, bool) {
    match t {
        TransitionType::Cut => ("Cut", "cut_transition", "prismcast-transition-cut", true),
        TransitionType::Fade => ("Fade", "fade_transition", "prismcast-transition-fade", false),
        TransitionType::FadeToBlack => (
            "Fade to Black",
            "fade_to_color_transition",
            "prismcast-transition-fade-to-black",
            false,
        ),
        TransitionType::Slide => (
            "Slide",
            "slide_transition",
            "prismcast-transition-slide",
            false,
        ),
        TransitionType::Stinger => (
            "Stinger",
            "obs_stinger_transition",
            "prismcast-transition-stinger",
            false,
        ),
        TransitionType::LumaWipe => (
            "Luma Wipe",
            "wipe_transition",
            "prismcast-transition-luma-wipe",
            false,
        ),
        TransitionType::CustomShader => (
            "Custom Shader",
            "prismcast_shader_transition",
            "prismcast-transition-custom-shader",
            false,
        ),
    }
}

/// Returns the canonical OBS transition display name for a [`TransitionType`].
pub fn transition_type_to_obs_name(t: TransitionType) -> &'static str {
    transition_type_metadata(t).0
}

/// Parses an OBS transition name (display name, kebab-case, or snake_case) into [`TransitionType`].
pub fn obs_name_to_transition_type(name: &str) -> Option<TransitionType> {
    let norm = name
        .trim()
        .to_ascii_lowercase()
        .replace(['_', ' '], "-");
    match norm.as_str() {
        "cut" | "cut-transition" => Some(TransitionType::Cut),
        "fade" | "fade-transition" => Some(TransitionType::Fade),
        "fade-to-black" | "fade-to-color" | "fade-to-color-transition" => {
            Some(TransitionType::FadeToBlack)
        }
        "slide" | "slide-transition" => Some(TransitionType::Slide),
        "stinger" | "obs-stinger-transition" => Some(TransitionType::Stinger),
        "luma-wipe" | "wipe" | "wipe-transition" => Some(TransitionType::LumaWipe),
        "custom-shader" | "shader" | "prismcast-shader-transition" => {
            Some(TransitionType::CustomShader)
        }
        _ => None,
    }
}

/// Returns the full list of transitions supported by PrismCast in OBS v5 format.
pub fn all_obs_transitions() -> Vec<ObsTransitionInfo> {
    [
        TransitionType::Cut,
        TransitionType::Fade,
        TransitionType::FadeToBlack,
        TransitionType::Slide,
        TransitionType::Stinger,
        TransitionType::LumaWipe,
        TransitionType::CustomShader,
    ]
    .into_iter()
    .map(|t| {
        let (name, kind, uuid, fixed) = transition_type_metadata(t);
        ObsTransitionInfo {
            transition_name: name.to_string(),
            transition_uuid: uuid.to_string(),
            transition_kind: kind.to_string(),
            transition_fixed: fixed,
            transition_configurable: !fixed,
        }
    })
    .collect()
}

/// Maps a PrismCast [`SourceType`] to an OBS `inputKind`.
pub fn source_type_to_obs_input_kind(st: SourceType) -> &'static str {
    match st {
        SourceType::ScreenCapture => "monitor_capture",
        SourceType::WindowCapture => "window_capture",
        SourceType::CameraInput => "dshow_input",
        SourceType::BrowserSource => "browser_source",
        SourceType::ImageSource => "image_source",
        SourceType::ColorSource => "color_source_v3",
        SourceType::TextSource => "text_gdiplus_v3",
        SourceType::AudioInput => "wasapi_input_capture",
        SourceType::AudioOutput => "wasapi_output_capture",
    }
}

/// Maps a PrismCast [`AudioChannelKind`] to `(inputKind, unversionedInputKind)`.
pub fn audio_channel_kind_to_obs_input_kind(kind: AudioChannelKind) -> (&'static str, &'static str) {
    match kind {
        AudioChannelKind::Desktop => ("wasapi_output_capture", "wasapi_output_capture"),
        AudioChannelKind::Mic => ("wasapi_input_capture", "wasapi_input_capture"),
        AudioChannelKind::App => ("wasapi_process_output_capture", "wasapi_process_output_capture"),
        AudioChannelKind::PartyVoice => ("wasapi_input_capture", "wasapi_input_capture"),
    }
}

// ------------------------------------------------------------------ Host Trait & DTOs

/// Boxed `Send` future alias used by [`ObsControlHost`] (avoids needing `async-trait`).
pub type ObsFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type ObsResult<T> = Result<T, ObsRequestError>;

/// Structured obs-websocket v5 request failure returned in `RequestResponse.requestStatus`.
#[derive(Debug, Clone)]
pub struct ObsRequestError {
    pub code: u16,
    pub comment: String,
}

impl ObsRequestError {
    pub fn new(code: u16, comment: impl Into<String>) -> Self {
        Self {
            code,
            comment: comment.into(),
        }
    }

    pub fn missing_field(field: &str) -> Self {
        Self::new(
            STATUS_MISSING_REQUEST_FIELD,
            format!("Missing required request field: {field}"),
        )
    }

    pub fn invalid_field(msg: impl Into<String>) -> Self {
        Self::new(STATUS_INVALID_REQUEST_FIELD, msg)
    }

    pub fn invalid_field_type(field: &str) -> Self {
        Self::new(
            STATUS_INVALID_REQUEST_FIELD_TYPE,
            format!("Invalid field type for '{field}'"),
        )
    }

    pub fn out_of_range(msg: impl Into<String>) -> Self {
        Self::new(STATUS_REQUEST_FIELD_OUT_OF_RANGE, msg)
    }

    pub fn resource_not_found(msg: impl Into<String>) -> Self {
        Self::new(STATUS_RESOURCE_NOT_FOUND, msg)
    }

    pub fn output_running(msg: impl Into<String>) -> Self {
        Self::new(STATUS_OUTPUT_RUNNING, msg)
    }

    pub fn output_not_running(msg: impl Into<String>) -> Self {
        Self::new(STATUS_OUTPUT_NOT_RUNNING, msg)
    }

    pub fn studio_mode_not_active() -> Self {
        Self::new(STATUS_STUDIO_MODE_NOT_ACTIVE, "Studio mode is not active")
    }

    pub fn action_failed(msg: impl Into<String>) -> Self {
        Self::new(STATUS_ACTION_FAILED, msg)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ObsStats {
    pub active_fps: f64,
    pub average_frame_render_time: f64,
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub available_disk_space: f64,
    pub render_skipped_frames: u64,
    pub render_total_frames: u64,
    pub output_skipped_frames: u64,
    pub output_total_frames: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ObsSceneInfo {
    pub scene_name: String,
    pub scene_uuid: String,
    pub scene_index: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ObsSceneList {
    pub current_program_scene_name: Option<String>,
    pub current_program_scene_uuid: Option<String>,
    pub current_preview_scene_name: Option<String>,
    pub current_preview_scene_uuid: Option<String>,
    pub scenes: Vec<ObsSceneInfo>,
}

#[derive(Debug, Clone)]
pub struct ObsSceneItemInfo {
    pub scene_item_id: i64,
    pub scene_item_index: usize,
    pub source_name: String,
    pub source_uuid: String,
    pub source_type: String,
    pub input_kind: Option<String>,
    pub scene_item_enabled: bool,
    pub scene_item_locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObsTransitionInfo {
    pub transition_name: String,
    pub transition_uuid: String,
    pub transition_kind: String,
    pub transition_fixed: bool,
    pub transition_configurable: bool,
}

#[derive(Debug, Clone)]
pub struct ObsCurrentTransition {
    pub transition_name: String,
    pub transition_uuid: String,
    pub transition_kind: String,
    pub transition_fixed: bool,
    pub transition_duration: Option<u32>,
    pub transition_configurable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObsInputInfo {
    pub input_name: String,
    pub input_uuid: String,
    pub input_kind: String,
    pub unversioned_input_kind: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ObsInputVolume {
    pub input_volume_mul: f64,
    pub input_volume_db: f64,
}

impl ObsInputVolume {
    pub fn from_db(db: f64) -> Self {
        Self {
            input_volume_mul: db_to_mul(db),
            input_volume_db: db,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ObsStreamStatus {
    pub output_active: bool,
    pub output_reconnecting: bool,
    pub output_duration_ms: u64,
    pub output_congestion: f64,
    pub output_bytes: u64,
    pub output_skipped_frames: u64,
    pub output_total_frames: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ObsRecordStatus {
    pub output_active: bool,
    pub output_paused: bool,
    pub output_duration_ms: u64,
    pub output_bytes: u64,
    pub output_path: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ObsReplayBufferStatus {
    pub output_active: bool,
    pub last_saved_path: Option<String>,
}

/// Real-time events broadcast over obs-websocket v5 opcode `5` (`Event`).
#[derive(Debug, Clone)]
pub enum ObsEvent {
    CurrentProgramSceneChanged {
        scene_name: String,
        scene_uuid: String,
    },
    CurrentPreviewSceneChanged {
        scene_name: String,
        scene_uuid: String,
    },
    SceneListChanged {
        scenes: Vec<ObsSceneInfo>,
    },
    StudioModeStateChanged {
        studio_mode_enabled: bool,
    },
    StreamStateChanged {
        output_active: bool,
        output_state: String,
    },
    RecordStateChanged {
        output_active: bool,
        output_state: String,
        output_path: Option<String>,
    },
    ReplayBufferStateChanged {
        output_active: bool,
        output_state: String,
    },
    ReplayBufferSaved {
        saved_replay_path: String,
    },
    VirtualcamStateChanged {
        output_active: bool,
        output_state: String,
    },
    InputMuteStateChanged {
        input_name: String,
        input_uuid: String,
        input_muted: bool,
    },
    InputVolumeChanged {
        input_name: String,
        input_uuid: String,
        input_volume_mul: f64,
        input_volume_db: f64,
    },
    SceneItemEnableStateChanged {
        scene_name: String,
        scene_uuid: String,
        scene_item_id: i64,
        scene_item_enabled: bool,
    },
    CurrentSceneTransitionChanged {
        transition_name: String,
        transition_uuid: String,
    },
    CurrentSceneTransitionDurationChanged {
        transition_duration: u32,
    },
}

impl ObsEvent {
    /// Returns `(eventType, eventIntent, eventData)` matching obs-websocket v5 wire format.
    pub fn to_wire(&self) -> (&'static str, u32, Value) {
        match self {
            ObsEvent::CurrentProgramSceneChanged {
                scene_name,
                scene_uuid,
            } => (
                "CurrentProgramSceneChanged",
                EVENT_SUB_SCENES,
                json!({
                    "sceneName": scene_name,
                    "sceneUuid": scene_uuid,
                }),
            ),
            ObsEvent::CurrentPreviewSceneChanged {
                scene_name,
                scene_uuid,
            } => (
                "CurrentPreviewSceneChanged",
                EVENT_SUB_SCENES,
                json!({
                    "sceneName": scene_name,
                    "sceneUuid": scene_uuid,
                }),
            ),
            ObsEvent::SceneListChanged { scenes } => (
                "SceneListChanged",
                EVENT_SUB_SCENES,
                json!({ "scenes": scenes }),
            ),
            ObsEvent::StudioModeStateChanged {
                studio_mode_enabled,
            } => (
                "StudioModeStateChanged",
                EVENT_SUB_UI,
                json!({ "studioModeEnabled": studio_mode_enabled }),
            ),
            ObsEvent::StreamStateChanged {
                output_active,
                output_state,
            } => (
                "StreamStateChanged",
                EVENT_SUB_OUTPUTS,
                json!({
                    "outputActive": output_active,
                    "outputState": output_state,
                }),
            ),
            ObsEvent::RecordStateChanged {
                output_active,
                output_state,
                output_path,
            } => (
                "RecordStateChanged",
                EVENT_SUB_OUTPUTS,
                json!({
                    "outputActive": output_active,
                    "outputState": output_state,
                    "outputPath": output_path,
                }),
            ),
            ObsEvent::ReplayBufferStateChanged {
                output_active,
                output_state,
            } => (
                "ReplayBufferStateChanged",
                EVENT_SUB_OUTPUTS,
                json!({
                    "outputActive": output_active,
                    "outputState": output_state,
                }),
            ),
            ObsEvent::ReplayBufferSaved { saved_replay_path } => (
                "ReplayBufferSaved",
                EVENT_SUB_OUTPUTS,
                json!({ "savedReplayPath": saved_replay_path }),
            ),
            ObsEvent::VirtualcamStateChanged {
                output_active,
                output_state,
            } => (
                "VirtualcamStateChanged",
                EVENT_SUB_OUTPUTS,
                json!({
                    "outputActive": output_active,
                    "outputState": output_state,
                }),
            ),
            ObsEvent::InputMuteStateChanged {
                input_name,
                input_uuid,
                input_muted,
            } => (
                "InputMuteStateChanged",
                EVENT_SUB_INPUTS,
                json!({
                    "inputName": input_name,
                    "inputUuid": input_uuid,
                    "inputMuted": input_muted,
                }),
            ),
            ObsEvent::InputVolumeChanged {
                input_name,
                input_uuid,
                input_volume_mul,
                input_volume_db,
            } => (
                "InputVolumeChanged",
                EVENT_SUB_INPUTS,
                json!({
                    "inputName": input_name,
                    "inputUuid": input_uuid,
                    "inputVolumeMul": input_volume_mul,
                    "inputVolumeDb": input_volume_db,
                }),
            ),
            ObsEvent::SceneItemEnableStateChanged {
                scene_name,
                scene_uuid,
                scene_item_id,
                scene_item_enabled,
            } => (
                "SceneItemEnableStateChanged",
                EVENT_SUB_SCENE_ITEMS,
                json!({
                    "sceneName": scene_name,
                    "sceneUuid": scene_uuid,
                    "sceneItemId": scene_item_id,
                    "sceneItemEnabled": scene_item_enabled,
                }),
            ),
            ObsEvent::CurrentSceneTransitionChanged {
                transition_name,
                transition_uuid,
            } => (
                "CurrentSceneTransitionChanged",
                EVENT_SUB_TRANSITIONS,
                json!({
                    "transitionName": transition_name,
                    "transitionUuid": transition_uuid,
                }),
            ),
            ObsEvent::CurrentSceneTransitionDurationChanged {
                transition_duration,
            } => (
                "CurrentSceneTransitionDurationChanged",
                EVENT_SUB_TRANSITIONS,
                json!({ "transitionDuration": transition_duration }),
            ),
        }
    }
}

/// Diffs two [`AppSettings`] snapshots and emits corresponding OBS v5 events for scene,
/// studio mode, transition, scene-item visibility, and audio mixer changes.
pub fn translate_app_settings_diff(prev: &AppSettings, next: &AppSettings) -> Vec<ObsEvent> {
    let mut events = Vec::new();

    if prev.scenes.active_scene_id != next.scenes.active_scene_id {
        if let Some(sc) = next
            .scenes
            .scenes
            .iter()
            .find(|s| s.id == next.scenes.active_scene_id)
        {
            events.push(ObsEvent::CurrentProgramSceneChanged {
                scene_name: sc.name.clone(),
                scene_uuid: sc.id.clone(),
            });
        }
    }

    if prev.scenes.preview_scene_id != next.scenes.preview_scene_id {
        if let Some(sc) = next
            .scenes
            .scenes
            .iter()
            .find(|s| s.id == next.scenes.preview_scene_id)
        {
            events.push(ObsEvent::CurrentPreviewSceneChanged {
                scene_name: sc.name.clone(),
                scene_uuid: sc.id.clone(),
            });
        }
    }

    if prev.scenes.studio_mode != next.scenes.studio_mode {
        events.push(ObsEvent::StudioModeStateChanged {
            studio_mode_enabled: next.scenes.studio_mode,
        });
    }

    if prev.scenes.transition_type != next.scenes.transition_type {
        let (name, _, uuid, _) = transition_type_metadata(next.scenes.transition_type);
        events.push(ObsEvent::CurrentSceneTransitionChanged {
            transition_name: name.to_string(),
            transition_uuid: uuid.to_string(),
        });
    }

    if prev.scenes.transition_duration_ms != next.scenes.transition_duration_ms {
        events.push(ObsEvent::CurrentSceneTransitionDurationChanged {
            transition_duration: next.scenes.transition_duration_ms,
        });
    }

    // Scene list add/remove/rename check
    let prev_scene_sig: Vec<(&str, &str)> = prev
        .scenes
        .scenes
        .iter()
        .map(|s| (s.id.as_str(), s.name.as_str()))
        .collect();
    let next_scene_sig: Vec<(&str, &str)> = next
        .scenes
        .scenes
        .iter()
        .map(|s| (s.id.as_str(), s.name.as_str()))
        .collect();
    if prev_scene_sig != next_scene_sig {
        let scenes = next
            .scenes
            .scenes
            .iter()
            .enumerate()
            .map(|(idx, s)| ObsSceneInfo {
                scene_name: s.name.clone(),
                scene_uuid: s.id.clone(),
                scene_index: idx,
            })
            .collect();
        events.push(ObsEvent::SceneListChanged { scenes });
    }

    // Scene item visibility changes
    for next_scene in &next.scenes.scenes {
        if let Some(prev_scene) = prev.scenes.scenes.iter().find(|s| s.id == next_scene.id) {
            for (idx, next_item) in next_scene.items.iter().enumerate() {
                if let Some(prev_item) = prev_scene.items.iter().find(|i| i.id == next_item.id) {
                    if prev_item.visible != next_item.visible {
                        events.push(ObsEvent::SceneItemEnableStateChanged {
                            scene_name: next_scene.name.clone(),
                            scene_uuid: next_scene.id.clone(),
                            scene_item_id: (idx + 1) as i64,
                            scene_item_enabled: next_item.visible,
                        });
                    }
                }
            }
        }
    }

    // Audio mixer mute & volume changes
    for next_ch in &next.audio_mixer.channels {
        if let Some(prev_ch) = prev
            .audio_mixer
            .channels
            .iter()
            .find(|c| c.id == next_ch.id)
        {
            if prev_ch.muted != next_ch.muted {
                events.push(ObsEvent::InputMuteStateChanged {
                    input_name: next_ch.name.clone(),
                    input_uuid: next_ch.id.clone(),
                    input_muted: next_ch.muted,
                });
            }
            if (prev_ch.volume_db - next_ch.volume_db).abs() > 0.01 {
                let db = next_ch.volume_db as f64;
                events.push(ObsEvent::InputVolumeChanged {
                    input_name: next_ch.name.clone(),
                    input_uuid: next_ch.id.clone(),
                    input_volume_mul: db_to_mul(db),
                    input_volume_db: db,
                });
            }
        }
    }

    events
}

/// Translates PrismCast `RpcEvent` status payloads (`multistream:status`, `recorder:status`,
/// `virtualcam:status` / `virtualCam:status`) into OBS v5 [`ObsEvent`] values.
pub fn translate_rpc_event(event_name: &str, payload: &Value) -> Vec<ObsEvent> {
    let mut out = Vec::new();
    match event_name {
        "multistream:status" => {
            if let Some(active) = payload.get("active").and_then(|v| v.as_bool()) {
                let state = if active {
                    "OBS_WEBSOCKET_OUTPUT_STARTED"
                } else {
                    "OBS_WEBSOCKET_OUTPUT_STOPPED"
                };
                out.push(ObsEvent::StreamStateChanged {
                    output_active: active,
                    output_state: state.to_string(),
                });
            }
        }
        "recorder:status" => {
            if let Some(recording) = payload.get("recording").and_then(|v| v.as_bool()) {
                let path = payload
                    .get("recordingPath")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let state = if recording {
                    "OBS_WEBSOCKET_OUTPUT_STARTED"
                } else {
                    "OBS_WEBSOCKET_OUTPUT_STOPPED"
                };
                out.push(ObsEvent::RecordStateChanged {
                    output_active: recording,
                    output_state: state.to_string(),
                    output_path: path,
                });
            }
            if let Some(saved) = payload.get("replaySavedPath").and_then(|v| v.as_str()) {
                if !saved.is_empty() {
                    out.push(ObsEvent::ReplayBufferSaved {
                        saved_replay_path: saved.to_string(),
                    });
                }
            }
        }
        "virtualcam:status" | "virtualCam:status" | "virtual-cam:status" => {
            if let Some(active) = payload.get("active").and_then(|v| v.as_bool()) {
                let state = if active {
                    "OBS_WEBSOCKET_OUTPUT_STARTED"
                } else {
                    "OBS_WEBSOCKET_OUTPUT_STOPPED"
                };
                out.push(ObsEvent::VirtualcamStateChanged {
                    output_active: active,
                    output_state: state.to_string(),
                });
            }
        }
        _ => {}
    }
    out
}

/// Backend control contract implemented by `AppEngine` (in `rpc.rs`) or test mocks.
pub trait ObsControlHost: Send + Sync + 'static {
    // -- General
    fn get_stats(&self) -> ObsFuture<'_, ObsResult<ObsStats>>;
    fn get_hotkey_list(&self) -> ObsFuture<'_, ObsResult<Vec<String>>>;
    fn trigger_hotkey_by_name(&self, hotkey_name: String) -> ObsFuture<'_, ObsResult<()>>;

    // -- Scenes & Studio Mode
    fn get_scene_list(&self) -> ObsFuture<'_, ObsResult<ObsSceneList>>;
    fn set_current_program_scene(
        &self,
        scene_name: Option<String>,
        scene_uuid: Option<String>,
    ) -> ObsFuture<'_, ObsResult<ObsSceneInfo>>;
    fn set_current_preview_scene(
        &self,
        scene_name: Option<String>,
        scene_uuid: Option<String>,
    ) -> ObsFuture<'_, ObsResult<ObsSceneInfo>>;
    fn get_studio_mode_enabled(&self) -> ObsFuture<'_, ObsResult<bool>>;
    fn set_studio_mode_enabled(&self, enabled: bool) -> ObsFuture<'_, ObsResult<()>>;
    fn trigger_studio_mode_transition(&self) -> ObsFuture<'_, ObsResult<()>>;

    // -- Scene Items
    fn get_scene_item_list(
        &self,
        scene_name: Option<String>,
        scene_uuid: Option<String>,
    ) -> ObsFuture<'_, ObsResult<Vec<ObsSceneItemInfo>>>;
    fn set_scene_item_enabled(
        &self,
        scene_name: Option<String>,
        scene_uuid: Option<String>,
        scene_item_id: i64,
        enabled: bool,
    ) -> ObsFuture<'_, ObsResult<()>>;

    // -- Transitions
    fn get_current_scene_transition(&self) -> ObsFuture<'_, ObsResult<ObsCurrentTransition>>;
    fn set_current_scene_transition(
        &self,
        transition_name: String,
    ) -> ObsFuture<'_, ObsResult<ObsCurrentTransition>>;
    fn set_current_scene_transition_duration(&self, duration_ms: u32) -> ObsFuture<'_, ObsResult<()>>;

    // -- Inputs (Audio Channels)
    fn get_input_list(
        &self,
        input_kind: Option<String>,
    ) -> ObsFuture<'_, ObsResult<Vec<ObsInputInfo>>>;
    fn get_input_mute(
        &self,
        input_name: Option<String>,
        input_uuid: Option<String>,
    ) -> ObsFuture<'_, ObsResult<bool>>;
    fn set_input_mute(
        &self,
        input_name: Option<String>,
        input_uuid: Option<String>,
        muted: bool,
    ) -> ObsFuture<'_, ObsResult<()>>;
    fn toggle_input_mute(
        &self,
        input_name: Option<String>,
        input_uuid: Option<String>,
    ) -> ObsFuture<'_, ObsResult<bool>>;
    fn get_input_volume(
        &self,
        input_name: Option<String>,
        input_uuid: Option<String>,
    ) -> ObsFuture<'_, ObsResult<ObsInputVolume>>;
    fn set_input_volume(
        &self,
        input_name: Option<String>,
        input_uuid: Option<String>,
        volume_db: f64,
    ) -> ObsFuture<'_, ObsResult<ObsInputVolume>>;

    // -- Stream / Record / Replay Buffer / Virtual Camera
    fn get_stream_status(&self) -> ObsFuture<'_, ObsResult<ObsStreamStatus>>;
    fn start_stream(&self) -> ObsFuture<'_, ObsResult<()>>;
    fn stop_stream(&self) -> ObsFuture<'_, ObsResult<()>>;

    fn get_record_status(&self) -> ObsFuture<'_, ObsResult<ObsRecordStatus>>;
    fn start_record(&self) -> ObsFuture<'_, ObsResult<()>>;
    fn stop_record(&self) -> ObsFuture<'_, ObsResult<Option<String>>>;

    fn get_replay_buffer_status(&self) -> ObsFuture<'_, ObsResult<ObsReplayBufferStatus>>;
    fn start_replay_buffer(&self) -> ObsFuture<'_, ObsResult<()>>;
    fn stop_replay_buffer(&self) -> ObsFuture<'_, ObsResult<()>>;
    fn save_replay_buffer(&self) -> ObsFuture<'_, ObsResult<Option<String>>>;

    fn get_virtual_cam_status(&self) -> ObsFuture<'_, ObsResult<bool>>;
    fn start_virtual_cam(&self) -> ObsFuture<'_, ObsResult<()>>;
    fn stop_virtual_cam(&self) -> ObsFuture<'_, ObsResult<()>>;

    /// Optional real-time OBS event stream emitted directly by the host.
    fn subscribe_events(&self) -> Option<broadcast::Receiver<ObsEvent>> {
        None
    }
}

// ------------------------------------------------------------------ Request Dispatcher

#[derive(Debug)]
struct SessionCounters {
    incoming: AtomicU64,
    outgoing: AtomicU64,
}

impl SessionCounters {
    fn new() -> Self {
        Self {
            incoming: AtomicU64::new(0),
            outgoing: AtomicU64::new(0),
        }
    }
}

fn opt_str(data: &Value, key: &str) -> Option<String> {
    data.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn req_bool(data: &Value, key: &str) -> ObsResult<bool> {
    let val = data
        .get(key)
        .ok_or_else(|| ObsRequestError::missing_field(key))?;
    val.as_bool()
        .ok_or_else(|| ObsRequestError::invalid_field_type(key))
}

fn req_i64(data: &Value, key: &str) -> ObsResult<i64> {
    let val = data
        .get(key)
        .ok_or_else(|| ObsRequestError::missing_field(key))?;
    val.as_i64()
        .ok_or_else(|| ObsRequestError::invalid_field_type(key))
}

fn req_u64(data: &Value, key: &str) -> ObsResult<u64> {
    let val = data
        .get(key)
        .ok_or_else(|| ObsRequestError::missing_field(key))?;
    val.as_u64()
        .ok_or_else(|| ObsRequestError::invalid_field_type(key))
}

fn require_scene_selector(data: &Value) -> ObsResult<(Option<String>, Option<String>)> {
    let name = opt_str(data, "sceneName");
    let uuid = opt_str(data, "sceneUuid");
    if name.is_none() && uuid.is_none() {
        return Err(ObsRequestError::missing_field("sceneName"));
    }
    Ok((name, uuid))
}

fn require_input_selector(data: &Value) -> ObsResult<(Option<String>, Option<String>)> {
    let name = opt_str(data, "inputName");
    let uuid = opt_str(data, "inputUuid");
    if name.is_none() && uuid.is_none() {
        return Err(ObsRequestError::missing_field("inputName"));
    }
    Ok((name, uuid))
}

fn current_platform_name() -> &'static str {
    #[cfg(windows)]
    {
        "windows"
    }
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        "linux"
    }
}

fn make_request_response_payload(
    request_type: &str,
    request_id: &str,
    outcome: ObsResult<Option<Value>>,
) -> Value {
    match outcome {
        Ok(Some(resp_data)) => json!({
            "requestType": request_type,
            "requestId": request_id,
            "requestStatus": {
                "result": true,
                "code": STATUS_SUCCESS,
            },
            "responseData": resp_data,
        }),
        Ok(None) => json!({
            "requestType": request_type,
            "requestId": request_id,
            "requestStatus": {
                "result": true,
                "code": STATUS_SUCCESS,
            },
        }),
        Err(err) => json!({
            "requestType": request_type,
            "requestId": request_id,
            "requestStatus": {
                "result": false,
                "code": err.code,
                "comment": err.comment,
            },
        }),
    }
}

async fn dispatch_obs_request(
    host: &Arc<dyn ObsControlHost>,
    counters: &SessionCounters,
    request_type: &str,
    data: &Value,
) -> ObsResult<Option<Value>> {
    match request_type {
        // ------------------------------------------------ General
        "GetVersion" => Ok(Some(json!({
            "obsVersion": OBS_COMPAT_STUDIO_VERSION,
            "obsWebSocketVersion": OBS_WS_PROTOCOL_VERSION,
            "rpcVersion": OBS_WS_RPC_VERSION,
            "availableRequests": AVAILABLE_REQUESTS,
            "supportedImageFormats": ["png", "jpg", "jpeg", "webp"],
            "platform": current_platform_name(),
            "platformDescription": format!(
                "PrismCast Studio {} (obs-websocket v5)",
                env!("CARGO_PKG_VERSION")
            ),
        }))),
        "GetStats" => {
            let stats = host.get_stats().await?;
            Ok(Some(json!({
                "cpuUsage": stats.cpu_usage,
                "memoryUsage": stats.memory_usage,
                "availableDiskSpace": stats.available_disk_space,
                "activeFps": stats.active_fps,
                "averageFrameRenderTime": stats.average_frame_render_time,
                "renderSkippedFrames": stats.render_skipped_frames,
                "renderTotalFrames": stats.render_total_frames,
                "outputSkippedFrames": stats.output_skipped_frames,
                "outputTotalFrames": stats.output_total_frames,
                "webSocketSessionIncomingMessages": counters.incoming.load(Ordering::Relaxed),
                "webSocketSessionOutgoingMessages": counters.outgoing.load(Ordering::Relaxed),
            })))
        }
        "GetHotkeyList" => {
            let hotkeys = host.get_hotkey_list().await?;
            Ok(Some(json!({ "hotkeys": hotkeys })))
        }
        "TriggerHotkeyByName" => {
            let name = opt_str(data, "hotkeyName")
                .ok_or_else(|| ObsRequestError::missing_field("hotkeyName"))?;
            host.trigger_hotkey_by_name(name).await?;
            Ok(None)
        }
        "Sleep" => {
            let millis = data
                .get("sleepMillis")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                .min(MAX_BATCH_SLEEP_MS);
            if millis > 0 {
                tokio::time::sleep(Duration::from_millis(millis)).await;
            }
            Ok(None)
        }
        "GetProfileList" => Ok(Some(json!({
            "currentProfileName": "PrismCast Studio",
            "profiles": ["PrismCast Studio"],
        }))),
        "GetSceneCollectionList" => Ok(Some(json!({
            "currentSceneCollectionName": "PrismCast Studio",
            "sceneCollections": ["PrismCast Studio"],
        }))),
        "GetVideoSettings" => Ok(Some(json!({
            "fpsNumerator": 60,
            "fpsDenominator": 1,
            "baseWidth": 1920,
            "baseHeight": 1080,
            "outputWidth": 1920,
            "outputHeight": 1080,
        }))),

        // ------------------------------------------------ Scenes & Studio Mode
        "GetSceneList" => {
            let list = host.get_scene_list().await?;
            let studio_enabled = host.get_studio_mode_enabled().await.unwrap_or(false);
            Ok(Some(json!({
                "currentProgramSceneName": list.current_program_scene_name,
                "currentProgramSceneUuid": list.current_program_scene_uuid,
                "currentPreviewSceneName": if studio_enabled { list.current_preview_scene_name } else { None },
                "currentPreviewSceneUuid": if studio_enabled { list.current_preview_scene_uuid } else { None },
                "scenes": list.scenes,
            })))
        }
        "GetCurrentProgramScene" => {
            let list = host.get_scene_list().await?;
            let name = list
                .current_program_scene_name
                .ok_or_else(|| ObsRequestError::resource_not_found("No active program scene"))?;
            let uuid = list.current_program_scene_uuid.unwrap_or_default();
            Ok(Some(json!({
                "sceneName": name,
                "sceneUuid": uuid,
                "currentProgramSceneName": name,
                "currentProgramSceneUuid": uuid,
            })))
        }
        "SetCurrentProgramScene" => {
            let (name, uuid) = require_scene_selector(data)?;
            host.set_current_program_scene(name, uuid).await?;
            Ok(None)
        }
        "GetCurrentPreviewScene" => {
            if !host.get_studio_mode_enabled().await? {
                return Err(ObsRequestError::studio_mode_not_active());
            }
            let list = host.get_scene_list().await?;
            let name = list
                .current_preview_scene_name
                .or(list.current_program_scene_name)
                .ok_or_else(|| ObsRequestError::resource_not_found("No preview scene"))?;
            let uuid = list
                .current_preview_scene_uuid
                .or(list.current_program_scene_uuid)
                .unwrap_or_default();
            Ok(Some(json!({
                "sceneName": name,
                "sceneUuid": uuid,
                "currentPreviewSceneName": name,
                "currentPreviewSceneUuid": uuid,
            })))
        }
        "SetCurrentPreviewScene" => {
            if !host.get_studio_mode_enabled().await? {
                return Err(ObsRequestError::studio_mode_not_active());
            }
            let (name, uuid) = require_scene_selector(data)?;
            host.set_current_preview_scene(name, uuid).await?;
            Ok(None)
        }
        "GetStudioModeEnabled" => {
            let enabled = host.get_studio_mode_enabled().await?;
            Ok(Some(json!({ "studioModeEnabled": enabled })))
        }
        "SetStudioModeEnabled" => {
            let enabled = req_bool(data, "studioModeEnabled")?;
            host.set_studio_mode_enabled(enabled).await?;
            Ok(None)
        }
        "TriggerStudioModeTransition" => {
            if !host.get_studio_mode_enabled().await? {
                return Err(ObsRequestError::studio_mode_not_active());
            }
            host.trigger_studio_mode_transition().await?;
            Ok(None)
        }

        // ------------------------------------------------ Scene Items
        "GetSceneItemList" => {
            let (name, uuid) = require_scene_selector(data)?;
            let items = host.get_scene_item_list(name, uuid).await?;
            let serialized: Vec<Value> = items
                .into_iter()
                .map(|item| {
                    json!({
                        "sceneItemId": item.scene_item_id,
                        "sceneItemIndex": item.scene_item_index,
                        "sourceName": item.source_name,
                        "sourceUuid": item.source_uuid,
                        "sourceType": item.source_type,
                        "inputKind": item.input_kind,
                        "sceneItemEnabled": item.scene_item_enabled,
                        "sceneItemLocked": item.scene_item_locked,
                        "isGroup": false,
                    })
                })
                .collect();
            Ok(Some(json!({ "sceneItems": serialized })))
        }
        "GetSceneItemId" => {
            let (name, uuid) = require_scene_selector(data)?;
            let source_name = opt_str(data, "sourceName")
                .ok_or_else(|| ObsRequestError::missing_field("sourceName"))?;
            let search_offset = data
                .get("searchOffset")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let items = host.get_scene_item_list(name, uuid).await?;
            let matches: Vec<&ObsSceneItemInfo> = items
                .iter()
                .filter(|i| i.source_name == source_name || i.source_uuid == source_name)
                .collect();
            let chosen = if search_offset >= 0 {
                matches.get(search_offset as usize).copied()
            } else {
                let back = search_offset.unsigned_abs() as usize;
                matches.len().checked_sub(back).and_then(|idx| matches.get(idx).copied())
            };
            let item = chosen.ok_or_else(|| {
                ObsRequestError::resource_not_found(format!(
                    "Scene item '{source_name}' not found"
                ))
            })?;
            Ok(Some(json!({ "sceneItemId": item.scene_item_id })))
        }
        "GetSceneItemEnabled" => {
            let (name, uuid) = require_scene_selector(data)?;
            let item_id = req_i64(data, "sceneItemId")?;
            let items = host.get_scene_item_list(name, uuid).await?;
            let item = items
                .iter()
                .find(|i| i.scene_item_id == item_id)
                .ok_or_else(|| {
                    ObsRequestError::resource_not_found(format!(
                        "Scene item id {item_id} not found"
                    ))
                })?;
            Ok(Some(json!({ "sceneItemEnabled": item.scene_item_enabled })))
        }
        "SetSceneItemEnabled" => {
            let (name, uuid) = require_scene_selector(data)?;
            let item_id = req_i64(data, "sceneItemId")?;
            let enabled = req_bool(data, "sceneItemEnabled")?;
            host.set_scene_item_enabled(name, uuid, item_id, enabled)
                .await?;
            Ok(None)
        }

        // ------------------------------------------------ Transitions
        "GetSceneTransitionList" => {
            let current = host.get_current_scene_transition().await?;
            let transitions = all_obs_transitions();
            Ok(Some(json!({
                "currentSceneTransitionName": current.transition_name,
                "currentSceneTransitionUuid": current.transition_uuid,
                "currentSceneTransitionKind": current.transition_kind,
                "transitions": transitions,
            })))
        }
        "GetCurrentSceneTransition" => {
            let cur = host.get_current_scene_transition().await?;
            Ok(Some(json!({
                "transitionName": cur.transition_name,
                "transitionUuid": cur.transition_uuid,
                "transitionKind": cur.transition_kind,
                "transitionFixed": cur.transition_fixed,
                "transitionDuration": cur.transition_duration,
                "transitionConfigurable": cur.transition_configurable,
                "transitionSettings": {},
            })))
        }
        "SetCurrentSceneTransition" => {
            let name = opt_str(data, "transitionName")
                .ok_or_else(|| ObsRequestError::missing_field("transitionName"))?;
            host.set_current_scene_transition(name).await?;
            Ok(None)
        }
        "SetCurrentSceneTransitionDuration" => {
            let raw = req_u64(data, "transitionDuration")?;
            if !(50..=20_000).contains(&raw) {
                return Err(ObsRequestError::out_of_range(
                    "transitionDuration must be between 50 and 20000 ms",
                ));
            }
            host.set_current_scene_transition_duration((raw as u32).clamp(50, 10_000))
                .await?;
            Ok(None)
        }

        // ------------------------------------------------ Inputs (Audio Channels)
        "GetInputList" => {
            let kind = opt_str(data, "inputKind");
            let inputs = host.get_input_list(kind).await?;
            Ok(Some(json!({ "inputs": inputs })))
        }
        "GetSpecialInputs" => {
            let inputs = host.get_input_list(None).await.unwrap_or_default();
            let desktop1 = inputs
                .iter()
                .find(|i| i.input_kind == "wasapi_output_capture")
                .map(|i| i.input_name.clone());
            let mic1 = inputs
                .iter()
                .find(|i| i.input_kind == "wasapi_input_capture")
                .map(|i| i.input_name.clone());
            Ok(Some(json!({
                "desktop1": desktop1,
                "desktop2": Value::Null,
                "mic1": mic1,
                "mic2": Value::Null,
                "mic3": Value::Null,
                "mic4": Value::Null,
            })))
        }
        "GetInputMute" => {
            let (name, uuid) = require_input_selector(data)?;
            let muted = host.get_input_mute(name, uuid).await?;
            Ok(Some(json!({ "inputMuted": muted })))
        }
        "SetInputMute" => {
            let (name, uuid) = require_input_selector(data)?;
            let muted = req_bool(data, "inputMuted")?;
            host.set_input_mute(name, uuid, muted).await?;
            Ok(None)
        }
        "ToggleInputMute" => {
            let (name, uuid) = require_input_selector(data)?;
            let muted = host.toggle_input_mute(name, uuid).await?;
            Ok(Some(json!({ "inputMuted": muted })))
        }
        "GetInputVolume" => {
            let (name, uuid) = require_input_selector(data)?;
            let vol = host.get_input_volume(name, uuid).await?;
            Ok(Some(json!({
                "inputVolumeMul": vol.input_volume_mul,
                "inputVolumeDb": vol.input_volume_db,
            })))
        }
        "SetInputVolume" => {
            let (name, uuid) = require_input_selector(data)?;
            let volume_db = if let Some(v_db) = data.get("inputVolumeDb") {
                let db = v_db
                    .as_f64()
                    .ok_or_else(|| ObsRequestError::invalid_field_type("inputVolumeDb"))?;
                if !db.is_finite() {
                    return Err(ObsRequestError::out_of_range("inputVolumeDb must be finite"));
                }
                db.clamp(MIN_DB as f64, MAX_DB as f64)
            } else if let Some(v_mul) = data.get("inputVolumeMul") {
                let mul = v_mul
                    .as_f64()
                    .ok_or_else(|| ObsRequestError::invalid_field_type("inputVolumeMul"))?;
                if !mul.is_finite() || mul < 0.0 || mul > 20.0 {
                    return Err(ObsRequestError::out_of_range(
                        "inputVolumeMul must be between 0.0 and 20.0",
                    ));
                }
                mul_to_db(mul)
            } else {
                return Err(ObsRequestError::missing_field(
                    "inputVolumeDb or inputVolumeMul",
                ));
            };
            host.set_input_volume(name, uuid, volume_db).await?;
            Ok(None)
        }

        // ------------------------------------------------ Stream / Record / Replay / VirtualCam
        "GetStreamStatus" => {
            let st = host.get_stream_status().await?;
            Ok(Some(json!({
                "outputActive": st.output_active,
                "outputReconnecting": st.output_reconnecting,
                "outputTimecode": format_obs_timecode(st.output_duration_ms),
                "outputDuration": st.output_duration_ms,
                "outputCongestion": st.output_congestion,
                "outputBytes": st.output_bytes,
                "outputSkippedFrames": st.output_skipped_frames,
                "outputTotalFrames": st.output_total_frames,
            })))
        }
        "StartStream" => {
            host.start_stream().await?;
            Ok(None)
        }
        "StopStream" => {
            host.stop_stream().await?;
            Ok(None)
        }
        "ToggleStream" => {
            let st = host.get_stream_status().await?;
            if st.output_active {
                host.stop_stream().await?;
                Ok(Some(json!({ "outputActive": false })))
            } else {
                host.start_stream().await?;
                Ok(Some(json!({ "outputActive": true })))
            }
        }
        "GetRecordStatus" => {
            let st = host.get_record_status().await?;
            Ok(Some(json!({
                "outputActive": st.output_active,
                "outputPaused": st.output_paused,
                "outputTimecode": format_obs_timecode(st.output_duration_ms),
                "outputDuration": st.output_duration_ms,
                "outputBytes": st.output_bytes,
            })))
        }
        "StartRecord" => {
            host.start_record().await?;
            Ok(None)
        }
        "StopRecord" => {
            let path = host.stop_record().await?;
            Ok(Some(json!({ "outputPath": path.unwrap_or_default() })))
        }
        "ToggleRecord" => {
            let st = host.get_record_status().await?;
            if st.output_active {
                let _ = host.stop_record().await?;
                Ok(Some(json!({ "outputActive": false })))
            } else {
                host.start_record().await?;
                Ok(Some(json!({ "outputActive": true })))
            }
        }
        "GetReplayBufferStatus" => {
            let st = host.get_replay_buffer_status().await?;
            Ok(Some(json!({ "outputActive": st.output_active })))
        }
        "StartReplayBuffer" => {
            host.start_replay_buffer().await?;
            Ok(None)
        }
        "StopReplayBuffer" => {
            host.stop_replay_buffer().await?;
            Ok(None)
        }
        "ToggleReplayBuffer" => {
            let st = host.get_replay_buffer_status().await?;
            if st.output_active {
                host.stop_replay_buffer().await?;
                Ok(Some(json!({ "outputActive": false })))
            } else {
                host.start_replay_buffer().await?;
                Ok(Some(json!({ "outputActive": true })))
            }
        }
        "SaveReplayBuffer" => {
            let _ = host.save_replay_buffer().await?;
            Ok(None)
        }
        "GetLastReplayBufferReplay" => {
            let st = host.get_replay_buffer_status().await?;
            Ok(Some(json!({
                "savedReplayPath": st.last_saved_path.unwrap_or_default(),
            })))
        }
        "GetVirtualCamStatus" => {
            let active = host.get_virtual_cam_status().await?;
            Ok(Some(json!({ "outputActive": active })))
        }
        "StartVirtualCam" => {
            host.start_virtual_cam().await?;
            Ok(None)
        }
        "StopVirtualCam" => {
            host.stop_virtual_cam().await?;
            Ok(None)
        }
        "ToggleVirtualCam" => {
            let active = host.get_virtual_cam_status().await?;
            if active {
                host.stop_virtual_cam().await?;
                Ok(Some(json!({ "outputActive": false })))
            } else {
                host.start_virtual_cam().await?;
                Ok(Some(json!({ "outputActive": true })))
            }
        }
        other => Err(ObsRequestError::new(
            STATUS_UNKNOWN_REQUEST_TYPE,
            format!("Unsupported requestType: {other}"),
        )),
    }
}

// ------------------------------------------------------------------ Connection Handler

struct ClientCountGuard(Arc<AtomicUsize>);

impl Drop for ClientCountGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

async fn send_json(
    tx: &mpsc::Sender<Message>,
    counters: &SessionCounters,
    value: &Value,
) -> bool {
    let Ok(raw) = serde_json::to_string(value) else {
        return false;
    };
    counters.outgoing.fetch_add(1, Ordering::Relaxed);
    tx.send(Message::Text(raw.into())).await.is_ok()
}

async fn send_close_code(tx: &mpsc::Sender<Message>, code: u16, reason: &'static str) {
    let frame = CloseFrame {
        code: CloseCode::from(code),
        reason: reason.into(),
    };
    let _ = tx.send(Message::Close(Some(frame))).await;
}

async fn handle_client_connection(
    stream: TcpStream,
    host: Arc<dyn ObsControlHost>,
    password: Option<String>,
    event_tx: broadcast::Sender<ObsEvent>,
    client_count: Arc<AtomicUsize>,
    shutdown: CancellationToken,
) {
    let mut ws_cfg = WebSocketConfig::default();
    ws_cfg.max_message_size = Some(MAX_OBS_WS_MESSAGE_BYTES);
    ws_cfg.max_frame_size = Some(MAX_OBS_WS_MESSAGE_BYTES);

    let Ok(ws_stream) = tokio_tungstenite::accept_async_with_config(stream, Some(ws_cfg)).await
    else {
        return;
    };

    client_count.fetch_add(1, Ordering::Relaxed);
    let _guard = ClientCountGuard(client_count);

    let (mut ws_write, mut ws_read) = ws_stream.split();
    let (tx, mut rx) = mpsc::channel::<Message>(OBS_WS_WRITE_QUEUE_CAP);

    let write_task: JoinHandle<()> = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let is_close = matches!(msg, Message::Close(_));
            if ws_write.send(msg).await.is_err() {
                break;
            }
            if is_close {
                let _ = ws_write.flush().await;
                let _ = ws_write.close().await;
                break;
            }
        }
    });

    let counters = Arc::new(SessionCounters::new());

    // Step 1: Send Op 0 (Hello)
    let auth_required = password.as_deref().is_some_and(|p| !p.is_empty());
    let expected_auth = if let Some(ref pwd) = password.filter(|p| !p.is_empty()) {
        let salt = generate_random_b64_32();
        let challenge = generate_random_b64_32();
        let expected = compute_obs_auth(pwd, &salt, &challenge);
        let hello = json!({
            "op": OP_HELLO,
            "d": {
                "obsStudioVersion": OBS_COMPAT_STUDIO_VERSION,
                "obsWebSocketVersion": OBS_WS_PROTOCOL_VERSION,
                "rpcVersion": OBS_WS_RPC_VERSION,
                "authentication": {
                    "challenge": challenge,
                    "salt": salt,
                }
            }
        });
        if !send_json(&tx, &counters, &hello).await {
            drop(tx);
            let _ = tokio::time::timeout(Duration::from_secs(1), write_task).await;
            return;
        }
        Some(expected)
    } else {
        let hello = json!({
            "op": OP_HELLO,
            "d": {
                "obsStudioVersion": OBS_COMPAT_STUDIO_VERSION,
                "obsWebSocketVersion": OBS_WS_PROTOCOL_VERSION,
                "rpcVersion": OBS_WS_RPC_VERSION,
            }
        });
        if !send_json(&tx, &counters, &hello).await {
            drop(tx);
            let _ = tokio::time::timeout(Duration::from_secs(1), write_task).await;
            return;
        }
        None
    };

    // Step 2: Wait for Op 1 (Identify) within timeout
    let subscriptions = Arc::new(AtomicU64::new(EVENT_SUB_NONE as u64));
    let identified = loop {
        let next_msg = tokio::select! {
            _ = shutdown.cancelled() => {
                send_close_code(&tx, 1001, "Server shutting down").await;
                break false;
            }
            res = tokio::time::timeout(Duration::from_secs(IDENTIFY_TIMEOUT_SECS), ws_read.next()) => {
                match res {
                    Ok(Some(Ok(m))) => m,
                    Ok(Some(Err(_))) | Ok(None) => break false,
                    Err(_) => {
                        send_close_code(&tx, CLOSE_NOT_IDENTIFIED, "Identify timeout").await;
                        break false;
                    }
                }
            }
        };

        match next_msg {
            Message::Ping(p) => {
                let _ = tx.try_send(Message::Pong(p));
            }
            Message::Pong(_) | Message::Frame(_) => {}
            Message::Close(_) => break false,
            Message::Binary(_) => {
                send_close_code(&tx, CLOSE_MESSAGE_DECODE_ERROR, "Binary frames unsupported").await;
                break false;
            }
            Message::Text(text) => {
                counters.incoming.fetch_add(1, Ordering::Relaxed);
                let Ok(val) = serde_json::from_str::<Value>(&text) else {
                    send_close_code(&tx, CLOSE_MESSAGE_DECODE_ERROR, "Invalid JSON").await;
                    break false;
                };
                let Some(op) = val.get("op").and_then(|v| v.as_u64()).map(|n| n as u8) else {
                    send_close_code(&tx, CLOSE_MISSING_DATA_FIELD, "Missing op").await;
                    break false;
                };
                if op != OP_IDENTIFY {
                    send_close_code(&tx, CLOSE_NOT_IDENTIFIED, "Must identify before sending messages")
                        .await;
                    break false;
                }
                let Some(d) = val.get("d") else {
                    send_close_code(&tx, CLOSE_MISSING_DATA_FIELD, "Missing d").await;
                    break false;
                };
                let Some(rpc_ver) = d.get("rpcVersion").and_then(|v| v.as_u64()) else {
                    send_close_code(&tx, CLOSE_MISSING_DATA_FIELD, "Missing rpcVersion").await;
                    break false;
                };
                if rpc_ver != OBS_WS_RPC_VERSION as u64 {
                    send_close_code(
                        &tx,
                        CLOSE_UNSUPPORTED_RPC_VERSION,
                        "Unsupported rpcVersion (expected 1)",
                    )
                    .await;
                    break false;
                }

                if auth_required {
                    let provided = d.get("authentication").and_then(|v| v.as_str());
                    let expected = expected_auth.as_deref().unwrap_or("");
                    if !verify_obs_auth(expected, provided) {
                        send_close_code(
                            &tx,
                            CLOSE_AUTHENTICATION_FAILED,
                            "Authentication failed",
                        )
                        .await;
                        break false;
                    }
                }

                let event_subs = d
                    .get("eventSubscriptions")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32)
                    .unwrap_or(EVENT_SUB_ALL);
                subscriptions.store(event_subs as u64, Ordering::Relaxed);

                let reply = json!({
                    "op": OP_IDENTIFIED,
                    "d": {
                        "negotiatedRpcVersion": OBS_WS_RPC_VERSION,
                    }
                });
                if !send_json(&tx, &counters, &reply).await {
                    break false;
                }
                break true;
            }
        }
    };

    if !identified {
        drop(tx);
        let _ = tokio::time::timeout(Duration::from_secs(1), write_task).await;
        return;
    }

    // Step 3: Spawn Op 5 (Event) forwarder gated by client's `eventSubscriptions` bitmask
    let mut evt_rx = event_tx.subscribe();
    let tx_evt = tx.clone();
    let subs_evt = subscriptions.clone();
    let counters_evt = counters.clone();
    let cancel_evt = shutdown.clone();
    let evt_task = tokio::spawn(async move {
        loop {
            let item = tokio::select! {
                _ = cancel_evt.cancelled() => break,
                res = evt_rx.recv() => res,
            };
            match item {
                Ok(ev) => {
                    let (event_type, intent, event_data) = ev.to_wire();
                    let mask = subs_evt.load(Ordering::Relaxed) as u32;
                    if (mask & intent) == 0 {
                        continue;
                    }
                    let msg = json!({
                        "op": OP_EVENT,
                        "d": {
                            "eventType": event_type,
                            "eventIntent": intent,
                            "eventData": event_data,
                        }
                    });
                    if let Ok(raw) = serde_json::to_string(&msg) {
                        counters_evt.outgoing.fetch_add(1, Ordering::Relaxed);
                        if tx_evt.send(Message::Text(raw.into())).await.is_err() {
                            break;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Step 4: Main identified message loop (Op 3 Reidentify, Op 6 Request, Op 8 RequestBatch)
    loop {
        let next_msg = tokio::select! {
            _ = shutdown.cancelled() => {
                send_close_code(&tx, 1001, "Server shutting down").await;
                break;
            }
            item = ws_read.next() => item,
        };
        let Some(Ok(msg)) = next_msg else {
            break;
        };

        match msg {
            Message::Ping(p) => {
                let _ = tx.try_send(Message::Pong(p));
            }
            Message::Pong(_) | Message::Frame(_) => {}
            Message::Close(_) => break,
            Message::Binary(_) => {
                send_close_code(&tx, CLOSE_MESSAGE_DECODE_ERROR, "Binary frames unsupported").await;
                break;
            }
            Message::Text(text) => {
                counters.incoming.fetch_add(1, Ordering::Relaxed);
                let Ok(val) = serde_json::from_str::<Value>(&text) else {
                    send_close_code(&tx, CLOSE_MESSAGE_DECODE_ERROR, "Invalid JSON").await;
                    break;
                };
                let Some(op) = val.get("op").and_then(|v| v.as_u64()).map(|n| n as u8) else {
                    send_close_code(&tx, CLOSE_MISSING_DATA_FIELD, "Missing op").await;
                    break;
                };
                let d = val.get("d").unwrap_or(&Value::Null);

                match op {
                    OP_IDENTIFY => {
                        send_close_code(&tx, CLOSE_ALREADY_IDENTIFIED, "Already identified").await;
                        break;
                    }
                    OP_REIDENTIFY => {
                        let new_subs = d
                            .get("eventSubscriptions")
                            .and_then(|v| v.as_u64())
                            .map(|n| n as u32)
                            .unwrap_or(EVENT_SUB_ALL);
                        subscriptions.store(new_subs as u64, Ordering::Relaxed);
                        let reply = json!({
                            "op": OP_IDENTIFIED,
                            "d": {
                                "negotiatedRpcVersion": OBS_WS_RPC_VERSION,
                            }
                        });
                        if !send_json(&tx, &counters, &reply).await {
                            break;
                        }
                    }
                    OP_REQUEST => {
                        let Some(req_type) = d.get("requestType").and_then(|v| v.as_str()) else {
                            send_close_code(&tx, CLOSE_MISSING_DATA_FIELD, "Missing requestType")
                                .await;
                            break;
                        };
                        let Some(req_id) = d.get("requestId").and_then(|v| v.as_str()) else {
                            send_close_code(&tx, CLOSE_MISSING_DATA_FIELD, "Missing requestId")
                                .await;
                            break;
                        };
                        let req_data = d.get("requestData").unwrap_or(&Value::Null);
                        let outcome =
                            dispatch_obs_request(&host, &counters, req_type, req_data).await;
                        let resp_d = make_request_response_payload(req_type, req_id, outcome);
                        let envelope = json!({
                            "op": OP_REQUEST_RESPONSE,
                            "d": resp_d,
                        });
                        if !send_json(&tx, &counters, &envelope).await {
                            break;
                        }
                    }
                    OP_REQUEST_BATCH => {
                        let Some(batch_id) = d.get("requestId").and_then(|v| v.as_str()) else {
                            send_close_code(&tx, CLOSE_MISSING_DATA_FIELD, "Missing requestId")
                                .await;
                            break;
                        };
                        let Some(requests) = d.get("requests").and_then(|v| v.as_array()) else {
                            send_close_code(
                                &tx,
                                CLOSE_INVALID_DATA_FIELD_TYPE,
                                "requests must be an array",
                            )
                            .await;
                            break;
                        };
                        let halt_on_failure = d
                            .get("haltOnFailure")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);

                        let mut results = Vec::with_capacity(requests.len());
                        for (idx, sub) in requests.iter().enumerate() {
                            let sub_type = sub
                                .get("requestType")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let sub_id = sub
                                .get("requestId")
                                .and_then(|v| v.as_str())
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("{batch_id}:{idx}"));
                            let sub_data = sub.get("requestData").unwrap_or(&Value::Null);
                            let outcome = if sub_type.is_empty() {
                                Err(ObsRequestError::missing_field("requestType"))
                            } else {
                                dispatch_obs_request(&host, &counters, sub_type, sub_data).await
                            };
                            let failed = outcome.is_err();
                            results.push(make_request_response_payload(
                                sub_type, &sub_id, outcome,
                            ));
                            if failed && halt_on_failure {
                                break;
                            }
                        }

                        let envelope = json!({
                            "op": OP_REQUEST_BATCH_RESPONSE,
                            "d": {
                                "requestId": batch_id,
                                "results": results,
                            }
                        });
                        if !send_json(&tx, &counters, &envelope).await {
                            break;
                        }
                    }
                    _ => {
                        send_close_code(&tx, CLOSE_UNKNOWN_OPCODE, "Unknown opcode").await;
                        break;
                    }
                }
            }
        }
    }

    evt_task.abort();
    drop(tx);
    let _ = tokio::time::timeout(Duration::from_secs(1), write_task).await;
}

// ------------------------------------------------------------------ Server Lifecycle Handle

/// Configuration for starting or restarting [`ObsWsServerHandle`].
#[derive(Debug, Clone)]
pub struct ObsWsConfig {
    pub port: u16,
    pub password: Option<String>,
    pub allow_lan: bool,
    pub rpc_ws_port: u16,
}

impl Default for ObsWsConfig {
    fn default() -> Self {
        Self {
            port: OBS_WS_DEFAULT_PORT,
            password: None,
            allow_lan: false,
            rpc_ws_port: RUST_RPC_PORT,
        }
    }
}

impl ObsWsConfig {
    /// Builds an [`ObsWsConfig`] from persisted [`ExternalControlSettings`] and optional secret password.
    pub fn from_settings(settings: &ExternalControlSettings, password: Option<String>) -> Self {
        let effective_password = if settings.obs_ws_has_password {
            password.filter(|p| !p.is_empty())
        } else {
            None
        };
        Self {
            port: settings.obs_ws_port,
            password: effective_password,
            allow_lan: settings.obs_ws_allow_lan,
            rpc_ws_port: RUST_RPC_PORT,
        }
    }
}

/// Manages the lifecycle of the embedded `obs-websocket` v5 TCP/WebSocket server.
pub struct ObsWsServerHandle {
    host: Arc<dyn ObsControlHost>,
    event_tx: broadcast::Sender<ObsEvent>,
    client_count: Arc<AtomicUsize>,
    rpc_ws_port: u16,
    rpc_ws_clients: Arc<AtomicUsize>,
    running: bool,
    bound_port: u16,
    error: Option<String>,
    cancel: Option<CancellationToken>,
    accept_task: Option<JoinHandle<()>>,
    host_event_task: Option<JoinHandle<()>>,
}

impl ObsWsServerHandle {
    /// Creates a new stopped server handle bound to `host`.
    pub fn new(host: Arc<dyn ObsControlHost>) -> Self {
        let (event_tx, _) = broadcast::channel::<ObsEvent>(512);
        Self {
            host,
            event_tx,
            client_count: Arc::new(AtomicUsize::new(0)),
            rpc_ws_port: RUST_RPC_PORT,
            rpc_ws_clients: Arc::new(AtomicUsize::new(0)),
            running: false,
            bound_port: OBS_WS_DEFAULT_PORT,
            error: None,
            cancel: None,
            accept_task: None,
            host_event_task: None,
        }
    }

    /// Attaches live RPC WebSocket port and client counter for unified [`ExternalControlStatus`].
    pub fn set_rpc_info(&mut self, rpc_ws_port: u16, rpc_ws_clients: Arc<AtomicUsize>) {
        self.rpc_ws_port = rpc_ws_port;
        self.rpc_ws_clients = rpc_ws_clients;
    }

    /// Returns a clone of the shared live obs-websocket connected client counter.
    pub fn client_count_handle(&self) -> Arc<AtomicUsize> {
        self.client_count.clone()
    }

    /// Returns the broadcast sender used to push real-time [`ObsEvent`] messages to clients.
    pub fn event_sender(&self) -> broadcast::Sender<ObsEvent> {
        self.event_tx.clone()
    }

    /// Broadcasts an [`ObsEvent`] to all identified clients subscribed to its event intent.
    pub fn broadcast_event(&self, event: ObsEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Starts the obs-websocket v5 server on `127.0.0.1:<port>` (or `0.0.0.0:<port>` when `allow_lan == true`).
    pub async fn start(&mut self, config: ObsWsConfig) -> ExternalControlStatus {
        if self.running {
            self.stop().await;
        }

        self.rpc_ws_port = config.rpc_ws_port;
        let bind_ip = if config.allow_lan {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        };
        let bind_addr = format!("{bind_ip}:{}", config.port);

        let listener = match TcpListener::bind(&bind_addr).await {
            Ok(l) => l,
            Err(err) => {
                self.running = false;
                self.bound_port = config.port;
                self.error = Some(format!("Falha ao abrir obs-websocket em {bind_addr}: {err}"));
                return self.status();
            }
        };

        let actual_port = listener
            .local_addr()
            .map(|a| a.port())
            .unwrap_or(config.port);

        let cancel = CancellationToken::new();
        let accept_cancel = cancel.clone();
        let host = self.host.clone();
        let password = config.password.filter(|p| !p.is_empty());
        let event_tx = self.event_tx.clone();
        let client_count = self.client_count.clone();

        // Forward optional host event stream into server broadcast channel
        let host_event_task = self.host.subscribe_events().map(|mut host_rx| {
            let fwd_tx = self.event_tx.clone();
            let fwd_cancel = cancel.clone();
            tokio::spawn(async move {
                loop {
                    let item = tokio::select! {
                        _ = fwd_cancel.cancelled() => break,
                        res = host_rx.recv() => res,
                    };
                    match item {
                        Ok(ev) => {
                            let _ = fwd_tx.send(ev);
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            })
        });

        let accept_task = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = accept_cancel.cancelled() => break,
                    res = listener.accept() => res,
                };
                let (stream, _) = match accepted {
                    Ok(pair) => pair,
                    Err(err) => {
                        log::warn!("[obs_ws] accept error: {err}");
                        tokio::time::sleep(Duration::from_millis(25)).await;
                        continue;
                    }
                };
                let h = host.clone();
                let pwd = password.clone();
                let etx = event_tx.clone();
                let cc = client_count.clone();
                let c = accept_cancel.clone();
                tokio::spawn(async move {
                    handle_client_connection(stream, h, pwd, etx, cc, c).await;
                });
            }
        });

        self.running = true;
        self.bound_port = actual_port;
        self.error = None;
        self.cancel = Some(cancel);
        self.accept_task = Some(accept_task);
        self.host_event_task = host_event_task;

        self.status()
    }

    /// Gracefully stops the obs-websocket v5 server and disconnects active sessions.
    pub async fn stop(&mut self) -> ExternalControlStatus {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        if let Some(task) = self.accept_task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
        }
        if let Some(task) = self.host_event_task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
        }
        self.running = false;
        self.error = None;
        self.status()
    }

    /// Restarts the obs-websocket server with new configuration (port, password, or LAN binding).
    pub async fn restart(&mut self, config: ObsWsConfig) -> ExternalControlStatus {
        self.stop().await;
        self.start(config).await
    }

    /// Returns whether the server is actively listening.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Returns the unified [`ExternalControlStatus`] snapshot.
    pub fn status(&self) -> ExternalControlStatus {
        ExternalControlStatus {
            obs_ws_running: self.running,
            obs_ws_port: self.bound_port,
            obs_ws_clients: self.client_count.load(Ordering::Relaxed),
            rpc_ws_port: self.rpc_ws_port,
            rpc_ws_clients: self.rpc_ws_clients.load(Ordering::Relaxed),
            error: self.error.clone(),
        }
    }
}

impl Drop for ObsWsServerHandle {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
    }
}

// ------------------------------------------------------------------ Unit Tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockState {
        scenes: Vec<(String, String, Vec<ObsSceneItemInfo>)>,
        active_scene_idx: usize,
        preview_scene_idx: usize,
        studio_mode: bool,
        transition: TransitionType,
        transition_duration_ms: u32,
        inputs: Vec<(String, String, String, bool, f64)>, // (id, name, kind, muted, db)
        streaming: bool,
        recording: bool,
        replay_active: bool,
        last_replay_path: Option<String>,
        virtual_cam: bool,
        triggered_hotkeys: Vec<String>,
    }

    struct MockHost {
        state: Mutex<MockState>,
        events: broadcast::Sender<ObsEvent>,
    }

    impl MockHost {
        fn new() -> Arc<Self> {
            let (events, _) = broadcast::channel(64);
            Arc::new(Self {
                events,
                state: Mutex::new(MockState {
                    scenes: vec![
                        (
                            "scene-gameplay".to_string(),
                            "Gameplay".to_string(),
                            vec![
                                ObsSceneItemInfo {
                                    scene_item_id: 1,
                                    scene_item_index: 0,
                                    source_name: "Monitor 1".to_string(),
                                    source_uuid: "src-mon1".to_string(),
                                    source_type: "OBS_SOURCE_TYPE_INPUT".to_string(),
                                    input_kind: Some("monitor_capture".to_string()),
                                    scene_item_enabled: true,
                                    scene_item_locked: false,
                                },
                                ObsSceneItemInfo {
                                    scene_item_id: 2,
                                    scene_item_index: 1,
                                    source_name: "Webcam".to_string(),
                                    source_uuid: "src-cam1".to_string(),
                                    source_type: "OBS_SOURCE_TYPE_INPUT".to_string(),
                                    input_kind: Some("dshow_input".to_string()),
                                    scene_item_enabled: true,
                                    scene_item_locked: false,
                                },
                            ],
                        ),
                        (
                            "scene-chat".to_string(),
                            "Just Chatting".to_string(),
                            vec![],
                        ),
                    ],
                    active_scene_idx: 0,
                    preview_scene_idx: 1,
                    studio_mode: false,
                    transition: TransitionType::Fade,
                    transition_duration_ms: 300,
                    inputs: vec![
                        (
                            "desktop".to_string(),
                            "Desktop Audio".to_string(),
                            "wasapi_output_capture".to_string(),
                            false,
                            0.0,
                        ),
                        (
                            "mic".to_string(),
                            "Mic/Aux".to_string(),
                            "wasapi_input_capture".to_string(),
                            false,
                            -3.0,
                        ),
                    ],
                    streaming: false,
                    recording: false,
                    replay_active: true,
                    last_replay_path: None,
                    virtual_cam: false,
                    triggered_hotkeys: Vec::new(),
                }),
            })
        }

        fn find_scene_idx(
            st: &MockState,
            name: Option<&str>,
            uuid: Option<&str>,
        ) -> ObsResult<usize> {
            st.scenes
                .iter()
                .position(|(id, n, _)| {
                    uuid.is_some_and(|u| u == id) || name.is_some_and(|nm| nm == n)
                })
                .ok_or_else(|| ObsRequestError::resource_not_found("Scene not found"))
        }

        fn find_input_idx(
            st: &MockState,
            name: Option<&str>,
            uuid: Option<&str>,
        ) -> ObsResult<usize> {
            st.inputs
                .iter()
                .position(|(id, n, _, _, _)| {
                    uuid.is_some_and(|u| u == id) || name.is_some_and(|nm| nm == n)
                })
                .ok_or_else(|| ObsRequestError::resource_not_found("Input not found"))
        }
    }

    impl ObsControlHost for MockHost {
        fn get_stats(&self) -> ObsFuture<'_, ObsResult<ObsStats>> {
            Box::pin(async {
                Ok(ObsStats {
                    active_fps: 60.0,
                    average_frame_render_time: 2.1,
                    cpu_usage: 8.5,
                    memory_usage: 240.0,
                    available_disk_space: 50_000.0,
                    render_skipped_frames: 0,
                    render_total_frames: 3600,
                    output_skipped_frames: 0,
                    output_total_frames: 3600,
                })
            })
        }

        fn get_hotkey_list(&self) -> ObsFuture<'_, ObsResult<Vec<String>>> {
            Box::pin(async {
                Ok(vec![
                    "toggle-mic-mute".to_string(),
                    "save-replay-clip".to_string(),
                ])
            })
        }

        fn trigger_hotkey_by_name(&self, hotkey_name: String) -> ObsFuture<'_, ObsResult<()>> {
            Box::pin(async move {
                self.state.lock().unwrap().triggered_hotkeys.push(hotkey_name);
                Ok(())
            })
        }

        fn get_scene_list(&self) -> ObsFuture<'_, ObsResult<ObsSceneList>> {
            Box::pin(async {
                let st = self.state.lock().unwrap();
                let scenes: Vec<ObsSceneInfo> = st
                    .scenes
                    .iter()
                    .enumerate()
                    .map(|(i, (id, name, _))| ObsSceneInfo {
                        scene_name: name.clone(),
                        scene_uuid: id.clone(),
                        scene_index: i,
                    })
                    .collect();
                let prog = &st.scenes[st.active_scene_idx];
                let prev = &st.scenes[st.preview_scene_idx];
                Ok(ObsSceneList {
                    current_program_scene_name: Some(prog.1.clone()),
                    current_program_scene_uuid: Some(prog.0.clone()),
                    current_preview_scene_name: Some(prev.1.clone()),
                    current_preview_scene_uuid: Some(prev.0.clone()),
                    scenes,
                })
            })
        }

        fn set_current_program_scene(
            &self,
            scene_name: Option<String>,
            scene_uuid: Option<String>,
        ) -> ObsFuture<'_, ObsResult<ObsSceneInfo>> {
            Box::pin(async move {
                let mut st = self.state.lock().unwrap();
                let idx = Self::find_scene_idx(&st, scene_name.as_deref(), scene_uuid.as_deref())?;
                st.active_scene_idx = idx;
                let info = ObsSceneInfo {
                    scene_name: st.scenes[idx].1.clone(),
                    scene_uuid: st.scenes[idx].0.clone(),
                    scene_index: idx,
                };
                let _ = self.events.send(ObsEvent::CurrentProgramSceneChanged {
                    scene_name: info.scene_name.clone(),
                    scene_uuid: info.scene_uuid.clone(),
                });
                Ok(info)
            })
        }

        fn set_current_preview_scene(
            &self,
            scene_name: Option<String>,
            scene_uuid: Option<String>,
        ) -> ObsFuture<'_, ObsResult<ObsSceneInfo>> {
            Box::pin(async move {
                let mut st = self.state.lock().unwrap();
                let idx = Self::find_scene_idx(&st, scene_name.as_deref(), scene_uuid.as_deref())?;
                st.preview_scene_idx = idx;
                Ok(ObsSceneInfo {
                    scene_name: st.scenes[idx].1.clone(),
                    scene_uuid: st.scenes[idx].0.clone(),
                    scene_index: idx,
                })
            })
        }

        fn get_studio_mode_enabled(&self) -> ObsFuture<'_, ObsResult<bool>> {
            Box::pin(async { Ok(self.state.lock().unwrap().studio_mode) })
        }

        fn set_studio_mode_enabled(&self, enabled: bool) -> ObsFuture<'_, ObsResult<()>> {
            Box::pin(async move {
                self.state.lock().unwrap().studio_mode = enabled;
                Ok(())
            })
        }

        fn trigger_studio_mode_transition(&self) -> ObsFuture<'_, ObsResult<()>> {
            Box::pin(async {
                let mut st = self.state.lock().unwrap();
                st.active_scene_idx = st.preview_scene_idx;
                Ok(())
            })
        }

        fn get_scene_item_list(
            &self,
            scene_name: Option<String>,
            scene_uuid: Option<String>,
        ) -> ObsFuture<'_, ObsResult<Vec<ObsSceneItemInfo>>> {
            Box::pin(async move {
                let st = self.state.lock().unwrap();
                let idx = Self::find_scene_idx(&st, scene_name.as_deref(), scene_uuid.as_deref())?;
                Ok(st.scenes[idx].2.clone())
            })
        }

        fn set_scene_item_enabled(
            &self,
            scene_name: Option<String>,
            scene_uuid: Option<String>,
            scene_item_id: i64,
            enabled: bool,
        ) -> ObsFuture<'_, ObsResult<()>> {
            Box::pin(async move {
                let mut st = self.state.lock().unwrap();
                let idx = Self::find_scene_idx(&st, scene_name.as_deref(), scene_uuid.as_deref())?;
                let item = st.scenes[idx]
                    .2
                    .iter_mut()
                    .find(|i| i.scene_item_id == scene_item_id)
                    .ok_or_else(|| ObsRequestError::resource_not_found("Scene item not found"))?;
                item.scene_item_enabled = enabled;
                Ok(())
            })
        }

        fn get_current_scene_transition(&self) -> ObsFuture<'_, ObsResult<ObsCurrentTransition>> {
            Box::pin(async {
                let st = self.state.lock().unwrap();
                let (name, kind, uuid, fixed) = transition_type_metadata(st.transition);
                Ok(ObsCurrentTransition {
                    transition_name: name.to_string(),
                    transition_uuid: uuid.to_string(),
                    transition_kind: kind.to_string(),
                    transition_fixed: fixed,
                    transition_duration: if fixed {
                        None
                    } else {
                        Some(st.transition_duration_ms)
                    },
                    transition_configurable: !fixed,
                })
            })
        }

        fn set_current_scene_transition(
            &self,
            transition_name: String,
        ) -> ObsFuture<'_, ObsResult<ObsCurrentTransition>> {
            Box::pin(async move {
                let parsed = obs_name_to_transition_type(&transition_name).ok_or_else(|| {
                    ObsRequestError::resource_not_found("Unknown transition name")
                })?;
                let mut st = self.state.lock().unwrap();
                st.transition = parsed;
                let (name, kind, uuid, fixed) = transition_type_metadata(parsed);
                Ok(ObsCurrentTransition {
                    transition_name: name.to_string(),
                    transition_uuid: uuid.to_string(),
                    transition_kind: kind.to_string(),
                    transition_fixed: fixed,
                    transition_duration: if fixed {
                        None
                    } else {
                        Some(st.transition_duration_ms)
                    },
                    transition_configurable: !fixed,
                })
            })
        }

        fn set_current_scene_transition_duration(
            &self,
            duration_ms: u32,
        ) -> ObsFuture<'_, ObsResult<()>> {
            Box::pin(async move {
                self.state.lock().unwrap().transition_duration_ms = duration_ms;
                Ok(())
            })
        }

        fn get_input_list(
            &self,
            input_kind: Option<String>,
        ) -> ObsFuture<'_, ObsResult<Vec<ObsInputInfo>>> {
            Box::pin(async move {
                let st = self.state.lock().unwrap();
                let list = st
                    .inputs
                    .iter()
                    .filter(|(_, _, k, _, _)| input_kind.as_deref().is_none_or(|f| f == k))
                    .map(|(id, name, kind, _, _)| ObsInputInfo {
                        input_name: name.clone(),
                        input_uuid: id.clone(),
                        input_kind: kind.clone(),
                        unversioned_input_kind: kind.clone(),
                    })
                    .collect();
                Ok(list)
            })
        }

        fn get_input_mute(
            &self,
            input_name: Option<String>,
            input_uuid: Option<String>,
        ) -> ObsFuture<'_, ObsResult<bool>> {
            Box::pin(async move {
                let st = self.state.lock().unwrap();
                let idx = Self::find_input_idx(&st, input_name.as_deref(), input_uuid.as_deref())?;
                Ok(st.inputs[idx].3)
            })
        }

        fn set_input_mute(
            &self,
            input_name: Option<String>,
            input_uuid: Option<String>,
            muted: bool,
        ) -> ObsFuture<'_, ObsResult<()>> {
            Box::pin(async move {
                let mut st = self.state.lock().unwrap();
                let idx = Self::find_input_idx(&st, input_name.as_deref(), input_uuid.as_deref())?;
                st.inputs[idx].3 = muted;
                Ok(())
            })
        }

        fn toggle_input_mute(
            &self,
            input_name: Option<String>,
            input_uuid: Option<String>,
        ) -> ObsFuture<'_, ObsResult<bool>> {
            Box::pin(async move {
                let mut st = self.state.lock().unwrap();
                let idx = Self::find_input_idx(&st, input_name.as_deref(), input_uuid.as_deref())?;
                st.inputs[idx].3 = !st.inputs[idx].3;
                let muted = st.inputs[idx].3;
                let _ = self.events.send(ObsEvent::InputMuteStateChanged {
                    input_name: st.inputs[idx].1.clone(),
                    input_uuid: st.inputs[idx].0.clone(),
                    input_muted: muted,
                });
                Ok(muted)
            })
        }

        fn get_input_volume(
            &self,
            input_name: Option<String>,
            input_uuid: Option<String>,
        ) -> ObsFuture<'_, ObsResult<ObsInputVolume>> {
            Box::pin(async move {
                let st = self.state.lock().unwrap();
                let idx = Self::find_input_idx(&st, input_name.as_deref(), input_uuid.as_deref())?;
                Ok(ObsInputVolume::from_db(st.inputs[idx].4))
            })
        }

        fn set_input_volume(
            &self,
            input_name: Option<String>,
            input_uuid: Option<String>,
            volume_db: f64,
        ) -> ObsFuture<'_, ObsResult<ObsInputVolume>> {
            Box::pin(async move {
                let mut st = self.state.lock().unwrap();
                let idx = Self::find_input_idx(&st, input_name.as_deref(), input_uuid.as_deref())?;
                st.inputs[idx].4 = volume_db;
                Ok(ObsInputVolume::from_db(volume_db))
            })
        }

        fn get_stream_status(&self) -> ObsFuture<'_, ObsResult<ObsStreamStatus>> {
            Box::pin(async {
                let st = self.state.lock().unwrap();
                Ok(ObsStreamStatus {
                    output_active: st.streaming,
                    output_reconnecting: false,
                    output_duration_ms: if st.streaming { 12_345 } else { 0 },
                    output_congestion: 0.0,
                    output_bytes: 0,
                    output_skipped_frames: 0,
                    output_total_frames: 0,
                })
            })
        }

        fn start_stream(&self) -> ObsFuture<'_, ObsResult<()>> {
            Box::pin(async {
                self.state.lock().unwrap().streaming = true;
                Ok(())
            })
        }

        fn stop_stream(&self) -> ObsFuture<'_, ObsResult<()>> {
            Box::pin(async {
                self.state.lock().unwrap().streaming = false;
                Ok(())
            })
        }

        fn get_record_status(&self) -> ObsFuture<'_, ObsResult<ObsRecordStatus>> {
            Box::pin(async {
                let st = self.state.lock().unwrap();
                Ok(ObsRecordStatus {
                    output_active: st.recording,
                    output_paused: false,
                    output_duration_ms: 0,
                    output_bytes: 0,
                    output_path: None,
                })
            })
        }

        fn start_record(&self) -> ObsFuture<'_, ObsResult<()>> {
            Box::pin(async {
                self.state.lock().unwrap().recording = true;
                Ok(())
            })
        }

        fn stop_record(&self) -> ObsFuture<'_, ObsResult<Option<String>>> {
            Box::pin(async {
                self.state.lock().unwrap().recording = false;
                Ok(Some("/tmp/prismcast-rec.mp4".to_string()))
            })
        }

        fn get_replay_buffer_status(&self) -> ObsFuture<'_, ObsResult<ObsReplayBufferStatus>> {
            Box::pin(async {
                let st = self.state.lock().unwrap();
                Ok(ObsReplayBufferStatus {
                    output_active: st.replay_active,
                    last_saved_path: st.last_replay_path.clone(),
                })
            })
        }

        fn start_replay_buffer(&self) -> ObsFuture<'_, ObsResult<()>> {
            Box::pin(async {
                self.state.lock().unwrap().replay_active = true;
                Ok(())
            })
        }

        fn stop_replay_buffer(&self) -> ObsFuture<'_, ObsResult<()>> {
            Box::pin(async {
                self.state.lock().unwrap().replay_active = false;
                Ok(())
            })
        }

        fn save_replay_buffer(&self) -> ObsFuture<'_, ObsResult<Option<String>>> {
            Box::pin(async {
                let path = "/tmp/prismcast-replay.mp4".to_string();
                self.state.lock().unwrap().last_replay_path = Some(path.clone());
                Ok(Some(path))
            })
        }

        fn get_virtual_cam_status(&self) -> ObsFuture<'_, ObsResult<bool>> {
            Box::pin(async { Ok(self.state.lock().unwrap().virtual_cam) })
        }

        fn start_virtual_cam(&self) -> ObsFuture<'_, ObsResult<()>> {
            Box::pin(async {
                self.state.lock().unwrap().virtual_cam = true;
                let _ = self.events.send(ObsEvent::VirtualcamStateChanged {
                    output_active: true,
                    output_state: "OBS_WEBSOCKET_OUTPUT_STARTED".to_string(),
                });
                Ok(())
            })
        }

        fn stop_virtual_cam(&self) -> ObsFuture<'_, ObsResult<()>> {
            Box::pin(async {
                self.state.lock().unwrap().virtual_cam = false;
                let _ = self.events.send(ObsEvent::VirtualcamStateChanged {
                    output_active: false,
                    output_state: "OBS_WEBSOCKET_OUTPUT_STOPPED".to_string(),
                });
                Ok(())
            })
        }

        fn subscribe_events(&self) -> Option<broadcast::Receiver<ObsEvent>> {
            Some(self.events.subscribe())
        }
    }

    async fn read_next_json<S>(ws: &mut S) -> Value
    where
        S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
    {
        let msg = tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .expect("timed out waiting for ws frame")
            .expect("ws stream ended unexpectedly")
            .expect("ws read error");
        let Message::Text(txt) = msg else {
            panic!("expected Text message, got {msg:?}");
        };
        serde_json::from_str(&txt).expect("valid JSON frame")
    }

    async fn read_until_op<S>(ws: &mut S, expected_op: u8) -> Value
    where
        S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
    {
        for _ in 0..16 {
            let val = read_next_json(ws).await;
            if val.get("op").and_then(|v| v.as_u64()) == Some(expected_op as u64) {
                return val;
            }
        }
        panic!("did not receive op {expected_op}");
    }

    #[test]
    fn test_compute_obs_auth_official_v5_spec_vector() {
        // Official obs-websocket v5 protocol specification vector:
        // secret = base64(sha256("supersecretpassword" + "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI="))
        //        = "H1IfVz1pSREUQzbFTVnX/Tyb+gMhMik5x7yUBCY0PTs="
        // auth   = base64(sha256(secret + "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeagNgGY3CDf8="))
        //        = "MSyByQH/oVoCRqijf2Vtog62v4/MQhHWBmtIRyz9mMQ="
        let auth = compute_obs_auth(
            "supersecretpassword",
            "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=",
            "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeagNgGY3CDf8=",
        );
        assert_eq!(auth, "MSyByQH/oVoCRqijf2Vtog62v4/MQhHWBmtIRyz9mMQ=");
        assert!(verify_obs_auth(&auth, Some(&auth)));
        assert!(!verify_obs_auth(&auth, Some("wrong")));
        assert!(!verify_obs_auth(&auth, None));
    }

    #[test]
    fn test_format_timecode_and_db_mul_helpers() {
        assert_eq!(format_obs_timecode(0), "00:00:00.000");
        assert_eq!(format_obs_timecode(3_723_456), "01:02:03.456");
        assert!((db_to_mul(0.0) - 1.0).abs() < 1e-6);
        assert!((mul_to_db(1.0) - 0.0).abs() < 1e-6);
        assert_eq!(obs_name_to_transition_type("Fade to Black"), Some(TransitionType::FadeToBlack));
        assert_eq!(obs_name_to_transition_type("luma-wipe"), Some(TransitionType::LumaWipe));
    }

    #[tokio::test]
    async fn test_obs_ws_v5_full_roundtrip_and_wrong_password_4009() {
        let host = MockHost::new();
        let mut server = ObsWsServerHandle::new(host.clone());
        let password = "stream-deck-secret-99".to_string();
        let st = server
            .start(ObsWsConfig {
                port: 0, // ephemeral OS-assigned port
                password: Some(password.clone()),
                allow_lan: false,
                rpc_ws_port: RUST_RPC_PORT,
            })
            .await;
        assert!(st.obs_ws_running, "server error: {:?}", st.error);
        let port = st.obs_ws_port;
        assert_ne!(port, 0);

        // --- 1. Wrong password must be rejected with OBS close code 4009 ---
        {
            let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}"))
                .await
                .expect("connect bad-auth client");
            let hello = read_next_json(&mut ws).await;
            assert_eq!(hello["op"], OP_HELLO);
            assert!(hello["d"]["authentication"]["challenge"].is_string());
            assert!(hello["d"]["authentication"]["salt"].is_string());

            let bad_ident = json!({
                "op": OP_IDENTIFY,
                "d": {
                    "rpcVersion": 1,
                    "authentication": "definitely-invalid-auth",
                }
            });
            ws.send(Message::Text(bad_ident.to_string().into()))
                .await
                .unwrap();

            let close_frame = tokio::time::timeout(Duration::from_secs(3), async {
                while let Some(item) = ws.next().await {
                    if let Ok(Message::Close(Some(cf))) = item {
                        return Some(cf);
                    }
                }
                None
            })
            .await
            .expect("timed out waiting for close frame")
            .expect("expected CloseFrame(4009)");
            assert_eq!(u16::from(close_frame.code), CLOSE_AUTHENTICATION_FAILED);
        }

        // --- 2. Valid authenticated session + full Stream Deck / Companion round-trip ---
        {
            let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}"))
                .await
                .expect("connect valid client");

            let hello = read_next_json(&mut ws).await;
            assert_eq!(hello["op"], OP_HELLO);
            assert_eq!(hello["d"]["obsWebSocketVersion"], OBS_WS_PROTOCOL_VERSION);
            assert_eq!(hello["d"]["rpcVersion"], 1);
            let salt = hello["d"]["authentication"]["salt"].as_str().unwrap();
            let challenge = hello["d"]["authentication"]["challenge"].as_str().unwrap();

            let auth = compute_obs_auth(&password, salt, challenge);
            let ident = json!({
                "op": OP_IDENTIFY,
                "d": {
                    "rpcVersion": 1,
                    "authentication": auth,
                    "eventSubscriptions": EVENT_SUB_ALL,
                }
            });
            ws.send(Message::Text(ident.to_string().into()))
                .await
                .unwrap();

            let identified = read_next_json(&mut ws).await;
            assert_eq!(identified["op"], OP_IDENTIFIED);
            assert_eq!(identified["d"]["negotiatedRpcVersion"], 1);
            assert!(server.status().obs_ws_clients >= 1);

            // GetVersion
            ws.send(Message::Text(
                json!({
                    "op": OP_REQUEST,
                    "d": { "requestType": "GetVersion", "requestId": "req-ver" }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
            let ver = read_until_op(&mut ws, OP_REQUEST_RESPONSE).await;
            assert_eq!(ver["d"]["requestId"], "req-ver");
            assert_eq!(ver["d"]["requestStatus"]["result"], true);
            assert_eq!(ver["d"]["requestStatus"]["code"], STATUS_SUCCESS);
            assert_eq!(ver["d"]["responseData"]["obsWebSocketVersion"], OBS_WS_PROTOCOL_VERSION);

            // GetSceneList
            ws.send(Message::Text(
                json!({
                    "op": OP_REQUEST,
                    "d": { "requestType": "GetSceneList", "requestId": "req-scenes" }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
            let scenes = read_until_op(&mut ws, OP_REQUEST_RESPONSE).await;
            assert_eq!(scenes["d"]["responseData"]["currentProgramSceneName"], "Gameplay");
            assert_eq!(scenes["d"]["responseData"]["scenes"].as_array().unwrap().len(), 2);

            // SetCurrentProgramScene -> also triggers CurrentProgramSceneChanged Op 5 event
            ws.send(Message::Text(
                json!({
                    "op": OP_REQUEST,
                    "d": {
                        "requestType": "SetCurrentProgramScene",
                        "requestId": "req-set-scene",
                        "requestData": { "sceneName": "Just Chatting" }
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
            let set_sc = read_until_op(&mut ws, OP_REQUEST_RESPONSE).await;
            assert_eq!(set_sc["d"]["requestId"], "req-set-scene");
            assert_eq!(set_sc["d"]["requestStatus"]["result"], true);

            // ToggleInputMute
            ws.send(Message::Text(
                json!({
                    "op": OP_REQUEST,
                    "d": {
                        "requestType": "ToggleInputMute",
                        "requestId": "req-mute",
                        "requestData": { "inputName": "Mic/Aux" }
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
            let mute_res = read_until_op(&mut ws, OP_REQUEST_RESPONSE).await;
            assert_eq!(mute_res["d"]["requestId"], "req-mute");
            assert_eq!(mute_res["d"]["responseData"]["inputMuted"], true);

            // ToggleVirtualCam
            ws.send(Message::Text(
                json!({
                    "op": OP_REQUEST,
                    "d": {
                        "requestType": "ToggleVirtualCam",
                        "requestId": "req-vcam"
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
            let vcam_res = read_until_op(&mut ws, OP_REQUEST_RESPONSE).await;
            assert_eq!(vcam_res["d"]["requestId"], "req-vcam");
            assert_eq!(vcam_res["d"]["responseData"]["outputActive"], true);

            // RequestBatch (Stream Deck Multi-Action: switch transition + mute + toggle stream)
            ws.send(Message::Text(
                json!({
                    "op": OP_REQUEST_BATCH,
                    "d": {
                        "requestId": "batch-sd-1",
                        "haltOnFailure": true,
                        "requests": [
                            {
                                "requestType": "SetCurrentSceneTransition",
                                "requestId": "b1",
                                "requestData": { "transitionName": "Stinger" }
                            },
                            {
                                "requestType": "GetSceneItemId",
                                "requestId": "b2",
                                "requestData": { "sceneName": "Gameplay", "sourceName": "Webcam" }
                            },
                            {
                                "requestType": "SetSceneItemEnabled",
                                "requestId": "b3",
                                "requestData": {
                                    "sceneName": "Gameplay",
                                    "sceneItemId": 2,
                                    "sceneItemEnabled": false
                                }
                            },
                            {
                                "requestType": "GetSceneItemEnabled",
                                "requestId": "b4",
                                "requestData": { "sceneName": "Gameplay", "sceneItemId": 2 }
                            }
                        ]
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();

            let batch_res = read_until_op(&mut ws, OP_REQUEST_BATCH_RESPONSE).await;
            assert_eq!(batch_res["d"]["requestId"], "batch-sd-1");
            let results = batch_res["d"]["results"].as_array().unwrap();
            assert_eq!(results.len(), 4);
            assert_eq!(results[0]["requestStatus"]["result"], true);
            assert_eq!(results[1]["responseData"]["sceneItemId"], 2);
            assert_eq!(results[2]["requestStatus"]["result"], true);
            assert_eq!(results[3]["responseData"]["sceneItemEnabled"], false);

            let _ = ws.close(None).await;
        }

        let stopped = server.stop().await;
        assert!(!stopped.obs_ws_running);
    }
}
