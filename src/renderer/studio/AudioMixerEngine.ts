import type { AudioChannel, AudioChannelStatus, AudioMixerSettings } from '@shared/protocol'
import { dbToLinear, MIN_DB } from '@shared/audio'
import workletUrl from './audio-worklets.ts?worker&url'

export interface ChannelLevel {
  /** pico e RMS depois do fader, em dBFS (-60 = silêncio) */
  peakDb: number
  rmsDb: number
  /** nível que chega na fonte, antes dos filtros: para acertar o noise gate */
  inputDb: number
}

export type AudioLevels = Record<string, ChannelLevel>

const SILENT: ChannelLevel = { peakDb: MIN_DB, rmsDb: MIN_DB, inputDb: MIN_DB }
const RAMP = 0.015

function toDb(v: number): number {
  return v <= 0.000_001 ? MIN_DB : Math.max(MIN_DB, 20 * Math.log10(v))
}

/**
 * Uma fonte de áudio do mixer, com a cadeia de filtros fixa:
 *
 *   entrada → atraso → passa-alta → noise gate → compressor → ganho → limitador
 *           → fader → mudo → (live, fone)
 *
 * Filtro desligado continua no caminho em modo neutro: religar não estala nem
 * precisa refazer as ligações.
 */
class ChannelStrip {
  readonly delay: DelayNode
  readonly highpass: BiquadFilterNode
  readonly gate: AudioWorkletNode | null
  readonly compressor: DynamicsCompressorNode
  readonly makeup: GainNode
  readonly gain: GainNode
  readonly limiter: DynamicsCompressorNode
  readonly fader: GainNode
  readonly mute: GainNode
  readonly toBus: GainNode
  readonly toMonitor: GainNode
  readonly meter: AnalyserNode
  readonly inputMeter: AnalyserNode
  private readonly buf = new Float32Array(1024)

  source: AudioNode | null = null
  /** o que abriu a fonte (para saber quando reabrir) */
  sourceKey = ''
  stream: MediaStream | null = null
  player: AudioWorkletNode | null = null
  status: AudioChannelStatus

  constructor(
    private readonly ctx: AudioContext,
    readonly id: string,
    bus: AudioNode,
    monitorBus: AudioNode,
    workletReady: boolean
  ) {
    this.status = { id, state: 'off', message: null }
    this.delay = ctx.createDelay(2)
    this.highpass = ctx.createBiquadFilter()
    this.highpass.type = 'highpass'
    this.gate = workletReady ? new AudioWorkletNode(ctx, 'noise-gate', { outputChannelCount: [2] }) : null
    this.compressor = ctx.createDynamicsCompressor()
    this.makeup = ctx.createGain()
    this.gain = ctx.createGain()
    this.limiter = ctx.createDynamicsCompressor()
    this.limiter.knee.value = 0
    this.limiter.ratio.value = 20
    this.limiter.attack.value = 0.001
    this.limiter.release.value = 0.06
    this.fader = ctx.createGain()
    this.mute = ctx.createGain()
    this.toBus = ctx.createGain()
    this.toMonitor = ctx.createGain()
    this.toMonitor.gain.value = 0
    this.meter = ctx.createAnalyser()
    this.meter.fftSize = 1024
    this.inputMeter = ctx.createAnalyser()
    this.inputMeter.fftSize = 1024

    const chain: AudioNode[] = [this.delay, this.highpass]
    if (this.gate) chain.push(this.gate)
    chain.push(this.compressor, this.makeup, this.gain, this.limiter, this.fader, this.mute)
    for (let i = 0; i < chain.length - 1; i++) chain[i].connect(chain[i + 1])
    this.highpass.connect(this.inputMeter)
    this.mute.connect(this.meter)
    this.mute.connect(this.toBus)
    this.mute.connect(this.toMonitor)
    this.toBus.connect(bus)
    this.toMonitor.connect(monitorBus)
  }

  attach(node: AudioNode | null): void {
    if (this.source) {
      try {
        this.source.disconnect()
      } catch {
        /* já desligado */
      }
    }
    this.source = node
    node?.connect(this.delay)
  }

