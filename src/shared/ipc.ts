/**
 * Superfície de IPC entre a interface do PrismCast Studio e o motor nativo Rust / Main.
 */

import type { QualitySettings, PresetId } from './quality'
import type {
  AudioMixerSettings,
  CanvasOrientation,
  ChatMessage,
  ConnectedViewer,
  DiscoveredHost,
  HostToViewer,
  HotkeyAction,
  HotkeyBindings,
  MultistreamStatus,
  MultistreamTarget,
  ReactionEvent,
  RecorderStatus,
  RejectReason,
  SourceKind,
  StreamWidget,
  StudioCompositorSettings,
  ViewerToHost
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
  rustEngineActive?: boolean
  hardwareEncoder?: string
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
  multistreamTargets: MultistreamTarget[]
  widgets: StreamWidget[]
}

export interface ViewerClosedInfo {
  reason: 'kicked' | 'ended' | 'rede' | 'local'
  message: string
}

export type Unsubscribe = () => void

export interface Api {
  app: {
    info(): Promise<AppInfo>
    minimize(): void
    toggleMaximize(): void
    close(): void
    setAlwaysOnTop(value: boolean): Promise<boolean>
    onMaximizeChange(cb: (maximized: boolean) => void): Unsubscribe
    onHotkey(cb: (action: HotkeyAction) => void): Unsubscribe
  }
  sources: {
    list(): Promise<DesktopSource[]>
    select(sourceId: string, withAudio: boolean): Promise<void>
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
    start(
      targets: MultistreamTarget[],
      fps: number
    ): Promise<{ ok: boolean; status?: MultistreamStatus; error?: string }>
    pushChunk(orientation: CanvasOrientation, chunk: Uint8Array): void
    stop(): Promise<MultistreamStatus>
  }
  recorder: {
    status(): Promise<RecorderStatus>
    start(): Promise<{ ok: boolean; status?: RecorderStatus; error?: string }>
    stop(): Promise<RecorderStatus>
    setReplayBuffer(enabled: boolean, seconds: number): Promise<RecorderStatus>
    pushChunk(chunk: Uint8Array): void
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

  sourcesList: 'sources:list',
  sourcesSelect: 'sources:select',

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
  recorderSaveReplay: 'recorder:save-replay',

  netInterfaces: 'net:interfaces',

  firewallStatus: 'firewall:status',
  firewallAuthorize: 'firewall:authorize',

  settingsGet: 'settings:get',
  settingsSet: 'settings:set'
} as const
