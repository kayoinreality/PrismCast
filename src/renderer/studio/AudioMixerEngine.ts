import type { AudioChannelStatus, AudioMixerSettings } from '@shared/protocol'
import { ensureBridge } from '@/bridge'
import { studioStore } from './StudioStore'

export interface ChannelLevel {
  peakDb: number
  rmsDb: number
  inputDb: number
}

export type AudioLevels = Record<string, ChannelLevel>

/**
 * Fachada fina de compatibilidade: o processamento de áudio/DSP/WASAPI roda
 * nativamente em `src-rust/audio_mixer.rs` (este stub mantém apenas a assinatura
 * esperada pelo wrapper legado de estúdio enquanto a UI migra 100% para StudioStore).
 */
export class AudioMixerEngine {
  private statuses = new Map<string, AudioChannelStatus>()
  private readonly offStatus: () => void
  onStatus: (() => void) | null = null

  constructor(initial: AudioMixerSettings) {
    const api = ensureBridge()
    this.offStatus = api.audio.onStatus((list) => {
      this.statuses = new Map(list.map((s) => [s.id, s]))
      this.onStatus?.()
    })
    void this.applyConfig(initial)
  }

  get outputTrack(): MediaStreamTrack | undefined {
    return undefined
  }

  async resume(): Promise<void> {}
  setSystemStream(_stream: MediaStream | null): void {}
  setSceneMics(_live: Set<string>, _used: Set<string>): void {}
  micOnAir(_id: string): boolean {
    return true
  }

  status(id: string): AudioChannelStatus {
    return this.statuses.get(id) ?? { id, state: 'ok', message: null }
  }

  async applyConfig(_next: AudioMixerSettings): Promise<void> {}
  retry(_id: string): void {}
  retryAllProblematic(): void {}

  readLevels(): AudioLevels {
    const out: AudioLevels = {}
    for (const [id, s] of Object.entries(studioStore.audioLevelsRef)) {
      out[id] = { peakDb: s.peakDb, rmsDb: s.peakDb, inputDb: s.inputDb }
    }
    return out
  }

  dispose(): void {
    this.offStatus()
  }
}
