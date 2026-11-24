import type { AppSettings } from '@shared/ipc'
import type {
  AudioChannelStatus,
  BrowserSourceSpec,
  LiveInfo,
  MultistreamStatus,
  RecorderStatus,
  Scene,
  SceneItem,
  TelemetryStats,
  VirtualCameraStatus,
  WidgetKit
} from '@shared/protocol'
import { ensureBridge } from '@/bridge'
import { AudioMixerEngine, type AudioLevels } from './AudioMixerEngine'
import { BrowserSourceCanvases } from './BrowserSourceCanvases'
import { CaptureManager, type Capture } from './CaptureManager'
import { Compositor } from './Compositor'
import { OutputManager } from './OutputManager'
import { DEFAULT_VIRTUAL_CAM_STATUS, studioStore } from './StudioStore'
import { effectiveUrl, hiddenWithoutCamera, restartsOnShow } from './widgets'
import { sceneMics } from './scenes'

export interface EngineState {
  live: boolean
  liveStartedAt: number | null
  goingLive: boolean
  multistream: MultistreamStatus
  recorder: RecorderStatus
  telemetry: TelemetryStats
  virtualCamStatus: VirtualCameraStatus
  captureVersion: number
  audioVersion: number
}

const IDLE_RECORDER: RecorderStatus = {
  recording: false,
  recordingPath: null,
  recordingDurationSec: 0,
  replayBufferActive: true,
  replayBufferedSeconds: 0,
  replaySavedPath: null
}

const IDLE_TELEMETRY: TelemetryStats = {
  compositorFps: 60,
  targetFps: 60,
  renderMsP95: 0,
  droppedFrames: 0,
  encodeBitrateKbps: 0,
  encoder: 'libx264',
  cpuFallback: false,
  micMutedSpeaking: false,
  replayBufferedSeconds: 0
}

export function browserSourceId(item: SceneItem, url = item.settings.url ?? ''): string {
  const s = item.settings
  return `${s.width ?? 1920}x${s.height ?? 1080}@${s.fps ?? 30}|${url}`
}

export function captureKeyOf(item: SceneItem): string | null {
  if (item.type === 'screen_capture' || item.type === 'window_capture') {
    return CaptureManager.displayKey(item.settings.captureId, item.settings.captureName)
  }
  if (item.type === 'camera_input') return CaptureManager.cameraKey(item.settings.deviceId)
  return null
}

export class StudioEngine {
  readonly captures = new CaptureManager()
  readonly browsers = new BrowserSourceCanvases()
  readonly mixer: AudioMixerEngine
  readonly compositor: Compositor
  private readonly outputs: OutputManager
  private readonly images = new Map<string, HTMLImageElement>()

  private settings: AppSettings
  private listeners = new Set<() => void>()
  private statusTimer: number
  private vcamPumpTimer: number | null = null
  private vcamPumpFps = 0
  private offVirtualCamStatus: (() => void) | null = null
  private transitionKeepAliveTimer: number | null = null
  private lastBrowserSpecs = ''
  private systemAudioRequested = false
  private kit: WidgetKit | null = null
  private lastProgramId: string | null = null
  private liveVertical = false
  private liveOpGen = 0
  private lastStatusSignature = ''

  private unlockAudio = (): void => {
    void this.mixer.resume()
  }
  private onBeforeUnload = (): void => {
    this.dispose()
  }

  state: EngineState = {
    live: false,
    liveStartedAt: null,
    goingLive: false,
    multistream: { active: false, encoder: '…', ffmpegFound: true, destinations: [] },
    recorder: IDLE_RECORDER,
    telemetry: IDLE_TELEMETRY,
    virtualCamStatus: { ...DEFAULT_VIRTUAL_CAM_STATUS },
    captureVersion: 0,
    audioVersion: 0
  }

