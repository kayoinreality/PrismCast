use crate::protocol::{
    AudioMixerSettings, CanvasOrientation, HotkeyBindings, MultistreamTarget, StreamProvider,
    StreamWidget, StudioCompositorSettings, WidgetKind,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualitySettings {
    pub max_height: Option<u32>,
    pub fps: u32,
    pub max_bitrate_kbps: u32,
    pub degradation: String,
    pub codec: String,
    pub audio: bool,
}

impl Default for QualitySettings {
    fn default() -> Self {
        Self {
            max_height: Some(1080),
            fps: 60,
            max_bitrate_kbps: 12000,
            degradation: "maintain-framerate".to_string(),
            codec: "auto".to_string(),
            audio: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub host_name: String,
    pub viewer_name: String,
    pub quality: QualitySettings,
    pub preset_id: String,
    pub require_pin: bool,
    pub preferred_interface: Option<String>,
    pub firewall_prompt_dismissed: bool,
    #[serde(default)]
    pub audio_mixer: AudioMixerSettings,
    #[serde(default)]
    pub studio: StudioCompositorSettings,
    #[serde(default)]
    pub hotkeys: HotkeyBindings,
    #[serde(default = "default_multistream_targets")]
    pub multistream_targets: Vec<MultistreamTarget>,
    #[serde(default = "default_widgets")]
    pub widgets: Vec<StreamWidget>,
}

pub fn default_multistream_targets() -> Vec<MultistreamTarget> {
    vec![
        MultistreamTarget {
            id: "twitch-main".to_string(),
            provider: StreamProvider::Twitch,
            name: "Twitch".to_string(),
            enabled: false,
            rtmpUrl: "rtmp://live.twitch.tv/app".to_string(),
            streamKey: String::new(),
            orientation: CanvasOrientation::Horizontal,
            bitrateKbps: 6000,
        },
        MultistreamTarget {
            id: "youtube-main".to_string(),
            provider: StreamProvider::Youtube,
            name: "YouTube Live".to_string(),
            enabled: false,
            rtmpUrl: "rtmp://a.rtmp.youtube.com/live2".to_string(),
            streamKey: String::new(),
            orientation: CanvasOrientation::Horizontal,
            bitrateKbps: 9000,
        },
        MultistreamTarget {
            id: "kick-main".to_string(),
            provider: StreamProvider::Kick,
            name: "Kick".to_string(),
            enabled: false,
            rtmpUrl: "rtmps://fa723fc1b171.global-contribute.live-video.net/app".to_string(),
            streamKey: String::new(),
            orientation: CanvasOrientation::Horizontal,
            bitrateKbps: 8000,
        },
        MultistreamTarget {
            id: "tiktok-vertical".to_string(),
            provider: StreamProvider::Tiktok,
            name: "TikTok Live (9:16)".to_string(),
            enabled: false,
            rtmpUrl: "rtmp://push.tiktokv.com/live".to_string(),
            streamKey: String::new(),
            orientation: CanvasOrientation::Vertical,
            bitrateKbps: 4500,
        },
    ]
}

pub fn default_widgets() -> Vec<StreamWidget> {
    vec![
        StreamWidget {
            id: "widget-streamelements".to_string(),
            name: "StreamElements Overlay".to_string(),
            kind: WidgetKind::StreamElements,
            enabled: false,
            url: "https://streamelements.com/overlay/SEU_OVERLAY_ID/SEU_TOKEN".to_string(),
            primaryText: "StreamElements AlertBox & Goals".to_string(),
            secondaryText: "Ativo em 16:9 e 9:16".to_string(),
            accentColor: "#6366f1".to_string(),
            x: 0.03,
            y: 0.04,
            width: 0.35,
            height: 0.16,
            verticalX: 0.06,
            verticalY: 0.08,
            showOnHorizontal: true,
            showOnVertical: true,
            opacity: 1.0,
        },
        StreamWidget {
            id: "widget-goal-bar".to_string(),
            name: "Meta de Seguidores / Sub Goal".to_string(),
            kind: WidgetKind::GoalBar,
            enabled: false,
            url: String::new(),
            primaryText: "META DA LIVE: 120 / 200".to_string(),
            secondaryText: "60% concluído".to_string(),
            accentColor: "#10b981".to_string(),
            x: 0.03,
            y: 0.88,
            width: 0.28,
            height: 0.07,
            verticalX: 0.08,
            verticalY: 0.86,
            showOnHorizontal: true,
            showOnVertical: true,
            opacity: 0.95,
        },
        StreamWidget {
            id: "widget-lower-third".to_string(),
            name: "Banner / Lower Third".to_string(),
            kind: WidgetKind::LowerThird,
            enabled: false,
            url: String::new(),
            primaryText: "AO VIVO NO PRISMCAST STUDIO".to_string(),
            secondaryText: "Twitch · YouTube · Kick · TikTok · LAN".to_string(),
            accentColor: "#8b5cf6".to_string(),
            x: 0.03,
            y: 0.78,
            width: 0.32,
            height: 0.08,
            verticalX: 0.06,
            verticalY: 0.76,
            showOnHorizontal: true,
            showOnVertical: true,
            opacity: 0.95,
        },
    ]
}

fn friendly_machine_name() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "PrismCast PC".to_string())
}

impl Default for AppSettings {
    fn default() -> Self {
        let name = friendly_machine_name();
        Self {
            host_name: name.clone(),
            viewer_name: name,
            quality: QualitySettings::default(),
            preset_id: "jogo".to_string(),
            require_pin: true,
            preferred_interface: None,
            firewall_prompt_dismissed: false,
            audio_mixer: AudioMixerSettings::default(),
            studio: StudioCompositorSettings::default(),
            hotkeys: HotkeyBindings::default(),
            multistream_targets: default_multistream_targets(),
            widgets: default_widgets(),
        }
    }
}

pub struct SettingsStore {
    path: PathBuf,
    cached: Mutex<AppSettings>,
}

impl SettingsStore {
    pub fn new() -> Self {
        let base = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("PrismCast");
        let path = base.join("settings.json");
        let initial = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<AppSettings>(&raw).ok())
            .unwrap_or_default();

        Self {
            path,
            cached: Mutex::new(initial),
        }
    }

    pub fn get(&self) -> AppSettings {
        self.cached.lock().unwrap().clone()
    }

    pub fn update(&self, patch: serde_json::Value) -> AppSettings {
        let mut guard = self.cached.lock().unwrap();
        if let Ok(mut current_val) = serde_json::to_value(&*guard) {
            if let (Some(cur_obj), Some(patch_obj)) =
                (current_val.as_object_mut(), patch.as_object())
            {
                for (k, v) in patch_obj {
                    cur_obj.insert(k.clone(), v.clone());
                }
            }
            if let Ok(updated) = serde_json::from_value::<AppSettings>(current_val) {
                *guard = updated;
            }
        }
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(serialized) = serde_json::to_string_pretty(&*guard) {
            let _ = fs::write(&self.path, serialized);
        }
        guard.clone()
    }
}
