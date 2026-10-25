import { contextBridge, ipcRenderer, type IpcRendererEvent } from 'electron'
import { CH } from '@shared/ipc'
import type {
  Api,
  AppInfo,
  AppSettings,
  FirewallStatus,
  HostStartOptions,
  HostStartResult,
  NetInterface,
  DesktopSource,
  OkResult,
  Unsubscribe,
  ViewerClosedInfo,
  ViewerConnectOptions,
  ViewerConnectResult
} from '@shared/ipc'
import type {
  BrowserSourceFrame,
  CanvasOrientation,
  ChatMessage,
  ConnectedViewer,
  DiscoveredHost,
  HostToViewer,
  HotkeyAction,
  MultistreamStatus,
  ReactionEvent,
  RecorderStatus,
  SourceKind,
  TwitchLoginState,
  ViewerToHost,
  WidgetKit
} from '@shared/protocol'

function subscribe<A extends unknown[]>(
  channel: string,
  cb: (...args: A) => void
): Unsubscribe {
  const listener = (_event: IpcRendererEvent, ...args: unknown[]): void => cb(...(args as A))
  ipcRenderer.on(channel, listener)
  return () => {
    ipcRenderer.removeListener(channel, listener)
  }
}

const api: Api = {
  app: {
    info: () => ipcRenderer.invoke(CH.appInfo) as Promise<AppInfo>,
    minimize: () => ipcRenderer.send(CH.appMinimize),
    toggleMaximize: () => ipcRenderer.send(CH.appToggleMaximize),
    close: () => ipcRenderer.send(CH.appClose),
    setAlwaysOnTop: (value) => ipcRenderer.invoke(CH.appAlwaysOnTop, value) as Promise<boolean>,
    onMaximizeChange: (cb) => subscribe<[boolean]>(CH.appMaximizeChanged, cb),
    onHotkey: (cb) => subscribe<[HotkeyAction]>(CH.appHotkey, cb),
    openExternal: (url) => ipcRenderer.send(CH.appOpenExternal, url),
    showInFolder: (path) => ipcRenderer.send(CH.appShowInFolder, path)
  },
  sources: {
    list: () => ipcRenderer.invoke(CH.sourcesList) as Promise<DesktopSource[]>,
    select: (sourceId, withAudio) =>
      ipcRenderer.invoke(CH.sourcesSelect, sourceId, withAudio) as Promise<void>
  },
  destinations: {
    setKey: (id, key) => ipcRenderer.invoke(CH.destSetKey, id, key) as Promise<OkResult>,
    twitchLogin: (id) => ipcRenderer.invoke(CH.destTwitchLogin, id) as Promise<void>,
    twitchCancel: () => ipcRenderer.send(CH.destTwitchCancel),
    onTwitchLogin: (cb) => subscribe<[TwitchLoginState]>(CH.destTwitchState, cb),
    unlink: (id) => ipcRenderer.invoke(CH.destUnlink, id) as Promise<void>
  },
  browserSources: {
    sync: (specs) => ipcRenderer.send(CH.bsSync, specs),
    reload: (id) => ipcRenderer.send(CH.bsReload, id),
    onFrame: (cb) => subscribe<[BrowserSourceFrame]>(CH.bsFrame, cb),
    ack: (id) => ipcRenderer.send(CH.bsAck, id)
  },
  kit: {
    load: (path) => ipcRenderer.invoke(CH.kitLoad, path) as Promise<WidgetKit | null>,
    pickFolder: () => ipcRenderer.invoke(CH.kitPickFolder) as Promise<string | null>
  },
  files: {
    pickImage: () => ipcRenderer.invoke(CH.filesPickImage) as Promise<string | null>,
    pickHtml: () => ipcRenderer.invoke(CH.filesPickHtml) as Promise<string | null>,
    readImage: (path) => ipcRenderer.invoke(CH.filesReadImage, path) as Promise<string | null>
  },
  host: {
    start: (opts: HostStartOptions) =>
      ipcRenderer.invoke(CH.hostStart, opts) as Promise<HostStartResult>,
    stop: () => ipcRenderer.invoke(CH.hostStop) as Promise<void>,
    kick: (viewerId) => ipcRenderer.invoke(CH.hostKick, viewerId) as Promise<void>,
    send: (viewerId, msg: HostToViewer) => ipcRenderer.send(CH.hostSend, viewerId, msg),
    broadcast: (msg: HostToViewer) => ipcRenderer.send(CH.hostBroadcast, msg),
    updateSource: (sourceName: string, sourceKind: SourceKind) =>
      ipcRenderer.send(CH.hostUpdateSource, sourceName, sourceKind),
    setViewerCount: (n) => ipcRenderer.send(CH.hostSetViewerCount, n),
    onViewers: (cb) => subscribe<[ConnectedViewer[]]>(CH.hostViewers, cb),
    onMessage: (cb) => subscribe<[string, ViewerToHost]>(CH.hostMessage, cb),
    onChat: (cb) => subscribe<[ChatMessage]>(CH.hostChat, cb),
    onReaction: (cb) => subscribe<[ReactionEvent]>(CH.hostReaction, cb)
  },
  discovery: {
    start: () => ipcRenderer.invoke(CH.discoveryStart) as Promise<void>,
    stop: () => ipcRenderer.invoke(CH.discoveryStop) as Promise<void>,
    refresh: () => ipcRenderer.send(CH.discoveryRefresh),
    onHosts: (cb) => subscribe<[DiscoveredHost[]]>(CH.discoveryHosts, cb)
  },
  viewer: {
    connect: (opts: ViewerConnectOptions) =>
      ipcRenderer.invoke(CH.viewerConnect, opts) as Promise<ViewerConnectResult>,
    send: (msg: ViewerToHost) => ipcRenderer.send(CH.viewerSend, msg),
    disconnect: () => ipcRenderer.send(CH.viewerDisconnect),
    onMessage: (cb) => subscribe<[HostToViewer]>(CH.viewerMessage, cb),
    onClosed: (cb) => subscribe<[ViewerClosedInfo]>(CH.viewerClosed, cb)
  },
  multistream: {
    status: () => ipcRenderer.invoke(CH.multistreamStatus) as Promise<MultistreamStatus>,
    start: (fps: number) =>
      ipcRenderer.invoke(CH.multistreamStart, fps) as Promise<{
        ok: boolean
        status: MultistreamStatus
        error?: string
      }>,
    pushChunk: (orientation: CanvasOrientation, chunk: Uint8Array) =>
      ipcRenderer.send(CH.multistreamPushChunk, orientation, chunk),
    stop: () => ipcRenderer.invoke(CH.multistreamStop) as Promise<MultistreamStatus>
  },
  recorder: {
    status: () => ipcRenderer.invoke(CH.recorderStatus) as Promise<RecorderStatus>,
    start: () =>
      ipcRenderer.invoke(CH.recorderStart) as Promise<{
        ok: boolean
        status?: RecorderStatus
        error?: string
      }>,
    stop: () => ipcRenderer.invoke(CH.recorderStop) as Promise<RecorderStatus>,
    setReplayBuffer: (enabled: boolean, seconds: number) =>
      ipcRenderer.invoke(CH.recorderSetReplay, enabled, seconds) as Promise<RecorderStatus>,
    pushChunk: (chunk: Uint8Array) => ipcRenderer.send(CH.recorderPushChunk, chunk),
    resetStream: () => ipcRenderer.send(CH.recorderReset),
    saveReplay: () =>
      ipcRenderer.invoke(CH.recorderSaveReplay) as Promise<{
        ok: boolean
        path?: string
        status?: RecorderStatus
        error?: string
      }>
  },
  net: {
    interfaces: () => ipcRenderer.invoke(CH.netInterfaces) as Promise<NetInterface[]>
  },
  firewall: {
    status: () => ipcRenderer.invoke(CH.firewallStatus) as Promise<FirewallStatus>,
    authorize: () =>
      ipcRenderer.invoke(CH.firewallAuthorize) as Promise<{ ok: boolean; error?: string }>
  },
  settings: {
    get: () => ipcRenderer.invoke(CH.settingsGet) as Promise<AppSettings>,
    set: (patch) => ipcRenderer.invoke(CH.settingsSet, patch) as Promise<AppSettings>,
    onChanged: (cb) => subscribe<[AppSettings]>(CH.settingsChanged, cb)
  }
}

contextBridge.exposeInMainWorld('api', api)
