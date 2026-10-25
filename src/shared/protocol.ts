/**
 * Contrato de rede e modelos compartilhados do PrismCast Studio.
 *
 * Compartilhado entre o motor nativo em Rust (`src-rust/protocol.rs`),
 * a ponte Electron/Main (`src/main`) e a interface Studio (`src/renderer`).
 */

export const PROTOCOL_VERSION = 1

export const DISCOVERY_PORT = 41234
export const SIGNALING_PORT = 41235
export const RUST_RPC_PORT = 41236

export const ANNOUNCE_INTERVAL_MS = 2000
export const HOST_TTL_MS = 6000

export const MAX_VIEWERS = 4

export const PIN_MAX_ATTEMPTS = 5
export const PIN_BLOCK_MS = 60_000

// ---------------------------------------------------------------- descoberta

export type SourceKind = 'screen' | 'window'

export interface HostAnnounce {
  t: 'announce'
  v: number
  id: string
  name: string
  ip: string
  port: number
  requiresPin: boolean
  sourceName: string
  sourceKind: SourceKind
  viewers: number
  maxViewers: number
  startedAt: number
}

export interface DiscoveryQuery {
  t: 'query'
  v: number
}

export interface HostBye {
  t: 'bye'
  v: number
  id: string
}

export type DiscoveryMessage = HostAnnounce | DiscoveryQuery | HostBye

export interface DiscoveredHost extends Omit<HostAnnounce, 't' | 'v'> {
  lastSeen: number
}

// -------------------------------------------- Multistream & Dual Output (9:16)

export type CanvasOrientation = 'horizontal' | 'vertical'

export type VerticalLayoutPreset =
  | 'split-cam-top'
  | 'blurred-backdrop'
  | 'center-crop'
  | 'pip-vertical'

export type StreamProvider = 'twitch' | 'youtube' | 'kick' | 'tiktok' | 'custom'

export interface MultistreamTarget {
  id: string
  provider: StreamProvider
  name: string
  enabled: boolean
  rtmpUrl: string
  streamKey: string
  /** Qual canvas alimenta este destino: Horizontal (16:9) ou Vertical (9:16 p/ TikTok & Shorts) */
  orientation: CanvasOrientation
  bitrateKbps: number
}

export interface MultistreamTargetStatus {
  id: string
  provider: StreamProvider
  name: string
  orientation: CanvasOrientation
  state: 'idle' | 'connecting' | 'live' | 'error'
  uptimeSec: number
  bytesSent: number
  lastError: string | null
}

export interface MultistreamStatus {
  active: boolean
  encoder: string
  horizontalLive: boolean
  verticalLive: boolean
  targets: MultistreamTargetStatus[]
}

// ------------------------------------------ Gerenciamento de Cenas & Fontes (OBS Style)

export type SourceType =
  | 'screen_capture'
  | 'window_capture'
  | 'camera_input'
  | 'audio_input'
  | 'audio_output'
  | 'browser_source'
  | 'image_source'
  | 'color_source'
  | 'text_source'

export interface SourceTransform {
  /** Posição X normalizada (0..1) em relação ao canvas */
  x: number
  /** Posição Y normalizada (0..1) em relação ao canvas */
  y: number
  /** Largura normalizada (0..1) */
  width: number
  /** Altura normalizada (0..1) */
  height: number
  /** Camada Z (maior = na frente) */
  zIndex: number
  cropTop?: number
  cropBottom?: number
  cropLeft?: number
  cropRight?: number
}

export interface SceneItem {
  id: string
  sourceId: string
  name: string
  type: SourceType
  visible: boolean
  locked: boolean
  /** Transform no canvas 16:9 Horizontal (1920x1080) */
  transform: SourceTransform
  /** Transform no canvas 9:16 Vertical (1080x1920) */
  verticalTransform: SourceTransform
  showOnHorizontal: boolean
  showOnVertical: boolean
  /** Configurações específicas da fonte (deviceId, windowId, url, text, color, volume, etc.) */
  settings: Record<string, any>
}

export interface Scene {
  id: string
  name: string
  items: SceneItem[]
  createdAt: number
}

export interface StudioScenesState {
  scenes: Scene[]
  activeSceneId: string
  previewSceneId: string
  studioMode: boolean
  transitionType: 'cut' | 'fade'
  transitionDurationMs: number
}

// ------------------------------------------ Sistema de Login & Plataformas de Streaming

export type PlatformId = 'twitch' | 'youtube' | 'kick' | 'tiktok' | 'prismcast'

export interface ConnectedAccount {
  platform: PlatformId
  username: string
  displayName: string
  avatarUrl: string
  connected: boolean
  rtmpUrl: string
  streamKey: string
  channelUrl?: string
  connectedAt?: number
}

export interface UserProfile {
  isLoggedIn: boolean
  id: string
  name: string
  email: string
  avatarUrl: string
  plan: 'free' | 'pro' | 'studio'
  accounts: Record<PlatformId, ConnectedAccount>
}

// -------------------------------------- Dispositivos de Áudio e Vídeo do Sistema

export interface MediaDeviceEntry {
  deviceId: string
  label: string
  kind: 'audioinput' | 'audiooutput' | 'videoinput'
  groupId: string
}

