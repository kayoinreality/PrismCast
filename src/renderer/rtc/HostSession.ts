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
  /** ICE que chegou antes da resposta: addIceCandidate falha sem remoteDescription. */
  pendingIce: RTCIceCandidateInit[]
  state: PeerState
  /** Conexao reversa opcional para Party Voice (retorno de voz do espectador). */
  voicePc: RTCPeerConnection | null
  voiceAudioEl: HTMLAudioElement | null
}

/**
 * Lado emissor do PrismCast Studio: uma RTCPeerConnection por espectador (malha LAN).
 *
 * Suporta:
 * - Troca de video/audio ao vivo via RTCRtpSender.replaceTrack sem renegociar
 * - Mudanca de bitrate/fps/codec ao vivo
 * - Retorno de voz opcional (Party Voice / GuestCam) dos espectadores na LAN
 */
export class HostSession {
  private readonly peers = new Map<string, PeerEntry>()
  private settings: QualitySettings
  private currentVideoTrack: MediaStreamTrack | null
  private currentAudioTrack: MediaStreamTrack | null
  private partyVoiceEnabled = false

  onStateChange: ((viewerId: string, state: PeerState) => void) | null = null

  constructor(
    private readonly stream: MediaStream,
    settings: QualitySettings,
    private readonly send: (viewerId: string, msg: HostToViewer) => void
  ) {
    this.settings = settings
    this.currentVideoTrack = stream.getVideoTracks()[0] ?? null
    this.currentAudioTrack = stream.getAudioTracks()[0] ?? null
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
    if (entry.voiceAudioEl) {
      entry.voiceAudioEl.srcObject = null
      entry.voiceAudioEl = null
    }
    if (entry.voicePc) {
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

  async addViewer(viewerId: string): Promise<void> {
    this.removeViewer(viewerId)

    const pc = new RTCPeerConnection({ iceServers: [], bundlePolicy: 'max-bundle' })

    const entry: PeerEntry = {
      pc,
      videoTransceiver: null,
      audioTransceiver: null,
      sampler: new StatsSampler('send'),
      pendingIce: [],
      state: 'conectando',
      voicePc: null,
      voiceAudioEl: null
    }
    this.peers.set(viewerId, entry)

    pc.onicecandidate = (event) => {
      if (!event.candidate) return
      this.send(viewerId, { t: 'ice', candidate: toCandidatePayload(event.candidate) })
    }

    pc.onconnectionstatechange = () => {
      switch (pc.connectionState) {
        case 'connected':
          this.setState(viewerId, 'conectado')
          break
        case 'disconnected':
          this.setState(viewerId, 'instavel')
          break
        case 'failed':
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
            maxFramerate: this.settings.fps
          }
        ]
      })
      applyCodecPreference(entry.videoTransceiver, this.settings.codec)
    }

    if (audioTrack) {
      entry.audioTransceiver = pc.addTransceiver(audioTrack, {
        direction: 'sendonly',
        streams: [this.stream]
      })
    }

    await this.negotiate(viewerId)
  }

  /**
   * Substitui o track de video ao vivo em todos os espectadores conectados
   * sem renegociar SDP e sem derrubar a chamada.
   */
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

  /**
   * Substitui o track de audio ao vivo em todos os espectadores conectados.
   */
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

    const offer = await entry.pc.createOffer()
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

    // Party Voice (espectador enviando microfone para o host)
    if (msg.t === 'voice-offer' && this.partyVoiceEnabled) {
      this.closeVoicePeer(entry)
      const voicePc = new RTCPeerConnection({ iceServers: [], bundlePolicy: 'max-bundle' })
      entry.voicePc = voicePc

      voicePc.onicecandidate = (ev) => {
        if (!ev.candidate) return
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
      const answer = await voicePc.createAnswer()
      await voicePc.setLocalDescription(answer)
      this.send(viewerId, { t: 'voice-answer', desc: { type: 'answer', sdp: answer.sdp ?? '' } })
      return
    }

    if (msg.t === 'voice-ice' && entry.voicePc?.remoteDescription) {
      await entry.voicePc.addIceCandidate(msg.candidate).catch(() => {})
    }
  }

  removeViewer(viewerId: string): void {
    const entry = this.peers.get(viewerId)
    if (!entry) return
    this.peers.delete(viewerId)
    this.closeVoicePeer(entry)
    entry.pc.onicecandidate = null
    entry.pc.onconnectionstatechange = null
    try {
      entry.pc.close()
    } catch {
      /* ja fechada */
    }
  }

  async updateSettings(next: QualitySettings): Promise<void> {
    const codecChanged = next.codec !== this.settings.codec
    this.settings = next

    const videoTrack = this.currentVideoTrack ?? this.stream.getVideoTracks()[0]
    if (videoTrack) {
      await videoTrack
        .applyConstraints({
          frameRate: { ideal: next.fps, max: next.fps },
          ...(next.maxHeight ? { height: { max: next.maxHeight } } : {})
        })
        .catch((err) => console.warn('[rtc] applyConstraints falhou:', err))
    }

    for (const [viewerId, entry] of this.peers) {
      const sender = entry.videoTransceiver?.sender
      if (sender) {
        const params = sender.getParameters()
        if (!params.encodings?.length) params.encodings = [{}]
        params.encodings[0].maxBitrate = next.maxBitrateKbps * 1000
        params.encodings[0].maxFramerate = next.fps
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
