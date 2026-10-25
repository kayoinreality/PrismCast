#![allow(non_snake_case)]
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const DISCOVERY_PORT: u16 = 41234;
pub const SIGNALING_PORT: u16 = 41235;
pub const RUST_RPC_PORT: u16 = 41236;

pub const ANNOUNCE_INTERVAL_MS: u64 = 2000;
pub const HOST_TTL_MS: u64 = 6000;
pub const MAX_VIEWERS: usize = 4;

pub const PIN_MAX_ATTEMPTS: u32 = 5;
pub const PIN_BLOCK_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Screen,
    Window,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CanvasOrientation {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerticalLayoutPreset {
    /// Corte central inteligente 9:16 + Webcam no topo/base
    SplitCamTop,
    /// Jogo centralizado com fundo desfocado 9:16
    BlurredBackdrop,
    /// Corte central em tela cheia 9:16
    CenterCrop,
    /// Picture-in-Picture vertical livre
    PipVertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamProvider {
    Twitch,
    Youtube,
    Kick,
    Tiktok,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultistreamTarget {
    pub id: String,
    pub provider: StreamProvider,
    pub name: String,
    pub enabled: bool,
    pub rtmpUrl: String,
    pub streamKey: String,
    /// Qual canvas alimenta este destino: Horizontal (16:9) ou Vertical (9:16, ex: TikTok / Shorts)
    pub orientation: CanvasOrientation,
    pub bitrateKbps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultistreamStatus {
    pub active: bool,
    pub encoder: String,
    pub horizontalLive: bool,
    pub verticalLive: bool,
    pub targets: Vec<MultistreamTargetStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WidgetKind {
    StreamElements,
    AlertBox,
    ChatOverlay,
    GoalBar,
    ViewerCounter,
    LowerThird,
    CustomUrl,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamWidget {
    pub id: String,
    pub name: String,
    pub kind: WidgetKind,
    pub enabled: bool,
    pub url: String,
    /// texto/meta opcional para widgets nativos (GoalBar, LowerThird, AlertBox)
    pub primaryText: String,
    pub secondaryText: String,
    pub accentColor: String,
    /// Posicao normalizada (0.0 a 1.0) no canvas 16:9
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Posicao normalizada (0.0 a 1.0) no canvas 9:16
    pub verticalX: f32,
    pub verticalY: f32,
    pub showOnHorizontal: bool,
    pub showOnVertical: bool,
    pub opacity: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioMixerSettings {
    pub systemAudioEnabled: bool,
    pub systemVolume: f32,
    pub systemMuted: bool,
    pub micEnabled: bool,
    pub micDeviceId: Option<String>,
    pub micVolume: f32,
    pub micMuted: bool,
    pub noiseSuppression: bool,
    pub echoCancellation: bool,
    pub compressorEnabled: bool,
    pub partyVoiceEnabled: bool,
}

impl Default for AudioMixerSettings {
    fn default() -> Self {
        Self {
            systemAudioEnabled: true,
            systemVolume: 1.0,
            systemMuted: false,
            micEnabled: false,
            micDeviceId: None,
            micVolume: 1.0,
            micMuted: false,
            noiseSuppression: true,
            echoCancellation: true,
            compressorEnabled: true,
            partyVoiceEnabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StudioCompositorSettings {
    pub dualOutputEnabled: bool,
    pub verticalLayout: VerticalLayoutPreset,
    pub webcamEnabled: bool,
    pub webcamDeviceId: Option<String>,
    pub webcamPosition: String, // "bottom-right" | "bottom-left" | "top-right" | "top-left"
    pub webcamScale: f32,
    pub webcamRound: bool,
    pub privacyShieldActive: bool,
    pub privacyShieldTitle: String,
    pub privacyShieldSubtitle: String,
    pub replayBufferSeconds: u32,
}

impl Default for StudioCompositorSettings {
    fn default() -> Self {
        Self {
            dualOutputEnabled: true,
            verticalLayout: VerticalLayoutPreset::SplitCamTop,
            webcamEnabled: false,
            webcamDeviceId: None,
            webcamPosition: "bottom-right".to_string(),
            webcamScale: 0.24,
            webcamRound: false,
            privacyShieldActive: false,
            privacyShieldTitle: "VOLTO JÁ".to_string(),
            privacyShieldSubtitle: "Transmissão em pausa temporária · PrismCast Studio".to_string(),
            replayBufferSeconds: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyBindings {
    pub toggleMicMute: String,
    pub toggleSystemMute: String,
    pub togglePrivacyShield: String,
    pub saveReplayClip: String,
    pub toggleVerticalPreview: String,
}

impl Default for HotkeyBindings {
    fn default() -> Self {
        Self {
            toggleMicMute: "Ctrl+Shift+M".to_string(),
            toggleSystemMute: "Ctrl+Shift+D".to_string(),
            togglePrivacyShield: "Ctrl+Shift+P".to_string(),
            saveReplayClip: "Ctrl+Shift+S".to_string(),
            toggleVerticalPreview: "Ctrl+Shift+V".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderStatus {
    pub recording: bool,
    pub recordingPath: Option<String>,
    pub recordingDurationSec: u64,
    pub replayBufferActive: bool,
    pub replayBufferedSeconds: f32,
    pub replaySavedPath: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub senderId: String,
    pub senderName: String,
    pub role: String, // "host" | "viewer" | "system"
    pub text: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

// ------------------------------------------------------------------ Sinalizacao WebRTC + Chat

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
