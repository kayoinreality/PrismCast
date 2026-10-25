import type { AudioMixerSettings } from '@shared/protocol'

export interface AudioLevels {
  system: number
  mic: number
}

/**
 * Motor de Mixagem de Audio Ao Vivo (inspirado no AudioService + SourceFiltersService do Streamlabs).
 *
 * Combina o loopback de audio do sistema (WASAPI via getDisplayMedia) e o microfone
 * do host (getUserMedia) em um unico MediaStreamTrack continuo de 48 kHz.
 */
export class AudioMixerEngine {
  private readonly ctx: AudioContext
  private readonly destination: MediaStreamAudioDestinationNode
  private readonly compressor: DynamicsCompressorNode
  private readonly masterGain: GainNode

  private sysSource: MediaStreamAudioSourceNode | null = null
  private readonly sysGain: GainNode
  private readonly sysAnalyser: AnalyserNode
  private readonly sysTimeData = new Float32Array(256)

  private micStream: MediaStream | null = null
  private micSource: MediaStreamAudioSourceNode | null = null
  private readonly micHighpass: BiquadFilterNode
  private readonly micGain: GainNode
  private readonly micAnalyser: AnalyserNode
  private readonly micTimeData = new Float32Array(256)

  private config: AudioMixerSettings
  private currentMicDeviceId: string | null = null
  private disposed = false

  constructor(initialConfig: AudioMixerSettings) {
    this.config = { ...initialConfig }
    this.ctx = new AudioContext({ sampleRate: 48000, latencyHint: 'interactive' })
    this.destination = this.ctx.createMediaStreamDestination()

    this.compressor = this.ctx.createDynamicsCompressor()
    this.compressor.threshold.value = -18
    this.compressor.knee.value = 12
    this.compressor.ratio.value = 4
    this.compressor.attack.value = 0.003
    this.compressor.release.value = 0.15

    this.masterGain = this.ctx.createGain()
    this.masterGain.gain.value = 1.0

    this.sysGain = this.ctx.createGain()
    this.sysAnalyser = this.ctx.createAnalyser()
    this.sysAnalyser.fftSize = 256
    this.sysAnalyser.smoothingTimeConstant = 0.55
    this.sysGain.connect(this.sysAnalyser)

    this.micHighpass = this.ctx.createBiquadFilter()
    this.micHighpass.type = 'highpass'
    this.micHighpass.frequency.value = initialConfig.noiseSuppression ? 85 : 20

    this.micGain = this.ctx.createGain()
    this.micAnalyser = this.ctx.createAnalyser()
    this.micAnalyser.fftSize = 256
    this.micAnalyser.smoothingTimeConstant = 0.55

    this.micHighpass.connect(this.micGain)
    this.micGain.connect(this.micAnalyser)

    this.rewireBus(initialConfig.compressorEnabled)
    this.applyGainValues(initialConfig)
  }

  private rewireBus(useCompressor: boolean): void {
    try {
      this.sysAnalyser.disconnect()
      this.micAnalyser.disconnect()
      this.compressor.disconnect()
      this.masterGain.disconnect()
    } catch {
      /* ignore */
    }

    if (useCompressor) {
      this.sysAnalyser.connect(this.compressor)
      this.micAnalyser.connect(this.compressor)
      this.compressor.connect(this.masterGain)
    } else {
      this.sysAnalyser.connect(this.masterGain)
      this.micAnalyser.connect(this.masterGain)
    }
    this.masterGain.connect(this.destination)
  }

  private applyGainValues(cfg: AudioMixerSettings): void {
    const now = this.ctx.currentTime
    const sysTarget =
      !cfg.systemAudioEnabled || cfg.systemMuted ? 0 : Math.max(0, Math.min(2, cfg.systemVolume))
    const micTarget =
      !cfg.micEnabled || cfg.micMuted ? 0 : Math.max(0, Math.min(2, cfg.micVolume))

    this.sysGain.gain.setTargetAtTime(sysTarget, now, 0.015)
    this.micGain.gain.setTargetAtTime(micTarget, now, 0.015)
    this.micHighpass.frequency.setTargetAtTime(cfg.noiseSuppression ? 85 : 20, now, 0.02)
  }

