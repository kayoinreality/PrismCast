//! Motor autoritativo unificado (`AppEngine`) + servidor RPC WebSocket autenticado.
//!
//! - Agrega todos os subsistemas Rust (`SettingsStore`, `TwitchDeviceLoginManager`,
//!   `TwitchApi`, `MultiChatHub`, `MultistreamEngine`, `RecorderEngine`,
//!   `AudioMixerEngine`, `DiscoveryEngine`, `SignalingServer`, `SignalingClient`,
//!   `HotkeyManager`).
//! - Elimina o deadlock de `recorder.saveReplay` (libera o lock antes do remux).
//! - Autenticação por token aleatório de 256-bit (`subtle::ConstantTimeEq`) e
//!   rejeição estrita de cabeçalho `Origin` no handshake (impede CSWSH).
//! - Canal único de escrita WebSocket + suporte a frames binários sem Base64:
//!   - `0x01` = chunk WebM Horizontal (`multistream`)
//!   - `0x02` = chunk WebM Vertical (`multistream`)
//!   - `0x03` = chunk WebM (`recorder`)
//!   - `0x10` = PCM f32le externo `[0x10][u16_be id_len][id_utf8][pcm_bytes...]`

use crate::audio_mixer::AudioMixerEngine;
use crate::auth_twitch::{
    validate_stream_key, TokenVault, TwitchApi, TwitchDeviceLoginManager,
};
use crate::capture::{self, CaptureManager, DirtyRect};
use crate::compositor::Compositor;
use crate::discovery::{AnnounceState, DiscoveryEngine};
use crate::firewall;
use crate::hotkeys::{detect_conflicts, HotkeyManager};
use crate::kit;
use crate::multichat::MultiChatHub;
use crate::multistream::MultistreamEngine;
use crate::net;
use crate::protocol::{
    AppSettings, CanvasOrientation, Destination, HostToViewer, KitConfig, LiveInfo, MultiChatEvent,
    PlatformId, SourceKind, TelemetryStats, TransitionType, TwitchLoginState, ViewerToHost,
    MAX_VIEWERS, RUST_RPC_PORT, SIGNALING_PORT,
};
use crate::recorder::{remux_to_mp4, RecorderEngine};
use crate::rtc_host::{apply_codec_preference, prefer_opus_stereo, RtcHostState};
use crate::signaling::{
    HostSessionConfig, SignalingClient, SignalingServer, ViewerClosedReason, ViewerConnectOutcome,
};
use crate::store::{
    stream_key_secret, twitch_refresh_secret, SettingsStore,
};
use futures_util::{SinkExt, StreamExt};
use rand::{Rng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

pub const BIN_TAG_MULTISTREAM_HORIZ: u8 = 0x01;
pub const BIN_TAG_MULTISTREAM_VERT: u8 = 0x02;
pub const BIN_TAG_RECORDER_CHUNK: u8 = 0x03;
pub const BIN_TAG_AUDIO_PCM: u8 = 0x10;

const MAX_RPC_WS_MESSAGE_BYTES: usize = 32 * 1024 * 1024;
const WS_WRITE_QUEUE_CAP: usize = 256;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn generate_pin() -> String {
    let n: u32 = rand::rng().random_range(0..1_000_000);
    format!("{n:06}")
}

/// Gera token aleatório de 256 bits em hexadecimal para autenticar a ponte RPC local.
pub fn generate_session_token() -> String {
    let mut raw = [0u8; 32];
    rand::rng().fill_bytes(&mut raw);
    let mut out = String::with_capacity(64);
    for b in raw {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Comparação em tempo constante do token RPC.
pub fn verify_token_constant_time(expected: &str, provided: Option<&str>) -> bool {
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

#[derive(Debug, Clone, Serialize)]
pub struct RpcEvent {
    pub event: String,
    pub payload: Value,
}

#[derive(Debug, Deserialize)]
pub struct RpcRequest {
    pub id: Option<String>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

struct StoreTokenVault {
    store: Arc<SettingsStore>,
}

impl TokenVault for StoreTokenVault {
    fn get_refresh_token(&self, destination_id: &str) -> Option<String> {
        self.store.get_secret(&twitch_refresh_secret(destination_id))
    }

    fn set_refresh_token(&self, destination_id: &str, token: Option<&str>) {
        let _ = self
            .store
            .set_secret(&twitch_refresh_secret(destination_id), token);
    }
}

/// Estado autoritativo único do PrismCast Studio em Rust.
pub struct AppEngine {
    pub store: Arc<SettingsStore>,
    pub twitch_login: Arc<TwitchDeviceLoginManager>,
    pub twitch_api: Arc<TwitchApi>,
    pub multichat: Arc<MultiChatHub>,
    pub multistream: Arc<AsyncMutex<MultistreamEngine>>,
    pub recorder: Arc<Mutex<RecorderEngine>>,
    pub audio_mixer: Arc<Mutex<AudioMixerEngine>>,
    pub capture: Arc<Mutex<CaptureManager>>,
    pub compositor: Arc<Mutex<Compositor>>,
    pub rtc_host: Arc<Mutex<RtcHostState>>,
    pub discovery: Arc<AsyncMutex<DiscoveryEngine>>,
    pub signaling: Arc<AsyncMutex<SignalingServer>>,
    pub viewer_client: Arc<AsyncMutex<SignalingClient>>,
    hotkeys: Mutex<Option<HotkeyManager>>,
    evt_tx: broadcast::Sender<RpcEvent>,
    bg_cancel: CancellationToken,
}

impl AppEngine {
    /// Cria e inicializa o `AppEngine` dentro de um runtime Tokio ativo.
    pub fn new() -> Arc<Self> {
        let store = Arc::new(SettingsStore::new());
        Self::with_store(store, true)
    }

    /// Construtor determinístico para testes (permite diretório isolado e desativar hotkeys OS).
    pub fn with_store(store: Arc<SettingsStore>, spawn_hotkeys: bool) -> Arc<Self> {
        let initial = store.snapshot();
        let vault = Arc::new(StoreTokenVault {
            store: store.clone(),
        });
        let twitch_api = Arc::new(TwitchApi::new(vault));
        let twitch_login = Arc::new(TwitchDeviceLoginManager::new());
        let multichat = MultiChatHub::new();

        let mut recorder = RecorderEngine::new();
        recorder.set_recording_dir(initial.recording_dir.as_deref());
        let _ = recorder.set_replay_buffer(true, initial.studio.replay_buffer_seconds);

        let mut multistream = MultistreamEngine::new();
        multistream.set_ffmpeg_path(initial.ffmpeg_path.clone());

        let audio_mixer = AudioMixerEngine::new(initial.audio_mixer.clone());
        let capture = CaptureManager::new(initial.studio.fps);
        let mut compositor = Compositor::new(initial.studio.clone());
        let prog_scene = initial
            .scenes
            .scenes
            .iter()
            .find(|s| s.id == initial.scenes.active_scene_id)
            .cloned()
            .or_else(|| initial.scenes.scenes.first().cloned());
        let prev_scene = initial
            .scenes
            .scenes
            .iter()
            .find(|s| s.id == initial.scenes.preview_scene_id)
            .cloned();
        compositor.set_scenes(
            prog_scene,
            prev_scene,
            initial.scenes.transition_type,
            initial.scenes.transition_duration_ms,
            now_ms(),
        );
        let rtc_host = RtcHostState::new(
            initial.quality.clone(),
            initial.audio_mixer.party_voice_enabled,
        );
        let discovery = DiscoveryEngine::new();
        discovery.set_preferred_interface(initial.preferred_interface.clone());

        let (evt_tx, _) = broadcast::channel::<RpcEvent>(512);
        let bg_cancel = CancellationToken::new();

        let engine = Arc::new(Self {
            store,
            twitch_login,
            twitch_api,
            multichat,
            multistream: Arc::new(AsyncMutex::new(multistream)),
            recorder: Arc::new(Mutex::new(recorder)),
            audio_mixer: Arc::new(Mutex::new(audio_mixer)),
            capture: Arc::new(Mutex::new(capture)),
            compositor: Arc::new(Mutex::new(compositor)),
            rtc_host: Arc::new(Mutex::new(rtc_host)),
            discovery: Arc::new(AsyncMutex::new(discovery)),
            signaling: Arc::new(AsyncMutex::new(SignalingServer::new())),
            viewer_client: Arc::new(AsyncMutex::new(SignalingClient::new())),
            hotkeys: Mutex::new(None),
            evt_tx,
            bg_cancel,
        });

        if spawn_hotkeys {
            let evt_hk = engine.evt_tx.clone();
            let hk = HotkeyManager::spawn(initial.hotkeys.clone(), move |action| {
                let _ = evt_hk.send(RpcEvent {
                    event: "hotkey:triggered".to_string(),
                    payload: json!({ "action": action.as_str() }),
                });
            });
            *engine.hotkeys.lock().unwrap_or_else(|e| e.into_inner()) = Some(hk);
        }

        // Encaminha eventos do MultiChatHub para o barramento unificado
        let mut chat_rx = engine.multichat.subscribe();
        let eng_chat = Arc::downgrade(&engine);
        let cancel_chat = engine.bg_cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel_chat.cancelled() => break,
                    item = chat_rx.recv() => match item {
                        Ok(ev) => {
                            if let Some(eng) = eng_chat.upgrade() {
                                eng.emit_serializable("chat:event", &ev);
                            } else {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        });

        // Configura multichat inicial se já houver canais salvos
        let chat_cfg = initial.chat.clone();
        let linked_login = initial
            .destinations
            .iter()
            .find(|d| d.platform == PlatformId::Twitch)
            .and_then(|d| d.account.as_ref())
            .map(|a| a.login.clone());
        let hub_init = engine.multichat.clone();
        tokio::spawn(async move {
            hub_init
                .configure(&chat_cfg, linked_login.as_deref(), false)
                .await;
        });

        // Loop periódico leve: VU meters (25 Hz) e telemetria real (1 Hz)
        let eng_bg = Arc::downgrade(&engine);
        let cancel_bg = engine.bg_cancel.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(40));
            let mut counter = 0u32;
            loop {
                tokio::select! {
                    _ = cancel_bg.cancelled() => break,
                    _ = tick.tick() => {}
                }
                let Some(eng) = eng_bg.upgrade() else {
                    break;
                };
                if eng.evt_tx.receiver_count() == 0 {
                    continue;
                }
                let levels = {
                    let mixer = eng.audio_mixer.lock().unwrap_or_else(|e| e.into_inner());
                    mixer.read_levels()
                };
                eng.emit_serializable("audio:levels", &levels);

                counter = counter.wrapping_add(1);
                if counter % 25 == 0 {
                    let stats = eng.collect_telemetry().await;
                    eng.emit_serializable("telemetry:stats", &stats);
                }
            }
        });

        engine
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RpcEvent> {
        self.evt_tx.subscribe()
    }

    pub fn emit(&self, event: &str, payload: Value) {
        let _ = self.evt_tx.send(RpcEvent {
            event: event.to_string(),
            payload,
        });
    }

    pub fn emit_serializable<T: Serialize>(&self, event: &str, payload: &T) {
        if let Ok(val) = serde_json::to_value(payload) {
            self.emit(event, val);
        }
    }

    pub fn is_busy(&self) -> bool {
        let rec_active = self
            .recorder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status()
            .recording;
        let ms_active = self
            .multistream
            .try_lock()
            .map(|m| m.engine_status().active)
            .unwrap_or(false);
        rec_active || ms_active
    }

    pub async fn collect_telemetry(&self) -> TelemetryStats {
        let settings = self.store.snapshot();
        let (prog, encoder, ffmpeg_found) = {
            let ms = self.multistream.lock().await;
            let st = ms.engine_status();
            (ms.telemetry_snapshot(), st.encoder, st.ffmpeg_found)
        };
        let comp_tel = self
            .compositor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .telemetry();
        let rec_status = self
            .recorder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status();
        let muted_speaking = self
            .audio_mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_mic_muted_speaking();

        let compositor_fps = if comp_tel.compositor_fps > 0.0 {
            comp_tel.compositor_fps
        } else {
            prog.fps
        };

        TelemetryStats {
            compositor_fps,
            target_fps: settings.studio.fps,
            render_ms_p95: comp_tel.render_ms_p95,
            dropped_frames: comp_tel.dropped_frames.saturating_add(prog.dropped_frames),
            encode_bitrate_kbps: prog.bitrate_kbps,
            encoder: encoder.clone(),
            cpu_fallback: ffmpeg_found && encoder == "libx264",
            mic_muted_speaking: muted_speaking,
            replay_buffered_seconds: rec_status.replayBufferedSeconds,
        }
    }

    fn linked_twitch(&self, settings: &AppSettings) -> Option<Destination> {
        settings
            .destinations
            .iter()
            .find(|d| d.platform == PlatformId::Twitch && d.account.is_some())
            .cloned()
    }

    pub async fn handle_binary(&self, frame: &[u8]) {
        let Some((&tag, payload)) = frame.split_first() else {
            return;
        };
        match tag {
            BIN_TAG_MULTISTREAM_HORIZ => {
                self.multistream
                    .lock()
                    .await
                    .push_chunk(CanvasOrientation::Horizontal, payload);
            }
            BIN_TAG_MULTISTREAM_VERT => {
                self.multistream
                    .lock()
                    .await
                    .push_chunk(CanvasOrientation::Vertical, payload);
            }
            BIN_TAG_RECORDER_CHUNK => {
                let mut rec = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
                rec.push_media_chunk(payload);
            }
            BIN_TAG_AUDIO_PCM => {
                if payload.len() < 2 {
                    return;
                }
                let id_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
                if payload.len() < 2 + id_len {
                    return;
                }
                if let Ok(ch_id) = std::str::from_utf8(&payload[2..2 + id_len]) {
                    let pcm = &payload[2 + id_len..];
                    self.audio_mixer
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push_external_pcm_bytes(ch_id, pcm);
                }
            }
            _ => {}
        }
    }

    pub async fn handle_command(self: &Arc<Self>, method: &str, params: Value) -> Value {
        match method {
            "app.info" => {
                let host = hostname::get()
                    .ok()
                    .and_then(|h| h.into_string().ok())
                    .unwrap_or_else(|| "PrismCast-PC".to_string());
                let (encoder, ffmpeg_found) = {
                    let mut ms = self.multistream.lock().await;
                    let probe = ms.refresh_probe().await;
                    (probe.encoder, probe.ffmpeg_found)
                };
                json!({
                    "name": "PrismCast Studio",
                    "version": env!("CARGO_PKG_VERSION"),
                    "hostname": host,
                    "engine": "rust-native",
                    "hardwareEncoder": encoder,
                    "ffmpegFound": ffmpeg_found,
                })
            }

            // ------------------------------------------------ Settings & Sistema
            "settings.get" => serde_json::to_value(self.store.snapshot()).unwrap_or(Value::Null),
            "settings.set" => {
                let prev = self.store.snapshot();
                let next = self.store.apply_patch(params);

                if next.preferred_interface != prev.preferred_interface {
                    self.discovery
                        .lock()
                        .await
                        .set_preferred_interface(next.preferred_interface.clone());
                }
                if next.recording_dir != prev.recording_dir {
                    self.recorder
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .set_recording_dir(next.recording_dir.as_deref());
                }
                if next.ffmpeg_path != prev.ffmpeg_path {
                    self.multistream
                        .lock()
                        .await
                        .set_ffmpeg_path(next.ffmpeg_path.clone());
                }
                if serde_json::to_string(&next.audio_mixer).ok()
                    != serde_json::to_string(&prev.audio_mixer).ok()
                {
                    self.audio_mixer
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .apply_settings(next.audio_mixer.clone());
                    self.rtc_host
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .set_party_voice_enabled(next.audio_mixer.party_voice_enabled);
                }
                if serde_json::to_string(&next.quality).ok()
                    != serde_json::to_string(&prev.quality).ok()
                {
                    self.rtc_host
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .update_settings(next.quality.clone());
                }
                if serde_json::to_string(&next.studio).ok()
                    != serde_json::to_string(&prev.studio).ok()
                {
                    self.capture
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .set_fps(next.studio.fps);
                    self.compositor
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .apply_settings(next.studio.clone());
                }
                if serde_json::to_string(&next.scenes).ok()
                    != serde_json::to_string(&prev.scenes).ok()
                {
                    let prog_scene = next
                        .scenes
                        .scenes
                        .iter()
                        .find(|s| s.id == next.scenes.active_scene_id)
                        .cloned();
                    let prev_scene = next
                        .scenes
                        .scenes
                        .iter()
                        .find(|s| s.id == next.scenes.preview_scene_id)
                        .cloned();
                    self.compositor
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .set_scenes(
                            prog_scene,
                            prev_scene,
                            next.scenes.transition_type,
                            next.scenes.transition_duration_ms,
                            now_ms(),
                        );
                }
                if serde_json::to_string(&next.hotkeys).ok()
                    != serde_json::to_string(&prev.hotkeys).ok()
                {
                    if let Some(hk) = self
                        .hotkeys
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .as_ref()
                    {
                        hk.update_bindings(next.hotkeys.clone());
                    }
                }
                if serde_json::to_string(&next.chat).ok() != serde_json::to_string(&prev.chat).ok() {
                    let linked_login = self
                        .linked_twitch(&next)
                        .and_then(|d| d.account)
                        .map(|a| a.login);
                    self.multichat
                        .configure(&next.chat, linked_login.as_deref(), false)
                        .await;
                }

                let val = serde_json::to_value(&next).unwrap_or(Value::Null);
                self.emit("settings:changed", val.clone());
                val
            }
            "hotkeys.conflicts" => {
                let bindings = self.store.snapshot().hotkeys;
                let conflicts: Vec<Value> = detect_conflicts(&bindings)
                    .into_iter()
                    .map(|c| {
                        json!({
                            "accelerator": c.accelerator,
                            "actions": c.actions.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                json!(conflicts)
            }
            "net.interfaces" => {
                let list = tokio::task::spawn_blocking(net::list_interfaces)
                    .await
                    .unwrap_or_default();
                serde_json::to_value(list).unwrap_or(json!([]))
            }
            "net.refreshInterfaces" => {
                let list = tokio::task::spawn_blocking(|| {
                    net::invalidate_cache();
                    net::list_interfaces()
                })
                .await
                .unwrap_or_default();
                serde_json::to_value(list).unwrap_or(json!([]))
            }
            "firewall.status" => {
                let st = tokio::task::spawn_blocking(firewall::check_status).await;
                match st {
                    Ok(v) => serde_json::to_value(v).unwrap_or(Value::Null),
                    Err(_) => Value::Null,
                }
            }
            "firewall.authorize" => {
                let res = tokio::task::spawn_blocking(firewall::authorize).await;
                match res {
                    Ok(Ok(())) => json!({ "ok": true }),
                    Ok(Err(e)) => json!({ "ok": false, "error": e }),
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                }
            }

            // ------------------------------------------------ Kit de Widgets (sem JS/vm)
            "kit.load" => {
                let path = params.get("path").and_then(|v| v.as_str()).map(str::to_string);
                let res = tokio::task::spawn_blocking(move || kit::load_kit(path.as_deref()))
                    .await
                    .ok()
                    .flatten();
                serde_json::to_value(res).unwrap_or(Value::Null)
            }
            "kit.readConfig" => {
                let path = params
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let res = tokio::task::spawn_blocking(move || kit::read_kit_config(&path))
                    .await
                    .ok()
                    .flatten();
                serde_json::to_value(res).unwrap_or(Value::Null)
            }
            "kit.writeConfig" => {
                let path = params
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let cfg_val = params.get("config").cloned().unwrap_or(Value::Null);
                let Ok(cfg) = serde_json::from_value::<KitConfig>(cfg_val) else {
                    return json!({ "ok": false, "error": "Configuração do kit inválida." });
                };
                match tokio::task::spawn_blocking(move || kit::write_kit_config(&path, &cfg)).await {
                    Ok(Ok(())) => json!({ "ok": true }),
                    Ok(Err(e)) => json!({ "ok": false, "error": e }),
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                }
            }

            // ------------------------------------------------ Destinos & Twitch OAuth/Helix
            "destinations.setKey" => {
                let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let raw_key = params.get("key").and_then(|v| v.as_str()).unwrap_or("");
                let settings = self.store.snapshot();
                let Some(dest) = settings.destinations.iter().find(|d| d.id == id) else {
                    return json!({ "ok": false, "error": "Destino não encontrado." });
                };
                match validate_stream_key(dest.platform, raw_key) {
                    Err(e) => json!({ "ok": false, "error": e }),
                    Ok(None) => {
                        let _ = self.store.set_secret(&stream_key_secret(id), None);
                        let next = self.store.update_destination(id, |d| {
                            d.has_key = false;
                            d.enabled = false;
                        });
                        self.emit_serializable("settings:changed", &next);
                        json!({ "ok": true })
                    }
                    Ok(Some(clean_key)) => {
                        if let Err(e) = self.store.set_secret(&stream_key_secret(id), Some(&clean_key))
                        {
                            return json!({ "ok": false, "error": e });
                        }
                        let next = self.store.update_destination(id, |d| {
                            d.has_key = true;
                            d.enabled = true;
                        });
                        self.emit_serializable("settings:changed", &next);
                        json!({ "ok": true })
                    }
                }
            }
            "destinations.unlink" => {
                let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let _ = self.store.set_secret(&stream_key_secret(id), None);
                let _ = self.store.set_secret(&twitch_refresh_secret(id), None);
                self.twitch_api.clear_cached_token().await;
                let next = self.store.update_destination(id, |d| {
                    d.has_key = false;
                    d.enabled = false;
                    d.account = None;
                });
                self.emit_serializable("settings:changed", &next);
                json!({ "ok": true })
            }
            "destinations.twitchLogin" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("twitch-main")
                    .to_string();
                let custom_cid = self.store.snapshot().twitch_client_id;
                let eng = self.clone();
                tokio::spawn(async move {
                    let res = eng
                        .twitch_login
                        .start_device_login(&id, &custom_cid, |st| {
                            eng.emit_serializable("dest:twitch-state", &st);
                        })
                        .await;
                    if let Ok(outcome) = res {
                        let _ = eng
                            .store
                            .set_secret(&stream_key_secret(&id), Some(&outcome.stream_key));
                        if let Some(ref rt) = outcome.refresh_token {
                            let _ = eng
                                .store
                                .set_secret(&twitch_refresh_secret(&id), Some(rt));
                        }
                        eng.twitch_api
                            .remember_access_token(&id, &outcome.access_token, outcome.expires_in)
                            .await;
                        let next = eng.store.update_destination(&id, |d| {
                            d.has_key = true;
                            d.enabled = true;
                            d.account = Some(outcome.account.clone());
                            d.name = "Twitch".to_string();
                        });
                        eng.emit_serializable("settings:changed", &next);
                    }
                });
                json!({ "ok": true })
            }
            "destinations.twitchCancel" => {
                self.twitch_login.cancel_device_login().await;
                self.emit_serializable("dest:twitch-state", &TwitchLoginState::Idle);
                json!({ "ok": true })
            }
            "twitch.capabilities" => {
                let settings = self.store.snapshot();
                let caps = TwitchApi::capabilities(
                    self.linked_twitch(&settings)
                        .and_then(|d| d.account)
                        .as_ref(),
                );
                serde_json::to_value(caps).unwrap_or(Value::Null)
            }
            "twitch.getChannelInfo" => {
                let settings = self.store.snapshot();
                let Some(dest) = self.linked_twitch(&settings) else {
                    return Value::Null;
                };
                let Some(acc) = dest.account.as_ref() else {
                    return Value::Null;
                };
                let info = self
                    .twitch_api
                    .get_channel_info(&dest.id, acc, &settings.twitch_client_id)
                    .await
                    .ok()
                    .flatten();
                serde_json::to_value(info).unwrap_or(Value::Null)
            }
            "twitch.updateChannelInfo" => {
                let settings = self.store.snapshot();
                let Some(dest) = self.linked_twitch(&settings) else {
                    return json!({ "ok": false, "error": "Entre com a Twitch em Destinos.", "needsRelogin": true });
                };
                let Some(acc) = dest.account.as_ref() else {
                    return json!({ "ok": false, "error": "Entre com a Twitch em Destinos.", "needsRelogin": true });
                };
                let info = serde_json::from_value::<LiveInfo>(params).unwrap_or_default();
                let res = self
                    .twitch_api
                    .update_channel_info(&dest.id, acc, &info, &settings.twitch_client_id)
                    .await;
                serde_json::to_value(res).unwrap_or(Value::Null)
            }
            "twitch.searchCategories" => {
                let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let settings = self.store.snapshot();
                let Some(dest) = self.linked_twitch(&settings) else {
                    return json!([]);
                };
                let cats = self
                    .twitch_api
                    .search_categories(&dest.id, query, &settings.twitch_client_id)
                    .await;
                serde_json::to_value(cats).unwrap_or(json!([]))
            }
            "twitch.sendChat" => {
                let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let target_channel = params
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let settings = self.store.snapshot();
                let Some(dest) = self.linked_twitch(&settings) else {
                    return json!({ "ok": false, "error": "Entre com a Twitch em Destinos.", "needsRelogin": true });
                };
                let Some(acc) = dest.account.as_ref() else {
                    return json!({ "ok": false, "error": "Entre com a Twitch em Destinos.", "needsRelogin": true });
                };
                let channel = if target_channel.trim().is_empty() {
                    settings.chat.channel.as_str()
                } else {
                    target_channel
                };
                let res = self
                    .twitch_api
                    .send_chat(&dest.id, acc, channel, text, &settings.twitch_client_id)
                    .await;
                serde_json::to_value(res).unwrap_or(Value::Null)
            }

            // ------------------------------------------------ Multichat
            "chat.multichatStatus" => {
                let st = self.multichat.status().await;
                serde_json::to_value(st).unwrap_or(json!([]))
            }
            "chat.multichatSnapshot" => {
                let msgs = self.multichat.snapshot_messages().await;
                serde_json::to_value(msgs).unwrap_or(json!([]))
            }
            "chat.reconnectMultichat" => {
                let settings = self.store.snapshot();
                let linked_login = self
                    .linked_twitch(&settings)
                    .and_then(|d| d.account)
                    .map(|a| a.login);
                self.multichat
                    .configure(&settings.chat, linked_login.as_deref(), true)
                    .await;
                json!({ "ok": true })
            }

            // ------------------------------------------------ Áudio nativo
            "audio.listApps" => {
                let apps = tokio::task::spawn_blocking(AudioMixerEngine::list_audio_apps)
                    .await
                    .unwrap_or_default();
                serde_json::to_value(apps).unwrap_or(json!([]))
            }
            "audio.listDevices" => {
                let devs = tokio::task::spawn_blocking(AudioMixerEngine::list_devices)
                    .await
                    .unwrap_or_default();
                let list: Vec<Value> = devs
                    .into_iter()
                    .map(|d| {
                        json!({
                            "id": d.id,
                            "name": d.name,
                            "kind": d.kind,
                            "isDefault": d.is_default,
                        })
                    })
                    .collect();
                json!(list)
            }
            "audio.levels" => {
                let levels = self
                    .audio_mixer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .read_levels();
                serde_json::to_value(levels).unwrap_or(json!([]))
            }
            "audio.statuses" => {
                let st = self
                    .audio_mixer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .statuses();
                serde_json::to_value(st).unwrap_or(json!([]))
            }
            "audio.setSceneMics" => {
                let live: Vec<String> = params
                    .get("live")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let used: Vec<String> = params
                    .get("used")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                self.audio_mixer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .set_scene_mics(live, used);
                json!({ "ok": true })
            }

            // ------------------------------------------------ LAN P2P Host / Discovery / Viewer
            "host.start" => {
                let settings = self.store.snapshot();
                let source_name = params
                    .get("sourceName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Tela Principal")
                    .to_string();
                let source_kind = match params.get("sourceKind").and_then(|v| v.as_str()) {
                    Some("window") => SourceKind::Window,
                    _ => SourceKind::Screen,
                };
                let host_name = params
                    .get("hostName")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&settings.host_name)
                    .to_string();
                let require_pin = params
                    .get("requirePin")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(settings.require_pin);
                let max_viewers = (params
                    .get("maxViewers")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(MAX_VIEWERS as u64) as usize)
                    .clamp(1, MAX_VIEWERS);

                let pin = if require_pin {
                    Some(generate_pin())
                } else {
                    None
                };
                let port = tokio::task::spawn_blocking(|| net::find_free_port(SIGNALING_PORT))
                    .await
                    .unwrap_or(SIGNALING_PORT);
                let pref = settings.preferred_interface.clone();
                let ip = tokio::task::spawn_blocking(move || net::resolve_local_address(pref.as_deref()))
                    .await
                    .unwrap_or_else(|_| "127.0.0.1".to_string());
                let session_id = Uuid::new_v4().to_string();

                let eng_viewers = self.clone();
                let eng_msg = self.clone();
                let sig_res = self
                    .signaling
                    .lock()
                    .await
                    .start(
                        port,
                        HostSessionConfig {
                            host_name: host_name.clone(),
                            source_name: source_name.clone(),
                            source_kind,
                            pin: pin.clone(),
                            max_viewers,
                        },
                        move |viewers| {
                            let count = viewers.len();
                            let eng_patch = eng_viewers.clone();
                            tokio::spawn(async move {
                                eng_patch
                                    .discovery
                                    .lock()
                                    .await
                                    .patch_announce(Some(count), None, None);
                            });
                            eng_viewers.emit_serializable("host:viewers", &viewers);
                            eng_viewers.emit_serializable("lan:viewers", &viewers);
                        },
                        move |viewer_id, msg| {
                            let payload = json!({ "viewerId": viewer_id, "msg": msg });
                            eng_msg.emit("host:message", payload.clone());
                            eng_msg.emit("lan:message", payload);
                        },
                    )
                    .await;

                if let Err(err) = sig_res {
                    return json!({ "ok": false, "error": err });
                }

                let eng_hosts = self.clone();
                let mut disc = self.discovery.lock().await;
                disc.set_preferred_interface(settings.preferred_interface);
                if let Err(err) = disc
                    .start_announcing(
                        AnnounceState {
                            id: session_id.clone(),
                            name: host_name,
                            ip: ip.clone(),
                            port,
                            requires_pin: pin.is_some(),
                            source_name,
                            source_kind,
                            viewers: 0,
                            max_viewers,
                            started_at: now_ms(),
                        },
                        move |hosts| {
                            eng_hosts.emit_serializable("discovery:hosts", &hosts);
                            eng_hosts.emit_serializable("lan:hosts", &hosts);
                        },
                    )
                    .await
                {
                    drop(disc);
                    self.signaling.lock().await.stop().await;
                    return json!({ "ok": false, "error": err });
                }

                json!({
                    "ok": true,
                    "sessionId": session_id,
                    "ip": ip,
                    "port": port,
                    "pin": pin,
                })
            }
            "host.stop" => {
                self.discovery.lock().await.stop_announcing().await;
                self.signaling.lock().await.stop().await;
                json!({ "ok": true })
            }
            "host.kick" => {
                if let Some(vid) = params.get("viewerId").and_then(|v| v.as_str()) {
                    let remaining = self.signaling.lock().await.kick(vid);
                    if let Some(list) = remaining {
                        let count = list.len();
                        self.discovery.lock().await.patch_announce(Some(count), None, None);
                        self.emit_serializable("host:viewers", &list);
                        self.emit_serializable("lan:viewers", &list);
                    }
                }
                json!({ "ok": true })
            }
            "host.send" => {
                if let (Some(vid), Some(msg_val)) = (
                    params.get("viewerId").and_then(|v| v.as_str()),
                    params.get("msg"),
                ) {
                    if let Ok(msg) = serde_json::from_value::<HostToViewer>(msg_val.clone()) {
                        self.signaling.lock().await.send(vid, &msg);
                    }
                }
                json!({ "ok": true })
            }
            "host.broadcast" => {
                if let Some(msg_val) = params.get("msg") {
                    if let Ok(msg) = serde_json::from_value::<HostToViewer>(msg_val.clone()) {
                        self.signaling.lock().await.broadcast(&msg);
                    }
                }
                json!({ "ok": true })
            }
            "host.updateSource" => {
                let source_name = params
                    .get("sourceName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let source_kind = match params.get("sourceKind").and_then(|v| v.as_str()) {
                    Some("window") => SourceKind::Window,
                    _ => SourceKind::Screen,
                };
                self.signaling
                    .lock()
                    .await
                    .update_source(source_name.clone(), source_kind);
                self.discovery
                    .lock()
                    .await
                    .patch_announce(None, Some(source_name), Some(source_kind));
                json!({ "ok": true })
            }
            "discovery.start" => {
                let eng_hosts = self.clone();
                let pref = self.store.snapshot().preferred_interface;
                let mut disc = self.discovery.lock().await;
                disc.set_preferred_interface(pref);
                if let Err(err) = disc
                    .start_browsing(move |hosts| {
                        eng_hosts.emit_serializable("discovery:hosts", &hosts);
                        eng_hosts.emit_serializable("lan:hosts", &hosts);
                    })
                    .await
                {
                    return json!({ "ok": false, "error": err });
                }
                json!({ "ok": true })
            }
            "discovery.stop" => {
                self.discovery.lock().await.stop_browsing().await;
                json!({ "ok": true })
            }
            "discovery.refresh" => {
                self.discovery.lock().await.refresh().await;
                json!({ "ok": true })
            }
            "viewer.connect" => {
                let ip = params
                    .get("ip")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let port = params.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
                let name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Espectador")
                    .to_string();
                let pin = params
                    .get("pin")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);

                let eng_msg = self.clone();
                let eng_closed = self.clone();
                let outcome = self
                    .viewer_client
                    .lock()
                    .await
                    .connect(
                        &ip,
                        port,
                        name,
                        pin,
                        move |msg| {
                            eng_msg.emit_serializable("viewer:message", &msg);
                        },
                        move |info| {
                            let reason = match info.reason {
                                ViewerClosedReason::Kicked => "kicked",
                                ViewerClosedReason::Ended => "ended",
                                ViewerClosedReason::Rede => "rede",
                                ViewerClosedReason::Local => "local",
                            };
                            eng_closed.emit(
                                "viewer:closed",
                                json!({ "reason": reason, "message": info.message }),
                            );
                        },
                    )
                    .await;

                match outcome {
                    ViewerConnectOutcome::Connected(w) => json!({
                        "ok": true,
                        "hostName": w.host_name,
                        "sourceName": w.source_name,
                        "sourceKind": w.source_kind,
                        "viewerId": w.viewer_id,
                    }),
                    ViewerConnectOutcome::Rejected {
                        reason,
                        message,
                        retry_after_ms,
                    } => json!({
                        "ok": false,
                        "reason": reason,
                        "message": message,
                        "retryAfterMs": retry_after_ms,
                    }),
                    ViewerConnectOutcome::NetworkError(message) => json!({
                        "ok": false,
                        "reason": "rede",
                        "message": message,
                    }),
                }
            }
            "viewer.send" => {
                if let Ok(msg) = serde_json::from_value::<ViewerToHost>(params) {
                    self.viewer_client.lock().await.send(&msg);
                }
                json!({ "ok": true })
            }
            "viewer.disconnect" => {
                self.viewer_client.lock().await.disconnect();
                json!({ "ok": true })
            }

            // ------------------------------------------------ Multistream
            "multistream.status" => {
                let st = self.multistream.lock().await.engine_status();
                serde_json::to_value(st).unwrap_or(Value::Null)
            }
            "multistream.probe" => {
                let settings = self.store.snapshot();
                let mut ms = self.multistream.lock().await;
                ms.set_ffmpeg_path(settings.ffmpeg_path.clone());
                let res = ms.refresh_probe().await;
                json!({ "ffmpegFound": res.ffmpeg_found, "encoder": res.encoder })
            }
            "multistream.start" => {
                let settings = self.store.snapshot();
                let fps = params
                    .get("fps")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(settings.studio.fps as u64) as u32;

                let dests_with_keys: Vec<(Destination, Option<String>)> = settings
                    .destinations
                    .into_iter()
                    .map(|d| {
                        let key = self.store.get_secret(&stream_key_secret(&d.id));
                        (d, key)
                    })
                    .collect();

                let res = self
                    .multistream
                    .lock()
                    .await
                    .start_destinations(&dests_with_keys, fps, settings.ffmpeg_path)
                    .await;
                match res {
                    Ok(status) => json!({ "ok": true, "status": status }),
                    Err(error) => {
                        let status = self.multistream.lock().await.engine_status();
                        json!({ "ok": false, "status": status, "error": error })
                    }
                }
            }
            "multistream.stop" => {
                let mut ms = self.multistream.lock().await;
                let _ = ms.stop_async().await;
                let status = ms.engine_status();
                json!({ "ok": true, "status": status })
            }

            // ------------------------------------------------ Recorder & Replay Buffer (sem deadlock!)
            "recorder.status" => {
                let st = self
                    .recorder
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .status();
                serde_json::to_value(st).unwrap_or(Value::Null)
            }
            "recorder.start" => {
                let res = self
                    .recorder
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .start_recording();
                match res {
                    Ok(status) => json!({ "ok": true, "status": status }),
                    Err(error) => json!({ "ok": false, "error": error }),
                }
            }
            "recorder.stop" => {
                let (status, finished_path) = self
                    .recorder
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .stop_recording_with_path();
                if let Some(webm) = finished_path {
                    let ffmpeg = self.store.snapshot().ffmpeg_path;
                    let rec_ref = self.recorder.clone();
                    let eng = self.clone();
                    tokio::spawn(async move {
                        let final_path = remux_to_mp4(webm, ffmpeg).await;
                        let st = {
                            let mut g = rec_ref.lock().unwrap_or_else(|e| e.into_inner());
                            g.mark_saved_replay_path(final_path);
                            g.status()
                        };
                        eng.emit_serializable("recorder:status", &st);
                    });
                }
                json!({ "ok": true, "status": status })
            }
            "recorder.setReplayBuffer" => {
                let enabled = params
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let seconds = params
                    .get("seconds")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(30) as u32;
                let status = self
                    .recorder
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .set_replay_buffer(enabled, seconds);
                json!({ "ok": true, "status": status })
            }
            "recorder.resetStream" => {
                self.recorder
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .reset_stream();
                json!({ "ok": true })
            }
            "recorder.saveReplay" => {
                // CORREÇÃO CRÍTICA DO DEADLOCK: o MutexGuard é dropado antes do match/remux!
                let save_res = {
                    let mut guard = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
                    guard.save_replay_webm()
                };
                match save_res {
                    Ok((webm_path, _)) => {
                        let ffmpeg = self.store.snapshot().ffmpeg_path;
                        let final_path = remux_to_mp4(webm_path, ffmpeg).await;
                        let status = {
                            let mut guard = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
                            guard.mark_saved_replay_path(final_path.clone());
                            guard.status()
                        };
                        json!({
                            "ok": true,
                            "path": final_path.to_string_lossy(),
                            "status": status,
                        })
                    }
                    Err(error) => json!({ "ok": false, "error": error }),
                }
            }

            // ------------------------------------------------ Captura, Compositor & RTC Host
            "capture.list" | "sources.list" => {
                let list = tokio::task::spawn_blocking(capture::list_capture_sources)
                    .await
                    .unwrap_or_default();
                serde_json::to_value(list).unwrap_or(json!([]))
            }
            "capture.pushBrowserDirtyRect" => {
                let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let full_w = params.get("fullWidth").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let full_h = params.get("fullHeight").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let x = params.get("x").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let y = params.get("y").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let width = params.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let height = params.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let data: Vec<u8> = params
                    .get("data")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_u64().map(|n| n as u8))
                            .collect()
                    })
                    .unwrap_or_default();
                let res = self
                    .capture
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push_browser_source_dirty_rect(
                        id,
                        full_w,
                        full_h,
                        DirtyRect {
                            x,
                            y,
                            width,
                            height,
                        },
                        &data,
                        now_ms(),
                    );
                match res {
                    Ok(()) => json!({ "ok": true }),
                    Err(error) => json!({ "ok": false, "error": error }),
                }
            }
            "studio.setLiveVertical" => {
                let active = params
                    .get("active")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let mut comp = self.compositor.lock().unwrap_or_else(|e| e.into_inner());
                comp.set_live_vertical(active);
                json!({ "ok": true, "verticalActive": comp.is_vertical_active() })
            }
            "studio.setScenes" => {
                let settings = self.store.snapshot();
                let prog_id = params
                    .get("activeSceneId")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&settings.scenes.active_scene_id);
                let prev_id = params
                    .get("previewSceneId")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&settings.scenes.preview_scene_id);
                let transition = params
                    .get("transitionType")
                    .and_then(|v| serde_json::from_value::<TransitionType>(v.clone()).ok())
                    .unwrap_or(settings.scenes.transition_type);
                let duration_ms = params
                    .get("transitionDurationMs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(settings.scenes.transition_duration_ms as u64)
                    as u32;

                let prog = settings.scenes.scenes.iter().find(|s| s.id == prog_id).cloned();
                let prev = settings.scenes.scenes.iter().find(|s| s.id == prev_id).cloned();
                self.compositor
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .set_scenes(prog, prev, transition, duration_ms, now_ms());
                json!({ "ok": true })
            }
            "studio.telemetry" | "telemetry.get" | "telemetry.stats" => {
                let stats = self.collect_telemetry().await;
                serde_json::to_value(stats).unwrap_or(Value::Null)
            }
            "rtc.preferOpusStereo" => {
                let sdp = params.get("sdp").and_then(|v| v.as_str()).unwrap_or("");
                let bitrate = params
                    .get("bitrateBps")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(192_000) as u32;
                json!({ "sdp": prefer_opus_stereo(sdp, bitrate) })
            }
            "rtc.applyCodecPreference" => {
                let sdp = params.get("sdp").and_then(|v| v.as_str()).unwrap_or("");
                let codec = params.get("codec").and_then(|v| v.as_str()).unwrap_or("auto");
                json!({ "sdp": apply_codec_preference(sdp, codec) })
            }
            "rtc.addPeer" => {
                let vid = params.get("viewerId").and_then(|v| v.as_str()).unwrap_or("");
                if !vid.is_empty() {
                    self.rtc_host
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .add_peer(vid.to_string());
                }
                json!({ "ok": true })
            }
            "rtc.removePeer" => {
                let vid = params.get("viewerId").and_then(|v| v.as_str()).unwrap_or("");
                self.rtc_host
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove_peer(vid);
                json!({ "ok": true })
            }
            "rtc.onVoiceOffer" => {
                let vid = params.get("viewerId").and_then(|v| v.as_str()).unwrap_or("");
                let drained = self
                    .rtc_host
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .on_voice_offer(vid);
                serde_json::to_value(drained).unwrap_or(Value::Null)
            }

            _ => json!({ "ok": false, "error": format!("Método desconhecido: {method}") }),
        }
    }

    /// Desligamento gracioso completo sem vazar processos filhos (FFmpeg/sockets/hotkeys).
    pub async fn shutdown(&self) {
        self.bg_cancel.cancel();
        self.twitch_login.cancel_device_login().await;
        self.multichat.dispose().await;
        {
            let mut ms = self.multistream.lock().await;
            let _ = ms.stop_async().await;
        }
        {
            let mut rec = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
            let _ = rec.stop_recording_with_path();
        }
        self.viewer_client.lock().await.disconnect();
        self.signaling.lock().await.stop().await;
        self.discovery.lock().await.dispose().await;
        if let Some(mut hk) = self
            .hotkeys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            hk.stop();
        }
    }
}

