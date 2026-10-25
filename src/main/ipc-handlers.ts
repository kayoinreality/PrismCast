import { randomInt, randomUUID } from 'node:crypto'
import { app, globalShortcut, ipcMain } from 'electron'
import { CH } from '@shared/ipc'
import type {
  AppInfo,
  AppSettings,
  HostStartOptions,
  HostStartResult,
  ViewerConnectOptions
} from '@shared/ipc'
import {
  MAX_VIEWERS,
  SIGNALING_PORT,
  type CanvasOrientation,
  type HostToViewer,
  type HotkeyAction,
  type MultistreamTarget,
  type SourceKind,
  type ViewerToHost
} from '@shared/protocol'
import { hostname } from 'node:os'
import { Discovery } from './discovery'
import { SignalingClient, SignalingServer } from './signaling'
import { armSource, disarmSource, listSources } from './capture'
import { findFreePort, listInterfaces, localAddress } from './net'
import { authorize as firewallAuthorize, status as firewallStatus } from './firewall'
import { getSettings, setSettings } from './store'
import { getMainWindow } from './window'
import { MultistreamManager } from './multistream'
import { RecorderManager } from './recorder'

const discovery = new Discovery()
const server = new SignalingServer()
const client = new SignalingClient()
const multistream = new MultistreamManager()
const recorder = new RecorderManager()

function toRenderer(channel: string, ...args: unknown[]): void {
  const win = getMainWindow()
  if (win && !win.isDestroyed()) win.webContents.send(channel, ...args)
}

function makePin(): string {
  return String(randomInt(0, 1_000_000)).padStart(6, '0')
}

discovery.onHosts = (hosts) => toRenderer(CH.discoveryHosts, hosts)

server.onViewers = (viewers) => {
  discovery.patchAnnounce({ viewers: viewers.length })
  toRenderer(CH.hostViewers, viewers)
}
server.onMessage = (viewerId, msg) => toRenderer(CH.hostMessage, viewerId, msg)
server.onChat = (msg) => toRenderer(CH.hostChat, msg)
server.onReaction = (reaction) => toRenderer(CH.hostReaction, reaction)

client.onMessage = (msg) => toRenderer(CH.viewerMessage, msg)
client.onClosed = (info) => toRenderer(CH.viewerClosed, info)

export function registerGlobalHotkeys(): void {
  try {
    globalShortcut.unregisterAll()
    const bindings = getSettings().hotkeys
    const entries: Array<[string, HotkeyAction]> = [
      [bindings.toggleMicMute, 'toggle-mic-mute'],
      [bindings.toggleSystemMute, 'toggle-system-mute'],
      [bindings.togglePrivacyShield, 'toggle-privacy-shield'],
      [bindings.saveReplayClip, 'save-replay-clip'],
      [bindings.toggleVerticalPreview, 'toggle-vertical-preview']
    ]
    for (const [accel, action] of entries) {
      if (!accel) continue
      const normalized = accel.replace(/^Ctrl\+/i, 'CommandOrControl+')
      try {
        globalShortcut.register(normalized, () => {
          toRenderer(CH.appHotkey, action)
        })
      } catch {
        /* acelerador invalido ou ocupado */
      }
    }
  } catch {
    /* chamado antes de app.whenReady */
  }
}

async function stopHosting(): Promise<void> {
  await discovery.stopAnnouncing()
  await server.stop()
  multistream.stop()
  recorder.resetStream()
  disarmSource()
}

