import { createWriteStream, mkdirSync, unlink, writeFileSync, type WriteStream } from 'node:fs'
import { spawn } from 'node:child_process'
import { join } from 'node:path'
import { homedir } from 'node:os'
import type { RecorderStatus } from '@shared/protocol'

interface ReplayChunk {
  timestampMs: number
  data: Buffer
}

/**
 * O MediaRecorder entrega WebM; o Windows abre MP4 em qualquer lugar. Remuxa sem
 * recodificar o video (rapido) e so apaga o WebM se o MP4 saiu inteiro.
 */
function remuxToMp4(webmPath: string): void {
  const mp4 = webmPath.replace(/\.webm$/i, '.mp4')
  const proc = spawn(
    'ffmpeg',
    ['-hide_banner', '-loglevel', 'error', '-y', '-i', webmPath, '-c:v', 'copy', '-c:a', 'aac', '-b:a', '160k', '-movflags', '+faststart', mp4],
    { windowsHide: true, stdio: 'ignore' }
  )
  proc.on('error', () => {
    /* sem FFmpeg: fica o WebM */
  })
  proc.on('exit', (code) => {
    if (code === 0) unlink(webmPath, () => {})
  })
}

function formatTimestamp(): string {
  const d = new Date()
  const pad = (n: number): string => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}_${pad(d.getHours())}-${pad(d.getMinutes())}-${pad(d.getSeconds())}`
}

/**
 * Gravador Local + Replay Buffer Circular.
 *
 * Inspirado no `SimpleReplayBuffer` e `RecordingModeService` do Streamlabs Desktop:
 * retém o segmento de inicialização WebM (`initSegment`) e uma fila circular
 * dos últimos N segundos (ex.: 30s) para salvar clipes instantâneos com 1 clique.
 */
export class RecorderManager {
  private readonly outputDir: string
  private recStream: WriteStream | null = null
  private recPath: string | null = null
  private recStartedAt = 0

  private replayActive = true
  private replayMaxSec = 30
  private replayStartedAt = Date.now()
  private initSegment: Buffer | null = null
  private replayChunks: ReplayChunk[] = []
  private lastSavedReplay: string | null = null

  constructor() {
    this.outputDir = join(homedir(), 'Videos', 'PrismCast')
    try {
      mkdirSync(this.outputDir, { recursive: true })
    } catch {
      /* criado sob demanda */
    }
  }

  startRecording(): { ok: boolean; status?: RecorderStatus; error?: string } {
    try {
      mkdirSync(this.outputDir, { recursive: true })
      const file = join(this.outputDir, `PrismCast_Live_${formatTimestamp()}.webm`)
      const stream = createWriteStream(file)
      if (this.initSegment) {
        stream.write(this.initSegment)
      }
      this.recStream = stream
      this.recPath = file
      this.recStartedAt = Date.now()
      return { ok: true, status: this.status() }
    } catch (err) {
      return { ok: false, error: (err as Error).message }
    }
  }

  stopRecording(): RecorderStatus {
    if (this.recStream) {
      const path = this.recPath
      try {
        this.recStream.end(() => {
          if (path) remuxToMp4(path)
        })
      } catch {
        /* fechado */
      }
      this.recStream = null
    }
    this.recStartedAt = 0
    return this.status()
  }

  setReplayBuffer(enabled: boolean, seconds: number): RecorderStatus {
    this.replayMaxSec = Math.min(180, Math.max(10, seconds || 30))
    this.replayActive = enabled
    if (!enabled) {
      this.replayChunks = []
      this.initSegment = null
    } else if (!this.replayStartedAt) {
      this.replayStartedAt = Date.now()
    }
    return this.status()
  }

  pushChunk(raw: Uint8Array): void {
    if (!raw || raw.byteLength === 0) return
    const buf = Buffer.from(raw)

    if (this.recStream && this.recStream.writable) {
      this.recStream.write(buf)
    }

    if (this.replayActive) {
      if (!this.initSegment) {
        this.initSegment = buf
        this.replayStartedAt = Date.now()
        return
      }
      const now = Date.now() - this.replayStartedAt
      this.replayChunks.push({ timestampMs: now, data: buf })
      const windowMs = this.replayMaxSec * 1000
      while (
        this.replayChunks.length > 0 &&
        now - this.replayChunks[0].timestampMs > windowMs
      ) {
        this.replayChunks.shift()
      }
    }
  }

  saveReplay(): { ok: boolean; path?: string; status?: RecorderStatus; error?: string } {
    if (!this.initSegment) {
      return {
        ok: false,
        error: 'O Replay Buffer ainda está capturando os primeiros segundos da transmissão.'
      }
    }
    try {
      mkdirSync(this.outputDir, { recursive: true })
      const path = join(
        this.outputDir,
        `PrismCast_Replay_${this.replayMaxSec}s_${formatTimestamp()}.webm`
      )
      const buffers = [this.initSegment, ...this.replayChunks.map((c) => c.data)]
      writeFileSync(path, Buffer.concat(buffers))
      remuxToMp4(path)
      this.lastSavedReplay = path.replace(/\.webm$/i, '.mp4')
      return { ok: true, path: this.lastSavedReplay, status: this.status() }
    } catch (err) {
      return { ok: false, error: (err as Error).message }
    }
  }

  resetStream(): void {
    this.stopRecording()
    this.initSegment = null
    this.replayChunks = []
  }

  status(): RecorderStatus {
    const recSec =
      this.recStream && this.recStartedAt > 0
        ? Math.floor((Date.now() - this.recStartedAt) / 1000)
        : 0

    let bufferedSec = 0
    if (this.replayChunks.length >= 2) {
      const span =
        this.replayChunks[this.replayChunks.length - 1].timestampMs -
        this.replayChunks[0].timestampMs
      bufferedSec = Math.min(this.replayMaxSec, Math.max(1, Math.round(span / 1000)))
    } else if (this.initSegment) {
      bufferedSec = 1
    }

    return {
      recording: this.recStream !== null,
      recordingPath: this.recPath,
      recordingDurationSec: recSec,
      replayBufferActive: this.replayActive,
      replayBufferedSeconds: bufferedSec,
      replaySavedPath: this.lastSavedReplay
    }
  }
}
