import type { DesktopSource } from '@shared/ipc'
import { ensureBridge } from '@/bridge'

export type CaptureState = 'idle' | 'loading' | 'live' | 'missing' | 'error'

export interface Capture {
  key: string
  video: HTMLVideoElement
  stream: MediaStream | null
  state: CaptureState
  label: string
}

export interface SyncCaptureSpec {
  open: () => Promise<{ stream: MediaStream; label: string } | null>
  /** Se false, registra a fonte em estado `idle` sem abrir `getDisplayMedia()` no boot. */
  autoStart?: boolean
  initialLabel?: string
}

type Entry = Capture

function makeVideo(): HTMLVideoElement {
  const v = document.createElement('video')
  v.muted = true
  v.playsInline = true
  v.autoplay = true
  return v
}

function isUserCanceledCapture(err: unknown): boolean {
  if (err instanceof DOMException) {
    return err.name === 'NotAllowedError' || err.name === 'AbortError'
  }
  return false
}

export class CaptureManager {
  private readonly entries = new Map<string, Entry>()
  private queue: Promise<unknown> = Promise.resolve()
  private fps = 60
  private systemAudio: MediaStream | null = null
  private systemAudioPromise: Promise<MediaStream | null> | null = null
  private disposed = false
  onChange: (() => void) | null = null

  setFps(fps: number): void {
    this.fps = fps
    for (const [key, entry] of this.entries) {
      if (key.startsWith('display:') && entry.stream) {
        const track = entry.stream.getVideoTracks()[0]
        if (track) {
          void track.applyConstraints({ frameRate: { ideal: fps, max: fps } }).catch(() => {})
        }
      }
    }
  }

  get(key: string): Capture | null {
    return this.entries.get(key) ?? null
  }

  hasSystemAudio(): boolean {
    return Boolean(
      this.systemAudio && this.systemAudio.getAudioTracks().some((t) => t.readyState === 'live')
    )
  }

  static displayKey(captureId: string | undefined, captureName: string | undefined): string {
    return `display:${captureId ?? 'primary'}:${captureName ?? ''}`
  }

  static cameraKey(deviceId: string | undefined): string {
    return `camera:${deviceId ?? 'default'}`
  }

  private serial<T>(fn: () => Promise<T>): Promise<T> {
    const next = this.queue.then(fn, fn)
    this.queue = next.catch(() => {})
    return next
  }

  private set(entry: Entry, patch: Partial<Entry>): void {
    Object.assign(entry, patch)
    this.onChange?.()
  }

  private runEntryOpen(
    key: string,
    entry: Entry,
    open: () => Promise<{ stream: MediaStream; label: string } | null>,
    userInitiated: boolean
  ): Promise<void> {
    const runOpen = async (): Promise<void> => {
      try {
        const res = await open()
        if (this.disposed || this.entries.get(key) !== entry) {
          res?.stream.getTracks().forEach((t) => t.stop())
          return
        }
        if (!res) {
          this.set(entry, { state: userInitiated ? 'idle' : 'missing' })
          return
        }
        entry.video.srcObject = res.stream
        await entry.video.play().catch(() => {})
        const track = res.stream.getVideoTracks()[0]
        track?.addEventListener('ended', () => {
          if (!this.disposed && this.entries.get(key) === entry) {
            this.set(entry, { state: 'missing', stream: null })
          }
        })
        this.set(entry, { stream: res.stream, state: 'live', label: res.label })
      } catch (err) {
        if (isUserCanceledCapture(err)) {
          if (!this.disposed && this.entries.get(key) === entry) {
            this.set(entry, { state: 'idle' })
          }
          return
        }
        console.warn('[capture] falhou:', key, err)
        if (!this.disposed && this.entries.get(key) === entry) {
          this.set(entry, { state: 'error' })
        }
      }
    }

    // Câmeras não usam sources.select() no main, portanto abrem fora da fila serial de display.
    if (key.startsWith('camera:')) {
      return runOpen()
    }
    return this.serial(runOpen)
  }

  sync(wanted: Map<string, SyncCaptureSpec>): void {
    if (this.disposed) return
    for (const [key, entry] of this.entries) {
      if (!wanted.has(key)) {
        entry.stream?.getTracks().forEach((t) => t.stop())
        entry.video.srcObject = null
        this.entries.delete(key)
      }
    }
    for (const [key, spec] of wanted) {
      if (this.entries.has(key)) continue
      const autoStart = spec.autoStart !== false
      const entry: Entry = {
        key,
        video: makeVideo(),
        stream: null,
        state: autoStart ? 'loading' : 'idle',
        label: spec.initialLabel ?? ''
      }
      this.entries.set(key, entry)

      if (autoStart) {
        void this.runEntryOpen(key, entry, spec.open, false)
      }
    }
    this.onChange?.()
  }

  /** Inicia (ou reinicia) uma captura sob demanda a partir de uma ação explícita do usuário. */
  start(
    key: string,
    open: () => Promise<{ stream: MediaStream; label: string } | null>,
    initialLabel = ''
  ): Promise<void> {
    if (this.disposed) return Promise.resolve()
    const existing = this.entries.get(key)
    if (existing) {
      existing.stream?.getTracks().forEach((t) => t.stop())
      existing.video.srcObject = null
    }
    const entry: Entry = {
      key,
      video: existing?.video ?? makeVideo(),
      stream: null,
      state: 'loading',
      label: initialLabel || existing?.label || ''
    }
    this.entries.set(key, entry)
    this.onChange?.()
    return this.runEntryOpen(key, entry, open, true)
  }

  retry(key: string): void {
    const entry = this.entries.get(key)
    if (!entry) return
    this.entries.delete(key)
    entry.stream?.getTracks().forEach((t) => t.stop())
    entry.video.srcObject = null
    this.onChange?.()
  }

