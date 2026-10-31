import type { AppSettings } from '@shared/ipc'
import type {
  AudioChannelStatus,
  BrowserSourceSpec,
  LiveInfo,
  MultistreamStatus,
  RecorderStatus,
  Scene,
  SceneItem,
  WidgetKit
} from '@shared/protocol'
import { AudioMixerEngine, type AudioLevels } from './AudioMixerEngine'
import { BrowserSourceCanvases } from './BrowserSourceCanvases'
import { CaptureManager, type Capture } from './CaptureManager'
import { Compositor } from './Compositor'
import { OutputManager } from './OutputManager'
import { effectiveUrl, hiddenWithoutCamera, restartsOnShow } from './widgets'
import { sceneMics } from './scenes'

export interface EngineState {
  live: boolean
  liveStartedAt: number | null
  goingLive: boolean
  multistream: MultistreamStatus
  recorder: RecorderStatus
  /** muda quando alguma captura muda de estado (para o editor se atualizar) */
  captureVersion: number
  /** muda quando uma fonte de áudio abre, cai ou fica esperando o programa */
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

/**
 * Motor do estúdio. Vive uma vez por janela, acima das telas, para trocar de
 * aba não derrubar a live nem reabrir capturas.
 */
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
  private lastBrowserSpecs = ''
  private systemAudioRequested = false
  private kit: WidgetKit | null = null
  private lastProgramId: string | null = null
  private unlockAudio = (): void => {
    void this.mixer.resume()
  }

  state: EngineState = {
    live: false,
    liveStartedAt: null,
    goingLive: false,
    multistream: { active: false, encoder: '…', ffmpegFound: true, destinations: [] },
    recorder: IDLE_RECORDER,
    captureVersion: 0,
    audioVersion: 0
  }

  constructor(settings: AppSettings) {
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
    this.captures.onChange = () => this.patch({ captureVersion: this.state.captureVersion + 1 })
    this.mixer.onStatus = () => this.patch({ audioVersion: this.state.audioVersion + 1 })
    this.captures.setFps(settings.studio.fps)
    this.statusTimer = window.setInterval(() => void this.pollStatus(), 1000)
    void window.api.recorder.setReplayBuffer(true, settings.studio.replayBufferSeconds)
    this.apply(settings)
  }

  // --------------------------------------------------------------- estado

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

  /** Id da janela do widget como ele está no ar (com a câmera ligada ou não). */
  browserIdOf(item: SceneItem): string {
    return browserSourceId(item, effectiveUrl(item, this.kit, this.settings.studio.cameraEnabled))
  }

  reloadBrowserSource(item: SceneItem): void {
    window.api.browserSources.reload(this.browserIdOf(item))
  }

  /** Recarrega todos os widgets do kit (depois de editar os textos dele). */
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

