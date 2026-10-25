/**
 * Superfície de IPC entre a interface do PrismCast Studio e o processo main.
 */

import type { QualitySettings, PresetId } from './quality'
import type {
  AudioMixerSettings,
  BrowserSourceFrame,
  BrowserSourceSpec,
  CanvasOrientation,
  ChatMessage,
  ConnectedViewer,
  Destination,
  DiscoveredHost,
  HostToViewer,
  HotkeyAction,
  HotkeyBindings,
  MultistreamStatus,
  PlatformId,
  ReactionEvent,
  RecorderStatus,
  RejectReason,
  SourceKind,
  StudioCompositorSettings,
  StudioScenesState,
  TwitchLoginState,
  ViewerToHost,
  WidgetKit
} from './protocol'

export interface DesktopSource {
  id: string
  name: string
  kind: SourceKind
  thumbnail: string
  appIcon: string | null
  displayId: string
}

export interface AppInfo {
  name: string
  version: string
  hostname: string
  isPackaged: boolean
}

export interface HostStartOptions {
  sourceId: string
  sourceName: string
  sourceKind: SourceKind
  hostName: string
  requirePin: boolean
  maxViewers: number
}

export type HostStartResult =
  | { ok: true; sessionId: string; ip: string; port: number; pin: string | null }
  | { ok: false; error: string }

export interface ViewerConnectOptions {
  ip: string
  port: number
  pin: string | null
  name: string
}

export type ViewerConnectResult =
  | {
      ok: true
      hostName: string
      sourceName: string
      sourceKind: SourceKind
      viewerId: string
    }
  | { ok: false; reason: RejectReason | 'rede'; message: string; retryAfterMs?: number }

export interface NetInterface {
  name: string
  address: string
  netmask: string
  broadcast: string
  isPreferred: boolean
}

export type NetworkProfile = 'private' | 'public' | 'domain' | 'unknown'

export interface FirewallStatus {
  ruleExists: boolean
  execPath: string
  command: string
  profile: NetworkProfile
  checked: boolean
}

export interface AppSettings {
  schemaVersion: number
  hostName: string
  viewerName: string
  quality: QualitySettings
  presetId: PresetId
  requirePin: boolean
  preferredInterface: string | null
  firewallPromptDismissed: boolean
  audioMixer: AudioMixerSettings
  studio: StudioCompositorSettings
  hotkeys: HotkeyBindings
  destinations: Destination[]
  scenes: StudioScenesState
  /** Client ID próprio para o login da Twitch. Vazio = o do PrismCast (embutido). */
  twitchClientId: string
  /** Pasta do kit de widgets (overlays HTML), ex. E:\kayoinreality-kit */
  kitPath: string | null
}

export interface ViewerClosedInfo {
  reason: 'kicked' | 'ended' | 'rede' | 'local'
  message: string
}

export type Unsubscribe = () => void

export type OkResult = { ok: true } | { ok: false; error: string }

