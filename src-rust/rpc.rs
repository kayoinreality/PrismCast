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
use crate::audio_runtime::AudioRuntime;
use crate::auth_twitch::{
    validate_stream_key, TokenVault, TwitchApi, TwitchDeviceLoginManager,
};
use crate::capture::{self, CaptureManager, DirtyRect};
use crate::compositor::Compositor;
use crate::discovery::{AnnounceState, DiscoveryEngine};
use crate::firewall;
use crate::gpu_capture::GpuCaptureEngine;
use crate::hotkeys::{detect_conflicts, HotkeyManager};
use crate::kit;
use crate::multichat::MultiChatHub;
use crate::multistream::MultistreamEngine;
use crate::net;
use crate::obs_ws::{
    audio_channel_kind_to_obs_input_kind, obs_name_to_transition_type,
    source_type_to_obs_input_kind, transition_type_metadata, translate_app_settings_diff,
    translate_rpc_event, ObsControlHost, ObsCurrentTransition, ObsEvent, ObsFuture, ObsInputInfo,
    ObsInputVolume, ObsRecordStatus, ObsReplayBufferStatus, ObsRequestError, ObsResult,
    ObsSceneInfo, ObsSceneItemInfo, ObsSceneList, ObsStats, ObsStreamStatus, ObsWsConfig,
    ObsWsServerHandle,
};
use crate::protocol::{
    AppSettings, AudioChannel, CanvasOrientation, Destination, ExternalControlStatus,
    GpuPipelineStatus, HostToViewer, HotkeyAction, HotkeyBindings, KitConfig, LiveInfo,
    MultiChatEvent, PlatformId, Scene, SourceKind, TelemetryStats, TransitionType,
    TwitchLoginState, UpdateState, ViewerToHost, MAX_DB, MAX_VIEWERS, MIN_DB, RUST_RPC_PORT,
    SIGNALING_PORT,
};
use crate::recorder::{remux_to_mp4, RecorderEngine};
use crate::rtc_host::{apply_codec_preference, prefer_opus_stereo, RtcHostState};
use crate::signaling::{
    HostSessionConfig, SignalingClient, SignalingServer, ViewerClosedReason, ViewerConnectOutcome,
};
use crate::store::{
    obs_ws_password_secret, stream_key_secret, twitch_refresh_secret, SettingsStore,
};
use crate::updater::UpdateManager;
use crate::virtual_cam::{self, VirtualCameraEngine};
use crate::vst_host;
use futures_util::{SinkExt, StreamExt};
use rand::{Rng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
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
pub const BIN_TAG_VIRTUAL_CAM_FRAME: u8 = 0x04;
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
    pub audio_runtime: Arc<AudioRuntime>,
    pub capture: Arc<Mutex<CaptureManager>>,
    pub gpu_capture: Arc<Mutex<GpuCaptureEngine>>,
    pub compositor: Arc<Mutex<Compositor>>,
    pub virtual_cam: Arc<Mutex<VirtualCameraEngine>>,
    pub obs_ws: Arc<AsyncMutex<Option<ObsWsServerHandle>>>,
    pub rpc_clients: Arc<AtomicUsize>,
    pub rtc_host: Arc<Mutex<RtcHostState>>,
    pub discovery: Arc<AsyncMutex<DiscoveryEngine>>,
    pub signaling: Arc<AsyncMutex<SignalingServer>>,
    pub viewer_client: Arc<AsyncMutex<SignalingClient>>,
    pub updater: Arc<UpdateManager>,
    hotkeys: Mutex<Option<HotkeyManager>>,
    evt_tx: broadcast::Sender<RpcEvent>,
    obs_event_tx: broadcast::Sender<ObsEvent>,
    bg_cancel: CancellationToken,
}

impl AppEngine {
    /// Cria e inicializa o `AppEngine` dentro de um runtime Tokio ativo.
    pub fn new() -> Arc<Self> {
        let store = Arc::new(SettingsStore::new());
        Self::with_store(store, true)
    }

    /// Construtor determinístico para testes (permite diretório isolado e desativar hotkeys/daemons OS).
    pub fn with_store(store: Arc<SettingsStore>, spawn_os_daemons: bool) -> Arc<Self> {
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
        multistream.set_dynamic_bitrate(initial.dynamic_bitrate.clone());

        let audio_mixer = Arc::new(Mutex::new(AudioMixerEngine::new(
            initial.audio_mixer.clone(),
        )));
        let audio_runtime = Arc::new(AudioRuntime::new(audio_mixer.clone()));
        let capture = CaptureManager::new(initial.studio.fps);
        let gpu_capture = GpuCaptureEngine::new(initial.gpu_pipeline.clone());
        let mut compositor = Compositor::new(initial.studio.clone());
        if initial.gpu_pipeline.prefer_wgpu && initial.gpu_pipeline.zero_copy_enabled {
            let _ = compositor.try_enable_gpu_backend();
        }
        compositor.set_custom_transition(initial.scenes.custom_transition.clone());
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
        compositor.set_scenes_with_config(
            prog_scene,
            prev_scene,
            initial.scenes.transition_type,
            initial.scenes.transition_duration_ms,
            Some(initial.scenes.custom_transition.clone()),
            now_ms(),
        );
        let virtual_cam = VirtualCameraEngine::new(initial.virtual_camera.clone());
        let rtc_host = RtcHostState::new(
            initial.quality.clone(),
            initial.audio_mixer.party_voice_enabled,
        );
        let discovery = DiscoveryEngine::new();
        discovery.set_preferred_interface(initial.preferred_interface.clone());

        let (evt_tx, _) = broadcast::channel::<RpcEvent>(512);
        let (obs_event_tx, _) = broadcast::channel::<ObsEvent>(512);
        let bg_cancel = CancellationToken::new();

        let engine = Arc::new(Self {
            store,
            twitch_login,
            twitch_api,
            multichat,
            multistream: Arc::new(AsyncMutex::new(multistream)),
            recorder: Arc::new(Mutex::new(recorder)),
            audio_mixer,
            audio_runtime,
            capture: Arc::new(Mutex::new(capture)),
            gpu_capture: Arc::new(Mutex::new(gpu_capture)),
            compositor: Arc::new(Mutex::new(compositor)),
            virtual_cam: Arc::new(Mutex::new(virtual_cam)),
            obs_ws: Arc::new(AsyncMutex::new(None)),
            rpc_clients: Arc::new(AtomicUsize::new(0)),
            rtc_host: Arc::new(Mutex::new(rtc_host)),
            discovery: Arc::new(AsyncMutex::new(discovery)),
            signaling: Arc::new(AsyncMutex::new(SignalingServer::new())),
            viewer_client: Arc::new(AsyncMutex::new(SignalingClient::new())),
            updater: Arc::new(UpdateManager::new()),
            hotkeys: Mutex::new(None),
            evt_tx,
            obs_event_tx,
            bg_cancel,
        });

        // Encaminha mudanças de status de captura de áudio por aplicativo para a UI
        let eng_audio_st = Arc::downgrade(&engine);
        engine.audio_runtime.set_status_callback(move |statuses| {
            if let Some(eng) = eng_audio_st.upgrade() {
                eng.emit_serializable("audio:status", &statuses);
            }
        });

        // Encaminha PCM mixado do Program Bus (48 kHz f32le) para o encoder nativo quando ativo
        let mut pcm_rx = engine.audio_runtime.subscribe_program_pcm();
        let eng_pcm = Arc::downgrade(&engine);
        let cancel_pcm = engine.bg_cancel.clone();
        tokio::spawn(async move {
            loop {
                let pkt = tokio::select! {
                    _ = cancel_pcm.cancelled() => break,
                    res = pcm_rx.recv() => res,
                };
                match pkt {
                    Ok(pcm) => {
                        let Some(eng) = eng_pcm.upgrade() else {
                            break;
                        };
                        if let Ok(mut ms) = eng.multistream.try_lock() {
                            if ms.is_native_mode() {
                                ms.push_native_audio_pcm(&pcm);
                            }
                        };
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        if spawn_os_daemons {
            let evt_hk = engine.evt_tx.clone();
            let hk = HotkeyManager::spawn(initial.hotkeys.clone(), move |action| {
                let _ = evt_hk.send(RpcEvent {
                    event: "hotkey:triggered".to_string(),
                    payload: Value::String(action.as_str().to_string()),
                });
            });
            *engine.hotkeys.lock().unwrap_or_else(|e| e.into_inner()) = Some(hk);

            // Inicia runtime de áudio em tempo real (10 ms DSP + captura isolada por aplicativo)
            engine.audio_runtime.start(&initial.audio_mixer);

            // Inicializa servidor obs-websocket v5 se habilitado nas configurações
            let eng_obs = engine.clone();
            tokio::spawn(async move {
                eng_obs.reconcile_obs_ws().await;
            });

            // Verificação automática não-bloqueante de novas versões no GitHub Releases
            let eng_upd = Arc::downgrade(&engine);
            let cancel_upd = engine.bg_cancel.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = cancel_upd.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(6)) => {}
                }
                let Some(eng) = eng_upd.upgrade() else {
                    return;
                };
                let cfg = eng.store.snapshot().updater;
                if !cfg.auto_check {
                    return;
                }
                let eng_ev = eng.clone();
                let st = eng
                    .updater
                    .check(cfg.include_prereleases, move |s| {
                        eng_ev.emit_serializable("updater:status", s);
                    })
                    .await;
                if cfg.auto_download
                    && st.state == UpdateState::Available
                    && st.asset.is_some()
                    && st.latest_version.as_deref() != cfg.skipped_version.as_deref()
                {
                    let eng_dl = eng.clone();
                    let _ = eng
                        .updater
                        .download(move |s| {
                            eng_dl.emit_serializable("updater:status", s);
                        })
                        .await;
                }
            });
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
                                eng.emit_serializable("multichat:event", &ev);
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

        // Loop periódico leve: VU meters (25 Hz) e telemetria real + ABR (1 Hz)
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
                    let abr = eng.multistream.lock().await.abr_telemetry();
                    if !abr.is_empty() {
                        eng.emit_serializable("abr:telemetry", &abr);
                    }
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
            payload: payload.clone(),
        });
        for obs_ev in translate_rpc_event(event, &payload) {
            let _ = self.obs_event_tx.send(obs_ev);
        }
    }

