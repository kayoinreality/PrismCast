import type { CanvasOrientation } from '@shared/protocol'
import { ensureBridge } from '@/bridge'

/**
 * Grava os canvases com MediaRecorder e distribui os pedaços para o motor Rust:
 *   - 16:9 -> 1 único encoder compartilhado quando liveH e gravação/replay rodam juntos
 *             (fazendo fan-out para multistream horizontal e recorder local — economiza 33–50% de GPU/CPU!)
 *   - 9:16 -> encoder dedicado apenas quando há destino vertical ligado
 *
 * Cada recorder serializa `e.data.arrayBuffer()` numa cadeia de Promise com token de geração (`gen`)
 * para nunca inverter chunks nem entregar pedaços atrasados após `stop()`.
 */

const MIME_CANDIDATES = [
  'video/x-matroska;codecs=avc1,opus',
  'video/webm;codecs=h264,opus',
  'video/webm;codecs=vp8,opus'
]

const TIMESLICE_MS = 200
const INTERMEDIATE_BPS = 24_000_000

function pickMime(): string {
  return typeof MediaRecorder !== 'undefined'
    ? (MIME_CANDIDATES.find((m) => MediaRecorder.isTypeSupported(m)) ?? 'video/webm')
    : 'video/webm'
}

type Sink = (chunk: Uint8Array) => void

interface ActiveRecorder {
  rec: MediaRecorder
  gen: number
}

export class OutputManager {
  private sharedH: ActiveRecorder | null = null
  private liveV: ActiveRecorder | null = null
  private nextGen = 1
  private streaming = false
  private verticalWanted = false
  private recording = false

  constructor(
    private readonly horizontalStream: () => MediaStream,
    private readonly verticalStream: () => MediaStream
  ) {}

  private spawnRecorder(stream: MediaStream, sink: Sink): ActiveRecorder {
    const gen = this.nextGen++
    const rec = new MediaRecorder(stream, {
      mimeType: pickMime(),
      videoBitsPerSecond: INTERMEDIATE_BPS,
      audioBitsPerSecond: 192_000
    })
    let chain = Promise.resolve()

    rec.ondataavailable = (e) => {
      if (e.data.size === 0) return
      const blob = e.data
      chain = chain
        .then(() => blob.arrayBuffer())
        .then((buf) => {
          // Descarta chunks que terminaram de ler após o recorder ser parado/trocado
          if (this.sharedH?.gen !== gen && this.liveV?.gen !== gen) return
          sink(new Uint8Array(buf))
        })
        .catch(() => {})
    }
    rec.onerror = (ev) => {
      console.warn('[OutputManager] erro no MediaRecorder:', ev)
    }
    rec.start(TIMESLICE_MS)
    return { rec, gen }
  }

  private stopActive(active: ActiveRecorder | null): null {
    if (active && active.rec.state !== 'inactive') {
      try {
        active.rec.stop()
      } catch {
        /* ignore */
      }
    }
    return null
  }

  private reconcile(restartHorizontal = false): void {
    const api = ensureBridge()
    const wantH = this.streaming || this.recording
    const wantV = this.streaming && this.verticalWanted

    if (!wantH) {
      this.sharedH = this.stopActive(this.sharedH)
    } else if (!this.sharedH || restartHorizontal) {
      this.sharedH = this.stopActive(this.sharedH)
      api.recorder.resetStream()
      this.sharedH = this.spawnRecorder(this.horizontalStream(), (chunk) => {
        if (this.streaming) api.multistream.pushChunk('horizontal', chunk)
        if (this.streaming || this.recording) api.recorder.pushChunk(chunk)
      })
    }

    if (!wantV) {
      this.liveV = this.stopActive(this.liveV)
    } else if (!this.liveV) {
      this.liveV = this.spawnRecorder(this.verticalStream(), (chunk: Uint8Array) => {
        if (this.streaming && this.verticalWanted) {
          const orient: CanvasOrientation = 'vertical'
          api.multistream.pushChunk(orient, chunk)
        }
      })
    }
  }

  setStreaming(on: boolean, withVertical: boolean): void {
    // Se a gravação local já estava rodando e a live vai entrar agora, reiniciamos o
    // encoder 16:9 uma vez para que o FFmpeg RTMP receba o cabeçalho EBML inicial.
    const startingLiveWhileRecording = on && !this.streaming && this.recording
    this.streaming = on
    this.verticalWanted = withVertical
    this.reconcile(startingLiveWhileRecording)
  }

  setRecording(on: boolean): void {
    this.recording = on
    this.reconcile(false)
  }

  dispose(): void {
    this.streaming = false
    this.verticalWanted = false
    this.recording = false
    this.reconcile(false)
  }
}
