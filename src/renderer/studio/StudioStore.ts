import { useEffect, useRef, useSyncExternalStore } from 'react'
import type { AppSettings } from '@shared/ipc'
import type {
  AudioLevelSample,
  AudioMixerSettings,
  ChatSourceStatus,
  ConnectedViewer,
  DiscoveredHost,
  LiveInfo,
  MultiChatMessage,
  MultistreamStatus,
  RecorderStatus,
  StudioScenesState,
  TelemetryStats
} from '@shared/protocol'
import { ensureBridge } from '@/bridge'

export type ToastTone = 'info' | 'good' | 'bad'

export interface ToastAction {
  label: string
  run: () => void
}

export interface ToastItem {
  id: number
  message: string
  tone: ToastTone
  action?: ToastAction
}

export interface StudioRootState {
  settings: AppSettings | null
  live: MultistreamStatus
  recorder: RecorderStatus
  telemetry: TelemetryStats
  chatMessages: MultiChatMessage[]
  chatStatus: ChatSourceStatus[]
  lanViewers: ConnectedViewer[]
  lanHosts: DiscoveredHost[]
  toasts: ToastItem[]
}

const DEFAULT_MULTISTREAM: MultistreamStatus = {
  active: false,
  encoder: 'libx264',
  ffmpegFound: true,
  destinations: []
}

const DEFAULT_RECORDER: RecorderStatus = {
  recording: false,
  recordingPath: null,
  recordingDurationSec: 0,
  replayBufferActive: true,
  replayBufferedSeconds: 0,
  replaySavedPath: null
}

const DEFAULT_TELEMETRY: TelemetryStats = {
  compositorFps: 0,
  targetFps: 60,
  renderMsP95: 0,
  droppedFrames: 0,
  encodeBitrateKbps: 0,
  encoder: 'libx264',
  cpuFallback: false,
  micMutedSpeaking: false,
  replayBufferedSeconds: 0
}

type LevelListener = (levels: Record<string, AudioLevelSample>) => void

/**
 * Store externo reativo baseado em `useSyncExternalStore` + seletores granulares.
 *
 * - Elimina re-renders globais em idle (1 Hz) e em sliders/toasts.
 * - Níveis VU (25 Hz) ficam em buffer mutável fora da árvore React para os medidores
 *   atualizarem via ref/DOM diretamente sem disparar renderizações React.
 */
class StudioStoreImpl {
  private state: StudioRootState = {
    settings: null,
    live: DEFAULT_MULTISTREAM,
    recorder: DEFAULT_RECORDER,
    telemetry: DEFAULT_TELEMETRY,
    chatMessages: [],
    chatStatus: [],
    lanViewers: [],
    lanHosts: [],
    toasts: []
  }

  private readonly listeners = new Set<() => void>()
  private readonly levelListeners = new Set<LevelListener>()
  readonly audioLevelsRef: Record<string, AudioLevelSample> = {}
  private nextToastId = 1
  private initialized = false
  private unsubscribers: Array<() => void> = []

  init(): void {
    if (this.initialized) return
    this.initialized = true
    const api = ensureBridge()

    void api.settings.get().then((settings) => {
      this.setState({ settings })
    })
    void api.multistream.status().then((live) => {
      this.setState({ live })
    })
    void api.recorder.status().then((recorder) => {
      this.setState({ recorder })
    })
    void api.chat.multichatStatus().then((chatStatus) => {
      this.setState({ chatStatus })
    })

    this.unsubscribers.push(
      api.settings.onChanged((settings) => {
        this.setState({ settings })
      }),
      api.host.onViewers((lanViewers) => {
        this.setState({ lanViewers })
      }),
      api.discovery.onHosts((lanHosts) => {
        this.setState({ lanHosts })
      }),
      api.chat.onMultichat((ev) => {
        if (ev.t === 'messages') {
          const seen = new Set(this.state.chatMessages.map((m) => `${m.platform}:${m.id}`))
          const fresh = ev.messages.filter((m) => !seen.has(`${m.platform}:${m.id}`))
          if (fresh.length === 0) return
          this.setState({
            chatMessages: [...this.state.chatMessages, ...fresh].slice(-400)
          })
        } else if (ev.t === 'delete') {
          this.setState({
            chatMessages: this.state.chatMessages.filter(
              (m) => !(m.platform === ev.platform && m.id === ev.id)
            )
          })
        } else if (ev.t === 'status') {
          this.setState({ chatStatus: ev.status })
        }
      })
    )

    if (api.audio.onLevels) {
      this.unsubscribers.push(
        api.audio.onLevels((samples) => {
          for (const s of samples) {
            this.audioLevelsRef[s.id] = s
          }
          for (const cb of this.levelListeners) cb(this.audioLevelsRef)
        })
      )
    }

    if (api.telemetry?.onStats) {
      this.unsubscribers.push(
        api.telemetry.onStats((telemetry) => {
          this.setState({ telemetry })
        })
      )
    }
  }