  constructor(settings: AppSettings) {
    const api = ensureBridge()
    this.settings = settings
    this.mixer = new AudioMixerEngine(settings.audioMixer)
    this.compositor = new Compositor(
      settings.studio,
      (item) => this.resolve(item),
      (item) => !this.settings.studio.cameraEnabled && hiddenWithoutCamera(item, this.kit)
    )
    this.outputs = new OutputManager(
      () => new MediaStream([this.compositor.programStream.getVideoTracks()[0], this.mixer.outputTrack]),
      () =>
        new MediaStream([this.compositor.verticalStream.getVideoTracks()[0], this.mixer.outputTrack])
    )
    window.addEventListener('pointerdown', this.unlockAudio, { passive: true })
    window.addEventListener('keydown', this.unlockAudio, { passive: true })
    window.addEventListener('beforeunload', this.onBeforeUnload)
    this.captures.onChange = () => this.patch({ captureVersion: this.state.captureVersion + 1 })
    this.mixer.onStatus = () => this.patch({ audioVersion: this.state.audioVersion + 1 })
    this.captures.setFps(settings.studio.fps)
    this.statusTimer = window.setInterval(() => void this.pollStatus(), 1000)
    void api.recorder.setReplayBuffer(true, settings.studio.replayBufferSeconds)

    if (api.virtualCam) {
      void api.virtualCam
        .status()
        .then((st) => this.applyVirtualCamStatus(st))
        .catch(() => {})
      this.offVirtualCamStatus = api.virtualCam.onStatus((st) => {
        this.applyVirtualCamStatus(st)
      })
    }

    this.apply(settings)
  }

  subscribe = (cb: () => void): (() => void) => {
    this.listeners.add(cb)
    return () => this.listeners.delete(cb)
  }

  getState = (): EngineState => this.state

  private patch(p: Partial<EngineState>): void {
    this.state = { ...this.state, ...p }
    for (const cb of this.listeners) cb()
  }

  readLevels(): AudioLevels {
    return this.mixer.readLevels()
  }

  capture(item: SceneItem): Capture | null {
    const key = captureKeyOf(item)
    return key ? this.captures.get(key) : null
  }

  retryCapture(item: SceneItem): void {
    const key = captureKeyOf(item)
    if (!key) return
    this.captures.retry(key)
    this.syncMedia()
  }

  browserIdOf(item: SceneItem): string {
    return browserSourceId(item, effectiveUrl(item, this.kit, this.settings.studio.cameraEnabled))
  }

  reloadBrowserSource(item: SceneItem): void {
    ensureBridge().browserSources.reload(this.browserIdOf(item))
  }

  reloadKitWidgets(): void {
    const st = this.settings.scenes
    for (const scene of st.scenes) {
      for (const item of scene.items) {
        if (item.type === 'browser_source' && item.settings.kitWidget) this.reloadBrowserSource(item)
      }
    }
  }

  audioStatus(id: string): AudioChannelStatus {
    return this.mixer.status(id)
  }

  setKit(kit: WidgetKit | null): void {
    this.kit = kit
    this.lastBrowserSpecs = ''
    this.syncMedia()
  }

  get currentKit(): WidgetKit | null {
    return this.kit
  }

  private programScene(): Scene | null {
    const st = this.settings.scenes
    return st.scenes.find((s) => s.id === st.activeSceneId) ?? st.scenes[0] ?? null
  }

  private previewScene(): Scene | null {
    const st = this.settings.scenes
    if (!st.studioMode) return null
    return st.scenes.find((s) => s.id === st.previewSceneId) ?? null
  }

