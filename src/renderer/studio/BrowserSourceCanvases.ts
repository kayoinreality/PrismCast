import type { BrowserSourceFrame } from '@shared/protocol'

/**
 * Um canvas por browser source, montado com os retângulos que o main manda.
 *
 * Os pixels chegam em BGRA; o VideoFrame do WebCodecs aceita esse formato
 * direto, então não há conversão de cor em JavaScript (seriam milhões de
 * pixels por segundo num widget animado).
 */
export class BrowserSourceCanvases {
  private readonly canvases = new Map<string, HTMLCanvasElement>()
  private readonly ready = new Set<string>()
  private readonly off: () => void

  constructor() {
    this.off = window.api.browserSources.onFrame((frame) => this.paint(frame))
  }

  get(id: string): HTMLCanvasElement | null {
    return this.ready.has(id) ? (this.canvases.get(id) ?? null) : null
  }

  private paint(f: BrowserSourceFrame): void {
    let canvas = this.canvases.get(f.id)
    if (!canvas) {
      canvas = document.createElement('canvas')
      this.canvases.set(f.id, canvas)
    }
    if (canvas.width !== f.fullWidth || canvas.height !== f.fullHeight) {
      canvas.width = f.fullWidth
      canvas.height = f.fullHeight
    }
    const ctx = canvas.getContext('2d')
    if (!ctx) return
    let frame: VideoFrame | null = null
    try {
      frame = new VideoFrame(f.data, {
        format: 'BGRA',
        codedWidth: f.width,
        codedHeight: f.height,
        timestamp: 0
      })
      ctx.clearRect(f.x, f.y, f.width, f.height)
      ctx.drawImage(frame, f.x, f.y, f.width, f.height)
      this.ready.add(f.id)
    } catch (err) {
      console.warn('[browser-source] quadro invalido:', err)
    } finally {
      frame?.close()
      window.api.browserSources.ack(f.id)
    }
  }

  dispose(): void {
    this.off()
    this.canvases.clear()
    this.ready.clear()
  }
}