  dispose(): void {
    for (const off of this.unsubscribers) off()
    this.unsubscribers = []
    this.initialized = false
  }

  private setState(patch: Partial<StudioRootState>): void {
    this.state = { ...this.state, ...patch }
    for (const cb of this.listeners) cb()
  }

  subscribe = (cb: () => void): (() => void) => {
    this.init()
    this.listeners.add(cb)
    return () => {
      this.listeners.delete(cb)
    }
  }

  getSnapshot = (): StudioRootState => this.state

  subscribeLevels = (cb: LevelListener): (() => void) => {
    this.init()
    this.levelListeners.add(cb)
    return () => {
      this.levelListeners.delete(cb)
    }
  }

  /** Aplica alteração otimista local e delega persistência/autoridade para o backend Rust. */
  async updateSettings(patch: Partial<AppSettings>): Promise<AppSettings> {
    const api = ensureBridge()
    if (this.state.settings) {
      this.setState({ settings: { ...this.state.settings, ...patch } })
    }
    const next = await api.settings.set(patch)
    this.setState({ settings: next })
    return next
  }

  setLiveStatus(live: MultistreamStatus): void {
    this.setState({ live })
  }

  setRecorderStatus(recorder: RecorderStatus): void {
    this.setState({ recorder })
  }

  pushToast(message: string, tone: ToastTone = 'info', action?: ToastAction): number {
    const id = this.nextToastId++
    this.setState({
      toasts: [...this.state.toasts.slice(-2), { id, message, tone, action }]
    })
    window.setTimeout(() => this.dismissToast(id), action ? 8000 : 4500)
    return id
  }

  dismissToast(id: number): void {
    const next = this.state.toasts.filter((t) => t.id !== id)
    if (next.length !== this.state.toasts.length) {
      this.setState({ toasts: next })
    }
  }
}

export const studioStore = new StudioStoreImpl()

function shallowEqual<T>(a: T, b: T): boolean {
  if (Object.is(a, b)) return true
  if (typeof a !== 'object' || a === null || typeof b !== 'object' || b === null) return false
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false
    for (let i = 0; i < a.length; i++) {
      if (!Object.is(a[i], b[i])) return false
    }
    return true
  }
  const ka = Object.keys(a) as Array<keyof T>
  const kb = Object.keys(b) as Array<keyof T>
  if (ka.length !== kb.length) return false
  for (const k of ka) {
    if (!Object.is(a[k], b[k])) return false
  }
  return true
}

/** Seletor granular com comparação rasa (re-renderiza só quando o slice selecionado muda). */
export function useStudioSelector<T>(selector: (state: StudioRootState) => T): T {
  const lastRef = useRef<{ has: boolean; value: T }>({ has: false, value: undefined as unknown as T })
  return useSyncExternalStore(studioStore.subscribe, () => {
    const next = selector(studioStore.getSnapshot())
    if (lastRef.current.has && shallowEqual(lastRef.current.value, next)) {
      return lastRef.current.value
    }
    lastRef.current = { has: true, value: next }
    return next
  })
}

export const useSettingsSlice = (): AppSettings | null => useStudioSelector((s) => s.settings)

export const useScenesSlice = (): StudioScenesState | null =>
  useStudioSelector((s) => s.settings?.scenes ?? null)

export const useMixerSlice = (): AudioMixerSettings | null =>
  useStudioSelector((s) => s.settings?.audioMixer ?? null)

export const useLiveInfoSlice = (): LiveInfo | null =>
  useStudioSelector((s) => s.settings?.live ?? null)

export const useLiveSlice = (): MultistreamStatus => useStudioSelector((s) => s.live)

export const useRecorderSlice = (): RecorderStatus => useStudioSelector((s) => s.recorder)

export const useTelemetrySlice = (): TelemetryStats => useStudioSelector((s) => s.telemetry)

export const useChatSlice = (): { messages: MultiChatMessage[]; status: ChatSourceStatus[] } =>
  useStudioSelector((s) => ({ messages: s.chatMessages, status: s.chatStatus }))

export const useLanSlice = (): { viewers: ConnectedViewer[]; hosts: DiscoveredHost[] } =>
  useStudioSelector((s) => ({ viewers: s.lanViewers, hosts: s.lanHosts }))

export const useToastsSlice = (): ToastItem[] => useStudioSelector((s) => s.toasts)

/** Inscreve em níveis VU a 25 Hz sem disparar re-renders React. */
export function useAudioLevelRef(
  channelId: string,
  onLevel: (sample: AudioLevelSample) => void
): void {
  const cbRef = useRef(onLevel)
  cbRef.current = onLevel
  useEffect(() => {
    return studioStore.subscribeLevels((all) => {
      const sample = all[channelId]
      if (sample) cbRef.current(sample)
    })
  }, [channelId])
}
