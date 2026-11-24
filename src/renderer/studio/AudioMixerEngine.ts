import type {
  AudioChannel,
  AudioChannelStatus,
  AudioLevelSample,
  AudioMixerSettings
} from '@shared/protocol'
import { dbToLinear, linearToDb, MIN_DB } from '@shared/audio'
import { ensureBridge } from '@/bridge'
import { studioStore } from './StudioStore'

export interface ChannelLevel {
  peakDb: number
  rmsDb: number
  inputDb: number
}

export type AudioLevels = Record<string, ChannelLevel>

interface ChannelChain {
  channel: AudioChannel
  inputBus: GainNode
  inputAnalyser: AnalyserNode
  highpass: BiquadFilterNode
  eqLow: BiquadFilterNode
  eqMid: BiquadFilterNode
  eqHigh: BiquadFilterNode
  compressor: DynamicsCompressorNode
  preGain: GainNode
  limiter: DynamicsCompressorNode
  faderGain: GainNode
  delay: DelayNode
  outputAnalyser: AnalyserNode
  masterSend: GainNode
  monitorSend: GainNode
  sourceNode: MediaStreamAudioSourceNode | null
  micStream: MediaStream | null
  micConstraintKey: string
  pcmCursor: number
  openingMic: boolean
  micGen: number
}

function measureAnalyser(
  analyser: AnalyserNode,
  buf: Float32Array<ArrayBuffer>
): { peakDb: number; rmsDb: number } {
  analyser.getFloatTimeDomainData(buf)
  let peak = 0
  let sumSq = 0
  for (let i = 0; i < buf.length; i++) {
    const v = Math.abs(buf[i])
    if (v > peak) peak = v
    sumSq += buf[i] * buf[i]
  }
  const rms = Math.sqrt(sumSq / buf.length)
  return {
    peakDb: linearToDb(peak),
    rmsDb: linearToDb(rms)
  }
}

function micConstraintSignature(ch: AudioChannel): string {
  return JSON.stringify([
    ch.deviceId ?? '',
    ch.filters.noiseSuppression,
    ch.filters.echoCancellation,
    ch.filters.autoGain
  ])
}

function describeMicError(err: unknown): string {
  const name = err instanceof DOMException ? err.name : ''
  if (name === 'NotAllowedError' || name === 'PermissionDeniedError') {
    return 'Permissão do microfone negada'
  }
  if (name === 'NotFoundError' || name === 'DevicesNotFoundError') {
    return 'Microfone não encontrado'
  }
  if (name === 'NotReadableError' || name === 'TrackStartError') {
    return 'Microfone ocupado por outro programa'
  }
  if (name === 'OverconstrainedError') {
    return 'Dispositivo selecionado indisponível'
  }
  return 'Falha ao abrir microfone'
}

/**
 * Motor híbrido de mixagem de áudio:
 *
 * 1. Sincroniza canais de aplicativo (`api.audio.sync`), recebe status reais (`audio:status`),
 *    níveis nativos (`audio:levels`) e PCM f32le (`audio:pcm`) do `AudioRuntime` Rust.
 * 2. Mantém um grafo WebAudio limpo para produzir `outputTrack` (usado pelo `OutputManager`
 *    e transmissão LAN WebRTC) e fallback completo de microfone/áudio do PC quando rodando
 *    sem captura nativa de PCM.
 */
export class AudioMixerEngine {
  private rustStatuses = new Map<string, AudioChannelStatus>()
  private localStatuses = new Map<string, AudioChannelStatus>()
  private rustLevels = new Map<string, AudioLevelSample>()
  private lastNativeLevelAt = 0

  private config: AudioMixerSettings
  private sceneLiveMics = new Set<string>()
  private sceneUsedMics = new Set<string>()
  private systemStream: MediaStream | null = null
  private lastAppSyncSig = ''

  private ctx: AudioContext | null = null
  private masterDest: MediaStreamAudioDestinationNode | null = null
  private masterTrack: MediaStreamTrack | null = null
  private chains = new Map<string, ChannelChain>()
  private meterBuf = new Float32Array(new ArrayBuffer(256 * 4))
  private disposed = false

