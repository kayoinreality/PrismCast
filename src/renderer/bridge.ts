import type {
  Api,
  AppInfo,
  AppSettings,
  DesktopSource,
  FirewallStatus,
  HostStartOptions,
  HostStartResult,
  NetInterface,
  OkResult,
  Unsubscribe,
  ViewerClosedInfo,
  ViewerConnectOptions,
  ViewerConnectResult
} from '@shared/ipc'
import type {
  AudioApp,
  AudioChannelStatus,
  AudioLevelSample,
  BrowserSourceFrame,
  BrowserSourceSpec,
  CanvasOrientation,
  ChatMessage,
  ConnectedViewer,
  DiscoveredHost,
  HostToViewer,
  HotkeyAction,
  HotkeyBindings,
  HotkeyConflict,
  KitConfig,
  LiveInfo,
  MultiChatEvent,
  MultiChatStatus,
  MultistreamStatus,
  ReactionEvent,
  RecorderStatus,
  SourceKind,
  TelemetryStats,
  TwitchActionResult,
  TwitchCapabilities,
  TwitchCategory,
  TwitchLoginState,
  ViewerToHost,
  WidgetKit
} from '@shared/protocol'

interface TauriInternals {
  invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T>
  transformCallback(cb: (payload: unknown) => void, once?: boolean): number
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: TauriInternals
    __PRISMCAST_RPC__?: { url?: string; token?: string }
  }
}

type EventCallback = (payload: unknown) => void

/**
 * Cliente RPC WebSocket autenticado com suporte a frames binários (stream 0x01/0x02/0x03)
 * para fallback quando rodando fora do Tauri v2 / preload do Electron.
 */
class RpcWsBridge {
  private ws: WebSocket | null = null
  private nextId = 1
  private pending = new Map<string, { resolve: (v: unknown) => void; reject: (e: Error) => void }>()
  private listeners = new Map<string, Set<EventCallback>>()
  private connecting: Promise<void> | null = null

  private resolveTarget(): { url: string; token: string } {
    const cfg = window.__PRISMCAST_RPC__
    const url = cfg?.url ?? 'ws://127.0.0.1:41236'
    const token = cfg?.token ?? ''
    return { url, token }
  }

  private async ensureConnected(): Promise<void> {
    if (this.ws?.readyState === WebSocket.OPEN) return
    if (this.connecting) return this.connecting

    this.connecting = new Promise<void>((resolve, reject) => {
      const { url, token } = this.resolveTarget()
      const ws = new WebSocket(token ? `${url}?token=${encodeURIComponent(token)}` : url)
      ws.binaryType = 'arraybuffer'
      this.ws = ws

      ws.onopen = () => {
        this.connecting = null
        resolve()
      }
      ws.onerror = () => {
        this.connecting = null
        reject(new Error('Falha ao conectar ao motor PrismCast Rust'))
      }
      ws.onclose = () => {
        this.connecting = null
        if (this.ws === ws) this.ws = null
        for (const p of this.pending.values()) {
          p.reject(new Error('Conexão com motor encerrada'))
        }
        this.pending.clear()
      }
      ws.onmessage = (ev) => {
        if (typeof ev.data !== 'string') return
        try {
          const msg = JSON.parse(ev.data) as {
            id?: string
            result?: unknown
            error?: string
            event?: string
            payload?: unknown
          }
          if (msg.id && this.pending.has(msg.id)) {
            const req = this.pending.get(msg.id)!
            this.pending.delete(msg.id)
            if (msg.error) req.reject(new Error(msg.error))
            else req.resolve(msg.result)
            return
          }
          if (msg.event) {
            const set = this.listeners.get(msg.event)
            if (set) {
              for (const cb of set) cb(msg.payload)
            }
          }
        } catch {
          /* ignore */
        }
      }
    })
    return this.connecting
  }

