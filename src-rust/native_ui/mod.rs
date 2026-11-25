//! Interface desktop 100% nativa em Rust (`eframe` + `egui` + `wgpu 24`).
//!
//! Substitui integralmente Tauri v2, WebView2 e React/Node.js:
//! - Palco GPU nativo alimentado por `NativeStudioPump` (DXGI / PrintWindow / wgpu).
//! - Seletor nativo de telas e janelas com miniaturas reais ao vivo (sem popup de navegador).
//! - Visual inspirado diretamente na landing page GitHub Pages (carbono fosco, vermilion tally,
//!   feixes espectrais do prisma e painéis translúcidos glassmorphism).

pub mod panels;
pub mod picker;
pub mod stage;
pub mod theme;

use crate::capture::CaptureStatus;
use crate::compositor::capture_key_for_item;
use crate::native_capture::{NativeStudioPump, PreviewSnapshot};
use crate::native_ui::picker::{PickerPurpose, PickerSelection, SourcePickerModal};
use crate::native_ui::stage::{StageAction, StageInteraction};
use crate::protocol::{
    AppSettings, AudioChannel, AudioChannelKind, AudioFilters, AudioLevelSample,
    CanvasOrientation, Destination, DiscoveredHost, ExternalControlStatus, GpuPipelineStatus,
    KitInfo, MultiChatEvent, MultiChatMessage, MultistreamEngineStatus, PlatformId,
    RecorderStatus, Scene, SceneItem, SourceKind, SourceSettings, SourceTransform, SourceType,
    StudioScenesState, TelemetryStats, TransitionType, TwitchCapabilities, TwitchLoginState,
    UpdateStatus, VirtualCameraStatus,
};
use crate::rpc::{AppEngine, RpcEvent};
use egui::{ColorImage, Context, TextureHandle, TextureOptions, Vec2, ViewportCommand};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavTab {
    Studio,
    Widgets,
    Destinations,
    Lan,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightTab {
    Properties,
    Chat,
}

#[derive(Debug, Clone)]
pub struct ToastNotice {
    pub message: String,
    pub is_error: bool,
    pub expires_at: Instant,
}

#[derive(Debug)]
pub enum BgUiMsg {
    Toast { message: String, is_error: bool },
    SettingsRefreshed(AppSettings),
    MultistreamStatus(MultistreamEngineStatus),
    RecorderStatus(RecorderStatus),
    VirtualCamStatus(VirtualCameraStatus),
    ExternalControlStatus(ExternalControlStatus),
    UpdateStatus(UpdateStatus),
    TwitchCapabilities(TwitchCapabilities),
    TwitchLogin(TwitchLoginState),
    KitLoaded(Option<KitInfo>),
    LanHostStarted { ip: String, port: u16, pin: Option<String> },
    LanHostStopped,
    DiscoveredHosts(Vec<DiscoveredHost>),
    FirewallChecked(bool),
    ExitRequested,
}

pub struct PrismCastNativeApp {
    pub engine: Arc<AppEngine>,
    pub rt: tokio::runtime::Handle,
    pub pump: NativeStudioPump,

    pub settings: AppSettings,
    pub undo_stack: Vec<StudioScenesState>,

    pub active_tab: NavTab,
    pub right_tab: RightTab,
    pub edit_vertical: bool,
    pub selected_item_id: Option<String>,
    pub filter_channel_id: Option<String>,

    pub picker: SourcePickerModal,
    pub stage_interaction: StageInteraction,

    // Texturas GPU ao vivo do palco
    pub tex_program_h: Option<TextureHandle>,
    pub tex_program_v: Option<TextureHandle>,
    pub tex_studio_preview: Option<TextureHandle>,
    last_preview_seq: u64,

    // Estado vivo vindo do AppEngine
    pub telemetry: TelemetryStats,
    pub gpu_status: GpuPipelineStatus,
    pub multistream: MultistreamEngineStatus,
    pub recorder: RecorderStatus,
    pub virtual_cam: VirtualCameraStatus,
    pub external_ctrl: ExternalControlStatus,
    pub updater: UpdateStatus,
    pub audio_levels: HashMap<String, AudioLevelSample>,
    pub capture_statuses: HashMap<String, CaptureStatus>,
    pub chat_messages: Vec<MultiChatMessage>,
    pub chat_input: String,
    pub twitch_caps: TwitchCapabilities,
    pub twitch_login: TwitchLoginState,
    pub loaded_kit: Option<KitInfo>,

    // LAN P2P
    pub lan_hosting: bool,
    pub lan_ip: String,
    pub lan_port: u16,
    pub lan_pin: Option<String>,
    pub lan_hosts: Vec<DiscoveredHost>,
    pub firewall_ok: Option<bool>,

    // Campos de edição temporários (chaves/senhas nunca exibidas em texto puro)
    pub dest_key_inputs: HashMap<String, String>,
    pub obs_password_input: String,
    pub new_scene_name: String,
    pub adding_scene: bool,
    pub new_dest_name: String,
    pub new_dest_server: String,

    // Modal de saída segura durante live/gravação
    pub confirm_quit_open: bool,
    pub force_close: bool,

    pub toast: Option<ToastNotice>,
    pub start_instant: Instant,
    last_status_poll: Instant,

    rpc_rx: tokio::sync::broadcast::Receiver<RpcEvent>,
    pub bg_tx: Sender<BgUiMsg>,
    bg_rx: Receiver<BgUiMsg>,
}

impl PrismCastNativeApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        engine: Arc<AppEngine>,
        rt: tokio::runtime::Handle,
    ) -> Self {
        theme::apply_prismcast_theme(&cc.egui_ctx);

        let mut settings = engine.store.snapshot();
        ensure_default_scene(&mut settings);
        let _ = engine
            .store
            .apply_patch(json!({ "scenes": settings.scenes }));

        // Ativa modo nativo no MultistreamEngine (sem MediaRecorder!)
        if let Ok(mut ms) = engine.multistream.try_lock() {
            ms.set_native_mode(true);
        }

        // Sincroniza compositor inicial
        sync_compositor_with_settings(&engine, &settings);

        let pump = NativeStudioPump::start(engine.clone());
        let rpc_rx = engine.subscribe();
        let (bg_tx, bg_rx) = mpsc::channel();

        let selected_item_id = settings
            .scenes
            .scenes
            .iter()
            .find(|s| s.id == settings.scenes.active_scene_id)
            .and_then(|s| s.items.first())
            .map(|i| i.id.clone());

        let app = Self {
            engine: engine.clone(),
            rt: rt.clone(),
            pump,
            settings,
            undo_stack: Vec::new(),
            active_tab: NavTab::Studio,
            right_tab: RightTab::Properties,
            edit_vertical: false,
            selected_item_id,
            filter_channel_id: None,
            picker: SourcePickerModal::default(),
            stage_interaction: StageInteraction::default(),
            tex_program_h: None,
            tex_program_v: None,
            tex_studio_preview: None,
            last_preview_seq: 0,
            telemetry: TelemetryStats::default(),
            gpu_status: GpuPipelineStatus::default(),
            multistream: MultistreamEngineStatus::default(),
            recorder: RecorderStatus::default(),
            virtual_cam: VirtualCameraStatus::default(),
            external_ctrl: ExternalControlStatus::default(),
            updater: UpdateStatus::default(),
            audio_levels: HashMap::new(),
            capture_statuses: HashMap::new(),
            chat_messages: Vec::new(),
            chat_input: String::new(),
            twitch_caps: TwitchCapabilities::default(),
            twitch_login: TwitchLoginState::Idle,
            loaded_kit: None,
            lan_hosting: false,
            lan_ip: String::new(),
            lan_port: 41235,
            lan_pin: None,
            lan_hosts: Vec::new(),
            firewall_ok: None,
            dest_key_inputs: HashMap::new(),
            obs_password_input: String::new(),
            new_scene_name: String::new(),
            adding_scene: false,
            new_dest_name: String::new(),
            new_dest_server: "rtmp://".to_string(),
            confirm_quit_open: false,
            force_close: false,
            toast: None,
            start_instant: Instant::now(),
            last_status_poll: Instant::now() - Duration::from_secs(5),
            rpc_rx,
            bg_tx,
            bg_rx,
        };

        app.refresh_async_statuses();
        if let Some(ref kp) = app.settings.kit_path.clone() {
            if !kp.trim().is_empty() {
                app.load_kit_async(kp.clone());
            }
        }
        app
    }

    pub fn notify(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some(ToastNotice {
            message: msg.into(),
            is_error,
            expires_at: Instant::now() + Duration::from_secs(4),
        });
    }

    pub fn push_undo(&mut self) {
        self.undo_stack.push(self.settings.scenes.clone());
        if self.undo_stack.len() > 40 {
            self.undo_stack.remove(0);
        }
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.settings.scenes = prev;
            self.persist_scenes();
            self.notify("Alteração desfeita (Ctrl+Z)", false);
        }
    }

    pub fn persist_settings(&self, patch: Value) {
        let engine = self.engine.clone();
        let tx = self.bg_tx.clone();
        self.rt.spawn(async move {
            let _ = engine.handle_command("settings_set", json!({ "patch": patch })).await;
            let snap = engine.store.snapshot();
            let _ = tx.send(BgUiMsg::SettingsRefreshed(snap));
        });
    }

    pub fn persist_scenes(&self) {
        sync_compositor_with_settings(&self.engine, &self.settings);
        if let Ok(val) = serde_json::to_value(&self.settings.scenes) {
            self.persist_settings(json!({ "scenes": val }));
        }
    }

    pub fn persist_studio(&self) {
        sync_compositor_with_settings(&self.engine, &self.settings);
        if let Ok(val) = serde_json::to_value(&self.settings.studio) {
            self.persist_settings(json!({ "studio": val }));
        }
    }

    pub fn persist_audio(&self) {
        if let Ok(val) = serde_json::to_value(&self.settings.audio_mixer) {
            self.persist_settings(json!({ "audioMixer": val }));
        }
    }

    pub fn persist_destinations(&self) {
        if let Ok(val) = serde_json::to_value(&self.settings.destinations) {
            self.persist_settings(json!({ "destinations": val }));
        }
    }

    pub fn editing_scene_id(&self) -> String {
        if self.settings.scenes.studio_mode && !self.settings.scenes.preview_scene_id.is_empty() {
            self.settings.scenes.preview_scene_id.clone()
        } else {
            self.settings.scenes.active_scene_id.clone()
        }
    }

    pub fn editing_scene(&self) -> Option<&Scene> {
        let id = self.editing_scene_id();
        self.settings
            .scenes
            .scenes
            .iter()
            .find(|s| s.id == id)
            .or_else(|| self.settings.scenes.scenes.first())
    }

    pub fn editing_scene_mut(&mut self) -> Option<&mut Scene> {
        let id = self.editing_scene_id();
        if let Some(idx) = self.settings.scenes.scenes.iter().position(|s| s.id == id) {
            return self.settings.scenes.scenes.get_mut(idx);
        }
        self.settings.scenes.scenes.first_mut()
    }

    pub fn program_scene(&self) -> Option<&Scene> {
        self.settings
            .scenes
            .scenes
            .iter()
            .find(|s| s.id == self.settings.scenes.active_scene_id)
            .or_else(|| self.settings.scenes.scenes.first())
    }

    pub fn refresh_async_statuses(&self) {
        let engine = self.engine.clone();
        let tx = self.bg_tx.clone();
        self.rt.spawn(async move {
            let ms = engine.handle_command("multistream_status", Value::Null).await;
            if let Ok(parsed) = serde_json::from_value::<MultistreamEngineStatus>(ms) {
                let _ = tx.send(BgUiMsg::MultistreamStatus(parsed));
            }
            let rec = engine.handle_command("recorder_status", Value::Null).await;
            if let Ok(parsed) = serde_json::from_value::<RecorderStatus>(rec) {
                let _ = tx.send(BgUiMsg::RecorderStatus(parsed));
            }
            let vc = engine.handle_command("virtual_cam_status", Value::Null).await;
            if let Ok(parsed) = serde_json::from_value::<VirtualCameraStatus>(vc) {
                let _ = tx.send(BgUiMsg::VirtualCamStatus(parsed));
            }
            let ext = engine.handle_command("external_control_status", Value::Null).await;
            if let Ok(parsed) = serde_json::from_value::<ExternalControlStatus>(ext) {
                let _ = tx.send(BgUiMsg::ExternalControlStatus(parsed));
            }
            let upd = engine.handle_command("updater_status", Value::Null).await;
            if let Ok(parsed) = serde_json::from_value::<UpdateStatus>(upd) {
                let _ = tx.send(BgUiMsg::UpdateStatus(parsed));
            }
            let tw = engine.handle_command("twitch_capabilities", Value::Null).await;
            if let Ok(parsed) = serde_json::from_value::<TwitchCapabilities>(tw) {
                let _ = tx.send(BgUiMsg::TwitchCapabilities(parsed));
            }
        });
    }

    pub fn load_kit_async(&self, path: String) {
        let engine = self.engine.clone();
        let tx = self.bg_tx.clone();
        self.rt.spawn(async move {
            let res = engine
                .handle_command("kit_load", json!({ "path": path }))
                .await;
            let info = serde_json::from_value::<KitInfo>(res).ok();
            let _ = tx.send(BgUiMsg::KitLoaded(info));
        });
    }

    pub fn toggle_live(&mut self) {
        let engine = self.engine.clone();
        let tx = self.bg_tx.clone();
        let is_live = self.multistream.active;
        let fps = self.settings.studio.fps;
        self.rt.spawn(async move {
            if is_live {
                let res = engine.handle_command("multistream_stop", Value::Null).await;
                if let Ok(st) = serde_json::from_value::<MultistreamEngineStatus>(res) {
                    let _ = tx.send(BgUiMsg::MultistreamStatus(st));
                }
                let _ = tx.send(BgUiMsg::Toast {
                    message: "Transmissão encerrada.".to_string(),
                    is_error: false,
                });
            } else {
                {
                    let mut ms = engine.multistream.lock().await;
                    ms.set_native_mode(true);
                }
                let res = engine
                    .handle_command("multistream_start", json!({ "fps": fps }))
                    .await;
                let ok = res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                if let Some(st_val) = res.get("status") {
                    if let Ok(st) = serde_json::from_value::<MultistreamEngineStatus>(st_val.clone()) {
                        let _ = tx.send(BgUiMsg::MultistreamStatus(st));
                    }
                }
                if ok {
                    let _ = tx.send(BgUiMsg::Toast {
                        message: "Você está AO VIVO!".to_string(),
                        is_error: false,
                    });
                } else {
                    let err = res
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Configure e habilite pelo menos um destino com chave de transmissão.");
                    let _ = tx.send(BgUiMsg::Toast {
                        message: err.to_string(),
                        is_error: true,
                    });
                }
            }
        });
    }

    pub fn toggle_recording(&mut self) {
        let engine = self.engine.clone();
        let tx = self.bg_tx.clone();
        let active = self.recorder.recording;
        self.rt.spawn(async move {
            let cmd = if active { "recorder_stop" } else { "recorder_start" };
            let res = engine.handle_command(cmd, Value::Null).await;
            if let Ok(st) = serde_json::from_value::<RecorderStatus>(res.clone()) {
                let _ = tx.send(BgUiMsg::RecorderStatus(st));
            } else if let Some(st_val) = res.get("status") {
                if let Ok(st) = serde_json::from_value::<RecorderStatus>(st_val.clone()) {
                    let _ = tx.send(BgUiMsg::RecorderStatus(st));
                }
            }
            let msg = if active {
                "Gravação salva."
            } else {
                "Gravação iniciada."
            };
            let _ = tx.send(BgUiMsg::Toast {
                message: msg.to_string(),
                is_error: false,
            });
        });
    }

    pub fn save_replay(&mut self) {
        let engine = self.engine.clone();
        let tx = self.bg_tx.clone();
        self.rt.spawn(async move {
            let res = engine.handle_command("recorder_save_replay", Value::Null).await;
            let ok = res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
            if ok {
                let path = res.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let _ = tx.send(BgUiMsg::Toast {
                    message: format!("Replay salvo: {path}"),
                    is_error: false,
                });
            } else {
                let err = res
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Buffer de replay ainda vazio.");
                let _ = tx.send(BgUiMsg::Toast {
                    message: err.to_string(),
                    is_error: true,
                });
            }
        });
    }

    pub fn apply_picker_selection(&mut self, sel: PickerSelection) {
        self.push_undo();
        match sel.purpose {
            PickerPurpose::AddNewSource => {
                let source_type = match sel.kind {
                    SourceKind::Screen => SourceType::ScreenCapture,
                    SourceKind::Window => SourceType::WindowCapture,
                };
                let new_id = format!("item-{}", Uuid::new_v4());
                let z = self
                    .editing_scene()
                    .map(|sc| {
                        sc.items
                            .iter()
                            .map(|i| i.transform.z_index)
                            .max()
                            .unwrap_or(-1)
                            + 1
                    })
                    .unwrap_or(0);
                let item = SceneItem {
                    id: new_id.clone(),
                    source_id: format!("src-{}", Uuid::new_v4()),
                    name: sel.source_name.clone(),
                    source_type,
                    visible: true,
                    locked: false,
                    transform: SourceTransform {
                        z_index: z,
                        ..Default::default()
                    },
                    vertical_transform: SourceTransform {
                        z_index: z,
                        ..Default::default()
                    },
                    show_on_horizontal: true,
                    show_on_vertical: true,
                    settings: SourceSettings {
                        capture_id: Some(sel.source_id),
                        capture_name: Some(sel.source_name.clone()),
                        ..Default::default()
                    },
                };
                if let Some(sc) = self.editing_scene_mut() {
                    sc.items.push(item);
                }
                self.selected_item_id = Some(new_id);
                self.persist_scenes();
                self.notify(format!("Capturando nativamente: {}", sel.source_name), false);
            }
            PickerPurpose::RebindItem { item_id } => {
                if let Some(sc) = self.editing_scene_mut() {
                    if let Some(it) = sc.items.iter_mut().find(|i| i.id == item_id) {
                        it.source_type = match sel.kind {
                            SourceKind::Screen => SourceType::ScreenCapture,
                            SourceKind::Window => SourceType::WindowCapture,
                        };
                        it.name = sel.source_name.clone();
                        it.settings.capture_id = Some(sel.source_id);
                        it.settings.capture_name = Some(sel.source_name.clone());
                    }
                }
                self.persist_scenes();
                self.notify(format!("Fonte atualizada para: {}", sel.source_name), false);
            }
        }
    }

    pub fn add_simple_source(&mut self, source_type: SourceType) {
        self.push_undo();
        let new_id = format!("item-{}", Uuid::new_v4());
        let z = self
            .editing_scene()
            .map(|sc| {
                sc.items
                    .iter()
                    .map(|i| i.transform.z_index)
                    .max()
                    .unwrap_or(-1)
                    + 1
            })
            .unwrap_or(0);

        let (name, settings, tf) = match source_type {
            SourceType::ColorSource => (
                "Cor sólida".to_string(),
                SourceSettings {
                    color: Some("#1c1c19".to_string()),
                    ..Default::default()
                },
                SourceTransform {
                    x: 0.1,
                    y: 0.1,
                    width: 0.35,
                    height: 0.35,
                    z_index: z,
                    ..Default::default()
                },
            ),
            SourceType::TextSource => (
                "Texto".to_string(),
                SourceSettings {
                    text: Some("PrismCast Studio".to_string()),
                    font_size: Some(48),
                    color: Some("#f1ede4".to_string()),
                    ..Default::default()
                },
                SourceTransform {
                    x: 0.1,
                    y: 0.8,
                    width: 0.4,
                    height: 0.12,
                    z_index: z,
                    ..Default::default()
                },
            ),
            SourceType::BrowserSource => (
                "Widget HTML".to_string(),
                SourceSettings {
                    url: Some("https://".to_string()),
                    width: Some(1920),
                    height: Some(1080),
                    fps: Some(30),
                    ..Default::default()
                },
                SourceTransform {
                    z_index: z,
                    ..Default::default()
                },
            ),
            SourceType::CameraInput => (
                "Câmera".to_string(),
                SourceSettings {
                    rounded: Some(true),
                    ..Default::default()
                },
                SourceTransform {
                    x: 0.72,
                    y: 0.64,
                    width: 0.25,
                    height: 0.32,
                    z_index: z,
                    ..Default::default()
                },
            ),
            SourceType::ImageSource => {
                let picked = rfd::FileDialog::new()
                    .add_filter("Imagens", &["png", "jpg", "jpeg", "webp", "gif", "bmp"])
                    .pick_file();
                let Some(path) = picked else {
                    return;
                };
                let fname = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Imagem")
                    .to_string();
                (
                    fname,
                    SourceSettings {
                        path: Some(path.to_string_lossy().to_string()),
                        ..Default::default()
                    },
                    SourceTransform {
                        x: 0.25,
                        y: 0.2,
                        width: 0.5,
                        height: 0.5,
                        z_index: z,
                        ..Default::default()
                    },
                )
            }
            _ => return,
        };

        let item = SceneItem {
            id: new_id.clone(),
            source_id: format!("src-{}", Uuid::new_v4()),
            name,
            source_type,
            visible: true,
            locked: false,
            transform: tf.clone(),
            vertical_transform: tf,
            show_on_horizontal: true,
            show_on_vertical: true,
            settings,
        };
        if let Some(sc) = self.editing_scene_mut() {
            sc.items.push(item);
        }
        self.selected_item_id = Some(new_id);
        self.persist_scenes();
    }

    pub fn apply_stage_actions(&mut self, actions: Vec<StageAction>) {
        let mut dirty = false;
        for act in actions {
            match act {
                StageAction::Select(id) => {
                    self.selected_item_id = id;
                    if self.selected_item_id.is_some() {
                        self.right_tab = RightTab::Properties;
                    }
                }
                StageAction::UpdateTransform {
                    item_id,
                    vertical,
                    transform,
                    commit_undo,
                } => {
                    if commit_undo {
                        self.push_undo();
                    }
                    if let Some(sc) = self.editing_scene_mut() {
                        if let Some(it) = sc.items.iter_mut().find(|i| i.id == item_id) {
                            if vertical {
                                it.vertical_transform = transform;
                            } else {
                                it.transform = transform;
                            }
                            dirty = true;
                        }
                    }
                }
                StageAction::BringForward(id) => {
                    self.push_undo();
                    if let Some(sc) = self.editing_scene_mut() {
                        if let Some(it) = sc.items.iter_mut().find(|i| i.id == id) {
                            it.transform.z_index += 1;
                            it.vertical_transform.z_index += 1;
                            dirty = true;
                        }
                    }
                }
                StageAction::SendBackward(id) => {
                    self.push_undo();
                    if let Some(sc) = self.editing_scene_mut() {
                        if let Some(it) = sc.items.iter_mut().find(|i| i.id == id) {
                            it.transform.z_index = (it.transform.z_index - 1).max(0);
                            it.vertical_transform.z_index = (it.vertical_transform.z_index - 1).max(0);
                            dirty = true;
                        }
                    }
                }
                StageAction::FillCanvas { item_id, vertical } => {
                    self.push_undo();
                    if let Some(sc) = self.editing_scene_mut() {
                        if let Some(it) = sc.items.iter_mut().find(|i| i.id == item_id) {
                            let tf = if vertical {
                                &mut it.vertical_transform
                            } else {
                                &mut it.transform
                            };
                            tf.x = 0.0;
                            tf.y = 0.0;
                            tf.width = 1.0;
                            tf.height = 1.0;
                            dirty = true;
                        }
                    }
                }
                StageAction::CenterCanvas { item_id, vertical } => {
                    self.push_undo();
                    if let Some(sc) = self.editing_scene_mut() {
                        if let Some(it) = sc.items.iter_mut().find(|i| i.id == item_id) {
                            let tf = if vertical {
                                &mut it.vertical_transform
                            } else {
                                &mut it.transform
                            };
                            tf.x = (1.0 - tf.width) * 0.5;
                            tf.y = (1.0 - tf.height) * 0.5;
                            dirty = true;
                        }
                    }
                }
                StageAction::ToggleLock(id) => {
                    self.push_undo();
                    if let Some(sc) = self.editing_scene_mut() {
                        if let Some(it) = sc.items.iter_mut().find(|i| i.id == id) {
                            it.locked = !it.locked;
                            dirty = true;
                        }
                    }
                }
                StageAction::RemoveItem(id) => {
                    self.push_undo();
                    if let Some(sc) = self.editing_scene_mut() {
                        sc.items.retain(|i| i.id != id);
                        dirty = true;
                    }
                    if self.selected_item_id.as_deref() == Some(id.as_str()) {
                        self.selected_item_id = None;
                    }
                }
                StageAction::OpenPickerForNewScreen => {
                    self.picker
                        .open(PickerPurpose::AddNewSource, SourceKind::Screen, None);
                }
                StageAction::OpenPickerForNewWindow => {
                    self.picker
                        .open(PickerPurpose::AddNewSource, SourceKind::Window, None);
                }
                StageAction::OpenPickerForItem(item_id) => {
                    let (tab, cur_id) = self
                        .editing_scene()
                        .and_then(|sc| sc.items.iter().find(|i| i.id == item_id))
                        .map(|it| {
                            let k = if it.source_type == SourceType::ScreenCapture {
                                SourceKind::Screen
                            } else {
                                SourceKind::Window
                            };
                            (k, it.settings.capture_id.clone())
                        })
                        .unwrap_or((SourceKind::Window, None));
                    self.picker
                        .open(PickerPurpose::RebindItem { item_id }, tab, cur_id);
                }
            }
        }
        if dirty {
            self.persist_scenes();
        }
    }

    fn drain_events(&mut self, ctx: &Context) {
        while let Ok(ev) = self.rpc_rx.try_recv() {
            match ev.event.as_str() {
                "audio:levels" => {
                    if let Ok(levels) = serde_json::from_value::<Vec<AudioLevelSample>>(ev.payload) {
                        for sample in levels {
                            self.audio_levels.insert(sample.id.clone(), sample);
                        }
                    }
                }
                "telemetry:stats" => {
                    if let Ok(st) = serde_json::from_value::<TelemetryStats>(ev.payload) {
                        self.telemetry = st;
                    }
                }
                "multichat:event" => {
                    if let Ok(mce) = serde_json::from_value::<MultiChatEvent>(ev.payload) {
                        match mce {
                            MultiChatEvent::Messages { messages } => {
                                self.chat_messages.extend(messages);
                                if self.chat_messages.len() > 200 {
                                    let drain = self.chat_messages.len() - 200;
                                    self.chat_messages.drain(0..drain);
                                }
                            }
                            MultiChatEvent::Delete { id, .. } => {
                                self.chat_messages.retain(|m| m.id != id);
                            }
                            MultiChatEvent::Status { .. } => {}
                        }
                    }
                }
                "dest:twitch-state" => {
                    if let Ok(st) = serde_json::from_value::<TwitchLoginState>(ev.payload) {
                        self.twitch_login = st;
                    }
                }
                "recorder:status" => {
                    if let Ok(st) = serde_json::from_value::<RecorderStatus>(ev.payload) {
                        self.recorder = st;
                    }
                }
                "discovery:hosts" => {
                    if let Ok(hosts) = serde_json::from_value::<Vec<DiscoveredHost>>(ev.payload) {
                        self.lan_hosts = hosts;
                    }
                }
                "updater:status" => {
                    if let Ok(st) = serde_json::from_value::<UpdateStatus>(ev.payload) {
                        self.updater = st;
                    }
                }
                _ => {}
            }
        }

        while let Ok(msg) = self.bg_rx.try_recv() {
            match msg {
                BgUiMsg::Toast { message, is_error } => self.notify(message, is_error),
                BgUiMsg::SettingsRefreshed(s) => {
                    self.settings = s;
                }
                BgUiMsg::MultistreamStatus(st) => self.multistream = st,
                BgUiMsg::RecorderStatus(st) => self.recorder = st,
                BgUiMsg::VirtualCamStatus(st) => self.virtual_cam = st,
                BgUiMsg::ExternalControlStatus(st) => self.external_ctrl = st,
                BgUiMsg::UpdateStatus(st) => self.updater = st,
                BgUiMsg::TwitchCapabilities(caps) => self.twitch_caps = caps,
                BgUiMsg::TwitchLogin(st) => self.twitch_login = st,
                BgUiMsg::KitLoaded(k) => self.loaded_kit = k,
                BgUiMsg::LanHostStarted { ip, port, pin } => {
                    self.lan_hosting = true;
                    self.lan_ip = ip;
                    self.lan_port = port;
                    self.lan_pin = pin;
                }
                BgUiMsg::LanHostStopped => {
                    self.lan_hosting = false;
                    self.lan_pin = None;
                }
                BgUiMsg::DiscoveredHosts(hosts) => self.lan_hosts = hosts,
                BgUiMsg::FirewallChecked(ok) => self.firewall_ok = Some(ok),
                BgUiMsg::ExitRequested => {
                    self.force_close = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
    }

    fn upload_live_previews(&mut self, ctx: &Context) {
        let seq = self.pump.previews.seq.load(Ordering::Acquire);
        if seq == self.last_preview_seq {
            return;
        }
        self.last_preview_seq = seq;

        fn update_tex(
            ctx: &Context,
            name: &str,
            slot: &mut Option<TextureHandle>,
            snap: Option<PreviewSnapshot>,
        ) {
            let Some(s) = snap else { return };
            if s.width == 0 || s.height == 0 {
                return;
            }
            let img = ColorImage::from_rgba_unmultiplied(
                [s.width as usize, s.height as usize],
                &s.rgba,
            );
            match slot {
                Some(tex) => tex.set(img, TextureOptions::LINEAR),
                None => *slot = Some(ctx.load_texture(name, img, TextureOptions::LINEAR)),
            }
        }

        let snap_h = self.pump.previews.program_h.lock().ok().and_then(|g| g.clone());
        update_tex(ctx, "stage_program_h", &mut self.tex_program_h, snap_h);

        if self.settings.studio.dual_output_enabled {
            let snap_v = self.pump.previews.program_v.lock().ok().and_then(|g| g.clone());
            update_tex(ctx, "stage_program_v", &mut self.tex_program_v, snap_v);
        }

        if self.settings.scenes.studio_mode {
            let snap_p = self
                .pump
                .previews
                .studio_preview
                .lock()
                .ok()
                .and_then(|g| g.clone());
            update_tex(ctx, "stage_studio_preview", &mut self.tex_studio_preview, snap_p);
        }

        // Atualiza snapshot rápido de telemetria e status de captura
        if let Ok(comp) = self.engine.compositor.lock() {
            self.telemetry = comp.telemetry();
            let (backend_name, adapter, driver) = comp.backend_info();
            if let Ok(gc) = self.engine.gpu_capture.lock() {
                self.gpu_status = gc.status(&backend_name, &adapter, &driver);
            }
        }
        let keys: Vec<String> = self
            .editing_scene()
            .map(|sc| sc.items.iter().filter_map(capture_key_for_item).collect())
            .unwrap_or_default();
        if let Ok(cm) = self.engine.capture.lock() {
            self.capture_statuses.clear();
            for k in keys {
                if let Some(st) = cm.status(&k) {
                    self.capture_statuses.insert(k, st);
                }
            }
        }
    }

    fn handle_global_shortcuts(&mut self, ctx: &Context) {
        if self.picker.open || ctx.wants_keyboard_input() {
            return;
        }
        let undo_pressed = ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z));
        if undo_pressed {
            self.undo();
            return;
        }
        if self.active_tab != NavTab::Studio {
            return;
        }
        let Some(sel_id) = self.selected_item_id.clone() else {
            return;
        };
        let del_pressed = ctx.input(|i| i.key_pressed(egui::Key::Delete));
        if del_pressed {
            self.apply_stage_actions(vec![StageAction::RemoveItem(sel_id)]);
            return;
        }
        let (dx, dy) = ctx.input(|i| {
            let step = if i.modifiers.shift { 0.01 } else { 0.002 };
            let mut x = 0.0f32;
            let mut y = 0.0f32;
            if i.key_pressed(egui::Key::ArrowLeft) {
                x -= step;
            }
            if i.key_pressed(egui::Key::ArrowRight) {
                x += step;
            }
            if i.key_pressed(egui::Key::ArrowUp) {
                y -= step;
            }
            if i.key_pressed(egui::Key::ArrowDown) {
                y += step;
            }
            (x, y)
        });
        if dx != 0.0 || dy != 0.0 {
            let vertical = self.edit_vertical;
            if let Some(sc) = self.editing_scene_mut() {
                if let Some(it) = sc.items.iter_mut().find(|i| i.id == sel_id && !i.locked) {
                    let tf = if vertical {
                        &mut it.vertical_transform
                    } else {
                        &mut it.transform
                    };
                    tf.x += dx;
                    tf.y += dy;
                    self.persist_scenes();
                }
            }
        }
    }
}