  private readonly unsubscribers: Array<() => void> = []
  onStatus: (() => void) | null = null

  constructor(initial: AudioMixerSettings) {
    this.config = initial
    const api = ensureBridge()

    this.unsubscribers.push(
      api.audio.onStatus((list) => {
        if (this.disposed) return
        this.rustStatuses = new Map(list.map((s) => [s.id, s]))
        this.onStatus?.()
      }),
      api.audio.onPcm((id, pcm) => {
        if (this.disposed || pcm.length === 0) return
        this.scheduleExternalPcm(id, pcm)
      })
    )

    if (api.audio.onLevels) {
      this.unsubscribers.push(
        api.audio.onLevels((samples) => {
          if (this.disposed) return
          let hasActiveSignal = false
          for (const s of samples) {
            this.rustLevels.set(s.id, s)
            studioStore.audioLevelsRef[s.id] = s
            if (s.peakDb > MIN_DB + 0.5 || s.inputDb > MIN_DB + 0.5) {
              hasActiveSignal = true
            }
          }
          if (hasActiveSignal || studioStore.getSnapshot().telemetry.audioRuntimeActive) {
            this.lastNativeLevelAt = performance.now()
          }
        })
      )
    }

    void this.applyConfig(initial)
  }

  private ensureContext(): AudioContext | null {
    if (this.disposed) return null
    if (!this.ctx) {
      if (typeof AudioContext === 'undefined') return null
      this.ctx = new AudioContext({ sampleRate: 48000 })
      this.masterDest = this.ctx.createMediaStreamDestination()
      this.masterTrack = this.masterDest.stream.getAudioTracks()[0] ?? null
    }
    return this.ctx
  }

  get outputTrack(): MediaStreamTrack {
    this.ensureContext()
    if (!this.masterTrack && this.masterDest) {
      this.masterTrack = this.masterDest.stream.getAudioTracks()[0]
    }
    return this.masterTrack!
  }

  async resume(): Promise<void> {
    const ctx = this.ensureContext()
    if (ctx && ctx.state === 'suspended') {
      await ctx.resume().catch(() => {})
    }
  }

  setSystemStream(stream: MediaStream | null): void {
    this.systemStream = stream
    const hasAudio = Boolean(stream && stream.getAudioTracks().length > 0)
    for (const chain of this.chains.values()) {
      if (chain.channel.kind !== 'desktop') continue
      this.attachDesktopSource(chain)
      if (hasAudio) {
        this.setLocalStatus(chain.channel.id, 'ok', null)
      } else {
        this.setLocalStatus(chain.channel.id, 'waiting', 'Aguardando áudio do PC…')
      }
    }
  }

  setSceneMics(live: Set<string>, used: Set<string>): void {
    this.sceneLiveMics = new Set(live)
    this.sceneUsedMics = new Set(used)
    for (const chain of this.chains.values()) {
      if (chain.channel.kind === 'mic') {
        this.applyChainParams(chain, chain.channel)
      }
    }
  }

  micOnAir(id: string): boolean {
    if (!this.sceneUsedMics.has(id)) return true
    return this.sceneLiveMics.has(id)
  }

  status(id: string): AudioChannelStatus {
    const rust = this.rustStatuses.get(id)
    if (rust) return rust
    return this.localStatuses.get(id) ?? { id, state: 'ok', message: null }
  }

  sync(force = false): void {
    const appChannels = this.config.channels
      .filter((c) => c.kind === 'app' && Boolean(c.appExe && c.appExe.trim()))
      .map((c) => ({ id: c.id, exe: c.appExe!.trim() }))
    const sig = JSON.stringify(appChannels)
    if (!force && sig === this.lastAppSyncSig) return
    this.lastAppSyncSig = sig
    ensureBridge().audio.sync(appChannels)
  }