// -------------------------------------- Widgets & StreamElements Overlays

export type WidgetKind =
  | 'streamelements'
  | 'alert-box'
  | 'chat-overlay'
  | 'goal-bar'
  | 'viewer-counter'
  | 'lower-third'
  | 'custom-url'

export interface StreamWidget {
  id: string
  name: string
  kind: WidgetKind
  enabled: boolean
  url: string
  primaryText: string
  secondaryText: string
  accentColor: string
  /** Posição normalizada (0..1) no canvas 16:9 */
  x: number
  y: number
  width: number
  height: number
  /** Posição normalizada (0..1) no canvas vertical 9:16 */
  verticalX: number
  verticalY: number
  showOnHorizontal: boolean
  showOnVertical: boolean
  opacity: number
}

// ------------------------------------------------ Mixer de Áudio & Estúdio

export interface AudioMixerSettings {
  systemAudioEnabled: boolean
  systemVolume: number
  systemMuted: boolean
  micEnabled: boolean
  micDeviceId: string | null
  micVolume: number
  micMuted: boolean
  noiseSuppression: boolean
  echoCancellation: boolean
  compressorEnabled: boolean
  partyVoiceEnabled: boolean
}

export interface StudioCompositorSettings {
  dualOutputEnabled: boolean
  verticalLayout: VerticalLayoutPreset
  webcamEnabled: boolean
  webcamDeviceId: string | null
  webcamPosition: 'bottom-right' | 'bottom-left' | 'top-right' | 'top-left'
  webcamScale: number
  webcamRound: boolean
  privacyShieldActive: boolean
  privacyShieldTitle: string
  privacyShieldSubtitle: string
  replayBufferSeconds: number
}

export interface HotkeyBindings {
  toggleMicMute: string
  toggleSystemMute: string
  togglePrivacyShield: string
  saveReplayClip: string
  toggleVerticalPreview: string
}

export type HotkeyAction =
  | 'toggle-mic-mute'
  | 'toggle-system-mute'
  | 'toggle-privacy-shield'
  | 'save-replay-clip'
  | 'toggle-vertical-preview'

export interface RecorderStatus {
  recording: boolean
  recordingPath: string | null
  recordingDurationSec: number
  replayBufferActive: boolean
  replayBufferedSeconds: number
  replaySavedPath: string | null
}

// ------------------------------------------------- Chat LAN & Reações Ao Vivo

export interface ChatMessage {
  id: string
  senderId: string
  senderName: string
  role: 'host' | 'viewer' | 'system'
  text: string
  timestamp: number
}

export interface ReactionEvent {
  id: string
  senderName: string
  emoji: string
  timestamp: number
}

// --------------------------------------------------------------- sinalizacao

export interface SessionDescriptionPayload {
  type: 'offer' | 'answer'
  sdp: string
}

export interface IceCandidatePayload {
  candidate: string
  sdpMid: string | null
  sdpMLineIndex: number | null
  usernameFragment?: string | null
}

export type RejectReason =
  | 'pin-invalido'
  | 'sala-cheia'
  | 'versao-incompativel'
  | 'bloqueado'
  | 'sem-transmissao'

export const REJECT_MESSAGES: Record<RejectReason, string> = {
  'pin-invalido': 'Código incorreto.',
  'sala-cheia': 'A transmissão já está com o número máximo de espectadores.',
  'versao-incompativel': 'As duas máquinas estão com versões diferentes do app.',
  bloqueado: 'Muitas tentativas erradas. Espere um minuto e tente de novo.',
  'sem-transmissao': 'Este computador não está transmitindo no momento.'
}

export type ViewerToHost =
  | { t: 'hello'; v: number; name: string; pin: string | null }
  | { t: 'answer'; desc: SessionDescriptionPayload }
  | { t: 'ice'; candidate: IceCandidatePayload }
  | { t: 'ping'; sentAt: number }
  | { t: 'chat'; text: string }
  | { t: 'reaction'; emoji: string }
  | { t: 'voice-offer'; desc: SessionDescriptionPayload }
  | { t: 'voice-ice'; candidate: IceCandidatePayload }
  | { t: 'bye' }

export type HostToViewer =
  | {
      t: 'welcome'
      viewerId: string
      hostName: string
      sourceName: string
      sourceKind: SourceKind
    }
  | { t: 'rejected'; reason: RejectReason; retryAfterMs?: number }
  | { t: 'offer'; desc: SessionDescriptionPayload }
  | { t: 'ice'; candidate: IceCandidatePayload }
  | { t: 'pong'; sentAt: number }
  | { t: 'chat'; message: ChatMessage }
  | { t: 'reaction'; reaction: ReactionEvent }
  | { t: 'source-changed'; sourceName: string; sourceKind: SourceKind }
  | { t: 'voice-answer'; desc: SessionDescriptionPayload }
  | { t: 'voice-ice'; candidate: IceCandidatePayload }
  | { t: 'kicked' }
  | { t: 'ended' }

export interface ConnectedViewer {
  id: string
  name: string
  address: string
  joinedAt: number
  pingMs: number | null
}