impl eframe::App for PrismCastNativeApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Intercepta fechamento acidental durante live/gravação
        if ctx.input(|i| i.viewport().close_requested()) && !self.force_close && self.engine.is_busy()
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.confirm_quit_open = true;
        }

        self.drain_events(ctx);
        self.upload_live_previews(ctx);
        self.handle_global_shortcuts(ctx);

        if self.last_status_poll.elapsed() >= Duration::from_secs(2) {
            self.last_status_poll = Instant::now();
            self.refresh_async_statuses();
        }

        if let Some(ref t) = self.toast {
            if Instant::now() >= t.expires_at {
                self.toast = None;
            }
        }

        // Fundo broadcast carbono + feixes espectrais (idêntico à página GH Pages)
        let screen_rect = ctx.screen_rect();
        let time_sec = self.start_instant.elapsed().as_secs_f64();
        theme::paint_spectral_backdrop(
            &ctx.layer_painter(egui::LayerId::background()),
            screen_rect,
            time_sec,
        );

        // Barra superior broadcast
        panels::show_top_bar(self, ctx, time_sec);

        // Rail de navegação esquerdo
        panels::show_nav_rail(self, ctx);

        // Conteúdo principal por aba
        match self.active_tab {
            NavTab::Studio => panels::show_studio_workspace(self, ctx),
            NavTab::Widgets => panels::show_widgets_screen(self, ctx),
            NavTab::Destinations => panels::show_destinations_screen(self, ctx),
            NavTab::Lan => panels::show_lan_screen(self, ctx),
            NavTab::Settings => panels::show_settings_screen(self, ctx),
        }

        // Modal nativo de seleção de telas/janelas com miniaturas reais
        if let Some(sel) = self.picker.show(ctx) {
            self.apply_picker_selection(sel);
        }

        // Modal de confirmação de saída durante live/gravação
        if self.confirm_quit_open {
            panels::show_confirm_quit_modal(self, ctx);
        }

        // Toast flutuante glassmorphism
        if let Some(toast) = self.toast.clone() {
            panels::show_toast_overlay(ctx, &toast);
        }

        // Mantém 60fps fluídos no palco e medidores VU vivos
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