  apply(settings: AppSettings): void {
    const prev = this.settings
    this.settings = settings
    if (prev.studio.fps !== settings.studio.fps) this.captures.setFps(settings.studio.fps)
    if (prev.studio.replayBufferSeconds !== settings.studio.replayBufferSeconds) {
      void ensureBridge().recorder.setReplayBuffer(
        this.state.recorder.replayBufferActive,
        settings.studio.replayBufferSeconds
      )
    }

    // Preserva o canvas 9:16 ativo enquanto uma live vertical ou câmera virtual vertical estiver no ar.
    const vcamVertical =
      this.state.virtualCamStatus.active && this.state.virtualCamStatus.orientation === 'vertical'
    this.compositor.setStudio({
      ...settings.studio,
      dualOutputEnabled:
        settings.studio.dualOutputEnabled || (this.state.live && this.liveVertical) || vcamVertical
    })

    const prevProgId = prev.scenes.activeSceneId
    const nextProg = this.programScene()
    const effOverride = nextProg?.transitionOverride ?? null
    const effType = effOverride?.transitionType ?? settings.scenes.transitionType
    const effMs = effOverride?.durationMs ?? settings.scenes.transitionDurationMs
    const effCustom = effOverride?.custom ?? settings.scenes.customTransition

    this.compositor.setScenes(nextProg, this.previewScene(), {
      type: settings.scenes.transitionType,
      ms: settings.scenes.transitionDurationMs,
      custom: settings.scenes.customTransition
    })

    if (
      prevProgId !== settings.scenes.activeSceneId &&
      effType !== 'cut' &&
      this.compositor.getOutgoingScene()
    ) {
      const holdMs =
        effType === 'stinger'
          ? Math.max(effMs, (effCustom?.stingerTransitionPointMs ?? 500) + 250)
          : effMs
      if (this.transitionKeepAliveTimer !== null) window.clearTimeout(this.transitionKeepAliveTimer)
      this.transitionKeepAliveTimer = window.setTimeout(() => {
        this.transitionKeepAliveTimer = null
        this.syncMedia()
      }, holdMs + 100)
    }

    const used = new Set(settings.scenes.scenes.flatMap((s) => [...sceneMics(s, false)]))
    this.mixer.setSceneMics(sceneMics(nextProg), used)
    void this.mixer.applyConfig(settings.audioMixer)

    const wantsDesktop = settings.audioMixer.channels.some((c) => c.kind === 'desktop')
    if (wantsDesktop && !this.systemAudioRequested) {
      this.systemAudioRequested = true
      void this.captures.acquireSystemAudio().then((s) => {
        if (s) this.mixer.setSystemStream(s)
        else this.systemAudioRequested = false
      })
    }
    this.syncVirtualCamPump()
    this.syncMedia()
    this.restartOnShow()
  }

  private restartOnShow(): void {
    const program = this.programScene()
    const id = program?.id ?? null
    if (id === this.lastProgramId) return
    const first = this.lastProgramId === null
    this.lastProgramId = id
    if (first || !program) return
    for (const item of program.items) {
      if (item.visible && restartsOnShow(item, this.kit)) this.reloadBrowserSource(item)
    }
  }

  private syncMedia(): void {
    const scenes = [
      this.programScene(),
      this.previewScene(),
      this.compositor.getOutgoingScene()
    ].filter((s): s is Scene => s !== null)
    const items = scenes.flatMap((s) => s.items)

    const wanted = new Map<string, () => Promise<{ stream: MediaStream; label: string } | null>>()
    const specs = new Map<string, BrowserSourceSpec>()
    const activeImages = new Set<string>()

    for (const item of items) {
      const s = item.settings
      if (item.type === 'screen_capture' || item.type === 'window_capture') {
        wanted.set(captureKeyOf(item)!, this.captures.openDisplay(s.captureId, s.captureName))
      } else if (item.type === 'camera_input') {
        if (this.settings.studio.cameraEnabled) {
          wanted.set(captureKeyOf(item)!, this.captures.openCamera(s.deviceId))
        }
      } else if (item.type === 'browser_source' && s.url) {
        if (!this.settings.studio.cameraEnabled && hiddenWithoutCamera(item, this.kit)) continue
        const url = effectiveUrl(item, this.kit, this.settings.studio.cameraEnabled)
        const id = browserSourceId(item, url)
        specs.set(id, { id, url, width: s.width ?? 1920, height: s.height ?? 1080, fps: s.fps ?? 30 })
      } else if (item.type === 'image_source' && s.path) {
        activeImages.add(s.path)
        if (!this.images.has(s.path)) this.loadImage(s.path)
      }
    }

    this.captures.sync(wanted)
    this.browsers.prune(new Set(specs.keys()))

    // Libera imagens que não pertencem mais às cenas ativas/prévia
    for (const path of this.images.keys()) {
      if (!activeImages.has(path)) {
        this.images.delete(path)
      }
    }

    const list = [...specs.values()]
    const key = JSON.stringify(list)
    if (key !== this.lastBrowserSpecs) {
      this.lastBrowserSpecs = key
      ensureBridge().browserSources.sync(list)
    }
  }

