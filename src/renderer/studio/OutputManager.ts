import type { CanvasOrientation } from '@shared/protocol'

/**
 * Grava os canvases com MediaRecorder e distribui os pedaços:
 *   16:9 -> FFmpeg dos destinos horizontais + gravação local/replay
 *   9:16 -> FFmpeg dos destinos verticais (só existe com destino vertical ligado)
 *
 * O WebM sai em H.264 de alto bitrate (o Chromium usa o encoder de hardware) e
 * é recodificado pelo FFmpeg no bitrate de cada plataforma.
 */

const MIME_CANDIDATES = [
  'video/x-matroska;codecs=avc1,opus',
  'video/webm;codecs=h264,opus',
  'video/webm;codecs=vp8,opus'
]

const TIMESLICE_MS = 200
const INTERMEDIATE_BPS = 24_000_000

function pickMime(): string {
  return MIME_CANDIDATES.find((m) => MediaRecorder.isTypeSupported(m)) ?? 'video/webm'
}

type Sink = (chunk: Uint8Array) => void

/**
 * Live e gravação usam MediaRecorders separados: cada FFmpeg precisa receber o
 * WebM desde o cabeçalho, então a live não pode "pegar carona" num gravador
 * que já estava rodando para a gravação local (e vice-versa).
 */
export class OutputManager {
  private liveH: MediaRecorder | null = null
  private liveV: MediaRecorder | null = null
  private local: MediaRecorder | null = null
  private streaming = false
  private verticalWanted = false
  private recording = false

  constructor(
    private readonly horizontalStream: () => MediaStream,
    private readonly verticalStream: () => MediaStream
  ) {}

  private makeRecorder(stream: MediaStream, sink: Sink): MediaRecorder {
    const rec = new MediaRecorder(stream, {
      mimeType: pickMime(),
      videoBitsPerSecond: INTERMEDIATE_BPS,
      audioBitsPerSecond: 192_000
    })
    rec.ondataavailable = (e) => {
      if (e.data.size === 0) return
      void e.data.arrayBuffer().then((buf) => sink(new Uint8Array(buf)))
    }
    rec.start(TIMESLICE_MS)
    return rec
  }

  private live(orientation: CanvasOrientation): Sink {
    return (chunk) => window.api.multistream.pushChunk(orientation, chunk)
  }

  /** Liga/desliga os gravadores conforme live e gravação. */
  private reconcile(): void {
    const toggle = (
      current: MediaRecorder | null,
      want: boolean,
      open: () => MediaRecorder
    ): MediaRecorder | null => {
      if (want && !current) return open()
      if (!want && current) {
        if (current.state !== 'inactive') current.stop()
        return null
      }
      return current
    }

    this.liveH = toggle(this.liveH, this.streaming, () =>
      this.makeRecorder(this.horizontalStream(), this.live('horizontal'))
    )
    this.liveV = toggle(this.liveV, this.streaming && this.verticalWanted, () =>
      this.makeRecorder(this.verticalStream(), this.live('vertical'))
    )
    // O gravador local roda durante a live também: é ele que alimenta o replay buffer.
    this.local = toggle(this.local, this.streaming || this.recording, () => {
      window.api.recorder.resetStream()
      return this.makeRecorder(this.horizontalStream(), (c) => window.api.recorder.pushChunk(c))
    })
  }

  setStreaming(on: boolean, withVertical: boolean): void {
    this.streaming = on
    this.verticalWanted = withVertical
    this.reconcile()
  }

  setRecording(on: boolean): void {
    this.recording = on
    this.reconcile()
  }

  dispose(): void {
    this.streaming = false
    this.recording = false
    this.reconcile()
  }
}