fn ensure_default_scene(settings: &mut AppSettings) {
    if settings.scenes.scenes.is_empty() {
        let scene_id = "scene-main".to_string();
        let default_screen = SceneItem {
            id: "item-screen-primary".to_string(),
            source_id: "src-screen-primary".to_string(),
            name: "Tela principal".to_string(),
            source_type: SourceType::ScreenCapture,
            visible: true,
            locked: false,
            transform: SourceTransform::default(),
            vertical_transform: SourceTransform::default(),
            show_on_horizontal: true,
            show_on_vertical: true,
            settings: SourceSettings {
                capture_id: Some("screen:0:0".to_string()),
                capture_name: Some("Tela 1".to_string()),
                ..Default::default()
            },
        };
        settings.scenes.scenes.push(Scene {
            id: scene_id.clone(),
            name: "Cena principal".to_string(),
            items: vec![default_screen],
            created_at: now_ms(),
            transition_override: None,
        });
        settings.scenes.active_scene_id = scene_id.clone();
        settings.scenes.preview_scene_id = scene_id;
    } else if settings.scenes.active_scene_id.is_empty() {
        settings.scenes.active_scene_id = settings.scenes.scenes[0].id.clone();
    }

    if settings.destinations.is_empty() {
        settings.destinations = default_destinations();
    }
}