  async invoke<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    await this.ensureConnected()
    const id = String(this.nextId++)
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, {
        resolve: resolve as (v: unknown) => void,
        reject
      })
      this.ws?.send(JSON.stringify({ id, method, params }))
    })
  }

  sendBinary(streamKind: number, channelIdx: number, payload: Uint8Array): void {
    if (this.ws?.readyState !== WebSocket.OPEN) return
    const frame = new Uint8Array(8 + payload.byteLength)
    // Header binário compacto: [magic:2 "PC", kind:1, channel:1, len:4 LE]
    frame[0] = 0x50
    frame[1] = 0x43
    frame[2] = streamKind & 0xff
    frame[3] = channelIdx & 0xff
    new DataView(frame.buffer).setUint32(4, payload.byteLength, true)
    frame.set(payload, 8)
    this.ws.send(frame.buffer)
  }

  subscribe(event: string, cb: EventCallback): Unsubscribe {
    let set = this.listeners.get(event)
    if (!set) {
      set = new Set()
      this.listeners.set(event, set)
    }
    set.add(cb)
    void this.ensureConnected().catch(() => {})
    return () => {
      set?.delete(cb)
    }
  }
}

const rpcBridge = new RpcWsBridge()
const tauriListeners = new Map<string, Set<EventCallback>>()
const tauriHooked = new Set<string>()

async function invokeBackend<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  const tauri = window.__TAURI_INTERNALS__
  if (tauri) {
    return tauri.invoke<T>(cmd, args)
  }
  return rpcBridge.invoke<T>(cmd, args)
}

function listenBackend<T>(event: string, cb: (payload: T) => void): Unsubscribe {
  const tauri = window.__TAURI_INTERNALS__
  if (tauri) {
    let set = tauriListeners.get(event)
    if (!set) {
      set = new Set()
      tauriListeners.set(event, set)
    }
    const wrapped: EventCallback = (p) => cb(p as T)
    set.add(wrapped)

    if (!tauriHooked.has(event)) {
      tauriHooked.add(event)
      const handlerId = tauri.transformCallback((raw: unknown) => {
        const evt = raw as { payload?: unknown }
        const payload = evt && typeof evt === 'object' && 'payload' in evt ? evt.payload : raw
        const cur = tauriListeners.get(event)
        if (cur) {
          for (const fn of cur) fn(payload)
        }
      })
      void tauri.invoke('plugin:event|listen', {
        event,
        target: { kind: 'Any' },
        handler: handlerId
      })
    }

    return () => {
      set?.delete(wrapped)
    }
  }
  return rpcBridge.subscribe(event, (p) => cb(p as T))
}

