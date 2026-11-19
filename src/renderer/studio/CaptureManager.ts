import type { DesktopSource } from '@shared/ipc'
import { ensureBridge } from '@/bridge'

export type CaptureState = 'loading' | 'live' | 'missing' | 'error'

export interface Capture {
  key: string
  video: HTMLVideoElement
  stream: MediaStream | null
  state: CaptureState
  label: string
}

type Entry = Capture

function makeVideo(): HTMLVideoElement {
  const v = document.createElement('video')
  v.muted = true
  v.playsInline = true
  v.autoplay = true
  return v
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

  sync(wanted: Map<string, () => Promise<{ stream: MediaStream; label: string } | null>>): void {
    if (this.disposed) return
    for (const [key, entry] of this.entries) {
      if (!wanted.has(key)) {
        entry.stream?.getTracks().forEach((t) => t.stop())
        entry.video.srcObject = null
        this.entries.delete(key)
      }
    }
    for (const [key, open] of wanted) {
      if (this.entries.has(key)) continue
      const entry: Entry = { key, video: makeVideo(), stream: null, state: 'loading', label: '' }
      this.entries.set(key, entry)

      const runOpen = async (): Promise<void> => {
        try {
          const res = await open()
          if (this.disposed || this.entries.get(key) !== entry) {
            res?.stream.getTracks().forEach((t) => t.stop())
            return
          }
          if (!res) {
            this.set(entry, { state: 'missing' })
            return
          }
          entry.video.srcObject = res.stream
          await entry.video.play().catch(() => {})
          const track = res.stream.getVideoTracks()[0]
          track?.addEventListener('ended', () => {
            if (!this.disposed && this.entries.get(key) === entry) {
              this.set(entry, { state: 'missing' })
            }
          })
          this.set(entry, { stream: res.stream, state: 'live', label: res.label })
        } catch (err) {
          console.warn('[capture] falhou:', key, err)
          if (!this.disposed && this.entries.get(key) === entry) {
            this.set(entry, { state: 'error' })
          }
        }
      }

      // Câmeras não usam sources.select() no main, portanto abrem fora da fila serial de display.
      if (key.startsWith('camera:')) {
        void runOpen()
      } else {
        void this.serial(runOpen)
      }
    }
    this.onChange?.()
  }

  retry(key: string): void {
    const entry = this.entries.get(key)
    if (!entry) return
    this.entries.delete(key)
    entry.stream?.getTracks().forEach((t) => t.stop())
    entry.video.srcObject = null
    this.onChange?.()
  }

  openDisplay(captureId: string | undefined, captureName: string | undefined) {
    return async (): Promise<{ stream: MediaStream; label: string } | null> => {
      const api = ensureBridge()
      const sources = await api.sources.list()
      const source = resolveSource(sources, captureId, captureName)
      if (!source) return null
      await api.sources.select(source.id, false)
      const stream = await navigator.mediaDevices.getDisplayMedia({
        video: { frameRate: { ideal: this.fps, max: this.fps } },
        audio: false
      })
      return { stream, label: source.name }
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
    this.systemAudioPromise ??= this.serial(async () => {
      try {
        const api = ensureBridge()
        const sources = await api.sources.list()
        const screen = sources.find((s) => s.kind === 'screen')
        if (!screen) {
          this.systemAudioPromise = null
          return null
        }
        await api.sources.select(screen.id, true)
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
        console.warn('[capture] audio do sistema indisponivel:', err)
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