  async applyConfig(next: AudioMixerSettings): Promise<void> {
    if (this.disposed) return
    this.config = next
    this.sync(false)

    const ctx = this.ensureContext()
    if (!ctx) return

    // Tenta aplicar saída de monitoramento quando suportado pelo navegador/WebView
    const sinkTarget = ctx as AudioContext & { setSinkId?: (id: string) => Promise<void> }
    if (typeof sinkTarget.setSinkId === 'function') {
      void sinkTarget.setSinkId(next.monitorDeviceId ?? '').catch(() => {})
    }

    const activeIds = new Set(next.channels.map((c) => c.id))
    for (const [id, chain] of this.chains) {
      if (!activeIds.has(id)) {
        this.disposeChain(chain)
        this.chains.delete(id)
        this.localStatuses.delete(id)
      }
    }

    for (const ch of next.channels) {
      let chain = this.chains.get(ch.id)
      if (!chain) {
        chain = this.createChain(ctx, ch)
        this.chains.set(ch.id, chain)
      }
      const prevSig = chain.micConstraintKey
      chain.channel = ch
      this.applyChainParams(chain, ch)

      if (ch.kind === 'mic') {
        const nextSig = micConstraintSignature(ch)
        if (!chain.micStream || prevSig !== nextSig) {
          void this.openMic(chain, ch)
        }
      } else if (ch.kind === 'desktop') {
        if (!chain.sourceNode && this.systemStream) {
          this.attachDesktopSource(chain)
        }
        if (!this.rustStatuses.has(ch.id)) {
          const hasAudio = Boolean(this.systemStream && this.systemStream.getAudioTracks().length > 0)
          this.setLocalStatus(
            ch.id,
            hasAudio ? 'ok' : 'waiting',
            hasAudio ? null : 'Aguardando captura de áudio do PC'
          )
        }
      } else if (ch.kind === 'app') {
        if (!this.rustStatuses.has(ch.id)) {
          if (ch.appExe) {
            this.setLocalStatus(ch.id, 'waiting', `Aguardando ${ch.appExe}…`)
          } else {
            this.setLocalStatus(ch.id, 'off', 'Nenhum programa selecionado')
          }
        }
      } else {
        this.setLocalStatus(ch.id, 'ok', null)
      }
    }
  }

  retry(id: string): void {
    const chain = this.chains.get(id)
    if (!chain) return
    if (chain.channel.kind === 'mic') {
      this.stopMicStream(chain)
      void this.openMic(chain, chain.channel)
    } else if (chain.channel.kind === 'app') {
      this.sync(true)
    } else if (chain.channel.kind === 'desktop' && this.systemStream) {
      this.attachDesktopSource(chain)
    }
  }

  retryAllProblematic(): void {
    let shouldSyncApps = false
    for (const [id, chain] of this.chains) {
      const st = this.status(id)
      if (st.state === 'error' || st.state === 'waiting') {
        if (chain.channel.kind === 'mic') {
          this.stopMicStream(chain)
          void this.openMic(chain, chain.channel)
        } else if (chain.channel.kind === 'app') {
          shouldSyncApps = true
        }
      }
    }
    if (shouldSyncApps) this.sync(true)
  }

  readLevels(): AudioLevels {
    const out: AudioLevels = {}
    const now = performance.now()
    const nativeFresh =
      Boolean(studioStore.getSnapshot().telemetry.audioRuntimeActive) ||
      now - this.lastNativeLevelAt < 1500

    for (const [id, s] of Object.entries(studioStore.audioLevelsRef)) {
      if (nativeFresh || s.peakDb > MIN_DB + 0.5 || s.inputDb > MIN_DB + 0.5) {
        out[id] = { peakDb: s.peakDb, rmsDb: s.peakDb, inputDb: s.inputDb }
      }
    }

    for (const [id, chain] of this.chains) {
      const existing = out[id]
      const input = measureAnalyser(chain.inputAnalyser, this.meterBuf)
      const output = measureAnalyser(chain.outputAnalyser, this.meterBuf)
      const muted = chain.channel.muted || (chain.channel.kind === 'mic' && !this.micOnAir(id))
      const webOutDb = muted ? MIN_DB : output.peakDb
      const webRmsDb = muted ? MIN_DB : output.rmsDb

      if (!existing || (existing.peakDb <= MIN_DB + 0.5 && existing.inputDb <= MIN_DB + 0.5)) {
        out[id] = {
          peakDb: webOutDb,
          rmsDb: webRmsDb,
          inputDb: input.peakDb
        }
        // Espelha no ref compartilhado quando o runtime nativo não está enviando sinal
        if (!nativeFresh) {
          studioStore.audioLevelsRef[id] = {
            id,
            peakDb: webOutDb,
            inputDb: input.peakDb,
            mutedSpeech: chain.channel.kind === 'mic' && chain.channel.muted && input.peakDb > -35
          }
        }
      }
    }

    return out
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    for (const off of this.unsubscribers) off()
    this.unsubscribers.length = 0
    for (const chain of this.chains.values()) {
      this.disposeChain(chain)
    }
    this.chains.clear()
    this.masterTrack?.stop()
    void this.ctx?.close().catch(() => {})
    this.masterTrack = null
    this.masterDest = null
    this.ctx = null
  }

