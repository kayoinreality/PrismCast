import type {
  ChatMessage,
  HostToViewer,
  ReactionEvent,
  SourceKind,
  ViewerToHost
} from '@shared/protocol'
import { StatsSampler, emptyStats, type StreamStats } from './stats'
import { toCandidatePayload } from './ice'

export type ViewerState = 'conectando' | 'recebendo' | 'instavel' | 'encerrado'

export class ViewerSession {
  private pc: RTCPeerConnection | null = null
  private voicePc: RTCPeerConnection | null = null
  private voiceStream: MediaStream | null = null
  private readonly sampler = new StatsSampler('receive')
  private pendingIce: RTCIceCandidateInit[] = []
  private state: ViewerState = 'conectando'

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
      const stream = event.streams[0]
      if (stream) this.onStream?.(stream)
    }

    pc.onconnectionstatechange = () => {
      switch (pc.connectionState) {
        case 'connected':
          this.setState('recebendo')
          break
        case 'disconnected':
          this.setState('instavel')
          break
        case 'failed':
        case 'closed':
          this.setState('encerrado')
          break
        default:
          this.setState('conectando')
      }
    }

    return pc
  }

  async handleMessage(msg: HostToViewer): Promise<void> {
    if (msg.t === 'offer') {
      const pc = this.ensurePeer()
      await pc.setRemoteDescription({ type: 'offer', sdp: msg.desc.sdp })

      for (const candidate of this.pendingIce.splice(0)) {
        await pc.addIceCandidate(candidate).catch(() => {})
      }

      const answer = await pc.createAnswer()
      await pc.setLocalDescription(answer)
      this.send({ t: 'answer', desc: { type: 'answer', sdp: answer.sdp ?? '' } })
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
      return
    }

    if (msg.t === 'voice-ice' && this.voicePc?.remoteDescription) {
      await this.voicePc.addIceCandidate(msg.candidate).catch(() => {})
    }
  }

  async setPartyVoiceActive(active: boolean): Promise<boolean> {
    this.stopPartyVoice()
    if (!active) return false

    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: { echoCancellation: true, noiseSuppression: true },
        video: false
      })
      this.voiceStream = stream
      const voicePc = new RTCPeerConnection({ iceServers: [], bundlePolicy: 'max-bundle' })
      this.voicePc = voicePc

      voicePc.onicecandidate = (ev) => {
        if (!ev.candidate) return
        this.send({ t: 'voice-ice', candidate: toCandidatePayload(ev.candidate) })
      }

      const audioTrack = stream.getAudioTracks()[0]
      if (audioTrack) {
        voicePc.addTransceiver(audioTrack, { direction: 'sendonly', streams: [stream] })
      }

      const offer = await voicePc.createOffer()
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
    this.stopPartyVoice()
    const pc = this.pc
    this.pc = null
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