  private loadImage(path: string): void {
    const img = new Image()
    this.images.set(path, img)
    void ensureBridge()
      .files.readImage(path)
      .then((url) => {
        if (url && this.images.get(path) === img) img.src = url
      })
      .catch(() => {})
  }

  private resolve(item: SceneItem): CanvasImageSource | null {
    switch (item.type) {
      case 'screen_capture':
      case 'window_capture':
      case 'camera_input': {
        const cap = this.capture(item)
        return cap && cap.state === 'live' && cap.video.readyState >= 2 ? cap.video : null
      }
      case 'browser_source':
        return item.settings.url ? this.browsers.get(this.browserIdOf(item)) : null
      case 'image_source': {
        const img = item.settings.path ? this.images.get(item.settings.path) : undefined
        return img && img.complete && img.naturalWidth > 0 ? img : null
      }
      default:
        return null
    }
  }

  async pushLiveInfo(info: LiveInfo): Promise<string | null> {
    const api = ensureBridge()
    const caps = await api.twitch.capabilities()
    if (!caps.linked || !caps.canEditInfo) return null
    const res = await api.twitch.updateChannelInfo(info)
    return res.ok ? null : res.error
  }

  async goLive(): Promise<string | null> {
    if (this.state.live || this.state.goingLive) return null
    const op = ++this.liveOpGen
    await this.mixer.resume()
    this.mixer.retryAllProblematic()
    this.patch({ goingLive: true })
    try {
      const res = await ensureBridge().multistream.start(this.settings.studio.fps)
      if (op !== this.liveOpGen) {
        // Cancelado por stopLive() enquanto o start ainda rodava
        if (res.ok) await ensureBridge().multistream.stop()
        return null
      }
      this.patch({ multistream: res.status })
      if (!res.ok) return res.error ?? 'Não foi possível iniciar a transmissão.'

      const startedIds = new Set(
        res.status.destinations
          .filter((d) => d.state === 'connecting' || d.state === 'live')
          .map((d) => d.id)
      )
      const vertical = this.settings.destinations.some(
        (d) => d.enabled && d.orientation === 'vertical' && (startedIds.size === 0 || startedIds.has(d.id))
      )
      this.liveVertical = vertical
      this.compositor.setStudio({
        ...this.settings.studio,
        dualOutputEnabled: this.settings.studio.dualOutputEnabled || vertical
      })
      this.outputs.setStreaming(true, vertical)
      this.patch({ live: true, liveStartedAt: Date.now() })
      return null
    } finally {
      if (op === this.liveOpGen) {
        this.patch({ goingLive: false })
      }
    }
  }

  async stopLive(): Promise<void> {
    this.liveOpGen++
    this.liveVertical = false
    this.outputs.setStreaming(false, false)
    const status = await ensureBridge().multistream.stop()
    this.compositor.setStudio(this.settings.studio)
    this.patch({ live: false, goingLive: false, liveStartedAt: null, multistream: status })
  }

  async toggleRecording(): Promise<string | null> {
    const api = ensureBridge()
    if (this.state.recorder.recording) {
      const status = await api.recorder.stop()
      this.outputs.setRecording(false)
      this.patch({ recorder: status })
      return null
    }
    await this.mixer.resume()
    this.outputs.setRecording(true)
    const res = await api.recorder.start()
    if (!res.ok) {
      this.outputs.setRecording(false)
      return res.error ?? 'Não foi possível gravar.'
    }
    if (res.status) this.patch({ recorder: res.status })
    return null
  }