  // ---------------------------------------------------------- preferências

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
    this.compositor.setStudio(settings.studio)
    this.compositor.setScenes(this.programScene(), this.previewScene(), {
      type: settings.scenes.transitionType,
      ms: settings.scenes.transitionDurationMs
    })
    const used = new Set(settings.scenes.scenes.flatMap((s) => [...sceneMics(s, false)]))
    this.mixer.setSceneMics(sceneMics(this.programScene()), used)
    void this.mixer.applyConfig(settings.audioMixer)
    const wantsDesktop = settings.audioMixer.channels.some((c) => c.kind === 'desktop')
    if (wantsDesktop && !this.systemAudioRequested) {
      this.systemAudioRequested = true
      void this.captures.acquireSystemAudio().then((s) => {
        if (s) this.mixer.setSystemStream(s)
        else this.systemAudioRequested = false
      })
    }
    this.syncMedia()
    this.restartOnShow()
  }

  /**
   * Cena que acabou de entrar no ar: contagem regressiva e cronômetro
   * recomeçam (a página fica viva fora do ar para o chat não perder mensagens).
   */
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

  /** Abre só o que a cena no ar e a cena em prévia usam. */
  private syncMedia(): void {
    const scenes = [this.programScene(), this.previewScene()].filter((s): s is Scene => s !== null)
    const items = scenes.flatMap((s) => s.items)

    const wanted = new Map<string, () => Promise<{ stream: MediaStream; label: string } | null>>()
    const specs = new Map<string, BrowserSourceSpec>()
    for (const item of items) {
      const s = item.settings
      if (item.type === 'screen_capture' || item.type === 'window_capture') {
        wanted.set(captureKeyOf(item)!, this.captures.openDisplay(s.captureId, s.captureName))
      } else if (item.type === 'camera_input') {
        // Câmera desligada no estúdio: nem abre (a luz da webcam fica apagada).
        if (this.settings.studio.cameraEnabled) wanted.set(captureKeyOf(item)!, this.captures.openCamera(s.deviceId))
      } else if (item.type === 'browser_source' && s.url) {
        if (!this.settings.studio.cameraEnabled && hiddenWithoutCamera(item, this.kit)) continue
        const url = effectiveUrl(item, this.kit, this.settings.studio.cameraEnabled)
        const id = browserSourceId(item, url)
        specs.set(id, { id, url, width: s.width ?? 1920, height: s.height ?? 1080, fps: s.fps ?? 30 })
      } else if (item.type === 'image_source' && s.path && !this.images.has(s.path)) {
        this.loadImage(s.path)
      }
    }
    this.captures.sync(wanted)

    const list = [...specs.values()]
    const key = JSON.stringify(list)
    if (key !== this.lastBrowserSpecs) {
      this.lastBrowserSpecs = key
      window.api.browserSources.sync(list)
    }
  }

  private loadImage(path: string): void {
    const img = new Image()
    this.images.set(path, img)
    void window.api.files.readImage(path).then((url) => {
      if (url) img.src = url
    })
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

  // -------------------------------------------------------- live e gravação

  /** Manda título e categoria para a Twitch (se houver login com essa permissão). */
  async pushLiveInfo(info: LiveInfo): Promise<string | null> {
    const caps = await window.api.twitch.capabilities()
    if (!caps.linked || !caps.canEditInfo) return null
    const res = await window.api.twitch.updateChannelInfo(info)
    return res.ok ? null : res.error
  }

  async goLive(): Promise<string | null> {
    if (this.state.live || this.state.goingLive) return null
    await this.mixer.resume()
    this.mixer.retryAllProblematic()
    this.patch({ goingLive: true })
    try {
      const res = await window.api.multistream.start(this.settings.studio.fps)
      this.patch({ multistream: res.status })
      if (!res.ok) return res.error ?? 'Não foi possível iniciar a transmissão.'
      const vertical = this.settings.destinations.some((d) => d.enabled && d.orientation === 'vertical')
      if (vertical && !this.settings.studio.dualOutputEnabled) {
        // Destino vertical precisa do canvas 9:16 sendo desenhado.
        this.compositor.setStudio({ ...this.settings.studio, dualOutputEnabled: true })
      }
      this.outputs.setStreaming(true, vertical)
      this.patch({ live: true, liveStartedAt: Date.now() })
      return null
    } finally {
      this.patch({ goingLive: false })
    }
  }

  async stopLive(): Promise<void> {
    this.outputs.setStreaming(false, false)
    const status = await window.api.multistream.stop()
    this.compositor.setStudio(this.settings.studio)
    this.patch({ live: false, liveStartedAt: null, multistream: status })
  }

  async toggleRecording(): Promise<string | null> {
    if (this.state.recorder.recording) {
      const status = await window.api.recorder.stop()
      this.outputs.setRecording(false)
      this.patch({ recorder: status })
      return null
    }
    await this.mixer.resume()
    this.outputs.setRecording(true)
    const res = await window.api.recorder.start()
    if (!res.ok) {
      this.outputs.setRecording(false)
      return res.error ?? 'Não foi possível gravar.'
    }
    if (res.status) this.patch({ recorder: res.status })
    return null
  }

  async saveReplay(): Promise<{ ok: boolean; message: string; path?: string }> {
    const res = await window.api.recorder.saveReplay()
    if (res.ok && res.path) {
      return { ok: true, message: `Replay salvo: ${res.path.split(/[\\/]/).pop()}`, path: res.path }
    }
    return {
      ok: false,
      message: this.state.live || this.state.recorder.recording
        ? (res.error ?? 'Não deu para salvar o replay.')
        : 'O replay só guarda os últimos segundos enquanto você está ao vivo ou gravando.'
    }
  }

  private async pollStatus(): Promise<void> {
    try {
      const [recorder, multistream] = await Promise.all([
        window.api.recorder.status(),
        window.api.multistream.status()
      ])
      const patch: Partial<EngineState> = { recorder, multistream }
      // Todos os destinos caíram: a live acabou de fato, mesmo sem clicar em parar.
      if (this.state.live && !multistream.active) {
        this.outputs.setStreaming(false, false)
        patch.live = false
        patch.liveStartedAt = null
      }
      this.patch(patch)
    } catch {
      /* janela fechando */
    }
  }

  /** Vídeo + áudio do programa, para o compartilhamento na rede local. */
  lanStream(): MediaStream {
    void this.mixer.resume()
    return new MediaStream([this.compositor.programStream.getVideoTracks()[0], this.mixer.outputTrack])
  }

  dispose(): void {
    window.removeEventListener('pointerdown', this.unlockAudio)
    window.removeEventListener('keydown', this.unlockAudio)
    window.clearInterval(this.statusTimer)
    this.outputs.dispose()
    this.compositor.dispose()
    this.captures.dispose()
    this.browsers.dispose()
    this.mixer.dispose()
    window.api.browserSources.sync([])
  }
}
