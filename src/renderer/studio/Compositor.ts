import type {
  CanvasOrientation,
  CustomShaderPreset,
  CustomTransitionConfig,
  Scene,
  SceneItem,
  SourceTransform,
  StudioCompositorSettings,
  TelemetryStats,
  TransitionEasing,
  TransitionType
} from '@shared/protocol'
import { dbToLinear } from '@shared/audio'
import { ensureBridge } from '@/bridge'

export type SourceResolver = (item: SceneItem) => CanvasImageSource | null

export const CANVAS_H = { width: 1920, height: 1080 }
export const CANVAS_V = { width: 1080, height: 1920 }
export const PREVIEW_H = { width: 960, height: 540 }

export const DEFAULT_CUSTOM_TRANSITION: CustomTransitionConfig = {
  stingerPath: null,
  stingerTransitionPointMs: 500,
  stingerAudioGainDb: 0,
  lumaMattePath: null,
  lumaPreset: 'horizontal-wipe',
  lumaSoftness: 0.15,
  lumaInvert: false,
  shaderPreset: 'zoom-blur',
  customWgsl: null,
  easing: 'ease-in-out'
}

export function applyTransitionEasing(easing: TransitionEasing, t: number): number {
  const x = Math.max(0, Math.min(1, t))
  switch (easing) {
    case 'linear':
      return x
    case 'cubic-out':
      return 1 - Math.pow(1 - x, 3)
    case 'expo-out':
      return x >= 1 ? 1 : 1 - Math.pow(2, -10 * x)
    case 'ease-in-out':
    default:
      return x < 0.5 ? 2 * x * x : 1 - Math.pow(-2 * x + 2, 2) / 2
  }
}

type Fit = 'contain' | 'cover' | 'stretch'

function fitFor(item: SceneItem): Fit {
  if (item.type === 'camera_input') return 'cover'
  if (item.type === 'browser_source') return 'stretch'
  return 'contain'
}

function sizeOf(src: CanvasImageSource): { w: number; h: number } {
  if (src instanceof HTMLVideoElement) return { w: src.videoWidth, h: src.videoHeight }
  if (src instanceof HTMLImageElement) return { w: src.naturalWidth, h: src.naturalHeight }
  if (src instanceof HTMLCanvasElement) return { w: src.width, h: src.height }
  return { w: 0, h: 0 }
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number): void {
  ctx.beginPath()
  ctx.roundRect(x, y, w, h, r)
}

function makeCanvas(w: number, h: number): HTMLCanvasElement {
  const c = document.createElement('canvas')
  c.width = w
  c.height = h
  return c
}

function mediaPathToUrls(rawPath: string): string[] {
  const p = rawPath.trim()
  if (!p) return []
  if (/^(https?:|file:|blob:|data:|asset:)/i.test(p)) return [p]
  const normalized = p.replace(/\\/g, '/')
  const fileUrl = normalized.startsWith('/')
    ? `file://${encodeURI(normalized)}`
    : `file:///${encodeURI(normalized)}`
  if (typeof window !== 'undefined' && window.__TAURI_INTERNALS__) {
    return [
      `https://asset.localhost/${encodeURIComponent(p)}`,
      `asset://localhost/${encodeURIComponent(p)}`,
      fileUrl
    ]
  }
  return [fileUrl]
}

interface LumaMaskCache {
  key: string
  width: number
  height: number
  field: Float32Array
  canvas: HTMLCanvasElement
  ctx: CanvasRenderingContext2D
  imageData: ImageData
}

export class Compositor {
  readonly program: HTMLCanvasElement
  readonly vertical: HTMLCanvasElement
  readonly preview: HTMLCanvasElement
  readonly programStream: MediaStream
  readonly verticalStream: MediaStream

  private readonly pCtx: CanvasRenderingContext2D
  private readonly vCtx: CanvasRenderingContext2D
  private readonly prCtx: CanvasRenderingContext2D
  private scratchH: HTMLCanvasElement | null = null
  private scratchHCtx: CanvasRenderingContext2D | null = null
  private scratchV: HTMLCanvasElement | null = null
  private scratchVCtx: CanvasRenderingContext2D | null = null
  private scratchAltH: HTMLCanvasElement | null = null
  private scratchAltHCtx: CanvasRenderingContext2D | null = null
  private scratchAltV: HTMLCanvasElement | null = null
  private scratchAltVCtx: CanvasRenderingContext2D | null = null

  // Stinger offscreen video state
  private stingerVideo: HTMLVideoElement | null = null
  private stingerLoadedPath: string | null = null
  private stingerReady = false
  private stingerFallbackUrls: string[] = []
  private stingerFallbackIdx = 0

  // Luma matte image & downscaled mask caches (H & V)
  private lumaImagePath: string | null = null
  private lumaImage: HTMLImageElement | null = null
  private lumaImageVersion = 0
  private lumaMaskH: LumaMaskCache | null = null
  private lumaMaskV: LumaMaskCache | null = null

  // Virtual camera BGRA readback cache
  private vcamCanvas: HTMLCanvasElement | null = null
  private vcamCtx: CanvasRenderingContext2D | null = null
  private vcamBgra: Uint8Array | null = null