    pub fn emit_serializable<T: Serialize>(&self, event: &str, payload: &T) {
        if let Ok(val) = serde_json::to_value(payload) {
            self.emit(event, val);
        }
    }

    pub fn emit_obs_event(&self, ev: ObsEvent) {
        let _ = self.obs_event_tx.send(ev);
    }

    /// Garante que o servidor `obs-websocket` v5 reflita o estado atual de `external_control`.
    pub async fn reconcile_obs_ws(self: &Arc<Self>) -> ExternalControlStatus {
        let settings = self.store.snapshot();
        let ext = settings.external_control;
        let password = self.store.get_secret(&obs_ws_password_secret());

        let mut guard = self.obs_ws.lock().await;
        if guard.is_none() {
            let mut handle = ObsWsServerHandle::new(self.clone());
            handle.set_rpc_info(RUST_RPC_PORT, self.rpc_clients.clone());
            *guard = Some(handle);
        }
        let handle = guard.as_mut().unwrap();
        let status = if ext.obs_ws_enabled {
            let cfg = ObsWsConfig::from_settings(&ext, password);
            handle.restart(cfg).await
        } else {
            handle.stop().await;
            handle.status()
        };
        drop(guard);
        self.emit_serializable("external-control:status", &status);
        status
    }

    pub async fn external_control_status(self: &Arc<Self>) -> ExternalControlStatus {
        let guard = self.obs_ws.lock().await;
        if let Some(ref handle) = *guard {
            handle.status()
        } else {
            let ext = self.store.snapshot().external_control;
            ExternalControlStatus {
                obs_ws_running: false,
                obs_ws_port: ext.obs_ws_port,
                obs_ws_clients: 0,
                rpc_ws_port: RUST_RPC_PORT,
                rpc_ws_clients: self.rpc_clients.load(Ordering::Relaxed),
                error: None,
            }
        }
    }