export function registerIpc(): void {
  // ------------------------------------------------------------------- app
  ipcMain.handle(CH.appInfo, (): AppInfo => {
    return {
      name: 'PrismCast Studio',
      version: app.getVersion(),
      hostname: hostname(),
      isPackaged: app.isPackaged,
      rustEngineActive: true,
      hardwareEncoder: multistream.detectedEncoder
    }
  })

  ipcMain.on(CH.appMinimize, () => getMainWindow()?.minimize())
  ipcMain.on(CH.appToggleMaximize, () => {
    const win = getMainWindow()
    if (!win) return
    win.isMaximized() ? win.unmaximize() : win.maximize()
  })
  ipcMain.on(CH.appClose, () => getMainWindow()?.close())
  ipcMain.handle(CH.appAlwaysOnTop, (_e, value: boolean) => {
    const win = getMainWindow()
    if (!win) return false
    win.setAlwaysOnTop(value)
    return win.isAlwaysOnTop()
  })

  // --------------------------------------------------------------- fontes
  ipcMain.handle(CH.sourcesList, () => listSources())
  ipcMain.handle(CH.sourcesSelect, (_e, sourceId: string, withAudio: boolean) => {
    armSource(sourceId, withAudio)
  })

  // ------------------------------------------------------------------ host
  ipcMain.handle(CH.hostStart, async (_e, opts: HostStartOptions): Promise<HostStartResult> => {
    try {
      await stopHosting()

      const settings = getSettings()
      const pin = opts.requirePin ? makePin() : null
      const port = await findFreePort(SIGNALING_PORT)
      const ip = await localAddress(settings.preferredInterface)
      const sessionId = randomUUID()
      const maxViewers = Math.min(Math.max(1, opts.maxViewers), MAX_VIEWERS)

      await server.start(port, {
        hostName: opts.hostName,
        sourceName: opts.sourceName,
        sourceKind: opts.sourceKind,
        pin,
        maxViewers
      })

      recorder.setReplayBuffer(true, settings.studio.replayBufferSeconds)

      discovery.preferredInterface = settings.preferredInterface
      await discovery.startAnnouncing({
        id: sessionId,
        name: opts.hostName,
        ip,
        port,
        requiresPin: pin !== null,
        sourceName: opts.sourceName,
        sourceKind: opts.sourceKind,
        viewers: 0,
        maxViewers,
        startedAt: Date.now()
      })

      return { ok: true, sessionId, ip, port, pin }
    } catch (err) {
      await stopHosting()
      return { ok: false, error: (err as Error).message }
    }
  })

  ipcMain.handle(CH.hostStop, () => stopHosting())

  ipcMain.handle(CH.hostKick, (_e, viewerId: string) => {
    server.kick(viewerId)
  })

  ipcMain.on(CH.hostSend, (_e, viewerId: string, msg: HostToViewer) => {
    server.send(viewerId, msg)
  })

  ipcMain.on(CH.hostBroadcast, (_e, msg: HostToViewer) => {
    server.broadcast(msg)
  })

  ipcMain.on(CH.hostUpdateSource, (_e, sourceName: string, sourceKind: SourceKind) => {
    server.updateSession({ sourceName, sourceKind })
    discovery.patchAnnounce({ sourceName, sourceKind })
  })

  ipcMain.on(CH.hostSetViewerCount, (_e, n: number) => {
    discovery.patchAnnounce({ viewers: n })
  })

  // ------------------------------------------------------------ descoberta
  ipcMain.handle(CH.discoveryStart, async () => {
    discovery.preferredInterface = getSettings().preferredInterface
    await discovery.startBrowsing()
  })
  ipcMain.handle(CH.discoveryStop, () => {
    discovery.stopBrowsing()
  })
  ipcMain.on(CH.discoveryRefresh, () => discovery.refresh())

  // ------------------------------------------------------------ espectador
  ipcMain.handle(CH.viewerConnect, (_e, opts: ViewerConnectOptions) => client.connect(opts))
  ipcMain.on(CH.viewerSend, (_e, msg: ViewerToHost) => client.send(msg))
  ipcMain.on(CH.viewerDisconnect, () => client.disconnect())

  // ----------------------------------------------------------- multistream
  ipcMain.handle(CH.multistreamStatus, () => multistream.status())
  ipcMain.handle(
    CH.multistreamStart,
    (_e, targets: MultistreamTarget[], fps: number) => multistream.start(targets, fps)
  )
  ipcMain.on(
    CH.multistreamPushChunk,
    (_e, orientation: CanvasOrientation, chunk: Uint8Array) => {
      multistream.pushChunk(orientation, chunk)
    }
  )
  ipcMain.handle(CH.multistreamStop, () => multistream.stop())

  // --------------------------------------------- gravador & replay buffer
  ipcMain.handle(CH.recorderStatus, () => recorder.status())
  ipcMain.handle(CH.recorderStart, () => recorder.startRecording())
  ipcMain.handle(CH.recorderStop, () => recorder.stopRecording())
  ipcMain.handle(CH.recorderSetReplay, (_e, enabled: boolean, seconds: number) =>
    recorder.setReplayBuffer(enabled, seconds)
  )
  ipcMain.on(CH.recorderPushChunk, (_e, chunk: Uint8Array) => {
    recorder.pushChunk(chunk)
  })
  ipcMain.handle(CH.recorderSaveReplay, () => recorder.saveReplay())

  // ------------------------------------------------------------------ rede
  ipcMain.handle(CH.netInterfaces, () => listInterfaces())

  // -------------------------------------------------------------- firewall
  ipcMain.handle(CH.firewallStatus, () => firewallStatus())
  ipcMain.handle(CH.firewallAuthorize, () => firewallAuthorize())

  // --------------------------------------------------------- preferencias
  ipcMain.handle(CH.settingsGet, () => getSettings())
  ipcMain.handle(CH.settingsSet, (_e, patch: Partial<AppSettings>) => {
    const next = setSettings(patch)
    discovery.preferredInterface = next.preferredInterface
    registerGlobalHotkeys()
    return next
  })
}

export async function shutdown(): Promise<void> {
  try {
    globalShortcut.unregisterAll()
  } catch {
    /* ignorar */
  }
  client.disconnect()
  await stopHosting()
  await discovery.dispose()
}
