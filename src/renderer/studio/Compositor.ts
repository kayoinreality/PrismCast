import type { Scene, SceneItem, SourceTransform, StudioCompositorSettings } from '@shared/protocol'

/** Devolve o que desenhar para uma fonte, ou null se ainda não tem imagem. */
export type SourceResolver = (item: SceneItem) => CanvasImageSource | null

export const CANVAS_H = { width: 1920, height: 1080 }
export const CANVAS_V = { width: 1080, height: 1920 }

type Fit = 'contain' | 'cover' | 'stretch'

function fitFor(item: SceneItem): Fit {
  if (item.type === 'camera_input') return 'cover'
  // O widget já é renderizado no tamanho da caixa; imagem mantém a proporção.
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

/**
 * Compositor das cenas: desenha a cena no ar em dois canvases (16:9 e 9:16) e,
 * no modo estúdio, a cena em prévia num terceiro. Os canvases são os mesmos
 * elementos mostrados no editor e gravados para a live, então o que se vê é
 * exatamente o que vai ao ar.
 */
export class Compositor {
  readonly program: HTMLCanvasElement
  readonly vertical: HTMLCanvasElement
  readonly preview: HTMLCanvasElement
  readonly programStream: MediaStream
  readonly verticalStream: MediaStream

  private readonly pCtx: CanvasRenderingContext2D
  private readonly vCtx: CanvasRenderingContext2D
  private readonly prCtx: CanvasRenderingContext2D

  private programScene: Scene | null = null
  private previewScene: Scene | null = null
  private outgoing: Scene | null = null
  private fadeStart = 0
  private fadeMs = 300

  private studio: StudioCompositorSettings
  private timer: number | null = null
  private disposed = false

  constructor(
    studio: StudioCompositorSettings,
    private readonly resolve: SourceResolver,
    /** fontes escondidas pelo estúdio (ex.: câmera desligada) */
    private readonly suppressed: (item: SceneItem) => boolean = () => false
  ) {
    this.studio = { ...studio }
    const make = (w: number, h: number): HTMLCanvasElement => {
      const c = document.createElement('canvas')
      c.width = w
      c.height = h
      return c
    }
    this.program = make(CANVAS_H.width, CANVAS_H.height)
    this.vertical = make(CANVAS_V.width, CANVAS_V.height)
    this.preview = make(CANVAS_H.width, CANVAS_H.height)
    this.pCtx = this.program.getContext('2d', { alpha: false })!
    this.vCtx = this.vertical.getContext('2d', { alpha: false })!
    this.prCtx = this.preview.getContext('2d', { alpha: false })!

    this.programStream = this.program.captureStream()
    this.verticalStream = this.vertical.captureStream()
    this.startLoop()
  }

  setStudio(next: StudioCompositorSettings): void {
    const fpsChanged = next.fps !== this.studio.fps
    this.studio = { ...next }
    if (fpsChanged) this.startLoop()
  }

  setScenes(program: Scene | null, preview: Scene | null, transition: { type: 'cut' | 'fade'; ms: number }): void {
    if (this.programScene && program && program.id !== this.programScene.id && transition.type === 'fade') {
      this.outgoing = this.programScene
      this.fadeStart = performance.now()
      this.fadeMs = Math.max(100, transition.ms)
    }
    this.programScene = program
    this.previewScene = preview
  }

  private startLoop(): void {
    if (this.timer !== null) window.clearInterval(this.timer)
    this.timer = window.setInterval(() => this.tick(), Math.round(1000 / this.studio.fps))
  }

  private tick(): void {
    if (this.disposed) return
    const now = performance.now()

    let fade = 1
    if (this.outgoing) {
      fade = Math.min(1, (now - this.fadeStart) / this.fadeMs)
      if (fade >= 1) this.outgoing = null
    }

    this.renderInto(this.pCtx, CANVAS_H.width, CANVAS_H.height, false, fade)
    if (this.studio.dualOutputEnabled) {
      this.renderInto(this.vCtx, CANVAS_V.width, CANVAS_V.height, true, fade)
    }
    if (this.previewScene) {
      this.prCtx.fillStyle = '#000'
      this.prCtx.fillRect(0, 0, CANVAS_H.width, CANVAS_H.height)
      this.drawScene(this.prCtx, this.previewScene, CANVAS_H.width, CANVAS_H.height, false)
    }
  }

  private renderInto(
    ctx: CanvasRenderingContext2D,
    W: number,
    H: number,
    vertical: boolean,
    fade: number
  ): void {
    ctx.fillStyle = '#000'
    ctx.fillRect(0, 0, W, H)

    if (this.outgoing && fade < 1) {
      ctx.globalAlpha = 1 - fade
      this.drawScene(ctx, this.outgoing, W, H, vertical)
      ctx.globalAlpha = fade
    }
    if (this.programScene) this.drawScene(ctx, this.programScene, W, H, vertical)
    ctx.globalAlpha = 1

    if (this.studio.privacyShieldActive) this.drawShield(ctx, W, H)
  }

  /** Desenha uma cena; usado também pelo editor para miniaturas. */
  drawScene(ctx: CanvasRenderingContext2D, scene: Scene, W: number, H: number, vertical: boolean): void {
    const items = scene.items
      .filter(
        (it) =>
          it.visible && it.type !== 'audio_input' && (vertical ? it.showOnVertical : it.showOnHorizontal) && !this.suppressed(it)
      )
      .sort((a, b) => (vertical ? a.verticalTransform.zIndex - b.verticalTransform.zIndex : a.transform.zIndex - b.transform.zIndex))
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
    const s = item.settings

    if (item.type === 'color_source') {
      ctx.fillStyle = s.color || '#000000'
      ctx.fillRect(x, y, w, h)
      return
    }

    if (item.type === 'text_source') {
      const size = Math.max(8, s.fontSize || 48) * (H / 1080)
      ctx.save()
      ctx.fillStyle = s.color || '#ffffff'
      ctx.font = `700 ${size}px "Segoe UI Variable Display", "Segoe UI", sans-serif`
      ctx.textAlign = 'center'
      ctx.textBaseline = 'middle'
      ctx.shadowColor = 'rgba(0,0,0,0.6)'
      ctx.shadowBlur = size * 0.25
      ctx.fillText(s.text || item.name, x + w / 2, y + h / 2, w)
      ctx.restore()
      return
    }

    const src = this.resolve(item)
    if (!src) return
    const { w: sw, h: sh } = sizeOf(src)
    if (!sw || !sh) return

    ctx.save()
    if (s.rounded) {
      roundRect(ctx, x, y, w, h, Math.min(w, h) * 0.06)
      ctx.clip()
    }
    const fit = fitFor(item)
    if (fit === 'stretch') {
      ctx.drawImage(src, x, y, w, h)
    } else if (fit === 'cover') {
      const scale = Math.max(w / sw, h / sh)
      const cw = w / scale
      const ch = h / scale
      ctx.drawImage(src, (sw - cw) / 2, (sh - ch) / 2, cw, ch, x, y, w, h)
    } else {
      const scale = Math.min(w / sw, h / sh)
      const dw = sw * scale
      const dh = sh * scale
      ctx.drawImage(src, x + (w - dw) / 2, y + (h - dh) / 2, dw, dh)
    }
    ctx.restore()
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
    if (this.timer !== null) window.clearInterval(this.timer)
    this.programStream.getTracks().forEach((t) => t.stop())
    this.verticalStream.getTracks().forEach((t) => t.stop())
  }
}