// ------------------------------------------------------------------ Validação & Servidor WebSocket RPC

/// Valida handshake HTTP do WebSocket RPC:
/// - Rejeita QUALQUER cabeçalho `Origin` (bloqueia CSWSH vindo de navegadores).
/// - Exige token em `Authorization: Bearer <token>` ou query `?token=<token>` via `ConstantTimeEq`.
pub fn validate_ws_handshake(
    req: &Request,
    expected_token: &str,
) -> Result<(), (http::StatusCode, &'static str)> {
    if req.headers().get("origin").is_some() {
        return Err((
            http::StatusCode::FORBIDDEN,
            "Origin header forbidden on local RPC",
        ));
    }

    let auth_bearer = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")))
        .map(str::trim);

    let query_token: Option<String> = req.uri().query().and_then(|q| {
        Url::parse(&format!("http://localhost/?{q}"))
            .ok()
            .and_then(|u| {
                u.query_pairs()
                    .find(|(k, _)| k == "token")
                    .map(|(_, v)| v.into_owned())
            })
    });

    let candidate = auth_bearer.or(query_token.as_deref());
    if !verify_token_constant_time(expected_token, candidate) {
        return Err((http::StatusCode::UNAUTHORIZED, "Invalid RPC token"));
    }

    Ok(())
}

