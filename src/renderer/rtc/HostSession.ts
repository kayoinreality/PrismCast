import type { HostToViewer, ViewerToHost } from '@shared/protocol'
import type { QualitySettings } from '@shared/quality'
import { applyCodecPreference, preferOpusStereo } from './sdp'
import { toCandidatePayload } from './ice'
import { StatsSampler, emptyStats, type StreamStats } from './stats'

export type PeerState = 'conectando' | 'conectado' | 'instavel' | 'encerrado'

interface PeerEntry {
  pc: RTCPeerConnection
  videoTransceiver: RTCRtpTransceiver | null
  audioTransceiver: RTCRtpTransceiver | null
  sampler: StatsSampler
  pendingIce: RTCIceCandidateInit[]
  pendingVoiceIce: RTCIceCandidateInit[]
  state: PeerState
  voicePc: RTCPeerConnection | null
  voiceAudioEl: HTMLAudioElement | null
  iceRestartTimer: ReturnType<typeof setTimeout> | null
  needsRenegotiate: boolean
}

export class HostSession {
  private readonly peers = new Map<string, PeerEntry>()
  private settings: QualitySettings
  private currentVideoTrack: MediaStreamTrack | null
  private currentAudioTrack: MediaStreamTrack | null
  private partyVoiceEnabled: boolean

  onStateChange: ((viewerId: string, state: PeerState) => void) | null = null

  constructor(
    private readonly stream: MediaStream,
    settings: QualitySettings,
    private readonly send: (viewerId: string, msg: HostToViewer) => void,
    initialPartyVoice = true
  ) {
    this.settings = settings
    this.currentVideoTrack = stream.getVideoTracks()[0] ?? null
    this.currentAudioTrack = stream.getAudioTracks()[0] ?? null
    this.partyVoiceEnabled = initialPartyVoice
  }

  get peerCount(): number {
    return this.peers.size
  }

  peerIds(): string[] {
    return [...this.peers.keys()]
  }

  setPartyVoiceEnabled(enabled: boolean): void {
    this.partyVoiceEnabled = enabled
    if (!enabled) {
      for (const entry of this.peers.values()) {
        this.closeVoicePeer(entry)
      }
    }
  }

  private closeVoicePeer(entry: PeerEntry): void {
    entry.pendingVoiceIce = []
    if (entry.voiceAudioEl) {
      try {
        entry.voiceAudioEl.pause()
      } catch {
        /* ignore */
      }
      entry.voiceAudioEl.srcObject = null
      entry.voiceAudioEl = null
    }
    if (entry.voicePc) {
      entry.voicePc.onicecandidate = null
      entry.voicePc.ontrack = null
      try {
        entry.voicePc.close()
      } catch {
        /* ignore */
      }
      entry.voicePc = null
    }
  }

  private setState(viewerId: string, state: PeerState): void {
    const entry = this.peers.get(viewerId)
    if (!entry || entry.state === state) return
    entry.state = state
    this.onStateChange?.(viewerId, state)
  }

  private scheduleIceRestart(viewerId: string, entry: PeerEntry): void {
    if (entry.iceRestartTimer) return
    entry.iceRestartTimer = setTimeout(() => {
      entry.iceRestartTimer = null
      if (this.peers.get(viewerId) !== entry) return
      if (entry.pc.connectionState === 'disconnected' || entry.pc.connectionState === 'failed') {
        try {
          entry.pc.restartIce()
          void this.negotiate(viewerId)
        } catch {
          /* ignore */
        }
      }
    }, 3000)
  }