  async saveReplay(): Promise<{ ok: boolean; message: string; path?: string }> {
    const res = await ensureBridge().recorder.saveReplay()
    if (res.ok && res.path) {
      return { ok: true, message: `Replay salvo: ${res.path.split(/[\\/]/).pop()}`, path: res.path }
    }
    return {
      ok: false,
      message:
        this.state.live || this.state.recorder.recording
          ? (res.error ?? 'Não deu para salvar o replay.')
          : 'O replay só guarda os últimos segundos enquanto você está ao vivo ou gravando.'
    }
  }

  async toggleVirtualCam(): Promise<{ ok: boolean; active: boolean; error?: string }> {
    const api = ensureBridge()
    if (!api.virtualCam) {
      return { ok: false, active: false, error: 'Câmera Virtual não suportada neste ambiente.' }
    }
    try {
      const next = this.state.virtualCamStatus.active
        ? await api.virtualCam.stop()
        : await api.virtualCam.start(this.settings.virtualCamera?.orientation ?? 'horizontal')
      this.applyVirtualCamStatus(next)
      if (next.error) {
        return { ok: false, active: next.active, error: next.error }
      }
      return { ok: true, active: next.active }
    } catch (err) {
      return {
        ok: false,
        active: this.state.virtualCamStatus.active,
        error: err instanceof Error ? err.message : 'Falha ao alternar a Câmera Virtual.'
      }
    }
  }

  private applyVirtualCamStatus(status: VirtualCameraStatus): void {
    studioStore.setVirtualCamStatus(status)
    this.patch({ virtualCamStatus: status })
    const vcamVertical = status.active && status.orientation === 'vertical'
    this.compositor.setStudio({
      ...this.settings.studio,
      dualOutputEnabled:
        this.settings.studio.dualOutputEnabled || (this.state.live && this.liveVertical) || vcamVertical
    })
    this.syncVirtualCamPump()
  }

  private syncVirtualCamPump(): void {
    const api = ensureBridge()
    const vcam = this.state.virtualCamStatus
    const snap = studioStore.getSnapshot()
    const gpuZeroCopyDirect = Boolean(snap.gpuStatus.zeroCopyActive || snap.telemetry.zeroCopyActive)
    const shouldPump = Boolean(vcam.active && api.virtualCam?.pushFrame && !gpuZeroCopyDirect)

    if (!shouldPump) {
      if (this.vcamPumpTimer !== null) {
        window.clearInterval(this.vcamPumpTimer)
        this.vcamPumpTimer = null
        this.vcamPumpFps = 0
      }
      return
    }

    const targetFps = Math.max(
      15,
      Math.min(60, this.settings.virtualCamera?.fps || vcam.fps || 30)
    )
    if (this.vcamPumpTimer !== null && this.vcamPumpFps === targetFps) return
    if (this.vcamPumpTimer !== null) window.clearInterval(this.vcamPumpTimer)

    this.vcamPumpFps = targetFps
    const intervalMs = Math.max(16, Math.round(1000 / targetFps))
    this.vcamPumpTimer = window.setInterval(() => {
      const cur = this.state.virtualCamStatus
      if (!cur.active) return
      const curSnap = studioStore.getSnapshot()
      if (curSnap.gpuStatus.zeroCopyActive || curSnap.telemetry.zeroCopyActive) {
        this.syncVirtualCamPump()
        return
      }
      const orient = cur.orientation ?? this.settings.virtualCamera?.orientation ?? 'horizontal'
      const defaultW = orient === 'vertical' ? 720 : 1280
      const defaultH = orient === 'vertical' ? 1280 : 720
      const targetW = cur.width > 0 ? Math.min(cur.width, 1920) : defaultW
      const targetH = cur.height > 0 ? Math.min(cur.height, 1920) : defaultH
      const frame = this.compositor.captureBgraFrame(orient, targetW, targetH)
      api.virtualCam?.pushFrame?.(frame.width, frame.height, frame.bgra)
    }, intervalMs)
  }

