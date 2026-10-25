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
  /** id do canal na Twitch (logins antigos não têm; o main busca quando precisar) */
  id?: string
  /** permissões que o login deu; logins da 2.2 só tinham a da chave */
  scopes?: string[]
}

/** Título e categoria da live. Na Twitch o PrismCast atualiza sozinho. */
export interface LiveInfo {
  title: string
  categoryId: string
  categoryName: string
}

export interface TwitchCategory {
  id: string
  name: string
  boxArtUrl: string
}

/** O que dá para fazer na Twitch com o login atual. */
export interface TwitchCapabilities {
  linked: boolean
  login: string | null
  /** mudar título/categoria */
  canEditInfo: boolean
  /** escrever no chat */
  canChat: boolean
}

export type TwitchActionResult = { ok: true } | { ok: false; error: string; needsRelogin?: boolean }

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
  /** microfone/entrada do mixer: só vai para a live nas cenas que têm esta fonte */
  | 'audio_input'

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
  /**
   * Widget: recarrega a página toda vez que a cena entra no ar (contagem
   * regressiva, cronômetro). Sem valor = o padrão do widget do kit.
   */
  restartOnShow?: boolean
  /**
   * janela: canal do mixer que captura o áudio deste programa
   * audio_input: o microfone do mixer que esta cena liga
   */
  audioChannelId?: string
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
  params: KitParam[]
  /** recarregar quando a cena entra no ar (contagem e cronômetro recomeçam) */
  restartOnShow?: boolean
  /** como desligar a parte da câmera do overlay quando o estúdio está sem câmera */
  camera?: { param: string; off: string } | 'hide'
}

export type KitParam =
  | { key: string; label: string; hint?: string; type: 'text'; placeholder: string }
  | { key: string; label: string; hint?: string; type: 'number'; placeholder: string; min?: number; max?: number; unit?: string }
  | { key: string; label: string; hint?: string; type: 'select'; options: Array<{ value: string; label: string }> }
  | { key: string; label: string; hint?: string; type: 'toggle'; on: string; off: string; defaultOn: boolean }

/** Textos do kit (src/config.js): valem para todas as telas de uma vez. */
export interface KitConfig {
  canal: string
  nome: string
  jogo: string
  contagem: number
  agenda: string
  pix: string
  redes: Array<{ rede: string; handle: string }>
  recados: string[]
  /** chaves que o PrismCast não conhece, preservadas ao salvar */
  extra: Record<string, unknown>
}

export interface WidgetKit {
  path: string
  name: string
  widgets: KitWidget[]
  /** O kit traz o widget de alertas do StreamElements (que se instala no site deles). */
  hasStreamElements: boolean
}

// ------------------------------------------------ Mixer de Áudio & Estúdio

/**
 * Filtros de uma fonte de áudio, na ordem em que o som passa por eles
 * (como a janela de Filtros do OBS).
 */
export interface AudioFilters {
  /** só microfone: processamento do próprio Chromium/Windows */
  noiseSuppression: boolean
  echoCancellation: boolean
  autoGain: boolean
  /** corta graves (ventoinha, batida na mesa). 0 = desligado */
  highpassHz: number
  gate: { enabled: boolean; thresholdDb: number; releaseMs: number }
  compressor: {
    enabled: boolean
    thresholdDb: number
    ratio: number
    attackMs: number
    releaseMs: number
    makeupDb: number
  }
  /** ganho antes do fader, em dB */
  gainDb: number
  limiter: { enabled: boolean; ceilingDb: number }
}

export type AudioChannelKind = 'desktop' | 'mic' | 'app'

/** off: só vai para a live · monitor: live e fone · monitor-only: só fone */
export type AudioMonitorMode = 'off' | 'monitor' | 'monitor-only'

export interface AudioChannel {
  id: string
  kind: AudioChannelKind
  name: string
  /** microfone: dispositivo (null = o padrão do Windows) */
  deviceId: string | null
  /** aplicativo: exe que é capturado (ex. "VALORANT-Win64-Shipping.exe") */
  appExe: string | null
  /** volume do fader, em dB (-60 = mudo, 0 = 100%, +12 no máximo) */
  volumeDb: number
  muted: boolean
  monitor: AudioMonitorMode
  /** atraso para sincronizar com a câmera, em ms */
  syncOffsetMs: number
  filters: AudioFilters
}

export interface AudioMixerSettings {
  channels: AudioChannel[]
  partyVoiceEnabled: boolean
}

/** Estado vivo de um canal (não vai para as preferências). */
export interface AudioChannelStatus {
  id: string
  state: 'ok' | 'waiting' | 'error' | 'off'
  message: string | null
}

/** Janela ou programa que pode virar fonte de áudio de aplicativo. */
export interface AudioApp {
  hwnd: number
  pid: number
  exe: string
  title: string
  /** tocando som agora */
  audio: boolean
}

export interface StudioCompositorSettings {
  /** Desligado: fontes de câmera somem e os overlays escondem a moldura da câmera. */
  cameraEnabled: boolean
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
  toggleCamera: string
}

export type HotkeyAction =
  | 'toggle-mic-mute'
  | 'toggle-system-mute'
  | 'toggle-privacy-shield'
  | 'save-replay-clip'
  | 'toggle-vertical-preview'
  | 'toggle-camera'

/** Chat da Twitch dentro do app e na janela separada. */
export interface ChatSettings {
  /** canal lido; vazio = o da conta da Twitch conectada */
  channel: string
  /** multichat: canal do YouTube (@handle, link do canal ou da live); vazio = desligado */
  youtube: string
  /** multichat: canal da Kick (o nome que vai em kick.com/nome); vazio = desligado */
  kick: string
  fontSize: number
  showTimestamps: boolean
  /** a janela separada fica por cima do jogo */
  popoutOnTop: boolean
  popoutBounds: { x: number; y: number; width: number; height: number } | null
}

// ------------------------------------------- Multichat (YouTube e Kick pelo main)

export type ChatPlatform = 'twitch' | 'youtube' | 'kick'

export type ChatFragment = { t: 'text'; text: string } | { t: 'emote'; url: string; name: string }

/** Mensagem de qualquer plataforma, já no formato que o chat do app e os widgets mostram. */
export interface MultiChatMessage {
  id: string
  platform: ChatPlatform
  kind: 'msg' | 'event'
  user: string
  name: string
  /** cor do nome (vazio = o chat escolhe uma pelo nome) */
  color: string
  badges: string[]
  fragments: ChatFragment[]
  ts: number
}

export interface MultiChatStatus {
  platform: Exclude<ChatPlatform, 'twitch'>
  /** off: sem canal configurado · waiting: canal sem live agora (tenta de novo sozinho) */
  state: 'off' | 'connecting' | 'connected' | 'waiting' | 'error'
  channel: string
  message: string | null
}

export type MultiChatEvent =
  | { t: 'messages'; messages: MultiChatMessage[] }
  | { t: 'delete'; platform: ChatPlatform; id: string }
  | { t: 'status'; status: MultiChatStatus[] }

/**
 * O que os widgets recebem do PrismCast (window.prismcast nas páginas abertas
 * como fonte de navegador). Só informação pública da live.
 */
export interface WidgetLiveState {
  title: string
  /** categoria/jogo escolhido no seletor da live */
  game: string
}

export type WidgetEvent = { t: 'live'; live: WidgetLiveState } | MultiChatEvent

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
