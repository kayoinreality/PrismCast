import type { BrowserSourceFrame } from '@shared/protocol'
import { ensureBridge } from '@/bridge'

interface CanvasEntry {
  canvas: HTMLCanvasElement
  ctx: CanvasRenderingContext2D | null
}

/**
 * Um canvas por browser source, montado com os retângulos dirty-rect BGRA.
 *
 * - Cacheia o `CanvasRenderingContext2D` por fonte.
 * - Sempre envia `ack(f.id)` no `finally` (mesmo se `!ctx`), evitando travar o
 *   controle de fluxo `inFlight` no backend.
 * - `prune(activeIds)` zera `width = height = 0` para liberar memória de textura GPU imediatamente.
 */
export class BrowserSourceCanvases {
  private readonly entries = new Map<string, CanvasEntry>()
  private readonly ready = new Set<string>()
  private readonly off: () => void

  constructor() {
    const api = ensureBridge()
    this.off = api.browserSources.onFrame((frame) => this.paint(frame))
  }

  get(id: string): HTMLCanvasElement | null {
    return this.ready.has(id) ? (this.entries.get(id)?.canvas ?? null) : null
  }

  prune(activeIds: Set<string>): void {
    for (const [id, entry] of this.entries) {
      if (!activeIds.has(id)) {
        entry.canvas.width = 0
        entry.canvas.height = 0
        this.entries.delete(id)
        this.ready.delete(id)
      }
    }
  }

  private paint(f: BrowserSourceFrame): void {
    const api = ensureBridge()
    let frame: VideoFrame | null = null
    try {
      let entry = this.entries.get(f.id)
      if (!entry) {
        const canvas = document.createElement('canvas')
        entry = { canvas, ctx: canvas.getContext('2d') }
        this.entries.set(f.id, entry)
      }
      if (entry.canvas.width !== f.fullWidth || entry.canvas.height !== f.fullHeight) {
        entry.canvas.width = f.fullWidth
        entry.canvas.height = f.fullHeight
        entry.ctx = entry.canvas.getContext('2d')
      }
      const ctx = entry.ctx
      if (!ctx) return

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
      api.browserSources.ack(f.id)
    }
  }

  dispose(): void {
    this.off()
    for (const entry of this.entries.values()) {
      entry.canvas.width = 0
      entry.canvas.height = 0
    }
    this.entries.clear()
    this.ready.clear()
  }
}
