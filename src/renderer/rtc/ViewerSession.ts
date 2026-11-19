import type {
  ChatMessage,
  HostToViewer,
  ReactionEvent,
  SourceKind,
  ViewerToHost
} from '@shared/protocol'
import { StatsSampler, emptyStats, type StreamStats } from './stats'
import { toCandidatePayload } from './ice'
import { preferOpusStereo } from './sdp'

export type ViewerState = 'conectando' | 'recebendo' | 'instavel' | 'encerrado'

export class ViewerSession {
  private pc: RTCPeerConnection | null = null
  private voicePc: RTCPeerConnection | null = null
  private voiceStream: MediaStream | null = null
  private readonly sampler = new StatsSampler('receive')
  private pendingIce: RTCIceCandidateInit[] = []
  private pendingVoiceIce: RTCIceCandidateInit[] = []
  private state: ViewerState = 'conectando'
  private closed = false
  private voiceGen = 0
  private fallbackStream: MediaStream | null = null
  private iceRestartTimer: ReturnType<typeof setTimeout> | null = null

  onStream: ((stream: MediaStream) => void) | null = null
  onStateChange: ((state: ViewerState) => void) | null = null
  onChat: ((chat: ChatMessage) => void) | null = null
  onReaction: ((reaction: ReactionEvent) => void) | null = null
  onSourceChanged: ((sourceName: string, sourceKind: SourceKind) => void) | null = null

  constructor(private readonly send: (msg: ViewerToHost) => void) {}

  private setState(next: ViewerState): void {
    if (this.state === next) return
    this.state = next
    this.onStateChange?.(next)
  }

  private ensurePeer(): RTCPeerConnection {
    if (this.pc) return this.pc

    const pc = new RTCPeerConnection({ iceServers: [], bundlePolicy: 'max-bundle' })
    this.pc = pc

    pc.onicecandidate = (event) => {
      if (!event.candidate) return
      this.send({ t: 'ice', candidate: toCandidatePayload(event.candidate) })
    }

    pc.ontrack = (event) => {
      let stream = event.streams[0]
      if (!stream) {
        if (!this.fallbackStream) this.fallbackStream = new MediaStream()
        this.fallbackStream.addTrack(event.track)
        stream = this.fallbackStream
      }
      this.onStream?.(stream)
    }

    pc.onconnectionstatechange = () => {
      switch (pc.connectionState) {
        case 'connected':
          if (this.iceRestartTimer) {
            clearTimeout(this.iceRestartTimer)
            this.iceRestartTimer = null
          }
          this.setState('recebendo')
          break
        case 'disconnected':
          this.setState('instavel')
          this.scheduleIceRestart(pc)
          break
        case 'failed':
          this.setState('instavel')
          this.scheduleIceRestart(pc)
          break
        case 'closed':
          this.setState('encerrado')
          break
        default:
          this.setState('conectando')
      }
    }

    return pc
  }

  private scheduleIceRestart(pc: RTCPeerConnection): void {
    if (this.closed || this.iceRestartTimer) return
    this.iceRestartTimer = setTimeout(() => {
      this.iceRestartTimer = null
      if (this.closed || this.pc !== pc) return
      if (pc.connectionState === 'disconnected' || pc.connectionState === 'failed') {
        try {
          pc.restartIce()
        } catch {
          /* ignore */
        }
      }
    }, 3000)
  }