  private async pollStatus(): Promise<void> {
    try {
      const api = ensureBridge()
      const [recorder, multistream] = await Promise.all([
        api.recorder.status(),
        api.multistream.status()
      ])
      studioStore.setLiveStatus(multistream)
      studioStore.setRecorderStatus(recorder)

      const activeBitrate = this.settings.destinations
        .filter((d) => d.enabled)
        .reduce((acc, d) => acc + d.bitrateKbps, 0)
      const micMutedSpeaking = Object.values(studioStore.audioLevelsRef).some((l) => l.mutedSpeech)
      const rustTel = studioStore.getSnapshot().telemetry
      const localTel = this.compositor.getTelemetry(
        multistream.encoder || 'libx264',
        multistream.active ? activeBitrate : 0,
        micMutedSpeaking,
        recorder.replayBufferedSeconds
      )
      const telemetry: TelemetryStats = {
        ...localTel,
        zeroCopyActive: rustTel.zeroCopyActive ?? studioStore.getSnapshot().gpuStatus.zeroCopyActive,
        compositorBackend: rustTel.compositorBackend ?? studioStore.getSnapshot().gpuStatus.compositorBackend,
        abrActive: rustTel.abrActive,
        abrTargetBitrateKbps: rustTel.abrTargetBitrateKbps,
        abrCongestionLevel: rustTel.abrCongestionLevel,
        virtualCamActive: rustTel.virtualCamActive ?? this.state.virtualCamStatus.active,
        externalClients: rustTel.externalClients ?? studioStore.getSnapshot().externalControlStatus.obsWsClients,
        audioRuntimeActive: rustTel.audioRuntimeActive
      }

      let nextLive = this.state.live
      let nextLiveStartedAt = this.state.liveStartedAt
      if (this.state.live && !multistream.active) {
        this.liveVertical = false
        this.outputs.setStreaming(false, false)
        const vcamVertical =
          this.state.virtualCamStatus.active && this.state.virtualCamStatus.orientation === 'vertical'
        this.compositor.setStudio({
          ...this.settings.studio,
          dualOutputEnabled: this.settings.studio.dualOutputEnabled || vcamVertical
        })
        nextLive = false
        nextLiveStartedAt = null
      }

      this.syncVirtualCamPump()

      // Só notifica assinantes React se algum valor relevante realmente mudou (elimina re-render 1Hz em idle).
      const sig = JSON.stringify({
        live: nextLive,
        rec: recorder,
        ms: multistream,
        fps: Math.round(telemetry.compositorFps),
        drop: telemetry.droppedFrames,
        muteSpeak: telemetry.micMutedSpeaking,
        vcam: this.state.virtualCamStatus.active,
        zc: Boolean(telemetry.zeroCopyActive),
        abr: Boolean(telemetry.abrActive)
      })
      if (sig !== this.lastStatusSignature) {
        this.lastStatusSignature = sig
        this.patch({
          live: nextLive,
          liveStartedAt: nextLiveStartedAt,
          recorder,
          multistream,
          telemetry
        })
      } else {
        this.state.telemetry = telemetry
      }
    } catch {
      /* janela fechando */
    }
  }

  /** Clona tracks para que `HostSession` nunca altere constraints do track mestre do compositor. */
  lanStream(): MediaStream {
    void this.mixer.resume()
    const vTrack = this.compositor.programStream.getVideoTracks()[0]
    const aTrack = this.mixer.outputTrack
    return new MediaStream([vTrack.clone(), aTrack.clone()])
  }

  dispose(): void {
    window.removeEventListener('pointerdown', this.unlockAudio)
    window.removeEventListener('keydown', this.unlockAudio)
    window.removeEventListener('beforeunload', this.onBeforeUnload)
    window.clearInterval(this.statusTimer)
    if (this.vcamPumpTimer !== null) window.clearInterval(this.vcamPumpTimer)
    if (this.offVirtualCamStatus) this.offVirtualCamStatus()
    if (this.transitionKeepAliveTimer !== null) window.clearTimeout(this.transitionKeepAliveTimer)
    this.outputs.dispose()
    this.compositor.dispose()
    this.captures.dispose()
    this.browsers.dispose()
    this.mixer.dispose()
    ensureBridge().browserSources.sync([])
  }
}