  private setLocalStatus(id: string, state: AudioChannelStatus['state'], message: string | null): void {
    const prev = this.localStatuses.get(id)
    if (prev && prev.state === state && prev.message === message) return
    this.localStatuses.set(id, { id, state, message })
    this.onStatus?.()
  }

  private createChain(ctx: AudioContext, ch: AudioChannel): ChannelChain {
    const inputBus = ctx.createGain()
    const inputAnalyser = ctx.createAnalyser()
    inputAnalyser.fftSize = 256

    const highpass = ctx.createBiquadFilter()
    highpass.type = 'highpass'
    highpass.Q.value = 0.707

    const eqLow = ctx.createBiquadFilter()
    eqLow.type = 'lowshelf'
    eqLow.frequency.value = 100

    const eqMid = ctx.createBiquadFilter()
    eqMid.type = 'peaking'
    eqMid.frequency.value = 1000
    eqMid.Q.value = 1.0

    const eqHigh = ctx.createBiquadFilter()
    eqHigh.type = 'highshelf'
    eqHigh.frequency.value = 8000

    const compressor = ctx.createDynamicsCompressor()
    const preGain = ctx.createGain()
    const limiter = ctx.createDynamicsCompressor()
    const faderGain = ctx.createGain()
    const delay = ctx.createDelay(1.0)
    const outputAnalyser = ctx.createAnalyser()
    outputAnalyser.fftSize = 256
    const masterSend = ctx.createGain()
    const monitorSend = ctx.createGain()

    inputBus
      .connect(inputAnalyser)
      .connect(highpass)
      .connect(eqLow)
      .connect(eqMid)
      .connect(eqHigh)
      .connect(compressor)
      .connect(preGain)
      .connect(limiter)
      .connect(faderGain)
      .connect(delay)
      .connect(outputAnalyser)

    if (this.masterDest) {
      outputAnalyser.connect(masterSend)
      masterSend.connect(this.masterDest)
    }
    outputAnalyser.connect(monitorSend)
    monitorSend.connect(ctx.destination)

    return {
      channel: ch,
      inputBus,
      inputAnalyser,
      highpass,
      eqLow,
      eqMid,
      eqHigh,
      compressor,
      preGain,
      limiter,
      faderGain,
      delay,
      outputAnalyser,
      masterSend,
      monitorSend,
      sourceNode: null,
      micStream: null,
      micConstraintKey: '',
      pcmCursor: 0,
      openingMic: false,
      micGen: 0
    }
  }

