import type {
  Scene,
  SceneItem,
  SourceTransform,
  StudioCompositorSettings,
  TelemetryStats,
  TransitionType
} from '@shared/protocol'

export type SourceResolver = (item: SceneItem) => CanvasImageSource | null

export const CANVAS_H = { width: 1920, height: 1080 }
export const CANVAS_V = { width: 1080, height: 1920 }
export const PREVIEW_H = { width: 960, height: 540 }

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

  private programScene: Scene | null = null
  private previewScene: Scene | null = null
  private outgoing: Scene | null = null
  private transitionType: TransitionType = 'fade'
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
    transition: { type: TransitionType; ms: number }
  ): void {
    if (transition.type === 'cut') {
      this.outgoing = null
    } else if (this.programScene && program && program.id !== this.programScene.id) {
      this.outgoing = this.programScene
      this.transitionType = transition.type
      this.fadeStart = performance.now()
      this.fadeMs = Math.max(80, transition.ms)
    }
    this.programScene = program
    this.previewScene = preview
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

  private ensureScratch(vertical: boolean): { canvas: HTMLCanvasElement; ctx: CanvasRenderingContext2D } {
    if (vertical) {
      if (!this.scratchV) {
        this.scratchV = makeCanvas(CANVAS_V.width, CANVAS_V.height)
        this.scratchVCtx = this.scratchV.getContext('2d', { alpha: false })!
      }
      return { canvas: this.scratchV, ctx: this.scratchVCtx! }
    }
    if (!this.scratchH) {
      this.scratchH = makeCanvas(CANVAS_H.width, CANVAS_H.height)
      this.scratchHCtx = this.scratchH.getContext('2d', { alpha: false })!
    }
    return { canvas: this.scratchH, ctx: this.scratchHCtx! }
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

    let progress = 1
    if (this.outgoing) {
      progress = Math.min(1, Math.max(0, (now - this.fadeStart) / this.fadeMs))
      if (progress >= 1) this.outgoing = null
    }

    this.renderInto(this.pCtx, CANVAS_H.width, CANVAS_H.height, false, progress)
    if (this.studio.dualOutputEnabled) {
      this.renderInto(this.vCtx, CANVAS_V.width, CANVAS_V.height, true, progress)
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
    progress: number
  ): void {
    ctx.fillStyle = '#000'
    ctx.fillRect(0, 0, W, H)

    if (this.outgoing && progress < 1) {
      const t = this.transitionType
      if (t === 'fade-to-black' || t === 'fade_to_black') {
        if (progress < 0.5) {
          this.drawScene(ctx, this.outgoing, W, H, vertical)
          ctx.fillStyle = `rgba(0,0,0,${progress * 2})`
          ctx.fillRect(0, 0, W, H)
        } else {
          if (this.programScene) this.drawScene(ctx, this.programScene, W, H, vertical)
          ctx.fillStyle = `rgba(0,0,0,${(1 - progress) * 2})`
          ctx.fillRect(0, 0, W, H)
        }
      } else if (t === 'slide') {
        const offset = Math.round(W * progress)
        ctx.save()
        ctx.translate(-offset, 0)
        this.drawScene(ctx, this.outgoing, W, H, vertical)
        ctx.restore()
        if (this.programScene) {
          const scratch = this.ensureScratch(vertical)
          scratch.ctx.fillStyle = '#000'
          scratch.ctx.fillRect(0, 0, W, H)
          this.drawScene(scratch.ctx, this.programScene, W, H, vertical)
          ctx.drawImage(scratch.canvas, W - offset, 0, W, H)
        }
      } else {
        // Fade composto em buffer offscreen para evitar artefato de alpha entre camadas da mesma cena
        this.drawScene(ctx, this.outgoing, W, H, vertical)
        if (this.programScene) {
          const scratch = this.ensureScratch(vertical)
          scratch.ctx.fillStyle = '#000'
          scratch.ctx.fillRect(0, 0, W, H)
          this.drawScene(scratch.ctx, this.programScene, W, H, vertical)
          ctx.save()
          ctx.globalAlpha = progress
          ctx.drawImage(scratch.canvas, 0, 0, W, H)
          ctx.restore()
        }
      }
    } else if (this.programScene) {
      this.drawScene(ctx, this.programScene, W, H, vertical)
    }

    if (this.studio.privacyShieldActive) this.drawShield(ctx, W, H)
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
    this.programStream.getTracks().forEach((t) => t.stop())
    this.verticalStream.getTracks().forEach((t) => t.stop())
    if (this.scratchH) {
      this.scratchH.width = 0
      this.scratchH.height = 0
    }
    if (this.scratchV) {
      this.scratchV.width = 0
      this.scratchV.height = 0
    }
  }
}