  get outputTrack(): MediaStreamTrack {
    return this.destination.stream.getAudioTracks()[0]
  }

  setSystemStream(stream: MediaStream | null): void {
    if (this.disposed) return
    if (this.ctx.state === 'suspended') {
      void this.ctx.resume().catch(() => {})
    }

    if (this.sysSource) {
      try {
        this.sysSource.disconnect()
      } catch {
        /* ignore */
      }
      this.sysSource = null
    }

    if (!stream || stream.getAudioTracks().length === 0) return

    try {
      const onlyAudio = new MediaStream([stream.getAudioTracks()[0]])
      this.sysSource = this.ctx.createMediaStreamSource(onlyAudio)
      this.sysSource.connect(this.sysGain)
    } catch (err) {
      console.warn('[AudioMixer] Falha ao anexar audio do sistema:', err)
    }
  }

  async applyConfig(next: AudioMixerSettings): Promise<void> {
    if (this.disposed) return
    if (this.ctx.state === 'suspended') {
      void this.ctx.resume().catch(() => {})
    }

    const prevCompressor = this.config.compressorEnabled
    const needMicAcquire =
      next.micEnabled &&
      (!this.micStream || next.micDeviceId !== this.currentMicDeviceId)
    const needMicStop = !next.micEnabled && this.micStream !== null

    this.config = { ...next }

    if (prevCompressor !== next.compressorEnabled) {
      this.rewireBus(next.compressorEnabled)
    }

    this.applyGainValues(next)

    if (needMicStop) {
      this.stopMic()
    } else if (needMicAcquire) {
      await this.acquireMic(next.micDeviceId, next.noiseSuppression, next.echoCancellation)
    }
  }

  private async acquireMic(
    deviceId: string | null,
    noiseSuppression: boolean,
    echoCancellation: boolean
  ): Promise<void> {
    this.stopMic()
    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          ...(deviceId ? { deviceId: { exact: deviceId } } : {}),
          echoCancellation,
          noiseSuppression,
          autoGainControl: false
        },
        video: false
      })
      if (this.disposed || !this.config.micEnabled) {
        stream.getTracks().forEach((t) => t.stop())
        return
      }
      this.micStream = stream
      this.currentMicDeviceId = deviceId
      this.micSource = this.ctx.createMediaStreamSource(stream)
      this.micSource.connect(this.micHighpass)
    } catch (err) {
      console.warn('[AudioMixer] Nao foi possivel capturar o microfone:', err)
    }
  }

  private stopMic(): void {
    if (this.micSource) {
      try {
        this.micSource.disconnect()
      } catch {
        /* ignore */
      }
      this.micSource = null
    }
    if (this.micStream) {
      this.micStream.getTracks().forEach((t) => t.stop())
      this.micStream = null
    }
    this.currentMicDeviceId = null
  }

  readLevels(): AudioLevels {
    if (this.disposed) return { system: 0, mic: 0 }
    return {
      system:
        !this.config.systemAudioEnabled || this.config.systemMuted
          ? 0
          : this.computeLevel(this.sysAnalyser, this.sysTimeData),
      mic:
        !this.config.micEnabled || this.config.micMuted
          ? 0
          : this.computeLevel(this.micAnalyser, this.micTimeData)
    }
  }

  private computeLevel(analyser: AnalyserNode, buf: Float32Array<ArrayBuffer>): number {
    if (!this.sysSource && analyser === this.sysAnalyser) return 0
    if (!this.micSource && analyser === this.micAnalyser) return 0
    analyser.getFloatTimeDomainData(buf)
    let sum = 0
    for (let i = 0; i < buf.length; i++) {
      sum += buf[i] * buf[i]
    }
    const rms = Math.sqrt(sum / buf.length)
    return Math.min(1, Math.pow(rms * 3.8, 0.65))
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.stopMic()
    if (this.sysSource) {
      try {
        this.sysSource.disconnect()
      } catch {
        /* ignore */
      }
      this.sysSource = null
    }
    void this.ctx.close().catch(() => {})
  }
}