  openDisplay(
    captureId: string | undefined,
    captureName: string | undefined,
    opts?: {
      withAudio?: boolean
      onAudioStream?: (stream: MediaStream) => void
    }
  ) {
    return async (): Promise<{ stream: MediaStream; label: string } | null> => {
      const api = ensureBridge()
      let resolvedName = captureName || 'Tela capturada'
      try {
        const sources = await api.sources.list()
        const source = resolveSource(sources, captureId, captureName)
        if (source) {
          resolvedName = source.name
          await api.sources.select(source.id, Boolean(opts?.withAudio)).catch(() => {})
        }
      } catch {
        /* fallback direto para o seletor nativo do WebView */
      }

      const videoConstraints: MediaTrackConstraints = {
        frameRate: { ideal: this.fps, max: this.fps }
      }
      let stream: MediaStream
      if (opts?.withAudio) {
        try {
          stream = await navigator.mediaDevices.getDisplayMedia({
            video: videoConstraints,
            audio: true
          })
        } catch (err) {
          if (isUserCanceledCapture(err)) throw err
          stream = await navigator.mediaDevices.getDisplayMedia({
            video: videoConstraints,
            audio: false
          })
        }
      } else {
        stream = await navigator.mediaDevices.getDisplayMedia({
          video: videoConstraints,
          audio: false
        })
      }

      const audioTracks = stream.getAudioTracks()
      if (audioTracks.length > 0 && opts?.onAudioStream) {
        const audioStream = new MediaStream(audioTracks)
        for (const t of audioTracks) {
          stream.removeTrack(t)
        }
        const primaryAudio = audioTracks[0]
        primaryAudio.addEventListener('ended', () => {
          if (this.systemAudio === audioStream) {
            this.systemAudio = null
            this.systemAudioPromise = null
          }
        })
        this.systemAudio?.getTracks().forEach((t) => t.stop())
        this.systemAudio = audioStream
        opts.onAudioStream(audioStream)
      }

      const trackLabel = stream.getVideoTracks()[0]?.label?.trim()
      return { stream, label: trackLabel || resolvedName }
    }
  }

  openCamera(deviceId: string | undefined) {
    return async (): Promise<{ stream: MediaStream; label: string } | null> => {
      try {
        const stream = await navigator.mediaDevices.getUserMedia({
          video: {
            ...(deviceId && deviceId !== 'default' ? { deviceId: { exact: deviceId } } : {}),
            width: { ideal: 1920 },
            height: { ideal: 1080 },
            frameRate: { ideal: 30 }
          },
          audio: false
        })
        return { stream, label: stream.getVideoTracks()[0]?.label ?? 'Câmera' }
      } catch (err) {
        if (deviceId && deviceId !== 'default') {
          try {
            const stream = await navigator.mediaDevices.getUserMedia({
              video: {
                width: { ideal: 1920 },
                height: { ideal: 1080 },
                frameRate: { ideal: 30 }
              },
              audio: false
            })
            return { stream, label: stream.getVideoTracks()[0]?.label ?? 'Câmera' }
          } catch {
            /* fallback falhou */
          }
        }
        throw err
      }
    }
  }

  acquireSystemAudio(): Promise<MediaStream | null> {
    if (this.disposed) return Promise.resolve(null)
    if (this.hasSystemAudio()) return Promise.resolve(this.systemAudio)
    this.systemAudioPromise ??= this.serial(async () => {
      try {
        const api = ensureBridge()
        try {
          const sources = await api.sources.list()
          const screen = sources.find((s) => s.kind === 'screen')
          if (screen) {
            await api.sources.select(screen.id, true).catch(() => {})
          }
        } catch {
          /* segue para getDisplayMedia */
        }
        const stream = await navigator.mediaDevices.getDisplayMedia({
          video: { frameRate: 1, width: 64, height: 36 },
          audio: true
        })
        stream.getVideoTracks().forEach((t) => {
          t.stop()
          stream.removeTrack(t)
        })
        if (this.disposed) {
          stream.getTracks().forEach((t) => t.stop())
          this.systemAudioPromise = null
          return null
        }
        const audioTrack = stream.getAudioTracks()[0]
        if (!audioTrack) {
          this.systemAudioPromise = null
          return null
        }
        audioTrack.addEventListener('ended', () => {
          if (this.systemAudio === stream) {
            this.systemAudio = null
            this.systemAudioPromise = null
          }
        })
        this.systemAudio = stream
        return stream
      } catch (err) {
        if (!isUserCanceledCapture(err)) {
          console.warn('[capture] audio do sistema indisponivel:', err)
        }
        this.systemAudioPromise = null
        return null
      }
    })
    return this.systemAudioPromise
  }

  dispose(): void {
    this.disposed = true
    for (const entry of this.entries.values()) {
      entry.stream?.getTracks().forEach((t) => t.stop())
      entry.video.srcObject = null
    }
    this.entries.clear()
    this.systemAudio?.getTracks().forEach((t) => t.stop())
    this.systemAudio = null
    this.systemAudioPromise = null
  }
}

export function resolveSource(
  sources: DesktopSource[],
  captureId: string | undefined,
  captureName: string | undefined
): DesktopSource | null {
  if (!captureId) return sources.find((s) => s.kind === 'screen') ?? null
  const byId = sources.find((s) => s.id === captureId)
  if (byId) return byId
  if (!captureName) return null
  const byName = sources.find((s) => s.name === captureName)
  if (byName) return byName
  const stem = captureName.split(' - ').pop()
  return stem ? (sources.find((s) => s.kind === 'window' && s.name.endsWith(stem)) ?? null) : null
}
