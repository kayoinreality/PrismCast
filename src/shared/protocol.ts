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

// -------------------------------------------- Destinos de transmissão (RTMP)

export type CanvasOrientation = 'horizontal' | 'vertical'

export type PlatformId = 'twitch' | 'youtube' | 'kick' | 'tiktok' | 'custom'

/** Conta vinculada por login de verdade (OAuth). Só existe quando a plataforma deu um token. */
export interface PlatformAccount {
  login: string
  displayName: string
  avatarUrl: string
}

/**
 * Um lugar para onde a live vai.
 *
 * A chave de transmissão NUNCA vem para o renderer: ela fica criptografada no
 * main (safeStorage) e só é lida na hora de abrir o FFmpeg. O renderer só sabe
 * se ela existe (`hasKey`).
 */
export interface Destination {
  id: string
  platform: PlatformId
  name: string
  enabled: boolean
  /** URL do servidor RTMP/RTMPS, sem a chave. */
  server: string
  hasKey: boolean
  /** Qual canvas alimenta este destino: 16:9 (Twitch, YouTube, Kick) ou 9:16 (TikTok). */
  orientation: CanvasOrientation
  bitrateKbps: number
  account: PlatformAccount | null
}

export interface DestinationStatus {
  id: string
  state: 'idle' | 'connecting' | 'live' | 'error'
  uptimeSec: number
  error: string | null
}

export interface MultistreamStatus {
  active: boolean
  encoder: string
  ffmpegFound: boolean
  destinations: DestinationStatus[]
}

/** Login da Twitch pelo fluxo "device code": o usuário digita o código em twitch.tv/activate. */
export type TwitchLoginState =
  | { step: 'idle' }
  | { step: 'code'; userCode: string; verificationUri: string; expiresAt: number }
  | { step: 'done'; account: PlatformAccount }
  | { step: 'error'; message: string }

// ------------------------------------------ Gerenciamento de Cenas & Fontes (OBS Style)

export type SourceType =
  | 'screen_capture'
  | 'window_capture'
  | 'camera_input'
  | 'browser_source'
  | 'image_source'
  | 'color_source'
  | 'text_source'

/** Configurações de cada tipo de fonte. Só os campos do tipo da fonte são usados. */
export interface SourceSettings {
  /** screen/window: id do desktopCapturer e o nome, para reencontrar a janela depois de reiniciar. */
  captureId?: string
  captureName?: string
  /** câmera */
  deviceId?: string
  deviceLabel?: string
  /** browser source: URL http(s) ou file:// e o tamanho da página renderizada */
  url?: string
  width?: number
  height?: number
  fps?: number
  /** widget do kit que originou esta fonte (para mostrar o nome amigável) */
  kitWidget?: string
  /** imagem */
  path?: string
  /** cor / texto */
  color?: string
  text?: string
  fontSize?: number
  /** borda arredondada para câmera e imagem */
  rounded?: boolean
}

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
  settings: SourceSettings
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

// -------------------------------------- Browser sources (widgets) renderizados fora da tela

export interface BrowserSourceSpec {
  id: string
  url: string
  width: number
  height: number
  fps: number
}

/** Pedaço da página que mudou: pixels BGRA (pré-multiplicados) do retângulo sujo. */
export interface BrowserSourceFrame {
  id: string
  fullWidth: number
  fullHeight: number
  x: number
  y: number
  width: number
  height: number
  data: Uint8Array
}

// -------------------------------------- Kit de widgets (pasta com overlays HTML)

export interface KitWidget {
  /** nome do arquivo, ex. "comecando.html" */
  file: string
  name: string
  description: string
  /** file:// da página */
  url: string
  /** tamanho sugerido da fonte no canvas 1920x1080 */
  width: number
  height: number
  /** tela cheia que cobre tudo (cena de pausa/abertura) ou overlay transparente */
  role: 'scene' | 'overlay' | 'frame' | 'chat'
  /** parâmetros de URL que a página entende, para o painel de propriedades */
  params: Array<{ key: string; label: string; placeholder: string }>
}

export interface WidgetKit {
  path: string
  name: string
  widgets: KitWidget[]
  /** O kit traz o widget de alertas do StreamElements (que se instala no site deles). */
  hasStreamElements: boolean
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
  /** Quadros por segundo do canvas e da saída RTMP. */
  fps: 30 | 60
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