    pub fn gpu_pipeline_status(&self) -> GpuPipelineStatus {
        let (backend, adapter, driver) = self
            .compositor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .backend_info();
        self.gpu_capture
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status(&backend, &adapter, &driver)
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
        let (prog, encoder, ffmpeg_found, (abr_active, abr_target_kbps, abr_congestion)) = {
            let ms = self.multistream.lock().await;
            let st = ms.engine_status();
            (
                ms.telemetry_snapshot(),
                st.encoder,
                st.ffmpeg_found,
                ms.abr_summary(),
            )
        };
        let comp_tel = self
            .compositor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .telemetry();
        let gpu_st = self.gpu_pipeline_status();
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
        let vcam_active = self
            .virtual_cam
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_active();
        let obs_clients = self
            .obs_ws
            .try_lock()
            .ok()
            .and_then(|g| g.as_ref().map(|h| h.status().obs_ws_clients))
            .unwrap_or(0);
        let rpc_clients = self.rpc_clients.load(Ordering::Relaxed);

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
            zero_copy_active: gpu_st.zero_copy_active,
            compositor_backend: gpu_st.compositor_backend,
            abr_active,
            abr_target_bitrate_kbps: abr_target_kbps,
            abr_congestion_level: abr_congestion,
            virtual_cam_active: vcam_active,
            external_clients: obs_clients.saturating_add(rpc_clients),
            audio_runtime_active: self.audio_runtime.is_active(),
        }
    }

    fn linked_twitch(&self, settings: &AppSettings) -> Option<Destination> {
        settings
            .destinations
            .iter()
            .find(|d| d.platform == PlatformId::Twitch && d.account.is_some())
            .cloned()
    }

    /// Aplica um patch parcial nas configurações, propaga mudanças para todos os
    /// subsistemas nativos (áudio, compositor/transições, GPU zero-copy, ABR, câmera virtual,
    /// hotkeys, multichat) e emite `settings:changed` + diffs `ObsEvent`.
    async fn apply_settings_patch_internal(&self, patch: Value) -> (AppSettings, AppSettings) {
        let prev = self.store.snapshot();
        let next = self.store.apply_patch(patch);

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
            if self.audio_runtime.is_active() {
                self.audio_runtime.sync_channels(&next.audio_mixer);
            }
            self.rtc_host
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_party_voice_enabled(next.audio_mixer.party_voice_enabled);
        }
        if serde_json::to_string(&next.quality).ok() != serde_json::to_string(&prev.quality).ok() {
            self.rtc_host
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .update_settings(next.quality.clone());
        }
        if serde_json::to_string(&next.studio).ok() != serde_json::to_string(&prev.studio).ok() {
            self.capture
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_fps(next.studio.fps);
            self.compositor
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .apply_settings(next.studio.clone());
        }
        if serde_json::to_string(&next.scenes).ok() != serde_json::to_string(&prev.scenes).ok() {
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
                .set_scenes_with_config(
                    prog_scene,
                    prev_scene,
                    next.scenes.transition_type,
                    next.scenes.transition_duration_ms,
                    Some(next.scenes.custom_transition.clone()),
                    now_ms(),
                );
        }
        if serde_json::to_string(&next.gpu_pipeline).ok()
            != serde_json::to_string(&prev.gpu_pipeline).ok()
        {
            self.gpu_capture
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .apply_settings(next.gpu_pipeline.clone());
            {
                let mut comp = self.compositor.lock().unwrap_or_else(|e| e.into_inner());
                if next.gpu_pipeline.prefer_wgpu && next.gpu_pipeline.zero_copy_enabled {
                    let _ = comp.try_enable_gpu_backend();
                } else {
                    comp.disable_gpu_backend();
                }
            }
            {
                let mut ms = self.multistream.lock().await;
                ms.set_native_mode(next.gpu_pipeline.native_encoder_pipe);
            }
            let gpu_st = self.gpu_pipeline_status();
            self.emit_serializable("gpu:status", &gpu_st);
        }
        if serde_json::to_string(&next.dynamic_bitrate).ok()
            != serde_json::to_string(&prev.dynamic_bitrate).ok()
        {
            self.multistream
                .lock()
                .await
                .set_dynamic_bitrate(next.dynamic_bitrate.clone());
        }
        if serde_json::to_string(&next.virtual_camera).ok()
            != serde_json::to_string(&prev.virtual_camera).ok()
        {
            let vcam_st = {
                let mut vcam = self.virtual_cam.lock().unwrap_or_else(|e| e.into_inner());
                vcam.apply_settings(next.virtual_camera.clone());
                vcam.status()
            };
            self.emit_serializable("virtual-cam:status", &vcam_st);
        }
        if serde_json::to_string(&next.hotkeys).ok() != serde_json::to_string(&prev.hotkeys).ok() {
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

        for ev in translate_app_settings_diff(&prev, &next) {
            self.emit_obs_event(ev);
        }

        let val = serde_json::to_value(&next).unwrap_or(Value::Null);
        self.emit("settings:changed", val);
        (prev, next)
    }

    pub async fn handle_binary(&self, frame: &[u8]) {
        // Suporta o cabeçalho de 8 bytes do bridge.ts: [0x50, 0x43, kind, channel, len_le32, ...payload]
        if frame.len() >= 8 && frame[0] == 0x50 && frame[1] == 0x43 {
            let kind = frame[2];
            let channel = frame[3];
            let payload = &frame[8..];
            match kind {
                1 => {
                    let orient = if channel == 1 {
                        CanvasOrientation::Vertical
                    } else {
                        CanvasOrientation::Horizontal
                    };
                    self.multistream.lock().await.push_chunk(orient, payload);
                }
                2 => {
                    let mut rec = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
                    rec.push_media_chunk(payload);
                }
                4 => {
                    // [width_le32, height_le32, ...bgra]
                    if payload.len() >= 8 {
                        let w = u32::from_le_bytes(payload[0..4].try_into().unwrap());
                        let h = u32::from_le_bytes(payload[4..8].try_into().unwrap());
                        let mut vcam = self.virtual_cam.lock().unwrap_or_else(|e| e.into_inner());
                        let _ = vcam.push_frame_bgra(w, h, &payload[8..]);
                    }
                }
                _ => {}
            }
            return;
        }

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
            BIN_TAG_VIRTUAL_CAM_FRAME => {
                if payload.len() >= 8 {
                    let w = u32::from_le_bytes(payload[0..4].try_into().unwrap());
                    let h = u32::from_le_bytes(payload[4..8].try_into().unwrap());
                    let mut vcam = self.virtual_cam.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = vcam.push_frame_bgra(w, h, &payload[8..]);
                }
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
            "app.info" | "app_info" => {
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
                    "isPackaged": !cfg!(debug_assertions),
                    "engine": "rust-native",
                    "hardwareEncoder": encoder,
                    "ffmpegFound": ffmpeg_found,
                })
            }
            "app_minimize" | "app_toggle_maximize" | "app_close" | "app_always_on_top"
            | "app_open_external" | "app_show_in_folder" | "chat_open_popout"
            | "chat_close_popout" => json!({ "ok": true }),
            "chat_is_popout_open" => Value::Bool(false),

            // ------------------------------------------------ Settings & Sistema
            "settings.get" | "settings_get" => {
                serde_json::to_value(self.store.snapshot()).unwrap_or(Value::Null)
            }
            "settings.set" | "settings_set" => {
                let patch = params.get("patch").cloned().unwrap_or(params);
                let (prev, next) = self.apply_settings_patch_internal(patch).await;
                if serde_json::to_string(&next.external_control).ok()
                    != serde_json::to_string(&prev.external_control).ok()
                {
                    let _ = self.reconcile_obs_ws().await;
                }
                serde_json::to_value(&next).unwrap_or(Value::Null)
            }
            "hotkeys.conflicts" | "hotkeys_conflicts" => {
                let bindings: HotkeyBindings = params
                    .get("bindings")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .or_else(|| serde_json::from_value(params).ok())
                    .unwrap_or_else(|| self.store.snapshot().hotkeys);
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
            "net.interfaces" | "net_interfaces" => {
                let list = tokio::task::spawn_blocking(net::list_interfaces)
                    .await
                    .unwrap_or_default();
                serde_json::to_value(list).unwrap_or(json!([]))
            }
            "net.refreshInterfaces" | "net_refresh_interfaces" => {
                let list = tokio::task::spawn_blocking(|| {
                    net::invalidate_cache();
                    net::list_interfaces()
                })
                .await
                .unwrap_or_default();
                serde_json::to_value(list).unwrap_or(json!([]))
            }
            "firewall.status" | "firewall_status" => {
                let st = tokio::task::spawn_blocking(firewall::check_status).await;
                match st {
                    Ok(v) => serde_json::to_value(v).unwrap_or(Value::Null),
                    Err(_) => Value::Null,
                }
            }
            "firewall.authorize" | "firewall_authorize" => {
                let res = tokio::task::spawn_blocking(firewall::authorize).await;
                match res {
                    Ok(Ok(())) => json!({ "ok": true }),
                    Ok(Err(e)) => json!({ "ok": false, "error": e }),
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                }
            }

            // ------------------------------------------------ Atualização Automática (GitHub Releases)
            "updater.status" | "updater_status" => {
                serde_json::to_value(self.updater.status()).unwrap_or(Value::Null)
            }
            "updater.check" | "updater_check" => {
                let include_pre = params
                    .get("includePrereleases")
                    .and_then(|v| v.as_bool())
                    .unwrap_or_else(|| self.store.snapshot().updater.include_prereleases);
                let eng = self.clone();
                let st = self
                    .updater
                    .check(include_pre, move |s| {
                        eng.emit_serializable("updater:status", s);
                    })
                    .await;
                serde_json::to_value(st).unwrap_or(Value::Null)
            }
            "updater.download" | "updater_download" => {
                let eng = self.clone();
                let st = self
                    .updater
                    .download(move |s| {
                        eng.emit_serializable("updater:status", s);
                    })
                    .await;
                serde_json::to_value(st).unwrap_or(Value::Null)
            }
            "updater.cancel" | "updater_cancel" => {
                let st = self.updater.cancel_download();
                self.emit_serializable("updater:status", &st);
                serde_json::to_value(st).unwrap_or(Value::Null)
            }
            "updater.install" | "updater_install" => {
                if self.is_busy() {
                    return json!({
                        "ok": false,
                        "shouldExit": false,
                        "error": "Encerre a transmissão ou gravação antes de instalar a atualização."
                    });
                }
                let eng = self.clone();
                let outcome = self.updater.install_prepared(move |s| {
                    eng.emit_serializable("updater:status", s);
                });
                json!({
                    "ok": outcome.ok,
                    "shouldExit": outcome.should_exit,
                    "message": outcome.message,
                    "error": if outcome.ok { Value::Null } else { Value::String(outcome.message.clone()) }
                })
            }

            // ------------------------------------------------ Kit de Widgets (sem JS/vm)
            "kit.load" | "kit_load" => {
                let path = params.get("path").and_then(|v| v.as_str()).map(str::to_string);
                let res = tokio::task::spawn_blocking(move || kit::load_kit(path.as_deref()))
                    .await
                    .ok()
                    .flatten();
                serde_json::to_value(res).unwrap_or(Value::Null)
            }
            "kit_pick_folder" | "files_pick_image" | "files_pick_html" | "files_pick_media" => {
                Value::Null
            }
            "files_read_image" | "files.readImage" => {
                let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
                if path.is_empty() {
                    return Value::Null;
                }
                let p = PathBuf::from(path);
                let Ok(bytes) = std::fs::read(&p) else {
                    return Value::Null;
                };
                if bytes.len() > 24 * 1024 * 1024 {
                    return Value::Null;
                }
                let ext = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("png")
                    .to_ascii_lowercase();
                let mime = match ext.as_str() {
                    "jpg" | "jpeg" => "image/jpeg",
                    "webp" => "image/webp",
                    "gif" => "image/gif",
                    "svg" => "image/svg+xml",
                    _ => "image/png",
                };
                use base64::Engine as _;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                Value::String(format!("data:{mime};base64,{b64}"))
            }
            "kit.readConfig" | "kit_read_config" => {
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
            "kit.writeConfig" | "kit_write_config" => {
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
            "destinations.setKey" | "dest_set_key" => {
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
            "destinations.unlink" | "dest_unlink" => {
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
            "destinations.twitchLogin" | "dest_twitch_login" => {
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
            "destinations.twitchCancel" | "dest_twitch_cancel" => {
                self.twitch_login.cancel_device_login().await;
                self.emit_serializable("dest:twitch-state", &TwitchLoginState::Idle);
                json!({ "ok": true })
            }
            "twitch.capabilities" | "twitch_capabilities" => {
                let settings = self.store.snapshot();
                let caps = TwitchApi::capabilities(
                    self.linked_twitch(&settings)
                        .and_then(|d| d.account)
                        .as_ref(),
                );
                serde_json::to_value(caps).unwrap_or(Value::Null)
            }
            "twitch.getChannelInfo" | "twitch_get_info" => {
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
            "twitch.updateChannelInfo" | "twitch_update_info" => {
                let settings = self.store.snapshot();
                let Some(dest) = self.linked_twitch(&settings) else {
                    return json!({ "ok": false, "error": "Entre com a Twitch em Destinos.", "needsRelogin": true });
                };
                let Some(acc) = dest.account.as_ref() else {
                    return json!({ "ok": false, "error": "Entre com a Twitch em Destinos.", "needsRelogin": true });
                };
                let info_val = params.get("info").cloned().unwrap_or(params);
                let info = serde_json::from_value::<LiveInfo>(info_val).unwrap_or_default();
                let res = self
                    .twitch_api
                    .update_channel_info(&dest.id, acc, &info, &settings.twitch_client_id)
                    .await;
                serde_json::to_value(res).unwrap_or(Value::Null)
            }
            "twitch.searchCategories" | "twitch_search_categories" => {
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
            "twitch.sendChat" | "twitch_send_chat" => {
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
            "chat.multichatStatus" | "multichat_status" => {
                let st = self.multichat.status().await;
                serde_json::to_value(st).unwrap_or(json!([]))
            }
            "chat.multichatSnapshot" | "multichat_snapshot" => {
                let msgs = self.multichat.snapshot_messages().await;
                serde_json::to_value(msgs).unwrap_or(json!([]))
            }
            "chat.reconnectMultichat" | "multichat_reconnect" => {
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
            "audio.listApps" | "audio_list_apps" => {
                let apps = tokio::task::spawn_blocking(AudioMixerEngine::list_audio_apps)
                    .await
                    .unwrap_or_default();
                serde_json::to_value(apps).unwrap_or(json!([]))
            }
            "audio.appForWindow" | "audio_app_for_window" => {
                let src_id = params
                    .get("captureId")
                    .or_else(|| params.get("sourceId"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let app = tokio::task::spawn_blocking(move || {
                    let apps = AudioMixerEngine::list_audio_apps();
                    apps.into_iter().find(|a| {
                        src_id.contains(&a.pid.to_string())
                            || (a.hwnd != 0 && src_id.contains(&a.hwnd.to_string()))
                            || (!a.exe.is_empty()
                                && src_id
                                    .to_ascii_lowercase()
                                    .contains(&a.exe.to_ascii_lowercase()))
                    })
                })
                .await
                .ok()
                .flatten();
                serde_json::to_value(app).unwrap_or(Value::Null)
            }
            "audio.sync" | "audio_sync" => {
                let settings = self.store.snapshot();
                if self.audio_runtime.is_active() {
                    self.audio_runtime.sync_channels(&settings.audio_mixer);
                }
                let st = self
                    .audio_mixer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .statuses();
                self.emit_serializable("audio:status", &st);
                json!({ "ok": true })
            }
            "audio.listDevices" | "audio_list_devices" => {
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
            "audio.levels" | "audio_levels" => {
                let levels = self
                    .audio_mixer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .read_levels();
                serde_json::to_value(levels).unwrap_or(json!([]))
            }
            "audio.statuses" | "audio_statuses" => {
                let st = self
                    .audio_mixer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .statuses();
                serde_json::to_value(st).unwrap_or(json!([]))
            }
            "audio.setSceneMics" | "audio_set_scene_mics" => {
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
            "audio.setChannelMute" | "audio_set_channel_mute" => {
                let id = params.get("channelId").or_else(|| params.get("id")).and_then(|v| v.as_str()).unwrap_or("");
                let muted = params.get("muted").and_then(|v| v.as_bool()).unwrap_or(false);
                let mut channels = self.store.snapshot().audio_mixer.channels;
                if let Some(ch) = channels.iter_mut().find(|c| c.id == id || c.name.eq_ignore_ascii_case(id)) {
                    ch.muted = muted;
                    let _ = self.apply_settings_patch_internal(json!({ "audioMixer": { "channels": channels } })).await;
                    json!({ "ok": true })
                } else {
                    json!({ "ok": false, "error": format!("Canal de áudio '{id}' não encontrado.") })
                }
            }
            "audio.setChannelVolume" | "audio_set_channel_volume" => {
                let id = params.get("channelId").or_else(|| params.get("id")).and_then(|v| v.as_str()).unwrap_or("");
                let volume_db = params
                    .get("volumeDb")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as f32;
                let mut channels = self.store.snapshot().audio_mixer.channels;
                if let Some(ch) = channels.iter_mut().find(|c| c.id == id || c.name.eq_ignore_ascii_case(id)) {
                    ch.volume_db = volume_db.clamp(MIN_DB, MAX_DB);
                    let _ = self.apply_settings_patch_internal(json!({ "audioMixer": { "channels": channels } })).await;
                    json!({ "ok": true })
                } else {
                    json!({ "ok": false, "error": format!("Canal de áudio '{id}' não encontrado.") })
                }
            }

            // ------------------------------------------------ Plugins VST3 / CLAP
            "plugins.scan" | "plugins_scan" => {
                let list = tokio::task::spawn_blocking(vst_host::scan_plugins)
                    .await
                    .unwrap_or_default();
                serde_json::to_value(list).unwrap_or(json!([]))
            }
            "plugins.openGui" | "plugins_open_gui" => {
                let channel_id = params
                    .get("channelId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let slot_id = params
                    .get("slotId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let res = self
                    .audio_mixer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .open_plugin_gui(&channel_id, &slot_id);
                match res {
                    Ok(()) => json!({ "ok": true }),
                    Err(error) => json!({ "ok": false, "error": error }),
                }
            }
            "plugins.setParam" | "plugins_set_param" => {
                let channel_id = params
                    .get("channelId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let slot_id = params
                    .get("slotId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let param_id = params.get("paramId").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let value = params.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let res = self
                    .audio_mixer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .set_plugin_param(&channel_id, &slot_id, param_id, value);
                match res {
                    Ok(()) => json!({ "ok": true }),
                    Err(error) => json!({ "ok": false, "error": error }),
                }
            }

            // ------------------------------------------------ GPU Zero-Copy & Dynamic Bitrate (ABR)
            "gpu.status" | "gpu_status" => {
                let st = self.gpu_pipeline_status();
                serde_json::to_value(st).unwrap_or(Value::Null)
            }
            "abr.status" | "abr_status" => {
                let list = self.multistream.lock().await.abr_telemetry();
                serde_json::to_value(list).unwrap_or(json!([]))
            }

            // ------------------------------------------------ Câmera Virtual (v4l2loopback / Windows ShMem + MF)
            "virtualCam.status" | "virtual_cam_status" => {
                let st = self
                    .virtual_cam
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .status();
                serde_json::to_value(st).unwrap_or(Value::Null)
            }
            "virtualCam.devices" | "virtual_cam_devices" => {
                let list = tokio::task::spawn_blocking(virtual_cam::list_virtual_camera_devices)
                    .await
                    .unwrap_or_default();
                serde_json::to_value(list).unwrap_or(json!([]))
            }
            "virtualCam.start" | "virtual_cam_start" => {
                let orientation = params
                    .get("orientation")
                    .and_then(|v| v.as_str())
                    .map(|s| match s {
                        "vertical" => CanvasOrientation::Vertical,
                        _ => CanvasOrientation::Horizontal,
                    });
                let st = {
                    let mut vcam = self.virtual_cam.lock().unwrap_or_else(|e| e.into_inner());
                    vcam.start(orientation)
                };
                self.emit_serializable("virtual-cam:status", &st);
                serde_json::to_value(st).unwrap_or(Value::Null)
            }
            "virtualCam.stop" | "virtual_cam_stop" => {
                let st = {
                    let mut vcam = self.virtual_cam.lock().unwrap_or_else(|e| e.into_inner());
                    vcam.stop()
                };
                self.emit_serializable("virtual-cam:status", &st);
                serde_json::to_value(st).unwrap_or(Value::Null)
            }
            "virtualCam.pushFrame" | "virtual_cam_push_frame" => {
                let width = params.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let height = params.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let bgra: Vec<u8> = params
                    .get("bgra")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_u64().map(|n| n as u8))
                            .collect()
                    })
                    .unwrap_or_default();
                let res = self
                    .virtual_cam
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push_frame_bgra(width, height, &bgra);
                match res {
                    Ok(()) => json!({ "ok": true }),
                    Err(error) => json!({ "ok": false, "error": error }),
                }
            }

            // ------------------------------------------------ Controle Externo (Stream Deck / obs-websocket v5)
            "externalControl.status" | "external_control_status" => {
                let st = self.external_control_status().await;
                serde_json::to_value(st).unwrap_or(Value::Null)
            }
            "externalControl.setObsPassword" | "external_control_set_obs_password" => {
                let raw_pw = params
                    .get("password")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let secret_opt = if raw_pw.is_empty() {
                    None
                } else {
                    Some(raw_pw.as_str())
                };
                if let Err(error) = self.store.set_secret(&obs_ws_password_secret(), secret_opt) {
                    return json!({ "ok": false, "error": error });
                }
                let (_, _) = self
                    .apply_settings_patch_internal(json!({
                        "externalControl": { "obsWsHasPassword": !raw_pw.is_empty() }
                    }))
                    .await;
                let _ = self.reconcile_obs_ws().await;
                json!({ "ok": true })
            }
            "externalControl.restart" | "external_control_restart" => {
                let st = self.reconcile_obs_ws().await;
                serde_json::to_value(st).unwrap_or(Value::Null)
            }

            // ------------------------------------------------ LAN P2P Host / Discovery / Viewer
            "host.start" | "host_start" => {
                let opts = params.get("opts").unwrap_or(&params);
                let settings = self.store.snapshot();
                let source_name = opts
                    .get("sourceName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Tela Principal")
                    .to_string();
                let source_kind = match opts.get("sourceKind").and_then(|v| v.as_str()) {
                    Some("window") => SourceKind::Window,
                    _ => SourceKind::Screen,
                };
                let host_name = opts
                    .get("hostName")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&settings.host_name)
                    .to_string();
                let require_pin = opts
                    .get("requirePin")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(settings.require_pin);
                let max_viewers = (opts
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
            "host.stop" | "host_stop" => {
                self.discovery.lock().await.stop_announcing().await;
                self.signaling.lock().await.stop().await;
                json!({ "ok": true })
            }
            "host.kick" | "host_kick" => {
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
            "host.send" | "host_send" => {
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
            "host.broadcast" | "host_broadcast" => {
                if let Some(msg_val) = params.get("msg") {
                    if let Ok(msg) = serde_json::from_value::<HostToViewer>(msg_val.clone()) {
                        self.signaling.lock().await.broadcast(&msg);
                    }
                }
                json!({ "ok": true })
            }
            "host.updateSource" | "host_update_source" => {
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
            "host.setViewerCount" | "host_set_viewer_count" => {
                let count = params
                    .get("n")
                    .or_else(|| params.get("count"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                self.discovery
                    .lock()
                    .await
                    .patch_announce(Some(count), None, None);
                json!({ "ok": true })
            }
            "discovery.start" | "discovery_start" => {
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
            "discovery.stop" | "discovery_stop" => {
                self.discovery.lock().await.stop_browsing().await;
                json!({ "ok": true })
            }
            "discovery.refresh" | "discovery_refresh" => {
                self.discovery.lock().await.refresh().await;
                json!({ "ok": true })
            }
            "viewer.connect" | "viewer_connect" => {
                let opts = params.get("opts").unwrap_or(&params);
                let ip = opts
                    .get("ip")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let port = opts.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
                let name = opts
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Espectador")
                    .to_string();
                let pin = opts
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
            "viewer.send" | "viewer_send" => {
                let msg_val = params.get("msg").cloned().unwrap_or(params);
                if let Ok(msg) = serde_json::from_value::<ViewerToHost>(msg_val) {
                    self.viewer_client.lock().await.send(&msg);
                }
                json!({ "ok": true })
            }
            "viewer.disconnect" | "viewer_disconnect" => {
                self.viewer_client.lock().await.disconnect();
                json!({ "ok": true })
            }

            // ------------------------------------------------ Multistream
            "multistream.status" | "multistream_status" => {
                let st = self.multistream.lock().await.engine_status();
                serde_json::to_value(st).unwrap_or(Value::Null)
            }
            "multistream.probe" | "multistream_probe" => {
                let settings = self.store.snapshot();
                let mut ms = self.multistream.lock().await;
                ms.set_ffmpeg_path(settings.ffmpeg_path.clone());
                let res = ms.refresh_probe().await;
                json!({ "ffmpegFound": res.ffmpeg_found, "encoder": res.encoder })
            }
            "multistream.start" | "multistream_start" => {
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
                    Ok(status) => {
                        self.emit_serializable("multistream:status", &status);
                        json!({ "ok": true, "status": status })
                    }
                    Err(error) => {
                        let status = self.multistream.lock().await.engine_status();
                        self.emit_serializable("multistream:status", &status);
                        json!({ "ok": false, "status": status, "error": error })
                    }
                }
            }
            "multistream.pushChunk" | "multistream_push_chunk" => {
                let orient = match params.get("orientation").and_then(|v| v.as_str()) {
                    Some("vertical") => CanvasOrientation::Vertical,
                    _ => CanvasOrientation::Horizontal,
                };
                if let Some(arr) = params.get("chunk").and_then(|v| v.as_array()) {
                    let bytes: Vec<u8> = arr
                        .iter()
                        .filter_map(|x| x.as_u64().map(|n| n as u8))
                        .collect();
                    self.multistream.lock().await.push_chunk(orient, &bytes);
                }
                json!({ "ok": true })
            }
            "multistream.stop" | "multistream_stop" => {
                let status = {
                    let mut ms = self.multistream.lock().await;
                    let _ = ms.stop_async().await;
                    ms.engine_status()
                };
                self.emit_serializable("multistream:status", &status);
                serde_json::to_value(status).unwrap_or(Value::Null)
            }

            // ------------------------------------------------ Recorder & Replay Buffer (sem deadlock!)
            "recorder.status" | "recorder_status" => {
                let st = self
                    .recorder
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .status();
                serde_json::to_value(st).unwrap_or(Value::Null)
            }
            "recorder.start" | "recorder_start" => {
                let res = self
                    .recorder
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .start_recording();
                match res {
                    Ok(status) => {
                        self.emit_serializable("recorder:status", &status);
                        json!({ "ok": true, "status": status })
                    }
                    Err(error) => json!({ "ok": false, "error": error }),
                }
            }
            "recorder.stop" | "recorder_stop" => {
                let (status, finished_path) = self
                    .recorder
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .stop_recording_with_path();
                self.emit_serializable("recorder:status", &status);
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
                serde_json::to_value(status).unwrap_or(Value::Null)
            }
            "recorder.setReplayBuffer" | "recorder_set_replay" => {
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
                self.emit_serializable("recorder:status", &status);
                serde_json::to_value(status).unwrap_or(Value::Null)
            }
            "recorder.pushChunk" | "recorder_push_chunk" => {
                if let Some(arr) = params.get("chunk").and_then(|v| v.as_array()) {
                    let bytes: Vec<u8> = arr
                        .iter()
                        .filter_map(|x| x.as_u64().map(|n| n as u8))
                        .collect();
                    let mut rec = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
                    rec.push_media_chunk(&bytes);
                }
                json!({ "ok": true })
            }
            "recorder.resetStream" | "recorder_reset" => {
                self.recorder
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .reset_stream();
                json!({ "ok": true })
            }
            "recorder.saveReplay" | "recorder_save_replay" => {
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
                        self.emit_serializable("recorder:status", &status);
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
            "capture.list" | "sources.list" | "sources_list" => {
                let list = tokio::task::spawn_blocking(capture::list_capture_sources)
                    .await
                    .unwrap_or_default();
                serde_json::to_value(list).unwrap_or(json!([]))
            }
            "sources.select" | "sources_select" => json!({ "ok": true }),
            "bs_sync" | "bs_reload" | "bs_ack" => json!({ "ok": true }),
            "capture.pushBrowserDirtyRect" | "capture_push_browser_dirty_rect" => {
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
            "studio.setLiveVertical" | "studio_set_live_vertical" => {
                let active = params
                    .get("active")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let mut comp = self.compositor.lock().unwrap_or_else(|e| e.into_inner());
                comp.set_live_vertical(active);
                json!({ "ok": true, "verticalActive": comp.is_vertical_active() })
            }
            "studio.setScenes" | "studio_set_scenes" => {
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
                    .set_scenes_with_config(
                        prog,
                        prev,
                        transition,
                        duration_ms,
                        Some(settings.scenes.custom_transition.clone()),
                        now_ms(),
                    );
                json!({ "ok": true })
            }
            "studio.setSceneItemEnabled" | "studio_set_scene_item_enabled" => {
                let scene_id = params
                    .get("sceneId")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let item_id = params.get("itemId").and_then(|v| v.as_str()).unwrap_or("");
                let enabled = params
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let mut scenes = self.store.snapshot().scenes.scenes;
                let target_scene = match scene_id {
                    Some(sid) => scenes.iter_mut().find(|s| s.id == sid || s.name.eq_ignore_ascii_case(&sid)),
                    None => {
                        let active = self.store.snapshot().scenes.active_scene_id;
                        scenes.iter_mut().find(|s| s.id == active)
                    }
                };
                if let Some(sc) = target_scene {
                    if let Some(item) = sc.items.iter_mut().find(|i| i.id == item_id || i.name.eq_ignore_ascii_case(item_id)) {
                        item.visible = enabled;
                        let _ = self
                            .apply_settings_patch_internal(json!({ "scenes": { "scenes": scenes } }))
                            .await;
                        json!({ "ok": true })
                    } else {
                        json!({ "ok": false, "error": format!("Item '{item_id}' não encontrado na cena.") })
                    }
                } else {
                    json!({ "ok": false, "error": "Cena não encontrada." })
                }
            }
            "studio.triggerTransition" | "studio_trigger_transition" => {
                let snap = self.store.snapshot().scenes;
                if snap.preview_scene_id.is_empty() || snap.preview_scene_id == snap.active_scene_id {
                    json!({ "ok": true })
                } else {
                    let _ = self
                        .apply_settings_patch_internal(json!({
                            "scenes": {
                                "activeSceneId": snap.preview_scene_id,
                                "previewSceneId": snap.active_scene_id,
                            }
                        }))
                        .await;
                    json!({ "ok": true })
                }
            }
            "studio.telemetry" | "telemetry.get" | "telemetry.stats" | "telemetry_get" => {
                let stats = self.collect_telemetry().await;
                serde_json::to_value(stats).unwrap_or(Value::Null)
            }
            "rtc.preferOpusStereo" | "rtc_prefer_opus_stereo" => {
                let sdp = params.get("sdp").and_then(|v| v.as_str()).unwrap_or("");
                let bitrate = params
                    .get("bitrateBps")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(192_000) as u32;
                json!({ "sdp": prefer_opus_stereo(sdp, bitrate) })
            }
            "rtc.applyCodecPreference" | "rtc_apply_codec_preference" => {
                let sdp = params.get("sdp").and_then(|v| v.as_str()).unwrap_or("");
                let codec = params.get("codec").and_then(|v| v.as_str()).unwrap_or("auto");
                json!({ "sdp": apply_codec_preference(sdp, codec) })
            }
            "rtc.addPeer" | "rtc_add_peer" => {
                let vid = params.get("viewerId").and_then(|v| v.as_str()).unwrap_or("");
                if !vid.is_empty() {
                    self.rtc_host
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .add_peer(vid.to_string());
                }
                json!({ "ok": true })
            }
            "rtc.removePeer" | "rtc_remove_peer" => {
                let vid = params.get("viewerId").and_then(|v| v.as_str()).unwrap_or("");
                self.rtc_host
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove_peer(vid);
                json!({ "ok": true })
            }
            "rtc.onVoiceOffer" | "rtc_on_voice_offer" => {
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

    /// Desligamento gracioso completo sem vazar processos filhos (FFmpeg/sockets/hotkeys/câmera virtual/áudio).
    pub async fn shutdown(&self) {
        self.bg_cancel.cancel();
        self.audio_runtime.stop();
        {
            let mut vcam = self.virtual_cam.lock().unwrap_or_else(|e| e.into_inner());
            let _ = vcam.stop();
        }
        if let Some(ref mut obs) = *self.obs_ws.lock().await {
            obs.stop().await;
        }
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

// ------------------------------------------------------------------ obs-websocket v5 Control Host

fn find_scene_index(
    scenes: &[Scene],
    scene_name: Option<&str>,
    scene_uuid: Option<&str>,
) -> Option<usize> {
    if let Some(uuid) = scene_uuid {
        if let Some(idx) = scenes.iter().position(|s| s.id == uuid) {
            return Some(idx);
        }
    }
    if let Some(name) = scene_name {
        if let Some(idx) = scenes
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name) || s.id == name)
        {
            return Some(idx);
        }
    }
    None
}

fn find_channel_index(
    channels: &[AudioChannel],
    input_name: Option<&str>,
    input_uuid: Option<&str>,
) -> Option<usize> {
    if let Some(uuid) = input_uuid {
        if let Some(idx) = channels.iter().position(|c| c.id == uuid) {
            return Some(idx);
        }
    }
    if let Some(name) = input_name {
        if let Some(idx) = channels
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name) || c.id == name)
        {
            return Some(idx);
        }
    }
    None
}

fn canonical_hotkey_action(name: &str) -> Option<HotkeyAction> {
    let norm = name.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    match norm.as_str() {
        "toggle-mic-mute" | "togglemicmute" => Some(HotkeyAction::ToggleMicMute),
        "toggle-system-mute" | "togglesystemmute" => Some(HotkeyAction::ToggleSystemMute),
        "toggle-privacy-shield" | "toggleprivacyshield" => Some(HotkeyAction::TogglePrivacyShield),
        "save-replay-clip" | "savereplayclip" => Some(HotkeyAction::SaveReplayClip),
        "toggle-vertical-preview" | "toggleverticalpreview" => {
            Some(HotkeyAction::ToggleVerticalPreview)
        }
        "toggle-camera" | "togglecamera" => Some(HotkeyAction::ToggleCamera),
        _ => None,
    }
}

impl ObsControlHost for AppEngine {
    fn get_stats(&self) -> ObsFuture<'_, ObsResult<ObsStats>> {
        Box::pin(async move {
            let stats = self.collect_telemetry().await;
            Ok(ObsStats {
                active_fps: stats.compositor_fps as f64,
                average_frame_render_time: stats.render_ms_p95 as f64,
                cpu_usage: 0.0,
                memory_usage: 0.0,
                available_disk_space: 0.0,
                render_skipped_frames: stats.dropped_frames,
                render_total_frames: 0,
                output_skipped_frames: stats.dropped_frames,
                output_total_frames: 0,
            })
        })
    }

    fn get_hotkey_list(&self) -> ObsFuture<'_, ObsResult<Vec<String>>> {
        Box::pin(async move {
            Ok(vec![
                HotkeyAction::ToggleMicMute.as_str().to_string(),
                HotkeyAction::ToggleSystemMute.as_str().to_string(),
                HotkeyAction::TogglePrivacyShield.as_str().to_string(),
                HotkeyAction::SaveReplayClip.as_str().to_string(),
                HotkeyAction::ToggleVerticalPreview.as_str().to_string(),
                HotkeyAction::ToggleCamera.as_str().to_string(),
            ])
        })
    }

    fn trigger_hotkey_by_name(&self, hotkey_name: String) -> ObsFuture<'_, ObsResult<()>> {
        Box::pin(async move {
            let Some(action) = canonical_hotkey_action(&hotkey_name) else {
                return Err(ObsRequestError::resource_not_found(format!(
                    "Unknown hotkey: {hotkey_name}"
                )));
            };
            self.emit("hotkey:triggered", Value::String(action.as_str().to_string()));
            Ok(())
        })
    }

    fn get_scene_list(&self) -> ObsFuture<'_, ObsResult<ObsSceneList>> {
        Box::pin(async move {
            let scenes_st = self.store.snapshot().scenes;
            let active = scenes_st
                .scenes
                .iter()
                .find(|s| s.id == scenes_st.active_scene_id);
            let preview = scenes_st
                .scenes
                .iter()
                .find(|s| s.id == scenes_st.preview_scene_id);
            let scenes = scenes_st
                .scenes
                .iter()
                .enumerate()
                .map(|(idx, s)| ObsSceneInfo {
                    scene_name: s.name.clone(),
                    scene_uuid: s.id.clone(),
                    scene_index: idx,
                })
                .collect();
            Ok(ObsSceneList {
                current_program_scene_name: active.map(|s| s.name.clone()),
                current_program_scene_uuid: active.map(|s| s.id.clone()),
                current_preview_scene_name: preview.map(|s| s.name.clone()),
                current_preview_scene_uuid: preview.map(|s| s.id.clone()),
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
            let scenes_st = self.store.snapshot().scenes;
            let Some(idx) = find_scene_index(
                &scenes_st.scenes,
                scene_name.as_deref(),
                scene_uuid.as_deref(),
            ) else {
                return Err(ObsRequestError::resource_not_found("Scene not found"));
            };
            let sc = scenes_st.scenes[idx].clone();
            let _ = self
                .apply_settings_patch_internal(json!({
                    "scenes": { "activeSceneId": sc.id }
                }))
                .await;
            Ok(ObsSceneInfo {
                scene_name: sc.name,
                scene_uuid: sc.id,
                scene_index: idx,
            })
        })
    }

    fn set_current_preview_scene(
        &self,
        scene_name: Option<String>,
        scene_uuid: Option<String>,
    ) -> ObsFuture<'_, ObsResult<ObsSceneInfo>> {
        Box::pin(async move {
            let scenes_st = self.store.snapshot().scenes;
            if !scenes_st.studio_mode {
                return Err(ObsRequestError::studio_mode_not_active());
            }
            let Some(idx) = find_scene_index(
                &scenes_st.scenes,
                scene_name.as_deref(),
                scene_uuid.as_deref(),
            ) else {
                return Err(ObsRequestError::resource_not_found("Scene not found"));
            };
            let sc = scenes_st.scenes[idx].clone();
            let _ = self
                .apply_settings_patch_internal(json!({
                    "scenes": { "previewSceneId": sc.id }
                }))
                .await;
            Ok(ObsSceneInfo {
                scene_name: sc.name,
                scene_uuid: sc.id,
                scene_index: idx,
            })
        })
    }

    fn get_studio_mode_enabled(&self) -> ObsFuture<'_, ObsResult<bool>> {
        Box::pin(async move { Ok(self.store.snapshot().scenes.studio_mode) })
    }

    fn set_studio_mode_enabled(&self, enabled: bool) -> ObsFuture<'_, ObsResult<()>> {
        Box::pin(async move {
            let _ = self
                .apply_settings_patch_internal(json!({
                    "scenes": { "studioMode": enabled }
                }))
                .await;
            Ok(())
        })
    }

    fn trigger_studio_mode_transition(&self) -> ObsFuture<'_, ObsResult<()>> {
        Box::pin(async move {
            let scenes_st = self.store.snapshot().scenes;
            if !scenes_st.studio_mode {
                return Err(ObsRequestError::studio_mode_not_active());
            }
            if !scenes_st.preview_scene_id.is_empty()
                && scenes_st.preview_scene_id != scenes_st.active_scene_id
            {
                let _ = self
                    .apply_settings_patch_internal(json!({
                        "scenes": {
                            "activeSceneId": scenes_st.preview_scene_id,
                            "previewSceneId": scenes_st.active_scene_id,
                        }
                    }))
                    .await;
            }
            Ok(())
        })
    }

    fn get_scene_item_list(
        &self,
        scene_name: Option<String>,
        scene_uuid: Option<String>,
    ) -> ObsFuture<'_, ObsResult<Vec<ObsSceneItemInfo>>> {
        Box::pin(async move {
            let scenes_st = self.store.snapshot().scenes;
            let Some(idx) = find_scene_index(
                &scenes_st.scenes,
                scene_name.as_deref(),
                scene_uuid.as_deref(),
            ) else {
                return Err(ObsRequestError::resource_not_found("Scene not found"));
            };
            let sc = &scenes_st.scenes[idx];
            let items = sc
                .items
                .iter()
                .enumerate()
                .map(|(i, item)| ObsSceneItemInfo {
                    scene_item_id: (i + 1) as i64,
                    scene_item_index: i,
                    source_name: item.name.clone(),
                    source_uuid: item.id.clone(),
                    source_type: "OBS_SOURCE_TYPE_INPUT".to_string(),
                    input_kind: Some(source_type_to_obs_input_kind(item.source_type).to_string()),
                    scene_item_enabled: item.visible,
                    scene_item_locked: item.locked,
                })
                .collect();
            Ok(items)
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
            let mut scenes = self.store.snapshot().scenes.scenes;
            let Some(s_idx) =
                find_scene_index(&scenes, scene_name.as_deref(), scene_uuid.as_deref())
            else {
                return Err(ObsRequestError::resource_not_found("Scene not found"));
            };
            if scene_item_id < 1 {
                return Err(ObsRequestError::out_of_range("sceneItemId must be >= 1"));
            }
            let item_idx = (scene_item_id - 1) as usize;
            let Some(item) = scenes[s_idx].items.get_mut(item_idx) else {
                return Err(ObsRequestError::resource_not_found("Scene item not found"));
            };
            item.visible = enabled;
            let _ = self
                .apply_settings_patch_internal(json!({ "scenes": { "scenes": scenes } }))
                .await;
            Ok(())
        })
    }

    fn get_current_scene_transition(&self) -> ObsFuture<'_, ObsResult<ObsCurrentTransition>> {
        Box::pin(async move {
            let scenes_st = self.store.snapshot().scenes;
            let (name, kind, uuid, fixed) = transition_type_metadata(scenes_st.transition_type);
            Ok(ObsCurrentTransition {
                transition_name: name.to_string(),
                transition_uuid: uuid.to_string(),
                transition_kind: kind.to_string(),
                transition_fixed: fixed,
                transition_duration: if fixed {
                    None
                } else {
                    Some(scenes_st.transition_duration_ms)
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
            let Some(t) = obs_name_to_transition_type(&transition_name) else {
                return Err(ObsRequestError::resource_not_found(format!(
                    "Unknown transition: {transition_name}"
                )));
            };
            let (_, next) = self
                .apply_settings_patch_internal(json!({
                    "scenes": { "transitionType": t }
                }))
                .await;
            let (name, kind, uuid, fixed) = transition_type_metadata(next.scenes.transition_type);
            Ok(ObsCurrentTransition {
                transition_name: name.to_string(),
                transition_uuid: uuid.to_string(),
                transition_kind: kind.to_string(),
                transition_fixed: fixed,
                transition_duration: if fixed {
                    None
                } else {
                    Some(next.scenes.transition_duration_ms)
                },
                transition_configurable: !fixed,
            })
        })
    }

    fn set_current_scene_transition_duration(&self, duration_ms: u32) -> ObsFuture<'_, ObsResult<()>> {
        Box::pin(async move {
            if !(50..=20_000).contains(&duration_ms) {
                return Err(ObsRequestError::out_of_range(
                    "transitionDuration must be between 50 and 20000 ms",
                ));
            }
            let _ = self
                .apply_settings_patch_internal(json!({
                    "scenes": { "transitionDurationMs": duration_ms }
                }))
                .await;
            Ok(())
        })
    }

    fn get_input_list(
        &self,
        input_kind: Option<String>,
    ) -> ObsFuture<'_, ObsResult<Vec<ObsInputInfo>>> {
        Box::pin(async move {
            let channels = self.store.snapshot().audio_mixer.channels;
            let list = channels
                .into_iter()
                .filter_map(|ch| {
                    let (kind, unversioned) = audio_channel_kind_to_obs_input_kind(ch.kind);
                    if let Some(ref filter) = input_kind {
                        if !filter.eq_ignore_ascii_case(kind)
                            && !filter.eq_ignore_ascii_case(unversioned)
                        {
                            return None;
                        }
                    }
                    Some(ObsInputInfo {
                        input_name: ch.name,
                        input_uuid: ch.id,
                        input_kind: kind.to_string(),
                        unversioned_input_kind: unversioned.to_string(),
                    })
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
            let channels = self.store.snapshot().audio_mixer.channels;
            let Some(idx) =
                find_channel_index(&channels, input_name.as_deref(), input_uuid.as_deref())
            else {
                return Err(ObsRequestError::resource_not_found("Audio input not found"));
            };
            Ok(channels[idx].muted)
        })
    }

    fn set_input_mute(
        &self,
        input_name: Option<String>,
        input_uuid: Option<String>,
        muted: bool,
    ) -> ObsFuture<'_, ObsResult<()>> {
        Box::pin(async move {
            let mut channels = self.store.snapshot().audio_mixer.channels;
            let Some(idx) =
                find_channel_index(&channels, input_name.as_deref(), input_uuid.as_deref())
            else {
                return Err(ObsRequestError::resource_not_found("Audio input not found"));
            };
            channels[idx].muted = muted;
            let _ = self
                .apply_settings_patch_internal(json!({ "audioMixer": { "channels": channels } }))
                .await;
            Ok(())
        })
    }

    fn toggle_input_mute(
        &self,
        input_name: Option<String>,
        input_uuid: Option<String>,
    ) -> ObsFuture<'_, ObsResult<bool>> {
        Box::pin(async move {
            let mut channels = self.store.snapshot().audio_mixer.channels;
            let Some(idx) =
                find_channel_index(&channels, input_name.as_deref(), input_uuid.as_deref())
            else {
                return Err(ObsRequestError::resource_not_found("Audio input not found"));
            };
            let next_muted = !channels[idx].muted;
            channels[idx].muted = next_muted;
            let _ = self
                .apply_settings_patch_internal(json!({ "audioMixer": { "channels": channels } }))
                .await;
            Ok(next_muted)
        })
    }

    fn get_input_volume(
        &self,
        input_name: Option<String>,
        input_uuid: Option<String>,
    ) -> ObsFuture<'_, ObsResult<ObsInputVolume>> {
        Box::pin(async move {
            let channels = self.store.snapshot().audio_mixer.channels;
            let Some(idx) =
                find_channel_index(&channels, input_name.as_deref(), input_uuid.as_deref())
            else {
                return Err(ObsRequestError::resource_not_found("Audio input not found"));
            };
            Ok(ObsInputVolume::from_db(channels[idx].volume_db as f64))
        })
    }

    fn set_input_volume(
        &self,
        input_name: Option<String>,
        input_uuid: Option<String>,
        volume_db: f64,
    ) -> ObsFuture<'_, ObsResult<ObsInputVolume>> {
        Box::pin(async move {
            let mut channels = self.store.snapshot().audio_mixer.channels;
            let Some(idx) =
                find_channel_index(&channels, input_name.as_deref(), input_uuid.as_deref())
            else {
                return Err(ObsRequestError::resource_not_found("Audio input not found"));
            };
            let clamped = (volume_db as f32).clamp(MIN_DB, MAX_DB);
            channels[idx].volume_db = clamped;
            let _ = self
                .apply_settings_patch_internal(json!({ "audioMixer": { "channels": channels } }))
                .await;
            Ok(ObsInputVolume::from_db(clamped as f64))
        })
    }

    fn get_stream_status(&self) -> ObsFuture<'_, ObsResult<ObsStreamStatus>> {
        Box::pin(async move {
            let ms = self.multistream.lock().await;
            let st = ms.engine_status();
            let prog = ms.telemetry_snapshot();
            let (_, _, congestion) = ms.abr_summary();
            Ok(ObsStreamStatus {
                output_active: st.active,
                output_reconnecting: st.destinations.iter().any(|d| d.state == "reconnecting"),
                output_duration_ms: st
                    .destinations
                    .iter()
                    .map(|d| d.uptime_sec)
                    .max()
                    .unwrap_or(0)
                    .saturating_mul(1000),
                output_congestion: congestion as f64,
                output_bytes: st
                    .destinations
                    .iter()
                    .map(|d| d.bytes_sent)
                    .sum::<u64>()
                    .max(prog.total_size_bytes),
                output_skipped_frames: prog.dropped_frames,
                output_total_frames: 0,
            })
        })
    }

    fn start_stream(&self) -> ObsFuture<'_, ObsResult<()>> {
        Box::pin(async move {
            {
                let ms = self.multistream.lock().await;
                if ms.engine_status().active {
                    return Err(ObsRequestError::output_running("Stream is already active"));
                }
            }
            let settings = self.store.snapshot();
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
                .start_destinations(&dests_with_keys, settings.studio.fps, settings.ffmpeg_path)
                .await;
            match res {
                Ok(status) => {
                    self.emit_serializable("multistream:status", &status);
                    Ok(())
                }
                Err(err) => Err(ObsRequestError::action_failed(err)),
            }
        })
    }

    fn stop_stream(&self) -> ObsFuture<'_, ObsResult<()>> {
        Box::pin(async move {
            let status = {
                let mut ms = self.multistream.lock().await;
                if !ms.engine_status().active {
                    return Err(ObsRequestError::output_not_running("Stream is not active"));
                }
                let _ = ms.stop_async().await;
                ms.engine_status()
            };
            self.emit_serializable("multistream:status", &status);
            Ok(())
        })
    }

    fn get_record_status(&self) -> ObsFuture<'_, ObsResult<ObsRecordStatus>> {
        Box::pin(async move {
            let st = self
                .recorder
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .status();
            Ok(ObsRecordStatus {
                output_active: st.recording,
                output_paused: false,
                output_duration_ms: st.recordingDurationSec.saturating_mul(1000),
                output_bytes: 0,
                output_path: st.recordingPath,
            })
        })
    }

    fn start_record(&self) -> ObsFuture<'_, ObsResult<()>> {
        Box::pin(async move {
            let res = {
                let mut rec = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
                if rec.status().recording {
                    return Err(ObsRequestError::output_running(
                        "Recording is already active",
                    ));
                }
                rec.start_recording()
            };
            match res {
                Ok(st) => {
                    self.emit_serializable("recorder:status", &st);
                    Ok(())
                }
                Err(err) => Err(ObsRequestError::action_failed(err)),
            }
        })
    }

    fn stop_record(&self) -> ObsFuture<'_, ObsResult<Option<String>>> {
        Box::pin(async move {
            let (status, finished_path) = {
                let mut rec = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
                if !rec.status().recording {
                    return Err(ObsRequestError::output_not_running(
                        "Recording is not active",
                    ));
                }
                rec.stop_recording_with_path()
            };
            self.emit_serializable("recorder:status", &status);
            let out_path = finished_path.as_ref().map(|p| p.to_string_lossy().into_owned());
            if let Some(webm) = finished_path {
                let ffmpeg = self.store.snapshot().ffmpeg_path;
                let rec_ref = self.recorder.clone();
                let evt_tx = self.evt_tx.clone();
                let obs_tx = self.obs_event_tx.clone();
                tokio::spawn(async move {
                    let final_path = remux_to_mp4(webm, ffmpeg).await;
                    let st = {
                        let mut g = rec_ref.lock().unwrap_or_else(|e| e.into_inner());
                        g.mark_saved_replay_path(final_path);
                        g.status()
                    };
                    if let Ok(val) = serde_json::to_value(&st) {
                        let _ = evt_tx.send(RpcEvent {
                            event: "recorder:status".to_string(),
                            payload: val.clone(),
                        });
                        for ev in translate_rpc_event("recorder:status", &val) {
                            let _ = obs_tx.send(ev);
                        }
                    }
                });
            }
            Ok(out_path)
        })
    }

    fn get_replay_buffer_status(&self) -> ObsFuture<'_, ObsResult<ObsReplayBufferStatus>> {
        Box::pin(async move {
            let st = self
                .recorder
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .status();
            Ok(ObsReplayBufferStatus {
                output_active: st.replayBufferActive,
                last_saved_path: st.replaySavedPath,
            })
        })
    }

    fn start_replay_buffer(&self) -> ObsFuture<'_, ObsResult<()>> {
        Box::pin(async move {
            let secs = self.store.snapshot().studio.replay_buffer_seconds;
            let st = {
                let mut rec = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
                if rec.status().replayBufferActive {
                    return Err(ObsRequestError::output_running(
                        "Replay buffer is already active",
                    ));
                }
                rec.set_replay_buffer(true, secs)
            };
            self.emit_serializable("recorder:status", &st);
            self.emit_obs_event(ObsEvent::ReplayBufferStateChanged {
                output_active: true,
                output_state: "OBS_WEBSOCKET_OUTPUT_STARTED".to_string(),
            });
            Ok(())
        })
    }

    fn stop_replay_buffer(&self) -> ObsFuture<'_, ObsResult<()>> {
        Box::pin(async move {
            let secs = self.store.snapshot().studio.replay_buffer_seconds;
            let st = {
                let mut rec = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
                if !rec.status().replayBufferActive {
                    return Err(ObsRequestError::output_not_running(
                        "Replay buffer is not active",
                    ));
                }
                rec.set_replay_buffer(false, secs)
            };
            self.emit_serializable("recorder:status", &st);
            self.emit_obs_event(ObsEvent::ReplayBufferStateChanged {
                output_active: false,
                output_state: "OBS_WEBSOCKET_OUTPUT_STOPPED".to_string(),
            });
            Ok(())
        })
    }

    fn save_replay_buffer(&self) -> ObsFuture<'_, ObsResult<Option<String>>> {
        Box::pin(async move {
            let save_res = {
                let mut guard = self.recorder.lock().unwrap_or_else(|e| e.into_inner());
                if !guard.status().replayBufferActive {
                    return Err(ObsRequestError::output_not_running(
                        "Replay buffer is not active",
                    ));
                }
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
                    self.emit_serializable("recorder:status", &status);
                    Ok(Some(final_path.to_string_lossy().into_owned()))
                }
                Err(err) => Err(ObsRequestError::action_failed(err)),
            }
        })
    }

    fn get_virtual_cam_status(&self) -> ObsFuture<'_, ObsResult<bool>> {
        Box::pin(async move {
            Ok(self
                .virtual_cam
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_active())
        })
    }

    fn start_virtual_cam(&self) -> ObsFuture<'_, ObsResult<()>> {
        Box::pin(async move {
            let st = {
                let mut vcam = self.virtual_cam.lock().unwrap_or_else(|e| e.into_inner());
                if vcam.is_active() {
                    return Err(ObsRequestError::output_running(
                        "Virtual camera is already active",
                    ));
                }
                vcam.start(None)
            };
            self.emit_serializable("virtual-cam:status", &st);
            if st.active {
                Ok(())
            } else {
                Err(ObsRequestError::action_failed(
                    st.error
                        .unwrap_or_else(|| "Failed to start virtual camera".to_string()),
                ))
            }
        })
    }

    fn stop_virtual_cam(&self) -> ObsFuture<'_, ObsResult<()>> {
        Box::pin(async move {
            let st = {
                let mut vcam = self.virtual_cam.lock().unwrap_or_else(|e| e.into_inner());
                if !vcam.is_active() {
                    return Err(ObsRequestError::output_not_running(
                        "Virtual camera is not active",
                    ));
                }
                vcam.stop()
            };
            self.emit_serializable("virtual-cam:status", &st);
            Ok(())
        })
    }

    fn subscribe_events(&self) -> Option<broadcast::Receiver<ObsEvent>> {
        Some(self.obs_event_tx.subscribe())
    }
}

