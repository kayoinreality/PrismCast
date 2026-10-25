import { spawn, execFileSync, type ChildProcessWithoutNullStreams } from 'node:child_process'
import type {
  CanvasOrientation,
  MultistreamStatus,
  MultistreamTarget,
  MultistreamTargetStatus
} from '@shared/protocol'

interface PipelineEntry {
  proc: ChildProcessWithoutNullStreams
  targetIds: string[]
}

function detectHardwareEncoder(): string {
  try {
    const out = execFileSync('ffmpeg', ['-hide_banner', '-encoders'], {
      encoding: 'utf8',
      timeout: 3000,
      windowsHide: true
    })
    for (const enc of ['h264_nvenc', 'h264_amf', 'h264_qsv']) {
      if (out.includes(enc)) return enc
    }
  } catch {
    /* fallback software */
  }
  return 'libx264'
}

function buildEndpoint(rtmpUrl: string, streamKey: string): string {
  return `${rtmpUrl.trim().replace(/\/+$/, '')}/${streamKey.trim().replace(/^\/+/, '')}`
}

/**
 * Motor de Multistreaming + Dual Output (Horizontal 16:9 e Vertical 9:16).
 *
 * Inspirado no `StreamingService` + `DualOutputService` + `RestreamService` do Streamlabs:
 * mantém pipelines FFmpeg independentes para o canvas horizontal (Twitch, YouTube, Kick)
 * e para o canvas vertical 9:16 (TikTok Live, YouTube Shorts).
 */
export class MultistreamManager {
  private readonly encoder = detectHardwareEncoder()
  private active = false
  private startedAt = 0
  private horizontalPipe: PipelineEntry | null = null
  private verticalPipe: PipelineEntry | null = null
  private targets: MultistreamTarget[] = []
  private readonly bytesSent = new Map<string, number>()
  private readonly errors = new Map<string, string>()

  get detectedEncoder(): string {
    return this.encoder
  }

  start(
    targets: MultistreamTarget[],
    fps: number
  ): { ok: boolean; status?: MultistreamStatus; error?: string } {
    this.stop()

    const enabled = targets.filter(
      (t) => t.enabled && t.rtmpUrl.trim().length > 0 && t.streamKey.trim().length > 0
    )

    if (enabled.length === 0) {
      return {
        ok: false,
        error:
          'Ative ao menos um provedor (Twitch, YouTube, Kick ou TikTok) e preencha a Stream Key.'
      }
    }

    const horiz = enabled.filter((t) => t.orientation === 'horizontal')
    const vert = enabled.filter((t) => t.orientation === 'vertical')

    this.bytesSent.clear()
    this.errors.clear()

    if (horiz.length > 0) {
      try {
        this.horizontalPipe = this.spawnFanout(horiz, fps, false)
      } catch (err) {
        for (const t of horiz) this.errors.set(t.id, (err as Error).message)
      }
    }

    if (vert.length > 0) {
      try {
        this.verticalPipe = this.spawnFanout(vert, fps, true)
      } catch (err) {
        for (const t of vert) this.errors.set(t.id, (err as Error).message)
      }
    }

    this.targets = enabled
    this.active = Boolean(this.horizontalPipe || this.verticalPipe)
    this.startedAt = this.active ? Date.now() : 0

    return { ok: this.active, status: this.status() }
  }

  private spawnFanout(
    targets: MultistreamTarget[],
    fps: number,
    isVertical: boolean
  ): PipelineEntry {
    const gop = Math.max(60, fps * 2)
    const args: string[] = [
      '-hide_banner',
      '-loglevel',
      'warning',
      '-fflags',
      '+genpts+nobuffer',
      '-i',
      'pipe:0'
    ]

    const targetIds: string[] = []
    for (const t of targets) {
      targetIds.push(t.id)
      const br = Math.min(20000, Math.max(1000, t.bitrateKbps || (isVertical ? 4500 : 6000)))
      const endpoint = buildEndpoint(t.rtmpUrl, t.streamKey)
      args.push(
        '-map',
        '0:v:0',
        '-map',
        '0:a:0?',
        '-c:v',
        this.encoder,
        '-b:v',
        `${br}k`,
        '-maxrate',
        `${br}k`,
        '-bufsize',
        `${br * 2}k`,
        '-g',
        String(gop),
        '-pix_fmt',
        'yuv420p'
      )
      if (this.encoder === 'libx264') {
        args.push('-preset', 'veryfast', '-tune', 'zerolatency')
      }
      args.push('-c:a', 'aac', '-b:a', '160k', '-ar', '48000', '-f', 'flv', endpoint)
    }

    const proc = spawn('ffmpeg', args, {
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true
    })

    proc.on('error', (err) => {
      for (const id of targetIds) {
        this.errors.set(id, `FFmpeg: ${err.message}`)
      }
    })

    proc.stdin.on('error', () => {
      /* tratado no write */
    })

    return { proc, targetIds }
  }

  pushChunk(orientation: CanvasOrientation, chunk: Uint8Array): void {
    if (!this.active || chunk.byteLength === 0) return
    const entry = orientation === 'vertical' ? this.verticalPipe : this.horizontalPipe
    if (!entry || entry.proc.killed || !entry.proc.stdin.writable) return

    try {
      entry.proc.stdin.write(Buffer.from(chunk))
      for (const id of entry.targetIds) {
        this.bytesSent.set(id, (this.bytesSent.get(id) ?? 0) + chunk.byteLength)
      }
    } catch (err) {
      for (const id of entry.targetIds) {
        this.errors.set(id, (err as Error).message)
      }
    }
  }

  stop(): MultistreamStatus {
    for (const pipe of [this.horizontalPipe, this.verticalPipe]) {
      if (!pipe) continue
      try {
        pipe.proc.stdin.end()
        pipe.proc.kill()
      } catch {
        /* ja encerrado */
      }
    }
    this.horizontalPipe = null
    this.verticalPipe = null
    this.active = false
    this.startedAt = 0
    return this.status()
  }

  status(): MultistreamStatus {
    const uptimeSec =
      this.active && this.startedAt > 0 ? Math.floor((Date.now() - this.startedAt) / 1000) : 0

    const targets: MultistreamTargetStatus[] = this.targets.map((t) => {
      const err = this.errors.get(t.id) ?? null
      return {
        id: t.id,
        provider: t.provider,
        name: t.name,
        orientation: t.orientation,
        state: err ? 'error' : this.active ? 'live' : 'idle',
        uptimeSec: this.active && !err ? uptimeSec : 0,
        bytesSent: this.bytesSent.get(t.id) ?? 0,
        lastError: err
      }
    })

    return {
      active: this.active,
      encoder: this.encoder,
      horizontalLive: this.horizontalPipe !== null,
      verticalLive: this.verticalPipe !== null,
      targets
    }
  }
}