  private programScene: Scene | null = null
  private previewScene: Scene | null = null
  private outgoing: Scene | null = null
  private transitionType: TransitionType = 'fade'
  private activeCustom: CustomTransitionConfig = { ...DEFAULT_CUSTOM_TRANSITION }
  private fadeStart = 0
  private fadeMs = 300

  private studio: StudioCompositorSettings
  private timer: number | null = null
  private disposed = false

  // Telemetria de renderização
  private frameCount = 0
  private lastFpsStamp = performance.now()
  private measuredFps = 60
  private droppedFrames = 0
  private renderTimes: number[] = []
  private expectedNextTick = 0

  constructor(
    studio: StudioCompositorSettings,
    private readonly resolve: SourceResolver,
    private readonly suppressed: (item: SceneItem) => boolean = () => false
  ) {
    this.studio = { ...studio }
    this.program = makeCanvas(CANVAS_H.width, CANVAS_H.height)
    this.vertical = makeCanvas(CANVAS_V.width, CANVAS_V.height)
    // Preview em 960x540 economiza 75% de fillrate no modo estúdio
    this.preview = makeCanvas(PREVIEW_H.width, PREVIEW_H.height)
    this.pCtx = this.program.getContext('2d', { alpha: false })!
    this.vCtx = this.vertical.getContext('2d', { alpha: false })!
    this.prCtx = this.preview.getContext('2d', { alpha: false })!

    this.programStream = this.program.captureStream()
    this.verticalStream = this.vertical.captureStream()
    this.measuredFps = this.studio.fps
    this.startLoop()
  }

  setStudio(next: StudioCompositorSettings): void {
    const fpsChanged = next.fps !== this.studio.fps
    this.studio = { ...next }
    if (fpsChanged) this.startLoop()
  }

  getOutgoingScene(): Scene | null {
    return this.outgoing
  }

  setScenes(
    program: Scene | null,
    preview: Scene | null,
    transition: { type: TransitionType; ms: number; custom?: CustomTransitionConfig }
  ): void {
    const override = program?.transitionOverride ?? null
    const effType: TransitionType = override?.transitionType ?? transition.type
    const effMs = override?.durationMs ?? transition.ms
    const effCustom: CustomTransitionConfig = {
      ...DEFAULT_CUSTOM_TRANSITION,
      ...(transition.custom ?? {}),
      ...(override?.custom ?? {})
    }

    // Pré-carrega mídia de stinger ou máscara luma para transição instantânea sem engasgos
    if (effCustom.stingerPath) this.preloadStinger(effCustom.stingerPath)
    if (effCustom.lumaMattePath) this.preloadLumaImage(effCustom.lumaMattePath)

    if (this.programScene && program && program.id !== this.programScene.id) {
      if (effType === 'cut') {
        this.outgoing = null
        this.stopStingerPlayback()
      } else {
        this.outgoing = this.programScene
        this.transitionType = effType
        this.activeCustom = effCustom
        this.fadeStart = performance.now()
        this.fadeMs = Math.max(80, effMs)

        if (effType === 'stinger') {
          const minStingerMs = Math.max(this.fadeMs, effCustom.stingerTransitionPointMs + 160)
          this.fadeMs = minStingerMs
          this.startStingerPlayback(effCustom)
        } else {
          this.stopStingerPlayback()
        }
      }
    } else if (effType === 'cut') {
      this.outgoing = null
      this.stopStingerPlayback()
    }

    this.programScene = program
    this.previewScene = preview
  }

  /**
   * Captura um quadro BGRA do canvas horizontal ou vertical em buffer reutilizável
   * para alimentar a Câmera Virtual quando o pipeline GPU zero-copy não está ativo.
   */
  captureBgraFrame(
    orientation: CanvasOrientation,
    targetWidth?: number,
    targetHeight?: number
  ): { width: number; height: number; bgra: Uint8Array } {
    const src = orientation === 'vertical' ? this.vertical : this.program
    const w = Math.max(
      16,
      Math.round(targetWidth ?? (orientation === 'vertical' ? 720 : 1280))
    )
    const h = Math.max(
      16,
      Math.round(targetHeight ?? (orientation === 'vertical' ? 1280 : 720))
    )

    if (!this.vcamCanvas || this.vcamCanvas.width !== w || this.vcamCanvas.height !== h) {
      this.vcamCanvas = makeCanvas(w, h)
      this.vcamCtx = this.vcamCanvas.getContext('2d', { alpha: false, willReadFrequently: true })
      this.vcamBgra = new Uint8Array(w * h * 4)
    }

    const ctx = this.vcamCtx!
    const bgra = this.vcamBgra!
    ctx.drawImage(src, 0, 0, w, h)
    const rgba = ctx.getImageData(0, 0, w, h).data
    for (let i = 0; i < rgba.length; i += 4) {
      bgra[i] = rgba[i + 2]
      bgra[i + 1] = rgba[i + 1]
      bgra[i + 2] = rgba[i]
      bgra[i + 3] = 255
    }
    return { width: w, height: h, bgra }
  }