  async addViewer(viewerId: string): Promise<void> {
    this.removeViewer(viewerId)

    const pc = new RTCPeerConnection({ iceServers: [], bundlePolicy: 'max-bundle' })

    const entry: PeerEntry = {
      pc,
      videoTransceiver: null,
      audioTransceiver: null,
      sampler: new StatsSampler('send'),
      pendingIce: [],
      pendingVoiceIce: [],
      state: 'conectando',
      voicePc: null,
      voiceAudioEl: null,
      iceRestartTimer: null,
      needsRenegotiate: false
    }
    this.peers.set(viewerId, entry)

    pc.onicecandidate = (event) => {
      if (!event.candidate) return
      this.send(viewerId, { t: 'ice', candidate: toCandidatePayload(event.candidate) })
    }

    pc.onconnectionstatechange = () => {
      switch (pc.connectionState) {
        case 'connected':
          if (entry.iceRestartTimer) {
            clearTimeout(entry.iceRestartTimer)
            entry.iceRestartTimer = null
          }
          this.setState(viewerId, 'conectado')
          break
        case 'disconnected':
          this.setState(viewerId, 'instavel')
          this.scheduleIceRestart(viewerId, entry)
          break
        case 'failed':
          this.setState(viewerId, 'instavel')
          this.scheduleIceRestart(viewerId, entry)
          break
        case 'closed':
          this.setState(viewerId, 'encerrado')
          break
        default:
          this.setState(viewerId, 'conectando')
      }
    }

    const videoTrack = this.currentVideoTrack ?? this.stream.getVideoTracks()[0]
    const audioTrack = this.currentAudioTrack ?? this.stream.getAudioTracks()[0]

    if (videoTrack) {
      entry.videoTransceiver = pc.addTransceiver(videoTrack, {
        direction: 'sendonly',
        streams: [this.stream],
        sendEncodings: [
          {
            maxBitrate: this.settings.maxBitrateKbps * 1000,
            maxFramerate: this.settings.fps,
            scaleResolutionDownBy: this.computeScaleDown(videoTrack, this.settings.maxHeight)
          }
        ]
      })
      applyCodecPreference(entry.videoTransceiver, this.settings.codec)
    }

    // Sempre cria transceiver de áudio para permitir replaceAudioTrack futuro sem renegociar SDP.
    entry.audioTransceiver = pc.addTransceiver(audioTrack ?? 'audio', {
      direction: 'sendonly',
      streams: [this.stream]
    })

    await this.negotiate(viewerId)
  }

  private computeScaleDown(track: MediaStreamTrack | null, maxHeight: number | null): number {
    if (!track || !maxHeight) return 1
    const srcH = track.getSettings().height ?? 1080
    return srcH > maxHeight ? Math.max(1, srcH / maxHeight) : 1
  }

  async replaceVideoTrack(newTrack: MediaStreamTrack): Promise<void> {
    this.currentVideoTrack = newTrack
    await Promise.all(
      [...this.peers.values()].map(async (entry) => {
        const sender = entry.videoTransceiver?.sender
        if (sender) {
          await sender.replaceTrack(newTrack).catch((err) => {
            console.warn('[rtc] replaceTrack(video) falhou:', err)
          })
        }
      })
    )
  }

  async replaceAudioTrack(newTrack: MediaStreamTrack | null): Promise<void> {
    this.currentAudioTrack = newTrack
    await Promise.all(
      [...this.peers.values()].map(async (entry) => {
        const sender = entry.audioTransceiver?.sender
        if (sender) {
          await sender.replaceTrack(newTrack).catch((err) => {
            console.warn('[rtc] replaceTrack(audio) falhou:', err)
          })
        }
      })
    )
  }

  private async negotiate(viewerId: string): Promise<void> {
    const entry = this.peers.get(viewerId)
    if (!entry) return
    if (entry.pc.signalingState !== 'stable') {
      entry.needsRenegotiate = true
      return
    }
    entry.needsRenegotiate = false

    const offer = await entry.pc.createOffer()
    if (this.peers.get(viewerId) !== entry) return
    const sdp = preferOpusStereo(offer.sdp ?? '')
    await entry.pc.setLocalDescription({ type: 'offer', sdp })

    this.applyDegradation(entry)

    this.send(viewerId, { t: 'offer', desc: { type: 'offer', sdp } })
  }

  private applyDegradation(entry: PeerEntry): void {
    const sender = entry.videoTransceiver?.sender
    if (!sender) return
    const params = sender.getParameters()
    params.degradationPreference = this.settings.degradation
    void sender.setParameters(params).catch((err) => {
      console.warn('[rtc] setParameters (degradacao) falhou:', err)
    })
  }