pub fn default_destinations() -> Vec<Destination> {
    vec![
        Destination {
            id: "twitch".to_string(),
            platform: PlatformId::Twitch,
            name: "Twitch".to_string(),
            enabled: false,
            server: "rtmp://live.twitch.tv/app".to_string(),
            has_key: false,
            orientation: CanvasOrientation::Horizontal,
            bitrate_kbps: 6000,
            account: None,
        },
        Destination {
            id: "youtube".to_string(),
            platform: PlatformId::Youtube,
            name: "YouTube".to_string(),
            enabled: false,
            server: "rtmp://a.rtmp.youtube.com/live2".to_string(),
            has_key: false,
            orientation: CanvasOrientation::Horizontal,
            bitrate_kbps: 8000,
            account: None,
        },
        Destination {
            id: "kick".to_string(),
            platform: PlatformId::Kick,
            name: "Kick".to_string(),
            enabled: false,
            server: "rtmps://fa723fc1b171.global-contribute.live-video.net/app".to_string(),
            has_key: false,
            orientation: CanvasOrientation::Horizontal,
            bitrate_kbps: 6000,
            account: None,
        },
        Destination {
            id: "tiktok".to_string(),
            platform: PlatformId::Tiktok,
            name: "TikTok Live".to_string(),
            enabled: false,
            server: "rtmp://push.tiktokv.com/live".to_string(),
            has_key: false,
            orientation: CanvasOrientation::Vertical,
            bitrate_kbps: 4500,
            account: None,
        },
    ]
}