export interface Api {
  app: {
    info(): Promise<AppInfo>
    minimize(): void
    toggleMaximize(): void
    close(): void
    setAlwaysOnTop(value: boolean): Promise<boolean>
    onMaximizeChange(cb: (maximized: boolean) => void): Unsubscribe
    onHotkey(cb: (action: HotkeyAction) => void): Unsubscribe
    openExternal(url: string): void
    showInFolder(path: string): void
  }
  sources: {
    list(): Promise<DesktopSource[]>
    select(sourceId: string, withAudio: boolean): Promise<void>
  }
  destinations: {
    /** Salva (criptografada) a chave de transmissão. Chave vazia apaga. */
    setKey(id: string, key: string): Promise<OkResult>
    twitchLogin(destinationId: string): Promise<void>
    twitchCancel(): void
    onTwitchLogin(cb: (state: TwitchLoginState) => void): Unsubscribe
    /** Desvincula a conta (apaga token e chave). */
    unlink(id: string): Promise<void>
  }
  browserSources: {
    sync(specs: BrowserSourceSpec[]): void
    reload(id: string): void
    onFrame(cb: (frame: BrowserSourceFrame) => void): Unsubscribe
    /** Confirma que o quadro foi desenhado; libera o próximo daquela fonte. */
    ack(id: string): void
  }
  kit: {
    load(path: string | null): Promise<WidgetKit | null>
    pickFolder(): Promise<string | null>
  }
  files: {
    pickImage(): Promise<string | null>
    pickHtml(): Promise<string | null>
    readImage(path: string): Promise<string | null>
  }
  host: {
    start(opts: HostStartOptions): Promise<HostStartResult>
    stop(): Promise<void>
    kick(viewerId: string): Promise<void>
    send(viewerId: string, msg: HostToViewer): void
    broadcast(msg: HostToViewer): void
    updateSource(sourceName: string, sourceKind: SourceKind): void
    setViewerCount(n: number): void
    onViewers(cb: (viewers: ConnectedViewer[]) => void): Unsubscribe
    onMessage(cb: (viewerId: string, msg: ViewerToHost) => void): Unsubscribe
    onChat(cb: (msg: ChatMessage) => void): Unsubscribe
    onReaction(cb: (reaction: ReactionEvent) => void): Unsubscribe
  }
  discovery: {
    start(): Promise<void>
    stop(): Promise<void>
    refresh(): void
    onHosts(cb: (hosts: DiscoveredHost[]) => void): Unsubscribe
  }
  viewer: {
    connect(opts: ViewerConnectOptions): Promise<ViewerConnectResult>
    send(msg: ViewerToHost): void
    disconnect(): void
    onMessage(cb: (msg: HostToViewer) => void): Unsubscribe
    onClosed(cb: (info: ViewerClosedInfo) => void): Unsubscribe
  }
  multistream: {
    status(): Promise<MultistreamStatus>
    start(fps: number): Promise<{ ok: boolean; status: MultistreamStatus; error?: string }>
    pushChunk(orientation: CanvasOrientation, chunk: Uint8Array): void
    stop(): Promise<MultistreamStatus>
  }
  recorder: {
    status(): Promise<RecorderStatus>
    start(): Promise<{ ok: boolean; status?: RecorderStatus; error?: string }>
    stop(): Promise<RecorderStatus>
    setReplayBuffer(enabled: boolean, seconds: number): Promise<RecorderStatus>
    pushChunk(chunk: Uint8Array): void
    resetStream(): void
    saveReplay(): Promise<{
      ok: boolean
      path?: string
      status?: RecorderStatus
      error?: string
    }>
  }
  net: {
    interfaces(): Promise<NetInterface[]>
  }
  firewall: {
    status(): Promise<FirewallStatus>
    authorize(): Promise<{ ok: boolean; error?: string }>
  }
  settings: {
    get(): Promise<AppSettings>
    set(patch: Partial<AppSettings>): Promise<AppSettings>
    onChanged(cb: (settings: AppSettings) => void): Unsubscribe
  }
}

/**
 * Client ID do app "PrismCast" registrado em dev.twitch.tv (tipo Público). Não é
 * segredo: clientes públicos não têm secret e o fluxo device code foi feito para
 * o ID ir dentro do programa. Quem quiser usar o próprio app sobrescreve em Destinos.
 */
export const PRISMCAST_TWITCH_CLIENT_ID = 'xsohreda00eufw5ifqsxtakmgwyys6'

/** Endereços de servidor conhecidos. Kick e TikTok mudam por conta: o usuário cola o dele. */
export const PLATFORM_INFO: Record<
  PlatformId,
  {
    label: string
    defaultServer: string
    orientation: CanvasOrientation
    bitrateKbps: number
    /** onde o usuário encontra a chave */
    keyUrl: string
    keyHint: string
    /** o servidor também precisa ser copiado do painel da plataforma */
    serverFromDashboard: boolean
  }