  async handleMessage(viewerId: string, msg: ViewerToHost): Promise<void> {
    const entry = this.peers.get(viewerId)
    if (!entry) return

    if (msg.t === 'answer') {
      await entry.pc.setRemoteDescription({ type: 'answer', sdp: msg.desc.sdp })
      for (const candidate of entry.pendingIce.splice(0)) {
        await entry.pc.addIceCandidate(candidate).catch(() => {})
      }
      if (entry.needsRenegotiate && entry.pc.signalingState === 'stable') {
        void this.negotiate(viewerId)
      }
      return
    }

    if (msg.t === 'ice') {
      if (!entry.pc.remoteDescription) {
        entry.pendingIce.push(msg.candidate)
        return
      }
      await entry.pc.addIceCandidate(msg.candidate).catch((err) => {
        console.warn('[rtc] addIceCandidate falhou:', err)
      })
      return
    }

    if (msg.t === 'voice-offer' && this.partyVoiceEnabled) {
      this.closeVoicePeer(entry)
      const voicePc = new RTCPeerConnection({ iceServers: [], bundlePolicy: 'max-bundle' })
      entry.voicePc = voicePc

      voicePc.onicecandidate = (ev) => {
        if (!ev.candidate || entry.voicePc !== voicePc) return
        this.send(viewerId, { t: 'voice-ice', candidate: toCandidatePayload(ev.candidate) })
      }
      voicePc.ontrack = (ev) => {
        const remoteStream = ev.streams[0] ?? new MediaStream([ev.track])
        const audio = new Audio()
        audio.srcObject = remoteStream
        audio.autoplay = true
        entry.voiceAudioEl = audio
        void audio.play().catch(() => {})
      }

      await voicePc.setRemoteDescription({ type: 'offer', sdp: msg.desc.sdp })
      for (const cand of entry.pendingVoiceIce.splice(0)) {
        await voicePc.addIceCandidate(cand).catch(() => {})
      }
      const answer = await voicePc.createAnswer()
      const sdp = preferOpusStereo(answer.sdp ?? '')
      await voicePc.setLocalDescription({ type: 'answer', sdp })
      this.send(viewerId, { t: 'voice-answer', desc: { type: 'answer', sdp } })
      return
    }

    if (msg.t === 'voice-ice') {
      if (!entry.voicePc?.remoteDescription) {
        entry.pendingVoiceIce.push(msg.candidate)
        return
      }
      await entry.voicePc.addIceCandidate(msg.candidate).catch(() => {})
    }
  }

  removeViewer(viewerId: string): void {
    const entry = this.peers.get(viewerId)
    if (!entry) return
    this.peers.delete(viewerId)
    if (entry.iceRestartTimer) {
      clearTimeout(entry.iceRestartTimer)
      entry.iceRestartTimer = null
    }
    this.closeVoicePeer(entry)
    entry.pc.onicecandidate = null
    entry.pc.onconnectionstatechange = null
    try {
      entry.pc.close()
    } catch {
      /* ja fechada */
    }
  }

  /**
   * Atualiza bitrate, framerate, escala e codec via RTCRtpSender.setParameters
   * sem chamar `videoTrack.applyConstraints()` (preservando o track do compositor/captura).
   */
  async updateSettings(next: QualitySettings): Promise<void> {
    const codecChanged = next.codec !== this.settings.codec
    this.settings = next
    const videoTrack = this.currentVideoTrack ?? this.stream.getVideoTracks()[0] ?? null
    const scaleDown = this.computeScaleDown(videoTrack, next.maxHeight)

    for (const [viewerId, entry] of this.peers) {
      const sender = entry.videoTransceiver?.sender
      if (sender) {
        const params = sender.getParameters()
        if (!params.encodings?.length) params.encodings = [{}]
        params.encodings[0].maxBitrate = next.maxBitrateKbps * 1000
        params.encodings[0].maxFramerate = next.fps
        params.encodings[0].scaleResolutionDownBy = scaleDown
        params.degradationPreference = next.degradation
        await sender.setParameters(params).catch((err) => {
          console.warn('[rtc] setParameters falhou:', err)
        })
      }

      if (codecChanged && entry.videoTransceiver) {
        applyCodecPreference(entry.videoTransceiver, next.codec)
        await this.negotiate(viewerId).catch((err) => {
          console.warn('[rtc] renegociacao apos troca de codec falhou:', err)
        })
      }
    }
  }

  async readStats(): Promise<Map<string, StreamStats>> {
    const out = new Map<string, StreamStats>()
    await Promise.all(
      [...this.peers.entries()].map(async ([viewerId, entry]) => {
        try {
          out.set(viewerId, await entry.sampler.read(entry.pc))
        } catch {
          out.set(viewerId, emptyStats())
        }
      })
    )
    return out
  }

  getState(viewerId: string): PeerState | null {
    return this.peers.get(viewerId)?.state ?? null
  }

  close(): void {
    for (const viewerId of [...this.peers.keys()]) this.removeViewer(viewerId)
  }
}