pub fn sync_compositor_with_settings(engine: &AppEngine, settings: &AppSettings) {
    if let Ok(mut comp) = engine.compositor.lock() {
        comp.apply_settings(settings.studio.clone());
        let prog = settings
            .scenes
            .scenes
            .iter()
            .find(|s| s.id == settings.scenes.active_scene_id)
            .cloned()
            .or_else(|| settings.scenes.scenes.first().cloned());
        let prev = if settings.scenes.studio_mode {
            settings
                .scenes
                .scenes
                .iter()
                .find(|s| s.id == settings.scenes.preview_scene_id)
                .cloned()
        } else {
            None
        };
        comp.set_scenes_with_config(
            prog,
            prev,
            settings.scenes.transition_type,
            settings.scenes.transition_duration_ms,
            Some(settings.scenes.custom_transition.clone()),
            now_ms(),
        );
    }
}

fn load_app_icon() -> Option<egui::IconData> {
    let png_bytes = include_bytes!("../../build/128x128@2x.png");
    let img = image::load_from_memory(png_bytes).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

/// Inicia a janela desktop 100% nativa em Rust (`eframe` + `wgpu 24`).
pub fn run_native_app(engine: Arc<AppEngine>, rt: tokio::runtime::Handle) -> Result<(), String> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(format!(
            "PrismCast Studio v{} — Nativo em Rust",
            env!("CARGO_PKG_VERSION")
        ))
        .with_inner_size(Vec2::new(1380.0, 860.0))
        .with_min_inner_size(Vec2::new(1024.0, 680.0));

    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "PrismCast Studio",
        options,
        Box::new(move |cc| Ok(Box::new(PrismCastNativeApp::new(cc, engine, rt)))),
    )
    .map_err(|e| format!("Falha ao iniciar interface nativa Rust: {e}"))
}