  apply(ch: AudioChannel): void {
    const t = this.ctx.currentTime
    const f = ch.filters
    this.delay.delayTime.setTargetAtTime(Math.max(0, Math.min(2, ch.syncOffsetMs / 1000)), t, RAMP)
    this.highpass.frequency.setTargetAtTime(f.highpassHz > 0 ? f.highpassHz : 10, t, RAMP)
    if (this.gate) {
      this.gate.parameters.get('thresholdDb')!.setValueAtTime(f.gate.enabled ? f.gate.thresholdDb : -100, t)
      this.gate.parameters.get('releaseMs')!.setValueAtTime(f.gate.releaseMs, t)
    }
    const c = f.compressor
    this.compressor.threshold.setValueAtTime(c.enabled ? c.thresholdDb : 0, t)
    this.compressor.ratio.setValueAtTime(c.enabled ? c.ratio : 1, t)
    this.compressor.knee.setValueAtTime(c.enabled ? 6 : 0, t)
    this.compressor.attack.setValueAtTime(c.attackMs / 1000, t)
    this.compressor.release.setValueAtTime(c.releaseMs / 1000, t)
    this.makeup.gain.setTargetAtTime(c.enabled ? dbToLinear(c.makeupDb) : 1, t, RAMP)
    this.gain.gain.setTargetAtTime(dbToLinear(f.gainDb), t, RAMP)
    this.limiter.threshold.setValueAtTime(f.limiter.enabled ? f.limiter.ceilingDb : 0, t)
    this.limiter.ratio.setValueAtTime(f.limiter.enabled ? 20 : 1, t)
    this.fader.gain.setTargetAtTime(dbToLinear(ch.volumeDb), t, RAMP)
    this.mute.gain.setTargetAtTime(ch.muted ? 0 : 1, t, RAMP)
    this.toBus.gain.setTargetAtTime(ch.monitor === 'monitor-only' ? 0 : 1, t, RAMP)
    this.toMonitor.gain.setTargetAtTime(ch.monitor === 'off' ? 0 : 1, t, RAMP)
  }

  private peak(analyser: AnalyserNode): { peak: number; rms: number } {
    analyser.getFloatTimeDomainData(this.buf)
    let peak = 0
    let sum = 0
    for (let i = 0; i < this.buf.length; i++) {
      const v = this.buf[i]
      const a = Math.abs(v)
      if (a > peak) peak = a
      sum += v * v
    }
    return { peak, rms: Math.sqrt(sum / this.buf.length) }
  }

  level(): ChannelLevel {
    if (!this.source) return SILENT
    const out = this.peak(this.meter)
    const input = this.peak(this.inputMeter)
    return { peakDb: toDb(out.peak), rmsDb: toDb(out.rms), inputDb: toDb(input.peak) }
  }

  dispose(): void {
    this.attach(null)
    this.stream?.getTracks().forEach((t) => t.stop())
    this.stream = null
    this.player?.port.close()
    this.player = null
    for (const n of [this.toBus, this.toMonitor, this.mute]) {
      try {
        n.disconnect()
      } catch {
        /* ignore */
      }
    }
  }
}

/**
 * Mixer de áudio da live, no estilo do OBS: cada fonte (áudio do PC,
 * microfones, um aplicativo só) tem fader, mudo, filtros e monitoramento, e
 * tudo vira uma única faixa de 48 kHz que vai para a live e a gravação.
 */
export class AudioMixerEngine {
  private readonly ctx: AudioContext
  private readonly destination: MediaStreamAudioDestinationNode
  private readonly bus: GainNode
  private readonly monitorBus: GainNode
  private readonly strips = new Map<string, ChannelStrip>()
  private readonly offPcm: () => void
  private readonly offStatus: () => void
  private workletReady = false
  private readonly workletLoading: Promise<void>
  private config: AudioMixerSettings
  private systemStream: MediaStream | null = null
  private appStatus = new Map<string, AudioChannelStatus>()
  private disposed = false
  onStatus: (() => void) | null = null

  constructor(initial: AudioMixerSettings) {
    this.config = initial
    this.ctx = new AudioContext({ sampleRate: 48000, latencyHint: 'interactive' })
    this.destination = this.ctx.createMediaStreamDestination()
    this.bus = this.ctx.createGain()
    this.bus.connect(this.destination)
    this.monitorBus = this.ctx.createGain()
    this.monitorBus.connect(this.ctx.destination)

    this.workletLoading = this.ctx.audioWorklet
      .addModule(workletUrl)
      .then(() => {
        this.workletReady = true
      })
      .catch((err) => console.warn('[mixer] worklets indisponíveis:', err))
      .finally(() => void this.applyConfig(this.config))

    this.offPcm = window.api.audio.onPcm((id, pcm) => {
      const player = this.strips.get(id)?.player
      if (player) player.port.postMessage(pcm, [pcm.buffer])
    })
    this.offStatus = window.api.audio.onStatus((list) => {
      this.appStatus = new Map(list.map((s) => [s.id, s]))
      this.onStatus?.()
    })
  }

  get outputTrack(): MediaStreamTrack {
    return this.destination.stream.getAudioTracks()[0]
  }

  setSystemStream(stream: MediaStream | null): void {
    this.systemStream = stream
    for (const ch of this.config.channels) {
      if (ch.kind === 'desktop') void this.openSource(ch, true)
    }
  }

  status(id: string): AudioChannelStatus {
    const app = this.appStatus.get(id)
    if (app) return app
    return this.strips.get(id)?.status ?? { id, state: 'off', message: null }
  }

