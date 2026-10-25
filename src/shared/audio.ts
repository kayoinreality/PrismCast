import type { AudioChannel, AudioChannelKind, AudioFilters, AudioMixerSettings } from './protocol'

/**
 * Padrões das fontes de áudio. Ficam no shared porque o main precisa deles para
 * migrar as preferências antigas e a interface para criar canais novos.
 */

export const MIN_DB = -60
export const MAX_DB = 12

export function defaultFilters(kind: AudioChannelKind): AudioFilters {
  const mic = kind === 'mic'
  return {
    noiseSuppression: mic,
    echoCancellation: mic,
    autoGain: false,
    highpassHz: mic ? 85 : 0,
    gate: { enabled: false, thresholdDb: -45, releaseMs: 150 },
    compressor: { enabled: mic, thresholdDb: -18, ratio: 4, attackMs: 6, releaseMs: 150, makeupDb: 0 },
    gainDb: 0,
    limiter: { enabled: mic, ceilingDb: -1 }
  }
}

const KIND_NAME: Record<AudioChannelKind, string> = {
  desktop: 'Áudio do PC',
  mic: 'Microfone',
  app: 'Aplicativo'
}

export function makeChannel(
  kind: AudioChannelKind,
  patch: Partial<AudioChannel> = {}
): AudioChannel {
  return {
    id: `${kind}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`,
    kind,
    name: KIND_NAME[kind],
    deviceId: null,
    appExe: null,
    volumeDb: 0,
    muted: false,
    monitor: 'off',
    syncOffsetMs: 0,
    filters: defaultFilters(kind),
    ...patch
  }
}

export function defaultMixer(): AudioMixerSettings {
  return {
    channels: [makeChannel('desktop', { id: 'desktop' }), makeChannel('mic', { id: 'mic' })],
    partyVoiceEnabled: true
  }
}

/** Completa um canal salvo por uma versão anterior com os campos novos. */
export function normalizeChannel(raw: Partial<AudioChannel>): AudioChannel | null {
  if (!raw || typeof raw.id !== 'string' || !raw.kind || !(raw.kind in KIND_NAME)) return null
  const base = makeChannel(raw.kind, { id: raw.id })
  const f = (raw.filters ?? {}) as Partial<AudioFilters>
  return {
    ...base,
    ...raw,
    filters: {
      ...base.filters,
      ...f,
      gate: { ...base.filters.gate, ...(f.gate ?? {}) },
      compressor: { ...base.filters.compressor, ...(f.compressor ?? {}) },
      limiter: { ...base.filters.limiter, ...(f.limiter ?? {}) }
    }
  } as AudioChannel
}

interface LegacyMixer {
  systemAudioEnabled?: boolean
  systemVolume?: number
  systemMuted?: boolean
  micEnabled?: boolean
  micDeviceId?: string | null
  micVolume?: number
  micMuted?: boolean
  noiseSuppression?: boolean
  echoCancellation?: boolean
  compressorEnabled?: boolean
  partyVoiceEnabled?: boolean
}

export const linearToDb = (v: number): number => (v <= 0.001 ? MIN_DB : Math.max(MIN_DB, 20 * Math.log10(v)))
export const dbToLinear = (db: number): number => (db <= MIN_DB ? 0 : Math.pow(10, db / 20))

/** Mixer da 2.2 (áudio do PC + um microfone) para canais. */
export function normalizeMixer(raw: unknown): AudioMixerSettings {
  const base = defaultMixer()
  if (!raw || typeof raw !== 'object') return base
  const r = raw as Partial<AudioMixerSettings> & LegacyMixer
  if (Array.isArray(r.channels)) {
    return {
      channels: r.channels.map(normalizeChannel).filter((c): c is AudioChannel => c !== null),
      partyVoiceEnabled: r.partyVoiceEnabled ?? true
    }
  }
  const channels: AudioChannel[] = []
  if (r.systemAudioEnabled !== false) {
    channels.push(
      makeChannel('desktop', {
        id: 'desktop',
        volumeDb: linearToDb(r.systemVolume ?? 1),
        muted: Boolean(r.systemMuted)
      })
    )
  }
  const mic = makeChannel('mic', {
    id: 'mic',
    deviceId: r.micDeviceId ?? null,
    volumeDb: linearToDb(r.micVolume ?? 1),
    muted: Boolean(r.micMuted) || r.micEnabled === false
  })
  mic.filters.noiseSuppression = r.noiseSuppression ?? true
  mic.filters.echoCancellation = r.echoCancellation ?? true
  mic.filters.compressor.enabled = r.compressorEnabled ?? true
  channels.push(mic)
  return { channels, partyVoiceEnabled: r.partyVoiceEnabled ?? true }
}