  private applyChainParams(chain: ChannelChain, ch: AudioChannel): void {
    const ctx = this.ctx
    if (!ctx) return
    const now = ctx.currentTime
    const f = ch.filters

    // Passa-alta: 10Hz equivale a praticamente desligado no domínio audível
    chain.highpass.frequency.setTargetAtTime(f.highpassHz > 0 ? Math.max(20, f.highpassHz) : 10, now, 0.015)

    // EQ 3 bandas
    const eqOn = Boolean(f.eqEnabled)
    chain.eqLow.gain.setTargetAtTime(eqOn ? (f.eqLowDb ?? 0) : 0, now, 0.015)
    chain.eqMid.gain.setTargetAtTime(eqOn ? (f.eqMidDb ?? 0) : 0, now, 0.015)
    chain.eqHigh.gain.setTargetAtTime(eqOn ? (f.eqHighDb ?? 0) : 0, now, 0.015)

    // Compressor
    if (f.compressor.enabled) {
      chain.compressor.threshold.setTargetAtTime(f.compressor.thresholdDb, now, 0.015)
      chain.compressor.ratio.setTargetAtTime(Math.max(1, f.compressor.ratio), now, 0.015)
      chain.compressor.attack.setTargetAtTime(Math.max(0.001, f.compressor.attackMs / 1000), now, 0.015)
      chain.compressor.release.setTargetAtTime(Math.max(0.01, f.compressor.releaseMs / 1000), now, 0.015)
    } else {
      chain.compressor.threshold.setTargetAtTime(0, now, 0.015)
      chain.compressor.ratio.setTargetAtTime(1, now, 0.015)
    }

    // Ganho de entrada + makeup do compressor
    const makeup = f.compressor.enabled ? f.compressor.makeupDb : 0
    chain.preGain.gain.setTargetAtTime(dbToLinear(f.gainDb + makeup), now, 0.015)

    // Limitador
    if (f.limiter.enabled) {
      chain.limiter.threshold.setTargetAtTime(f.limiter.ceilingDb, now, 0.01)
      chain.limiter.ratio.setTargetAtTime(20, now, 0.01)
      chain.limiter.attack.setTargetAtTime(0.001, now, 0.01)
      chain.limiter.release.setTargetAtTime(0.05, now, 0.01)
    } else {
      chain.limiter.threshold.setTargetAtTime(0, now, 0.01)
      chain.limiter.ratio.setTargetAtTime(1, now, 0.01)
    }

    // Fader + mudo + microfone fora da cena
    const onAir = ch.kind !== 'mic' || this.micOnAir(ch.id)
    const faderLin = ch.muted || !onAir ? 0 : dbToLinear(ch.volumeDb)
    chain.faderGain.gain.setTargetAtTime(faderLin, now, 0.015)

    // Atraso de sincronia (0..1000ms)
    const delaySec = Math.max(0, Math.min(1, (ch.syncOffsetMs || 0) / 1000))
    chain.delay.delayTime.setTargetAtTime(delaySec, now, 0.02)

    // Roteamento live vs monitoramento no fone
    const toMaster = ch.monitor === 'monitor-only' ? 0 : 1
    const toMonitor = ch.monitor === 'off' ? 0 : 1
    chain.masterSend.gain.setTargetAtTime(toMaster, now, 0.015)
    chain.monitorSend.gain.setTargetAtTime(toMonitor, now, 0.015)
  }

  private attachDesktopSource(chain: ChannelChain): void {
    const ctx = this.ctx
    if (!ctx) return
    if (chain.sourceNode) {
      try {
        chain.sourceNode.disconnect()
      } catch {
        /* ignore */
      }
      chain.sourceNode = null
    }
    if (!this.systemStream || this.systemStream.getAudioTracks().length === 0) return
    try {
      const node = ctx.createMediaStreamSource(this.systemStream)
      node.connect(chain.inputBus)
      chain.sourceNode = node
    } catch {
      /* ignore */
    }
  }