  async handleMessage(msg: HostToViewer): Promise<void> {
    if (this.closed) return
    if (msg.t === 'offer') {
      const pc = this.ensurePeer()
      await pc.setRemoteDescription({ type: 'offer', sdp: msg.desc.sdp })

      for (const candidate of this.pendingIce.splice(0)) {
        await pc.addIceCandidate(candidate).catch(() => {})
      }

      const answer = await pc.createAnswer()
      const sdp = preferOpusStereo(answer.sdp ?? '')
      await pc.setLocalDescription({ type: 'answer', sdp })
      this.send({ t: 'answer', desc: { type: 'answer', sdp } })
      return
    }

    if (msg.t === 'ice') {
      const pc = this.pc
      if (!pc?.remoteDescription) {
        this.pendingIce.push(msg.candidate)
        return
      }
      await pc.addIceCandidate(msg.candidate).catch((err) => {
        console.warn('[rtc] addIceCandidate falhou:', err)
      })
      return
    }

    if (msg.t === 'chat') {
      this.onChat?.(msg.message)
      return
    }

    if (msg.t === 'reaction') {
      this.onReaction?.(msg.reaction)
      return
    }

    if (msg.t === 'source-changed') {
      this.onSourceChanged?.(msg.sourceName, msg.sourceKind)
      return
    }

    if (msg.t === 'voice-answer' && this.voicePc) {
      await this.voicePc.setRemoteDescription({ type: 'answer', sdp: msg.desc.sdp }).catch(() => {})
      for (const cand of this.pendingVoiceIce.splice(0)) {
        await this.voicePc.addIceCandidate(cand).catch(() => {})
      }
      return
    }

    if (msg.t === 'voice-ice') {
      if (!this.voicePc?.remoteDescription) {
        this.pendingVoiceIce.push(msg.candidate)
        return
      }
      await this.voicePc.addIceCandidate(msg.candidate).catch(() => {})
    }
  }

  async setPartyVoiceActive(active: boolean): Promise<boolean> {
    const gen = ++this.voiceGen
    this.stopPartyVoice()
    if (!active || this.closed) return false

    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: { echoCancellation: true, noiseSuppression: true },
        video: false
      })
      if (this.closed || gen !== this.voiceGen) {
        stream.getTracks().forEach((t) => t.stop())
        return false
      }
      this.voiceStream = stream
      const voicePc = new RTCPeerConnection({ iceServers: [], bundlePolicy: 'max-bundle' })
      this.voicePc = voicePc

      voicePc.onicecandidate = (ev) => {
        if (!ev.candidate || this.closed || gen !== this.voiceGen) return
        this.send({ t: 'voice-ice', candidate: toCandidatePayload(ev.candidate) })
      }

      const audioTrack = stream.getAudioTracks()[0]
      if (audioTrack) {
        voicePc.addTransceiver(audioTrack, { direction: 'sendonly', streams: [stream] })
      }

      const offer = await voicePc.createOffer()
      if (this.closed || gen !== this.voiceGen) {
        this.stopPartyVoice()
        return false
      }
      await voicePc.setLocalDescription(offer)
      this.send({ t: 'voice-offer', desc: { type: 'offer', sdp: offer.sdp ?? '' } })
      return true
    } catch (err) {
      console.warn('[ViewerSession] Falha ao ativar Party Voice:', err)
      this.stopPartyVoice()
      return false
    }
  }

  private stopPartyVoice(): void {
    this.pendingVoiceIce = []
    if (this.voiceStream) {
      this.voiceStream.getTracks().forEach((t) => t.stop())
      this.voiceStream = null
    }
    if (this.voicePc) {
      try {
        this.voicePc.close()
      } catch {
        /* ignore */
      }
      this.voicePc = null
    }
  }

  async readStats(): Promise<StreamStats> {
    if (!this.pc) return emptyStats()
    try {
      return await this.sampler.read(this.pc)
    } catch {
      return emptyStats()
    }
  }

  close(): void {
    this.closed = true
    this.voiceGen++
    if (this.iceRestartTimer) {
      clearTimeout(this.iceRestartTimer)
      this.iceRestartTimer = null
    }
    this.stopPartyVoice()
    const pc = this.pc
    this.pc = null
    this.fallbackStream = null
    this.pendingIce = []
    if (!pc) return
    pc.onicecandidate = null
    pc.ontrack = null
    pc.onconnectionstatechange = null
    try {
      pc.close()
    } catch {
      /* ja fechada */
    }
    this.setState('encerrado')
  }
}