  getTelemetry(
    encoder: string,
    encodeBitrateKbps: number,
    micMutedSpeaking: boolean,
    replayBufferedSeconds: number
  ): TelemetryStats {
    let p95 = 0
    if (this.renderTimes.length > 0) {
      const sorted = [...this.renderTimes].sort((a, b) => a - b)
      p95 = sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95))]
    }
    return {
      compositorFps: Math.round(this.measuredFps * 10) / 10,
      targetFps: this.studio.fps,
      renderMsP95: Math.round(p95 * 100) / 100,
      droppedFrames: this.droppedFrames,
      encodeBitrateKbps,
      encoder,
      cpuFallback: encoder === 'libx264',
      micMutedSpeaking,
      replayBufferedSeconds
    }
  }

  private preloadStinger(path: string): void {
    const clean = path.trim()
    if (!clean || clean === this.stingerLoadedPath) return
    this.stingerLoadedPath = clean
    this.stingerReady = false

    if (this.stingerVideo) {
      this.stingerVideo.pause()
      this.stingerVideo.removeAttribute('src')
      this.stingerVideo.load()
    } else {
      this.stingerVideo = document.createElement('video')
      this.stingerVideo.playsInline = true
      this.stingerVideo.preload = 'auto'
      this.stingerVideo.crossOrigin = 'anonymous'
    }

    const video = this.stingerVideo
    const urls = mediaPathToUrls(clean)
    this.stingerFallbackUrls = urls
    this.stingerFallbackIdx = 0

    video.onloadeddata = () => {
      if (this.stingerLoadedPath === clean) {
        this.stingerReady = true
      }
    }
    video.onerror = () => {
      if (this.stingerLoadedPath !== clean) return
      this.stingerFallbackIdx++
      if (this.stingerFallbackIdx < this.stingerFallbackUrls.length) {
        video.src = this.stingerFallbackUrls[this.stingerFallbackIdx]
        video.load()
      } else {
        this.stingerReady = false
      }
    }

    if (urls.length > 0) {
      video.src = urls[0]
      video.load()
    }
  }

  private startStingerPlayback(custom: CustomTransitionConfig): void {
    if (custom.stingerPath) {
      this.preloadStinger(custom.stingerPath)
    }
    const video = this.stingerVideo
    if (!video || !custom.stingerPath) return
    if (Number.isFinite(video.duration) && video.duration > 0.1) {
      this.fadeMs = Math.max(this.fadeMs, Math.round(video.duration * 1000))
    }
    try {
      video.currentTime = 0
    } catch {
      /* ainda carregando */
    }
    const gainLin = Math.max(0, Math.min(1, dbToLinear(custom.stingerAudioGainDb)))
    video.muted = custom.stingerAudioGainDb <= -60 || gainLin <= 0.001
    video.volume = gainLin
    void video.play().catch(() => {
      video.muted = true
      void video.play().catch(() => {})
    })
  }

  private stopStingerPlayback(): void {
    if (!this.stingerVideo) return
    try {
      this.stingerVideo.pause()
      this.stingerVideo.currentTime = 0
    } catch {
      /* ignore */
    }
  }

  private preloadLumaImage(path: string): void {
    const clean = path.trim()
    if (!clean || clean === this.lumaImagePath) return
    this.lumaImagePath = clean
    const img = new Image()
    this.lumaImage = img
    img.onload = () => {
      if (this.lumaImagePath === clean) {
        this.lumaImageVersion++
      }
    }
    void ensureBridge()
      .files.readImage(clean)
      .then((dataUrl) => {
        if (dataUrl && this.lumaImagePath === clean) {
          img.src = dataUrl
        }
      })
      .catch(() => {})
  }

  private ensureScratch(vertical: boolean): { canvas: HTMLCanvasElement; ctx: CanvasRenderingContext2D } {
    if (vertical) {
      if (!this.scratchV) {
        this.scratchV = makeCanvas(CANVAS_V.width, CANVAS_V.height)
        this.scratchVCtx = this.scratchV.getContext('2d', { alpha: true })!
      }
      return { canvas: this.scratchV, ctx: this.scratchVCtx! }
    }
    if (!this.scratchH) {
      this.scratchH = makeCanvas(CANVAS_H.width, CANVAS_H.height)
      this.scratchHCtx = this.scratchH.getContext('2d', { alpha: true })!
    }
    return { canvas: this.scratchH, ctx: this.scratchHCtx! }
  }

  private ensureScratchAlt(vertical: boolean): { canvas: HTMLCanvasElement; ctx: CanvasRenderingContext2D } {
    if (vertical) {
      if (!this.scratchAltV) {
        this.scratchAltV = makeCanvas(CANVAS_V.width, CANVAS_V.height)
        this.scratchAltVCtx = this.scratchAltV.getContext('2d', { alpha: true })!
      }
      return { canvas: this.scratchAltV, ctx: this.scratchAltVCtx! }
    }
    if (!this.scratchAltH) {
      this.scratchAltH = makeCanvas(CANVAS_H.width, CANVAS_H.height)
      this.scratchAltHCtx = this.scratchAltH.getContext('2d', { alpha: true })!
    }
    return { canvas: this.scratchAltH, ctx: this.scratchAltHCtx! }
  }

  private ensureLumaMask(vertical: boolean, custom: CustomTransitionConfig): LumaMaskCache {
    const mw = vertical ? 135 : 240
    const mh = vertical ? 240 : 135
    const useCustomImg =
      Boolean(custom.lumaMattePath) &&
      (custom.lumaPreset === 'custom' || Boolean(custom.lumaMattePath)) &&
      Boolean(this.lumaImage && this.lumaImage.complete && this.lumaImage.naturalWidth > 0)
    const key = `${custom.lumaPreset}|${useCustomImg ? `${this.lumaImagePath}:${this.lumaImageVersion}` : ''}|${mw}x${mh}`
    let cache = vertical ? this.lumaMaskV : this.lumaMaskH

    if (cache && cache.key === key) {
      return cache
    }

    const canvas = cache?.canvas ?? makeCanvas(mw, mh)
    const ctx = cache?.ctx ?? canvas.getContext('2d', { alpha: true, willReadFrequently: true })!
    const imageData = cache?.imageData ?? ctx.createImageData(mw, mh)
    const field = cache?.field ?? new Float32Array(mw * mh)

    // Inicializa RGB como branco (só o canal alpha varia a cada quadro)
    const px = imageData.data
    for (let i = 0; i < mw * mh; i++) {
      px[i * 4] = 255
      px[i * 4 + 1] = 255
      px[i * 4 + 2] = 255
      px[i * 4 + 3] = 0
    }

    if (useCustomImg && this.lumaImage) {
      ctx.clearRect(0, 0, mw, mh)
      ctx.drawImage(this.lumaImage, 0, 0, mw, mh)
      const imgBytes = ctx.getImageData(0, 0, mw, mh).data
      for (let i = 0; i < mw * mh; i++) {
        const r = imgBytes[i * 4]
        const g = imgBytes[i * 4 + 1]
        const b = imgBytes[i * 4 + 2]
        field[i] = (0.299 * r + 0.587 * g + 0.114 * b) / 255
      }
    } else {
      const preset = custom.lumaPreset || 'horizontal-wipe'
      for (let y = 0; y < mh; y++) {
        const v = mh > 1 ? y / (mh - 1) : 0
        for (let x = 0; x < mw; x++) {
          const u = mw > 1 ? x / (mw - 1) : 0
          let lum = u
          switch (preset) {
            case 'vertical-wipe':
              lum = v
              break
            case 'diagonal':
              lum = (u + v) * 0.5
              break
            case 'radial':
              lum = Math.min(1, Math.hypot((u - 0.5) * 2, (v - 0.5) * 2) / Math.SQRT2)
              break
            case 'barn-door':
              lum = Math.abs(u - 0.5) * 2
              break
            case 'horizontal-wipe':
            default:
              lum = u
              break
          }
          field[y * mw + x] = lum
        }
      }
    }

    cache = { key, width: mw, height: mh, field, canvas, ctx, imageData }
    if (vertical) this.lumaMaskV = cache
    else this.lumaMaskH = cache
    return cache
  }

  private startLoop(): void {
    if (this.timer !== null) window.clearTimeout(this.timer)
    this.expectedNextTick = performance.now()
    const step = (): void => {
      if (this.disposed) return
      const interval = 1000 / Math.max(15, this.studio.fps)
      const now = performance.now()
      if (now - this.expectedNextTick > interval * 1.75) {
        const missed = Math.floor((now - this.expectedNextTick) / interval)
        this.droppedFrames += Math.max(1, missed)
        this.expectedNextTick = now
      }
      this.tick(now)
      this.expectedNextTick += interval
      const delay = Math.max(1, Math.round(this.expectedNextTick - performance.now()))
      this.timer = window.setTimeout(step, delay)
    }
    this.timer = window.setTimeout(step, Math.round(1000 / this.studio.fps))
  }

  private tick(now: number): void {
    if (this.disposed) return
    const t0 = performance.now()

    let rawProgress = 1
    let elapsedMs = 0
    if (this.outgoing) {
      elapsedMs = Math.max(0, now - this.fadeStart)
      rawProgress = Math.min(1, elapsedMs / Math.max(1, this.fadeMs))
      if (rawProgress >= 1) {
        this.outgoing = null
        this.stopStingerPlayback()
      }
    }

    const easedProgress = applyTransitionEasing(this.activeCustom.easing, rawProgress)

    this.renderInto(this.pCtx, CANVAS_H.width, CANVAS_H.height, false, rawProgress, easedProgress, elapsedMs)
    if (this.studio.dualOutputEnabled) {
      this.renderInto(this.vCtx, CANVAS_V.width, CANVAS_V.height, true, rawProgress, easedProgress, elapsedMs)
    }
    if (this.previewScene) {
      this.prCtx.fillStyle = '#000'
      this.prCtx.fillRect(0, 0, PREVIEW_H.width, PREVIEW_H.height)
      this.drawScene(this.prCtx, this.previewScene, PREVIEW_H.width, PREVIEW_H.height, false)
    }

    const dt = performance.now() - t0
    this.renderTimes.push(dt)
    if (this.renderTimes.length > 60) this.renderTimes.shift()

    this.frameCount++
    const elapsed = now - this.lastFpsStamp
    if (elapsed >= 1000) {
      this.measuredFps = (this.frameCount * 1000) / elapsed
      this.frameCount = 0
      this.lastFpsStamp = now
    }
  }

  private renderInto(
    ctx: CanvasRenderingContext2D,
    W: number,
    H: number,
    vertical: boolean,
    rawProgress: number,
    easedProgress: number,
    elapsedMs: number
  ): void {
    ctx.fillStyle = '#000'
    ctx.fillRect(0, 0, W, H)

    if (this.outgoing && rawProgress < 1) {
      const t = this.transitionType
      if (t === 'fade-to-black' || t === 'fade_to_black') {
        if (easedProgress < 0.5) {
          this.drawScene(ctx, this.outgoing, W, H, vertical)
          ctx.fillStyle = `rgba(0,0,0,${easedProgress * 2})`
          ctx.fillRect(0, 0, W, H)
        } else {
          if (this.programScene) this.drawScene(ctx, this.programScene, W, H, vertical)
          ctx.fillStyle = `rgba(0,0,0,${(1 - easedProgress) * 2})`
          ctx.fillRect(0, 0, W, H)
        }
      } else if (t === 'slide') {
        const offset = Math.round(W * easedProgress)
        ctx.save()
        ctx.translate(-offset, 0)
        this.drawScene(ctx, this.outgoing, W, H, vertical)
        ctx.restore()
        if (this.programScene) {
          const scratch = this.ensureScratch(vertical)
          scratch.ctx.globalCompositeOperation = 'source-over'
          scratch.ctx.fillStyle = '#000'
          scratch.ctx.fillRect(0, 0, W, H)
          this.drawScene(scratch.ctx, this.programScene, W, H, vertical)
          ctx.drawImage(scratch.canvas, W - offset, 0, W, H)
        }
      } else if (t === 'stinger') {
        this.renderStinger(ctx, W, H, vertical, rawProgress, easedProgress, elapsedMs)
      } else if (t === 'luma-wipe') {
        this.renderLumaWipe(ctx, W, H, vertical, easedProgress)
      } else if (t === 'custom-shader') {
        this.renderCustomShader(ctx, W, H, vertical, easedProgress, this.activeCustom.shaderPreset)
      } else {
        // Fade composto em buffer offscreen para evitar artefato de alpha entre camadas da mesma cena
        this.drawScene(ctx, this.outgoing, W, H, vertical)
        if (this.programScene) {
          const scratch = this.ensureScratch(vertical)
          scratch.ctx.globalCompositeOperation = 'source-over'
          scratch.ctx.fillStyle = '#000'
          scratch.ctx.fillRect(0, 0, W, H)
          this.drawScene(scratch.ctx, this.programScene, W, H, vertical)
          ctx.save()
          ctx.globalAlpha = easedProgress
          ctx.drawImage(scratch.canvas, 0, 0, W, H)
          ctx.restore()
        }
      }
    } else if (this.programScene) {
      this.drawScene(ctx, this.programScene, W, H, vertical)
    }

    if (this.studio.privacyShieldActive) this.drawShield(ctx, W, H)
  }

  private renderStinger(
    ctx: CanvasRenderingContext2D,
    W: number,
    H: number,
    vertical: boolean,
    rawProgress: number,
    easedProgress: number,
    elapsedMs: number
  ): void {
    const cutMs = Math.min(this.fadeMs * 0.92, Math.max(0, this.activeCustom.stingerTransitionPointMs))
    const switched = elapsedMs >= cutMs
    if (!switched && this.outgoing) {
      this.drawScene(ctx, this.outgoing, W, H, vertical)
    } else if (this.programScene) {
      this.drawScene(ctx, this.programScene, W, H, vertical)
    }

    const vid = this.stingerVideo
    if (vid && this.stingerReady && vid.readyState >= 2 && vid.videoWidth > 0) {
      ctx.drawImage(vid, 0, 0, W, H)
      return
    }

    // Fallback visual elegante em camadas diagonais sincronizado ao ponto de corte
    const cutFrac = Math.max(0.15, Math.min(0.85, cutMs / Math.max(1, this.fadeMs)))
    const p =
      rawProgress <= cutFrac
        ? 0.5 * (rawProgress / cutFrac)
        : 0.5 + 0.5 * ((rawProgress - cutFrac) / Math.max(0.01, 1 - cutFrac))
    const sweep = applyTransitionEasing(this.activeCustom.easing, p)

    const skew = W * 0.22
    const span = W + skew * 2
    const headX = -skew + sweep * (span + W * 1.15)

    ctx.save()
    const drawRibbon = (offset: number, width: number, fill: string | CanvasGradient): void => {
      const x = headX - offset
      ctx.beginPath()
      ctx.moveTo(x, 0)
      ctx.lineTo(x + width, 0)
      ctx.lineTo(x + width - skew, H)
      ctx.lineTo(x - skew, H)
      ctx.closePath()
      ctx.fillStyle = fill
      ctx.fill()
    }

    const grad = ctx.createLinearGradient(0, 0, W, H)
    grad.addColorStop(0, '#0f1320')
    grad.addColorStop(0.5, '#191f33')
    grad.addColorStop(1, '#0b0d12')

    drawRibbon(W * 0.12, W * 0.18, 'rgba(124, 106, 255, 0.92)')
    drawRibbon(W * 0.26, W * 1.15, grad)
    drawRibbon(W * 1.28, W * 0.14, 'rgba(23, 212, 178, 0.92)')

    // Brilho sutil central próximo do pico da cobertura
    const peak = Math.max(0, 1 - Math.abs(easedProgress - 0.5) * 3.2)
    if (peak > 0.01) {
      ctx.globalAlpha = peak * 0.35
      ctx.fillStyle = '#7c6aff'
      ctx.fillRect(0, H * 0.495, W, Math.max(2, H * 0.008))
    }
    ctx.restore()
  }

  private renderLumaWipe(
    ctx: CanvasRenderingContext2D,
    W: number,
    H: number,
    vertical: boolean,
    eased: number
  ): void {
    if (this.outgoing) this.drawScene(ctx, this.outgoing, W, H, vertical)
    if (!this.programScene) return

    const cache = this.ensureLumaMask(vertical, this.activeCustom)
    const softness = Math.max(0.004, Math.min(0.5, this.activeCustom.lumaSoftness))
    const invert = Boolean(this.activeCustom.lumaInvert)
    const edge = eased * (1 + softness)
    const invSoft = 1 / softness
    const field = cache.field
    const bytes = cache.imageData.data

    for (let i = 0; i < field.length; i++) {
      const m = invert ? 1 - field[i] : field[i]
      let a = (edge - m) * invSoft
      if (a <= 0) a = 0
      else if (a >= 1) a = 1
      bytes[i * 4 + 3] = (a * 255) | 0
    }
    cache.ctx.putImageData(cache.imageData, 0, 0)

    const scratch = this.ensureScratch(vertical)
    scratch.ctx.save()
    scratch.ctx.globalCompositeOperation = 'source-over'
    scratch.ctx.clearRect(0, 0, W, H)
    scratch.ctx.fillStyle = '#000'
    scratch.ctx.fillRect(0, 0, W, H)
    this.drawScene(scratch.ctx, this.programScene, W, H, vertical)
    scratch.ctx.globalCompositeOperation = 'destination-in'
    scratch.ctx.drawImage(cache.canvas, 0, 0, W, H)
    scratch.ctx.restore()

    ctx.drawImage(scratch.canvas, 0, 0, W, H)
  }

  private renderCustomShader(
    ctx: CanvasRenderingContext2D,
    W: number,
    H: number,
    vertical: boolean,
    eased: number,
    preset: CustomShaderPreset
  ): void {
    if (!this.outgoing || !this.programScene) {
      if (this.programScene) this.drawScene(ctx, this.programScene, W, H, vertical)
      return
    }

    const fromBuf = this.ensureScratchAlt(vertical)
    fromBuf.ctx.globalCompositeOperation = 'source-over'
    fromBuf.ctx.fillStyle = '#000'
    fromBuf.ctx.fillRect(0, 0, W, H)
    this.drawScene(fromBuf.ctx, this.outgoing, W, H, vertical)

    const toBuf = this.ensureScratch(vertical)
    toBuf.ctx.globalCompositeOperation = 'source-over'
    toBuf.ctx.fillStyle = '#000'
    toBuf.ctx.fillRect(0, 0, W, H)
    this.drawScene(toBuf.ctx, this.programScene, W, H, vertical)

    const cx = W / 2
    const cy = H / 2

    if (preset === 'zoom-blur') {
      const intensity = Math.sin(eased * Math.PI)
      const outScale = 1 + eased * 0.28
      const inScale = 1 + (1 - eased) * 0.28

      // Camadas radiais de zoom-blur na cena de saída
      ctx.save()
      for (let i = 0; i < 3; i++) {
        const s = outScale + i * 0.045 * intensity
        ctx.globalAlpha = (1 - eased) * (i === 0 ? 0.65 : 0.22)
        ctx.setTransform(s, 0, 0, s, cx - cx * s, cy - cy * s)
        ctx.drawImage(fromBuf.canvas, 0, 0, W, H)
      }
      // Cena de entrada convergindo do zoom-blur para 1.0x
      for (let i = 0; i < 3; i++) {
        const s = inScale + i * 0.04 * intensity
        ctx.globalAlpha = eased * (i === 0 ? 0.72 : 0.18)
        ctx.setTransform(s, 0, 0, s, cx - cx * s, cy - cy * s)
        ctx.drawImage(toBuf.canvas, 0, 0, W, H)
      }
      ctx.restore()
      return
    }

    if (preset === 'radial-wipe') {
      ctx.drawImage(fromBuf.canvas, 0, 0, W, H)
      const radius = Math.hypot(cx, cy) + 8
      const startAngle = -Math.PI / 2
      const endAngle = startAngle + eased * Math.PI * 2

      ctx.save()
      ctx.beginPath()
      ctx.moveTo(cx, cy)
      ctx.arc(cx, cy, radius, startAngle, endAngle)
      ctx.closePath()
      ctx.clip()
      ctx.drawImage(toBuf.canvas, 0, 0, W, H)
      ctx.restore()

      // Feixe suave acompanhando a varredura radial
      if (eased > 0.01 && eased < 0.99) {
        ctx.save()
        ctx.strokeStyle = 'rgba(124, 106, 255, 0.7)'
        ctx.lineWidth = Math.max(3, W * 0.003)
        ctx.beginPath()
        ctx.moveTo(cx, cy)
        ctx.lineTo(cx + Math.cos(endAngle) * radius, cy + Math.sin(endAngle) * radius)
        ctx.stroke()
        ctx.restore()
      }
      return
    }

    if (preset === 'glitch-cut') {
      const glitchPeak = Math.sin(eased * Math.PI)
      const activeSrc = eased < 0.5 ? fromBuf.canvas : toBuf.canvas
      const altSrc = eased < 0.5 ? toBuf.canvas : fromBuf.canvas
      ctx.drawImage(activeSrc, 0, 0, W, H)

      const slices = 14
      const sliceH = Math.ceil(H / slices)
      const seed = Math.floor(eased * 28)

      ctx.save()
      for (let i = 0; i < slices; i++) {
        const hash = Math.sin((i + 1) * 127.1 + seed * 311.7) * 43758.5453
        const frac = hash - Math.floor(hash)
        if (frac < 0.35 + (1 - glitchPeak) * 0.45) continue
        const shift = (frac - 0.5) * W * 0.18 * glitchPeak
        const sy = i * sliceH
        const useAlt = frac > 0.72
        ctx.drawImage(useAlt ? altSrc : activeSrc, 0, sy, W, sliceH, shift, sy, W, sliceH)

        // Aberração cromática leve nas fatias deslocadas
        ctx.globalCompositeOperation = 'screen'
        ctx.fillStyle = frac > 0.65 ? 'rgba(23, 212, 178, 0.14)' : 'rgba(255, 59, 79, 0.14)'
        ctx.fillRect(0, sy, W, Math.max(2, sliceH * 0.35))
        ctx.globalCompositeOperation = 'source-over'
      }
      ctx.restore()
      return
    }

    if (preset === 'swirl') {
      const twist = Math.sin(eased * Math.PI)
      const angleOut = eased * Math.PI * 0.42
      const angleIn = -(1 - eased) * Math.PI * 0.42
      const scale = 1 + twist * 0.32

      ctx.save()
      ctx.translate(cx, cy)
      ctx.rotate(angleOut)
      ctx.scale(scale, scale)
      ctx.globalAlpha = 1 - eased
      ctx.drawImage(fromBuf.canvas, -cx, -cy, W, H)
      ctx.restore()

      ctx.save()
      ctx.translate(cx, cy)
      ctx.rotate(angleIn)
      ctx.scale(scale, scale)
      ctx.globalAlpha = eased
      ctx.drawImage(toBuf.canvas, -cx, -cy, W, H)
      ctx.restore()

      if (twist > 0.05) {
        const grad = ctx.createRadialGradient(cx, cy, Math.min(W, H) * 0.18, cx, cy, Math.hypot(cx, cy))
        grad.addColorStop(0, 'rgba(124, 106, 255, 0)')
        grad.addColorStop(1, `rgba(11, 13, 18, ${(twist * 0.45).toFixed(3)})`)
        ctx.fillStyle = grad
        ctx.fillRect(0, 0, W, H)
      }
      return
    }

    // custom-wgsl preview: varredura de onda prismática fluida no Canvas 2D
    // (o código WGSL customizado é compilado nativamente pelo compositor wgpu em Rust)
    ctx.drawImage(fromBuf.canvas, 0, 0, W, H)
    const strips = 36
    const stripH = Math.ceil(H / strips)
    const waveWidth = W * 0.12
    ctx.save()
    for (let i = 0; i < strips; i++) {
      const y = i * stripH
      const phase = (i / strips) * Math.PI * 3 + eased * Math.PI * 4
      const cutX = Math.max(0, Math.min(W, (eased * 1.15 - 0.075) * W + Math.sin(phase) * waveWidth * Math.sin(eased * Math.PI)))
      if (cutX > 0) {
        ctx.drawImage(toBuf.canvas, 0, y, cutX, stripH, 0, y, cutX, stripH)
      }
      if (cutX > 4 && cutX < W - 4) {
        ctx.fillStyle = 'rgba(124, 106, 255, 0.35)'
        ctx.fillRect(cutX - 4, y, 8, stripH)
      }
    }
    ctx.restore()
  }

  drawScene(ctx: CanvasRenderingContext2D, scene: Scene, W: number, H: number, vertical: boolean): void {
    const items = scene.items
      .filter(
        (it) =>
          it.visible &&
          it.type !== 'audio_input' &&
          (vertical ? it.showOnVertical : it.showOnHorizontal) &&
          !this.suppressed(it)
      )
      .sort((a, b) =>
        vertical
          ? a.verticalTransform.zIndex - b.verticalTransform.zIndex
          : a.transform.zIndex - b.transform.zIndex
      )
    const base = ctx.globalAlpha
    for (const item of items) {
      ctx.globalAlpha = base
      this.drawItem(ctx, item, vertical ? item.verticalTransform : item.transform, W, H)
    }
    ctx.globalAlpha = base
  }

  private drawItem(ctx: CanvasRenderingContext2D, item: SceneItem, tf: SourceTransform, W: number, H: number): void {
    const x = tf.x * W
    const y = tf.y * H
    const w = tf.width * W
    const h = tf.height * H
    if (w <= 0 || h <= 0) return
    const opacity = typeof tf.opacity === 'number' ? Math.max(0, Math.min(1, tf.opacity)) : 1
    if (opacity <= 0) return

    const s = item.settings
    const prevAlpha = ctx.globalAlpha
    ctx.globalAlpha = prevAlpha * opacity

    if (item.type === 'color_source') {
      ctx.fillStyle = s.color || '#000000'
      ctx.fillRect(x, y, w, h)
      ctx.globalAlpha = prevAlpha
      return
    }

    if (item.type === 'text_source') {
      const size = Math.max(8, s.fontSize || 48) * (H / 1080)
      ctx.save()
      if (tf.flipX) {
        ctx.translate(x + w / 2, y + h / 2)
        ctx.scale(-1, 1)
        ctx.translate(-(x + w / 2), -(y + h / 2))
      }
      ctx.fillStyle = s.color || '#ffffff'
      ctx.font = `700 ${size}px "Segoe UI Variable Display", "Segoe UI", sans-serif`
      ctx.textAlign = 'center'
      ctx.textBaseline = 'middle'
      ctx.shadowColor = 'rgba(0,0,0,0.6)'
      ctx.shadowBlur = size * 0.25
      ctx.fillText(s.text || item.name, x + w / 2, y + h / 2, w)
      ctx.restore()
      ctx.globalAlpha = prevAlpha
      return
    }

    const src = this.resolve(item)
    if (!src) {
      ctx.globalAlpha = prevAlpha
      return
    }
    const { w: sw, h: sh } = sizeOf(src)
    if (!sw || !sh) {
      ctx.globalAlpha = prevAlpha
      return
    }

    // Recorte (crop normalizado 0..0.9 em relação à fonte)
    const cL = Math.max(0, Math.min(0.9, tf.cropLeft ?? 0))
    const cR = Math.max(0, Math.min(0.9 - cL, tf.cropRight ?? 0))
    const cT = Math.max(0, Math.min(0.9, tf.cropTop ?? 0))
    const cB = Math.max(0, Math.min(0.9 - cT, tf.cropBottom ?? 0))
    const sx = cL * sw
    const sy = cT * sh
    const sWidth = Math.max(1, (1 - cL - cR) * sw)
    const sHeight = Math.max(1, (1 - cT - cB) * sh)

    ctx.save()
    if (s.rounded) {
      roundRect(ctx, x, y, w, h, Math.min(w, h) * 0.06)
      ctx.clip()
    }
    if (tf.flipX) {
      ctx.translate(x + w / 2, y + h / 2)
      ctx.scale(-1, 1)
      ctx.translate(-(x + w / 2), -(y + h / 2))
    }

    const fit = fitFor(item)
    if (fit === 'stretch') {
      ctx.drawImage(src, sx, sy, sWidth, sHeight, x, y, w, h)
    } else if (fit === 'cover') {
      const scale = Math.max(w / sWidth, h / sHeight)
      const cw = w / scale
      const ch = h / scale
      ctx.drawImage(src, sx + (sWidth - cw) / 2, sy + (sHeight - ch) / 2, cw, ch, x, y, w, h)
    } else {
      const scale = Math.min(w / sWidth, h / sHeight)
      const dw = sWidth * scale
      const dh = sHeight * scale
      ctx.drawImage(src, sx, sy, sWidth, sHeight, x + (w - dw) / 2, y + (h - dh) / 2, dw, dh)
    }
    ctx.restore()
    ctx.globalAlpha = prevAlpha
  }

  private drawShield(ctx: CanvasRenderingContext2D, W: number, H: number): void {
    const unit = Math.min(W, H) / 1080
    ctx.fillStyle = '#0b0d12'
    ctx.fillRect(0, 0, W, H)
    ctx.textAlign = 'center'
    ctx.textBaseline = 'middle'
    ctx.fillStyle = '#f3f5f9'
    ctx.font = `800 ${96 * unit}px "Segoe UI Variable Display", "Segoe UI", sans-serif`
    ctx.fillText(this.studio.privacyShieldTitle || 'VOLTO JÁ', W / 2, H / 2 - 30 * unit, W * 0.9)
    ctx.fillStyle = '#98a1b5'
    ctx.font = `500 ${34 * unit}px "Segoe UI Variable Text", "Segoe UI", sans-serif`
    ctx.fillText(this.studio.privacyShieldSubtitle || '', W / 2, H / 2 + 60 * unit, W * 0.9)
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    if (this.timer !== null) window.clearTimeout(this.timer)
    this.stopStingerPlayback()
    if (this.stingerVideo) {
      this.stingerVideo.removeAttribute('src')
      this.stingerVideo.load()
      this.stingerVideo = null
    }
    this.lumaImage = null
    this.lumaMaskH = null
    this.lumaMaskV = null
    this.vcamBgra = null
    this.programStream.getTracks().forEach((t) => t.stop())
    this.verticalStream.getTracks().forEach((t) => t.stop())
    for (const c of [this.scratchH, this.scratchV, this.scratchAltH, this.scratchAltV, this.vcamCanvas]) {
      if (c) {
        c.width = 0
        c.height = 0
      }
    }
  }
}