pub fn transition_label(t: TransitionType) -> &'static str {
    match t {
        TransitionType::Cut => "Corte",
        TransitionType::Fade => "Esmaecer",
        TransitionType::FadeToBlack => "Fade to Black",
        TransitionType::Slide => "Deslizar",
        TransitionType::Stinger => "Stinger",
        TransitionType::LumaWipe => "Luma Wipe",
        TransitionType::CustomShader => "Shader WGSL",
    }
}

pub fn source_type_badge(t: SourceType) -> &'static str {
    match t {
        SourceType::ScreenCapture => "TELA",
        SourceType::WindowCapture => "JANELA",
        SourceType::CameraInput => "CAM",
        SourceType::BrowserSource => "HTML",
        SourceType::ImageSource => "IMG",
        SourceType::ColorSource => "COR",
        SourceType::TextSource => "TXT",
        SourceType::AudioInput => "MIC",
        SourceType::AudioOutput => "AUD",
    }
}

pub fn ensure_window_audio_channel(settings: &mut AppSettings, item_name: &str) -> String {
    let ch_id = format!("app-{}", Uuid::new_v4());
    settings.audio_mixer.channels.push(AudioChannel {
        id: ch_id.clone(),
        kind: AudioChannelKind::App,
        name: format!("Áudio · {item_name}"),
        app_exe: Some(item_name.to_string()),
        filters: AudioFilters::default_for(AudioChannelKind::App),
        ..Default::default()
    });
    ch_id
}