> = {
  twitch: {
    label: 'Twitch',
    defaultServer: 'rtmp://live.twitch.tv/app',
    orientation: 'horizontal',
    bitrateKbps: 6000,
    keyUrl: 'https://dashboard.twitch.tv/settings/stream',
    keyHint: 'formato live_123456789_…',
    serverFromDashboard: false
  },
  youtube: {
    label: 'YouTube',
    defaultServer: 'rtmp://a.rtmp.youtube.com/live2',
    orientation: 'horizontal',
    bitrateKbps: 9000,
    keyUrl: 'https://studio.youtube.com/channel/UC/livestreaming',
    keyHint: 'formato xxxx-xxxx-xxxx-xxxx-xxxx',
    serverFromDashboard: false
  },
  kick: {
    label: 'Kick',
    defaultServer: 'rtmps://fa723fc1b171.global-contribute.live-video.net/app',
    orientation: 'horizontal',
    bitrateKbps: 8000,
    keyUrl: 'https://kick.com/dashboard/settings/stream',
    keyHint: 'começa com sk_',
    serverFromDashboard: true
  },
  tiktok: {
    label: 'TikTok',
    defaultServer: '',
    orientation: 'vertical',
    bitrateKbps: 4500,
    keyUrl: 'https://livecenter.tiktok.com/producer',
    keyHint: 'servidor e chave mudam a cada live',
    serverFromDashboard: true
  },
  custom: {
    label: 'RTMP personalizado',
    defaultServer: '',
    orientation: 'horizontal',
    bitrateKbps: 6000,
    keyUrl: '',
    keyHint: 'chave do seu servidor',
    serverFromDashboard: true
  }
}

export const CH = {
  appInfo: 'app:info',
  appMinimize: 'app:minimize',
  appToggleMaximize: 'app:toggle-maximize',
  appClose: 'app:close',
  appAlwaysOnTop: 'app:always-on-top',
  appMaximizeChanged: 'app:maximize-changed',
  appHotkey: 'app:hotkey',
  appOpenExternal: 'app:open-external',
  appShowInFolder: 'app:show-in-folder',

  sourcesList: 'sources:list',
  sourcesSelect: 'sources:select',

  destSetKey: 'dest:set-key',
  destTwitchLogin: 'dest:twitch-login',
  destTwitchCancel: 'dest:twitch-cancel',
  destTwitchState: 'dest:twitch-state',
  destUnlink: 'dest:unlink',

  bsSync: 'bs:sync',
  bsReload: 'bs:reload',
  bsFrame: 'bs:frame',
  bsAck: 'bs:ack',

  kitLoad: 'kit:load',
  kitPickFolder: 'kit:pick-folder',

  filesPickImage: 'files:pick-image',
  filesPickHtml: 'files:pick-html',
  filesReadImage: 'files:read-image',

  hostStart: 'host:start',
  hostStop: 'host:stop',
  hostKick: 'host:kick',
  hostSend: 'host:send',
  hostBroadcast: 'host:broadcast',
  hostUpdateSource: 'host:update-source',
  hostSetViewerCount: 'host:set-viewer-count',
  hostViewers: 'host:viewers',
  hostMessage: 'host:message',
  hostChat: 'host:chat',
  hostReaction: 'host:reaction',

  discoveryStart: 'discovery:start',
  discoveryStop: 'discovery:stop',
  discoveryRefresh: 'discovery:refresh',
  discoveryHosts: 'discovery:hosts',

  viewerConnect: 'viewer:connect',
  viewerSend: 'viewer:send',
  viewerDisconnect: 'viewer:disconnect',
  viewerMessage: 'viewer:message',
  viewerClosed: 'viewer:closed',

  multistreamStatus: 'multistream:status',
  multistreamStart: 'multistream:start',
  multistreamPushChunk: 'multistream:push-chunk',
  multistreamStop: 'multistream:stop',

  recorderStatus: 'recorder:status',
  recorderStart: 'recorder:start',
  recorderStop: 'recorder:stop',
  recorderSetReplay: 'recorder:set-replay',
  recorderPushChunk: 'recorder:push-chunk',
  recorderReset: 'recorder:reset',
  recorderSaveReplay: 'recorder:save-replay',

  netInterfaces: 'net:interfaces',

  firewallStatus: 'firewall:status',
  firewallAuthorize: 'firewall:authorize',

  settingsGet: 'settings:get',
  settingsSet: 'settings:set',
  settingsChanged: 'settings:changed'
} as const