  async applyConfig(next: AudioMixerSettings): Promise<void> {
    if (this.disposed) return
    this.config = next
    if (this.ctx.state === 'suspended') void this.ctx.resume().catch(() => {})
    await this.workletLoading
    if (this.disposed || this.config !== next) return

    const ids = new Set(next.channels.map((c) => c.id))
    for (const [id, strip] of this.strips) {
      if (!ids.has(id)) {
        strip.dispose()
        this.strips.delete(id)
      }
    }
    for (const ch of next.channels) {
      let strip = this.strips.get(ch.id)
      if (!strip) {
        strip = new ChannelStrip(this.ctx, ch.id, this.bus, this.monitorBus, this.workletReady)
        this.strips.set(ch.id, strip)
      }
      strip.apply(ch)
      void this.openSource(ch, false)
    }
    window.api.audio.sync(
      next.channels.filter((c) => c.kind === 'app' && c.appExe).map((c) => ({ id: c.id, exe: c.appExe! }))
    )
  }

  private sourceKey(ch: AudioChannel): string {
    switch (ch.kind) {
      case 'desktop':
        return `desktop:${this.systemStream?.id ?? 'none'}`
      case 'mic':
        // O processamento do Chromium (ruído, eco, ganho automático) é escolhido ao abrir o microfone.
        return `mic:${ch.deviceId ?? 'default'}:${ch.filters.noiseSuppression}:${ch.filters.echoCancellation}:${ch.filters.autoGain}`
      case 'app':
        return `app:${ch.appExe ?? ''}`
    }
  }

  private setStatus(strip: ChannelStrip, state: AudioChannelStatus['state'], message: string | null): void {
    strip.status = { id: strip.id, state, message }
    this.onStatus?.()
  }

  private async openSource(ch: AudioChannel, force: boolean): Promise<void> {
    const strip = this.strips.get(ch.id)
    if (!strip) return
    const key = this.sourceKey(ch)
    if (!force && strip.sourceKey === key) return
    strip.sourceKey = key
    strip.stream?.getTracks().forEach((t) => t.stop())
    strip.stream = null
    strip.player = null
    strip.attach(null)

    if (ch.kind === 'desktop') {
      const track = this.systemStream?.getAudioTracks()[0]
      if (!track) {
        this.setStatus(strip, 'waiting', 'Abrindo o áudio do PC…')
        return
      }
      strip.attach(this.ctx.createMediaStreamSource(new MediaStream([track])))
      this.setStatus(strip, 'ok', null)
      return
    }

    if (ch.kind === 'app') {
      if (!this.workletReady) {
        this.setStatus(strip, 'error', 'O áudio de aplicativo não carregou.')
        return
      }
      if (!ch.appExe) {
        this.setStatus(strip, 'off', 'Escolha o programa nas propriedades')
        return
      }
      const player = new AudioWorkletNode(this.ctx, 'pcm-player', { numberOfInputs: 0, outputChannelCount: [2] })
      strip.player = player
      strip.attach(player)
      return
    }

    const open = (deviceId: string | null): Promise<MediaStream> =>
      navigator.mediaDevices.getUserMedia({
        audio: {
          ...(deviceId ? { deviceId: { exact: deviceId } } : {}),
          echoCancellation: ch.filters.echoCancellation,
          noiseSuppression: ch.filters.noiseSuppression,
          autoGainControl: ch.filters.autoGain,
          channelCount: { ideal: 2 }
        },
        video: false
      })
    try {
      let stream: MediaStream
      let fallback = false
      try {
        stream = await open(ch.deviceId)
      } catch (err) {
        // Microfone escolhido sumiu (USB em outra porta, headset desligado): usa o padrão em vez de ficar mudo.
        const name = (err as DOMException).name
        if (!ch.deviceId || (name !== 'NotFoundError' && name !== 'OverconstrainedError')) throw err
        stream = await open(null)
        fallback = true
      }
      if (this.disposed || strip.sourceKey !== key || !this.strips.has(ch.id)) {
        stream.getTracks().forEach((t) => t.stop())
        return
      }
      strip.stream = stream
      strip.attach(this.ctx.createMediaStreamSource(stream))
      this.setStatus(strip, 'ok', fallback ? 'O microfone escolhido não está conectado; usando o padrão do Windows.' : null)
    } catch (err) {
      const name = (err as DOMException).name
      this.setStatus(
        strip,
        'error',
        name === 'NotFoundError' || name === 'OverconstrainedError'
          ? 'Microfone não encontrado. Escolha outro nas propriedades.'
          : name === 'NotAllowedError'
            ? 'O Windows bloqueou o microfone (Privacidade → Microfone).'
            : 'Não deu para abrir o microfone.'
      )
    }
  }

  /** Reabre um microfone que sumiu (cabo solto, Bluetooth). */
  retry(id: string): void {
    const ch = this.config.channels.find((c) => c.id === id)
    if (ch) void this.openSource(ch, true)
  }

  readLevels(): AudioLevels {
    const out: AudioLevels = {}
    for (const [id, strip] of this.strips) out[id] = strip.level()
    return out
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.offPcm()
    this.offStatus()
    window.api.audio.sync([])
    for (const strip of this.strips.values()) strip.dispose()
    this.strips.clear()
    void this.ctx.close().catch(() => {})
  }
}