  private async openMic(chain: ChannelChain, ch: AudioChannel): Promise<void> {
    if (this.disposed) return
    // Se o backend Rust já está capturando este canal nativamente em estado 'ok',
    // evitamos abrir duas vezes o mesmo microfone no WebView quando sem necessidade.
    const rustSt = this.rustStatuses.get(ch.id)
    if (
      rustSt?.state === 'ok' &&
      studioStore.getSnapshot().telemetry.audioRuntimeActive &&
      this.rustLevels.has(ch.id)
    ) {
      return
    }

    if (typeof navigator === 'undefined' || !navigator.mediaDevices?.getUserMedia) {
      this.setLocalStatus(ch.id, 'waiting', 'Captura de microfone indisponível neste navegador')
      return
    }

    const gen = ++chain.micGen
    const sig = micConstraintSignature(ch)
    chain.micConstraintKey = sig
    chain.openingMic = true
    this.setLocalStatus(ch.id, 'waiting', 'Abrindo microfone…')

    const audioConstraints: MediaTrackConstraints = {
      ...(ch.deviceId && ch.deviceId !== 'default' ? { deviceId: { exact: ch.deviceId } } : {}),
      noiseSuppression: ch.filters.noiseSuppression,
      echoCancellation: ch.filters.echoCancellation,
      autoGainControl: ch.filters.autoGain
    }

    try {
      let stream: MediaStream
      try {
        stream = await navigator.mediaDevices.getUserMedia({ audio: audioConstraints, video: false })
      } catch (err) {
        // Fallback para dispositivo padrão se o deviceId salvo desconectou
        if (ch.deviceId && err instanceof DOMException && err.name === 'OverconstrainedError') {
          stream = await navigator.mediaDevices.getUserMedia({
            audio: {
              noiseSuppression: ch.filters.noiseSuppression,
              echoCancellation: ch.filters.echoCancellation,
              autoGainControl: ch.filters.autoGain
            },
            video: false
          })
        } else {
          throw err
        }
      }

      if (this.disposed || chain.micGen !== gen || this.chains.get(ch.id) !== chain) {
        stream.getTracks().forEach((t) => t.stop())
        return
      }

      this.stopMicStream(chain)
      chain.micStream = stream
      chain.openingMic = false
      const ctx = this.ensureContext()
      if (ctx) {
        const node = ctx.createMediaStreamSource(stream)
        node.connect(chain.inputBus)
        chain.sourceNode = node
      }
      this.setLocalStatus(ch.id, 'ok', null)
    } catch (err) {
      if (this.disposed || chain.micGen !== gen) return
      chain.openingMic = false
      this.setLocalStatus(ch.id, 'error', describeMicError(err))
    }
  }

  private scheduleExternalPcm(id: string, interleavedStereo: Float32Array): void {
    const ctx = this.ctx
    if (!ctx || ctx.state !== 'running') return
    const chain = this.chains.get(id)
    if (!chain) return
    const frames = Math.floor(interleavedStereo.length / 2)
    if (frames <= 0) return

    const audioBuf = ctx.createBuffer(2, frames, 48000)
    const left = audioBuf.getChannelData(0)
    const right = audioBuf.getChannelData(1)
    for (let i = 0; i < frames; i++) {
      left[i] = interleavedStereo[i * 2]
      right[i] = interleavedStereo[i * 2 + 1]
    }
    const src = ctx.createBufferSource()
    src.buffer = audioBuf
    src.connect(chain.inputBus)

    const now = ctx.currentTime
    if (chain.pcmCursor < now || chain.pcmCursor > now + 0.35) {
      chain.pcmCursor = now + 0.02
    }
    src.start(chain.pcmCursor)
    chain.pcmCursor += audioBuf.duration
  }

  private stopMicStream(chain: ChannelChain): void {
    if (chain.sourceNode) {
      try {
        chain.sourceNode.disconnect()
      } catch {
        /* ignore */
      }
      chain.sourceNode = null
    }
    if (chain.micStream) {
      chain.micStream.getTracks().forEach((t) => t.stop())
      chain.micStream = null
    }
  }

  private disposeChain(chain: ChannelChain): void {
    chain.micGen++
    this.stopMicStream(chain)
    const nodes: AudioNode[] = [
      chain.inputBus,
      chain.inputAnalyser,
      chain.highpass,
      chain.eqLow,
      chain.eqMid,
      chain.eqHigh,
      chain.compressor,
      chain.preGain,
      chain.limiter,
      chain.faderGain,
      chain.delay,
      chain.outputAnalyser,
      chain.masterSend,
      chain.monitorSend
    ]
    for (const n of nodes) {
      try {
        n.disconnect()
      } catch {
        /* ignore */
      }
    }
  }
}