/** Cria uma implementação completa da interface `Api` apontando para Tauri v2 ou RPC Rust. */
export function createRustBridgeApi(): Api {
  return {
    app: {
      info: () => invokeBackend<AppInfo>('app_info'),
      minimize: () => void invokeBackend('app_minimize'),
      toggleMaximize: () => void invokeBackend('app_toggle_maximize'),
      close: () => void invokeBackend('app_close'),
      setAlwaysOnTop: (value) => invokeBackend<boolean>('app_always_on_top', { value }),
      onMaximizeChange: (cb) => listenBackend<boolean>('app:maximize-changed', cb),
      onHotkey: (cb) => listenBackend<HotkeyAction>('hotkey:triggered', cb),
      openExternal: (url) => void invokeBackend('app_open_external', { url }),
      showInFolder: (path) => void invokeBackend('app_show_in_folder', { path })
    },
    sources: {
      list: () => invokeBackend<DesktopSource[]>('sources_list'),
      select: (sourceId, withAudio) => invokeBackend<void>('sources_select', { sourceId, withAudio })
    },
    destinations: {
      setKey: (id, key) => invokeBackend<OkResult>('dest_set_key', { id, key }),
      twitchLogin: (id) => invokeBackend<void>('dest_twitch_login', { id }),
      twitchCancel: () => void invokeBackend('dest_twitch_cancel'),
      onTwitchLogin: (cb) => listenBackend<TwitchLoginState>('dest:twitch-state', cb),
      unlink: (id) => invokeBackend<void>('dest_unlink', { id })
    },
    browserSources: {
      sync: (specs: BrowserSourceSpec[]) => void invokeBackend('bs_sync', { specs }),
      reload: (id: string) => void invokeBackend('bs_reload', { id }),
      onFrame: (cb) => listenBackend<BrowserSourceFrame>('bs:frame', cb),
      ack: (id: string) => void invokeBackend('bs_ack', { id })
    },
    kit: {
      load: (path) => invokeBackend<WidgetKit | null>('kit_load', { path }),
      pickFolder: () => invokeBackend<string | null>('kit_pick_folder'),
      readConfig: (path) => invokeBackend<KitConfig | null>('kit_read_config', { path }),
      writeConfig: (path, config) => invokeBackend<OkResult>('kit_write_config', { path, config })
    },
    audio: {
      listApps: () => invokeBackend<AudioApp[]>('audio_list_apps'),
      appForWindow: (captureId) => invokeBackend<AudioApp | null>('audio_app_for_window', { captureId }),
      sync: (channels) => void invokeBackend('audio_sync', { channels }),
      onPcm: (cb) =>
        listenBackend<{ id: string; pcm: number[] }>('audio:pcm', (p) => cb(p.id, new Float32Array(p.pcm))),
      onStatus: (cb) => listenBackend<AudioChannelStatus[]>('audio:status', cb),
      onLevels: (cb) => listenBackend<AudioLevelSample[]>('audio:levels', cb)
    },
    telemetry: {
      get: () => invokeBackend<TelemetryStats>('telemetry_get'),
      onStats: (cb) => listenBackend<TelemetryStats>('telemetry:stats', cb)
    },
    hotkeys: {
      conflicts: (bindings: HotkeyBindings) =>
        invokeBackend<HotkeyConflict[]>('hotkeys_conflicts', { bindings })
    },
    twitch: {
      capabilities: () => invokeBackend<TwitchCapabilities>('twitch_capabilities'),
      getChannelInfo: () => invokeBackend<LiveInfo | null>('twitch_get_info'),
      updateChannelInfo: (info) => invokeBackend<TwitchActionResult>('twitch_update_info', { info }),
      searchCategories: (query) => invokeBackend<TwitchCategory[]>('twitch_search_categories', { query }),
      sendChat: (text) => invokeBackend<TwitchActionResult>('twitch_send_chat', { text })
    },
    chat: {
      openPopout: () => void invokeBackend('chat_open_popout'),
      closePopout: () => void invokeBackend('chat_close_popout'),
      isPopoutOpen: () => invokeBackend<boolean>('chat_is_popout_open'),
      onPopoutChange: (cb) => listenBackend<boolean>('chat:popout-changed', cb),
      onMultichat: (cb) => listenBackend<MultiChatEvent>('multichat:event', cb),
      multichatStatus: () => invokeBackend<MultiChatStatus[]>('multichat_status'),
      reconnectMultichat: () => void invokeBackend('multichat_reconnect')
    },
    files: {
      pickImage: () => invokeBackend<string | null>('files_pick_image'),
      pickHtml: () => invokeBackend<string | null>('files_pick_html'),
      readImage: (path) => invokeBackend<string | null>('files_read_image', { path })
    },
    host: {
      start: (opts: HostStartOptions) => invokeBackend<HostStartResult>('host_start', { opts }),
      stop: () => invokeBackend<void>('host_stop'),
      kick: (viewerId: string) => invokeBackend<void>('host_kick', { viewerId }),
      send: (viewerId: string, msg: HostToViewer) => void invokeBackend('host_send', { viewerId, msg }),
      broadcast: (msg: HostToViewer) => void invokeBackend('host_broadcast', { msg }),
      updateSource: (sourceName: string, sourceKind: SourceKind) =>
        void invokeBackend('host_update_source', { sourceName, sourceKind }),
      setViewerCount: (n: number) => void invokeBackend('host_set_viewer_count', { n }),
      onViewers: (cb) => listenBackend<ConnectedViewer[]>('host:viewers', cb),
      onMessage: (cb) =>
        listenBackend<{ viewerId: string; msg: ViewerToHost }>('host:message', (p) => cb(p.viewerId, p.msg)),
      onChat: (cb) => listenBackend<ChatMessage>('host:chat', cb),
      onReaction: (cb) => listenBackend<ReactionEvent>('host:reaction', cb)
    },
    discovery: {
      start: () => invokeBackend<void>('discovery_start'),
      stop: () => invokeBackend<void>('discovery_stop'),
      refresh: () => void invokeBackend('discovery_refresh'),
      onHosts: (cb) => listenBackend<DiscoveredHost[]>('discovery:hosts', cb)
    },
    viewer: {
      connect: (opts: ViewerConnectOptions) => invokeBackend<ViewerConnectResult>('viewer_connect', { opts }),
      send: (msg: ViewerToHost) => void invokeBackend('viewer_send', { msg }),
      disconnect: () => void invokeBackend('viewer_disconnect'),
      onMessage: (cb) => listenBackend<HostToViewer>('viewer:message', cb),
      onClosed: (cb) => listenBackend<ViewerClosedInfo>('viewer:closed', cb)
    },
    multistream: {
      status: () => invokeBackend<MultistreamStatus>('multistream_status'),
      start: (fps: number) =>
        invokeBackend<{ ok: boolean; status: MultistreamStatus; error?: string }>('multistream_start', { fps }),
      pushChunk: (orientation: CanvasOrientation, chunk: Uint8Array) => {
        if (window.__TAURI_INTERNALS__) {
          void invokeBackend('multistream_push_chunk', { orientation, chunk: Array.from(chunk) })
        } else {
          rpcBridge.sendBinary(1, orientation === 'vertical' ? 1 : 0, chunk)
        }
      },
      stop: () => invokeBackend<MultistreamStatus>('multistream_stop')
    },
    recorder: {
      status: () => invokeBackend<RecorderStatus>('recorder_status'),
      start: () => invokeBackend<{ ok: boolean; status?: RecorderStatus; error?: string }>('recorder_start'),
      stop: () => invokeBackend<RecorderStatus>('recorder_stop'),
      setReplayBuffer: (enabled: boolean, seconds: number) =>
        invokeBackend<RecorderStatus>('recorder_set_replay', { enabled, seconds }),
      pushChunk: (chunk: Uint8Array) => {
        if (window.__TAURI_INTERNALS__) {
          void invokeBackend('recorder_push_chunk', { chunk: Array.from(chunk) })
        } else {
          rpcBridge.sendBinary(2, 0, chunk)
        }
      },
      resetStream: () => void invokeBackend('recorder_reset'),
      saveReplay: () =>
        invokeBackend<{ ok: boolean; path?: string; status?: RecorderStatus; error?: string }>(
          'recorder_save_replay'
        )
    },
    net: {
      interfaces: () => invokeBackend<NetInterface[]>('net_interfaces')
    },
    firewall: {
      status: () => invokeBackend<FirewallStatus>('firewall_status'),
      authorize: () => invokeBackend<{ ok: boolean; error?: string }>('firewall_authorize')
    },
    settings: {
      get: () => invokeBackend<AppSettings>('settings_get'),
      set: (patch: Partial<AppSettings>) => invokeBackend<AppSettings>('settings_set', { patch }),
      onChanged: (cb) => listenBackend<AppSettings>('settings:changed', cb)
    }
  }
}

/**
 * Garante que `window.api` exista antes de montar o React:
 * - No Electron preload já vem preenchido (no-op).
 * - No Tauri v2 ou browser dev conecta automaticamente ao motor Rust.
 */
export function ensureBridge(): Api {
  if (!window.api) {
    window.api = createRustBridgeApi()
  }
  return window.api
}