pub struct RpcServerConfig {
    pub port: u16,
    pub token: String,
}

impl Default for RpcServerConfig {
    fn default() -> Self {
        Self {
            port: RUST_RPC_PORT,
            token: generate_session_token(),
        }
    }
}

pub async fn run_rpc_server(
    engine: Arc<AppEngine>,
    cfg: RpcServerConfig,
    shutdown: CancellationToken,
) -> Result<(), String> {
    let addr = format!("127.0.0.1:{}", cfg.port);
    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("Falha ao abrir porta RPC {addr}: {e}"))?;

    let token = Arc::new(cfg.token);

    loop {
        let accepted = tokio::select! {
            _ = shutdown.cancelled() => break,
            res = listener.accept() => res,
        };
        let (stream, _) = match accepted {
            Ok(pair) => pair,
            Err(err) => {
                log::warn!("[rpc] erro transiente no accept: {err}");
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
        };

        let eng = engine.clone();
        let expected_token = token.clone();
        let conn_cancel = shutdown.clone();

        tokio::spawn(async move {
            let mut ws_cfg = WebSocketConfig::default();
            ws_cfg.max_message_size = Some(MAX_RPC_WS_MESSAGE_BYTES);
            ws_cfg.max_frame_size = Some(MAX_RPC_WS_MESSAGE_BYTES);

            let ws_res = tokio_tungstenite::accept_hdr_async_with_config(
                stream,
                move |req: &Request, res: Response| -> Result<Response, ErrorResponse> {
                    match validate_ws_handshake(req, &expected_token) {
                        Ok(()) => Ok(res),
                        Err((status, msg)) => Err(http::Response::builder()
                            .status(status)
                            .body(Some(msg.to_string()))
                            .unwrap()),
                    }
                },
                Some(ws_cfg),
            )
            .await;

            let Ok(ws_stream) = ws_res else {
                return;
            };

            let (mut ws_write, mut ws_read) = ws_stream.split();
            let (tx, mut rx) = mpsc::channel::<Message>(WS_WRITE_QUEUE_CAP);

            let write_task: JoinHandle<()> = tokio::spawn(async move {
                while let Some(msg) = rx.recv().await {
                    if ws_write.send(msg).await.is_err() {
                        break;
                    }
                }
            });

            let mut evt_rx = eng.subscribe();
            let tx_evt = tx.clone();
            let evt_cancel = conn_cancel.clone();
            let evt_forward = tokio::spawn(async move {
                loop {
                    let item = tokio::select! {
                        _ = evt_cancel.cancelled() => break,
                        r = evt_rx.recv() => r,
                    };
                    match item {
                        Ok(ev) => {
                            if let Ok(raw) = serde_json::to_string(&ev) {
                                // Eventos de telemetria/VU podem usar try_send para não bloquear a fila
                                if ev.event == "audio:levels" || ev.event == "telemetry:stats" {
                                    let _ = tx_evt.try_send(Message::Text(raw.into()));
                                } else if tx_evt.send(Message::Text(raw.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            log::debug!("[rpc] forwarder lagged ({skipped} eventos)");
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });

            loop {
                let item = tokio::select! {
                    _ = conn_cancel.cancelled() => break,
                    msg = ws_read.next() => msg,
                };
                let Some(Ok(msg)) = item else {
                    break;
                };

                match msg {
                    Message::Text(text) => {
                        let Ok(req) = serde_json::from_str::<RpcRequest>(&text) else {
                            continue;
                        };
                        let eng_call = eng.clone();
                        let tx_reply = tx.clone();
                        tokio::spawn(async move {
                            let req_id = req.id;
                            let response = eng_call.handle_command(&req.method, req.params).await;
                            if let Some(id) = req_id {
                                let reply = json!({
                                    "id": id,
                                    "result": response,
                                });
                                if let Ok(raw) = serde_json::to_string(&reply) {
                                    let _ = tx_reply.send(Message::Text(raw.into())).await;
                                }
                            }
                        });
                    }
                    Message::Binary(bytes) => {
                        eng.handle_binary(&bytes).await;
                    }
                    Message::Ping(payload) => {
                        let _ = tx.try_send(Message::Pong(payload));
                    }
                    Message::Pong(_) => {}
                    Message::Close(_) => break,
                    Message::Frame(_) => {}
                }
            }

            evt_forward.abort();
            write_task.abort();
        });
    }

    Ok(())
}

#[allow(dead_code)]
fn _keep_unused_imports_happy() {
    let _: Option<PathBuf> = None;
    let _: Option<MultiChatEvent> = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_engine(tag: &str) -> (Arc<AppEngine>, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "prismcast-rpc-test-{}-{}-{}",
            tag,
            std::process::id(),
            now_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(SettingsStore::with_dir(dir.clone()));
        let engine = AppEngine::with_store(store, false);
        (engine, dir)
    }

    #[test]
    fn test_token_constant_time_and_origin_rejection() {
        let token = generate_session_token();
        assert_eq!(token.len(), 64);

        let ok_req = http::Request::builder()
            .uri(format!("ws://127.0.0.1:41236/?token={token}"))
            .body(())
            .unwrap();
        assert!(validate_ws_handshake(&ok_req, &token).is_ok());

        let bearer_req = http::Request::builder()
            .uri("ws://127.0.0.1:41236/")
            .header("Authorization", format!("Bearer {token}"))
            .body(())
            .unwrap();
        assert!(validate_ws_handshake(&bearer_req, &token).is_ok());

        let bad_token_req = http::Request::builder()
            .uri("ws://127.0.0.1:41236/?token=wrong")
            .body(())
            .unwrap();
        assert_eq!(
            validate_ws_handshake(&bad_token_req, &token).unwrap_err().0,
            http::StatusCode::UNAUTHORIZED
        );

        let evil_origin_req = http::Request::builder()
            .uri(format!("ws://127.0.0.1:41236/?token={token}"))
            .header("Origin", "https://evil.example")
            .body(())
            .unwrap();
        assert_eq!(
            validate_ws_handshake(&evil_origin_req, &token).unwrap_err().0,
            http::StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn test_save_replay_no_deadlock() {
        let (engine, dir) = test_engine("save-replay");

        // Alimenta init segment + 1 cluster WebM via frame binário (sem Base64!)
        let mut init_frame = vec![BIN_TAG_RECORDER_CHUNK];
        init_frame.extend_from_slice(&[0x1A, 0x45, 0xDF, 0xA3, 0x01, 0x02]);
        engine.handle_binary(&init_frame).await;

        let mut cluster_frame = vec![BIN_TAG_RECORDER_CHUNK];
        cluster_frame.extend_from_slice(&[0x1F, 0x43, 0xB6, 0x75, 0xAA, 0xBB]);
        engine.handle_binary(&cluster_frame).await;

        let res = tokio::time::timeout(
            Duration::from_secs(3),
            engine.handle_command("recorder.saveReplay", Value::Null),
        )
        .await
        .expect("recorder.saveReplay must not deadlock");

        assert_eq!(res.get("ok").and_then(|v| v.as_bool()), Some(true));
        let path_str = res.get("path").and_then(|v| v.as_str()).unwrap();
        assert!(std::path::Path::new(path_str).exists());

        engine.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_command_routing_and_settings_deep_merge() {
        let (engine, dir) = test_engine("routing");

        let next = engine
            .handle_command(
                "settings.set",
                json!({
                    "quality": { "fps": 60 },
                    "hotkeys": {
                        "toggleMicMute": "Ctrl+Shift+M",
                        "toggleSystemMute": "Ctrl+Shift+M"
                    }
                }),
            )
            .await;

        assert_eq!(next.pointer("/quality/fps").and_then(|v| v.as_u64()), Some(60));
        assert_eq!(
            next.pointer("/quality/maxHeight").and_then(|v| v.as_u64()),
            Some(1080)
        );

        let conflicts = engine
            .handle_command("hotkeys.conflicts", Value::Null)
            .await;
        let arr = conflicts.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0].get("accelerator").and_then(|v| v.as_str()),
            Some("CommandOrControl+Shift+M")
        );

        let info = engine.handle_command("app.info", Value::Null).await;
        assert_eq!(
            info.get("version").and_then(|v| v.as_str()),
            Some(env!("CARGO_PKG_VERSION"))
        );

        engine.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