// ------------------------------------------------------------------ Validação & Servidor WebSocket RPC

/// Valida handshake HTTP do WebSocket RPC:
/// - Por padrão rejeita QUALQUER cabeçalho `Origin` (bloqueia CSWSH vindo de navegadores).
/// - Opcionalmente permite origens explicitamente configuradas em `allowed_origins` (nunca `*`).
/// - Exige token em `Authorization: Bearer <token>` ou query `?token=<token>` via `ConstantTimeEq`.
pub fn validate_ws_handshake_with_origins(
    req: &Request,
    expected_token: &str,
    allowed_origins: &[String],
) -> Result<(), (http::StatusCode, &'static str)> {
    if let Some(origin_val) = req.headers().get("origin") {
        let origin_str = origin_val.to_str().unwrap_or("").trim();
        let allowed = !origin_str.is_empty()
            && allowed_origins.iter().any(|candidate| {
                let c = candidate.trim();
                !c.is_empty() && c != "*" && c.eq_ignore_ascii_case(origin_str)
            });
        if !allowed {
            return Err((
                http::StatusCode::FORBIDDEN,
                "Origin header forbidden on local RPC",
            ));
        }
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

/// Validação estrita padrão sem lista de origens permitidas.
pub fn validate_ws_handshake(
    req: &Request,
    expected_token: &str,
) -> Result<(), (http::StatusCode, &'static str)> {
    validate_ws_handshake_with_origins(req, expected_token, &[])
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
        let allowed_origins = eng.store.snapshot().external_control.allow_browser_origins;

        tokio::spawn(async move {
            let mut ws_cfg = WebSocketConfig::default();
            ws_cfg.max_message_size = Some(MAX_RPC_WS_MESSAGE_BYTES);
            ws_cfg.max_frame_size = Some(MAX_RPC_WS_MESSAGE_BYTES);

            let ws_res = tokio_tungstenite::accept_hdr_async_with_config(
                stream,
                move |req: &Request, res: Response| -> Result<Response, ErrorResponse> {
                    match validate_ws_handshake_with_origins(req, &expected_token, &allowed_origins)
                    {
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

            eng.rpc_clients.fetch_add(1, Ordering::Relaxed);
            let ext_st = eng.external_control_status().await;
            eng.emit_serializable("external-control:status", &ext_st);

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
            eng.rpc_clients.fetch_sub(1, Ordering::Relaxed);
            let ext_st = eng.external_control_status().await;
            eng.emit_serializable("external-control:status", &ext_st);
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

    #[test]
    fn test_origin_allowlist_and_wildcard_rejection() {
        let token = generate_session_token();
        let req_allowed = http::Request::builder()
            .uri(format!("ws://127.0.0.1:41236/?token={token}"))
            .header("Origin", "https://deck.local")
            .body(())
            .unwrap();

        // Sem allowlist: rejeita qualquer Origin
        assert_eq!(
            validate_ws_handshake_with_origins(&req_allowed, &token, &[])
                .unwrap_err()
                .0,
            http::StatusCode::FORBIDDEN
        );
        // Wildcard '*' nunca é aceito por segurança contra CSWSH
        assert_eq!(
            validate_ws_handshake_with_origins(&req_allowed, &token, &["*".to_string()])
                .unwrap_err()
                .0,
            http::StatusCode::FORBIDDEN
        );
        // Origem exata configurada: aceita
        assert!(validate_ws_handshake_with_origins(
            &req_allowed,
            &token,
            &["https://deck.local".to_string()]
        )
        .is_ok());
    }

    #[tokio::test]
    async fn test_pro_production_rpc_and_obs_control_host() {
        let (engine, dir) = test_engine("pro-prod");

        // 1. Plugins scan, GPU status, ABR status, Virtual Camera devices
        let plugins = engine.handle_command("plugins_scan", Value::Null).await;
        assert!(plugins.is_array());

        let gpu = engine.handle_command("gpu_status", Value::Null).await;
        assert!(gpu.get("compositorBackend").is_some());

        let abr = engine.handle_command("abr_status", Value::Null).await;
        assert!(abr.is_array());

        let vcam_devs = engine
            .handle_command("virtual_cam_devices", Value::Null)
            .await;
        assert!(vcam_devs.is_array());

        // 2. Virtual camera start + binary frame push + stop
        let started = engine
            .handle_command("virtual_cam_start", json!({ "orientation": "horizontal" }))
            .await;
        assert!(started.get("active").is_some());

        // Header de 8 bytes do bridge.ts com kind = 4 (Virtual Camera BGRA)
        let mut frame = vec![0x50, 0x43, 0x04, 0x00, 0x18, 0x00, 0x00, 0x00];
        frame.extend_from_slice(&2u32.to_le_bytes());
        frame.extend_from_slice(&2u32.to_le_bytes());
        frame.extend_from_slice(&[255u8; 16]);
        engine.handle_binary(&frame).await;

        let stopped = engine.handle_command("virtual_cam_stop", Value::Null).await;
        assert_eq!(stopped.get("active").and_then(|v| v.as_bool()), Some(false));

        // 3. OBS WebSocket v5 password + ObsControlHost live control
        let pw_res = engine
            .handle_command(
                "external_control_set_obs_password",
                json!({ "password": "stream-deck-secret" }),
            )
            .await;
        assert_eq!(pw_res.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert!(engine.store.snapshot().external_control.obs_ws_has_password);

        let mut obs_rx = engine.subscribe_events().expect("obs event stream");

        // Ativar Studio Mode e trocar transição via ObsControlHost
        engine.set_studio_mode_enabled(true).await.unwrap();
        assert!(engine.get_studio_mode_enabled().await.unwrap());

        let tr = engine
            .set_current_scene_transition("Luma Wipe".to_string())
            .await
            .unwrap();
        assert_eq!(tr.transition_name, "Luma Wipe");
        engine
            .set_current_scene_transition_duration(650)
            .await
            .unwrap();
        assert_eq!(
            engine.store.snapshot().scenes.transition_duration_ms,
            650
        );

        // Toggle mute no canal de áudio padrão via ObsControlHost
        let inputs = engine.get_input_list(None).await.unwrap();
        assert!(!inputs.is_empty());
        let first_uuid = inputs[0].input_uuid.clone();
        let before_mute = engine
            .get_input_mute(None, Some(first_uuid.clone()))
            .await
            .unwrap();
        let after_mute = engine
            .toggle_input_mute(None, Some(first_uuid.clone()))
            .await
            .unwrap();
        assert_eq!(after_mute, !before_mute);

        // Verifica que os ObsEvents em tempo real foram emitidos
        let mut saw_studio_ev = false;
        let mut saw_mute_ev = false;
        while let Ok(ev) = obs_rx.try_recv() {
            match ev {
                ObsEvent::StudioModeStateChanged {
                    studio_mode_enabled: true,
                } => saw_studio_ev = true,
                ObsEvent::InputMuteStateChanged { .. } => saw_mute_ev = true,
                _ => {}
            }
        }
        assert!(saw_studio_ev, "StudioModeStateChanged must be emitted");
        assert!(saw_mute_ev, "InputMuteStateChanged must be emitted");

        // Verifica telemetria enriquecida
        let tel = engine.collect_telemetry().await;
        assert!(!tel.compositor_backend.is_empty());

        engine.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
