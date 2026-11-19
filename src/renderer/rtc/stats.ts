/**
 * Leitura de RTCStatsReport.
 *
 * O bitrate nao existe pronto no relatorio: e a diferenca de bytes entre duas
 * amostras dividida pelo tempo. Por isso o sampler guarda estado — cada
 * conexao precisa do seu.
 */

export type QualityLimitation = 'none' | 'cpu' | 'bandwidth' | 'other'

export interface StreamStats {
  bitrateKbps: number
  audioBitrateKbps: number
  fps: number
  width: number
  height: number
  /** Perda acumulada em % dos pacotes esperados. */
  lossPct: number
  rttMs: number | null
  jitterMs: number | null
  codec: string
  /** Só no host: o que esta segurando a qualidade. */
  limitedBy: QualityLimitation
  /** Só no espectador: quadros descartados por atraso de decodificacao. */
  framesDropped: number
}

export function emptyStats(): StreamStats {
  return {
    bitrateKbps: 0,
    audioBitrateKbps: 0,
    fps: 0,
    width: 0,
    height: 0,
    lossPct: 0,
    rttMs: null,
    jitterMs: null,
    codec: '—',
    limitedBy: 'none',
    framesDropped: 0
  }
}

interface Sample {
  videoTimestamp: number
  audioTimestamp: number
  videoBytes: number
  audioBytes: number
  packetsLost: number
  packetsTotal: number
}

/** Os tipos do DOM cobrem so parte do relatorio; o resto vem daqui. */
interface RawStat {
  type: string
  kind?: string
  mediaType?: string
  timestamp: number
  bytesSent?: number
  bytesReceived?: number
  framesPerSecond?: number
  frameWidth?: number
  frameHeight?: number
  packetsLost?: number
  packetsReceived?: number
  packetsSent?: number
  jitter?: number
  roundTripTime?: number
  currentRoundTripTime?: number
  codecId?: string
  mimeType?: string
  qualityLimitationReason?: string
  framesDropped?: number
  nominated?: boolean
  state?: string
  remoteId?: string
}

function shortCodec(mimeType: string | undefined): string {
  if (!mimeType) return '—'
  const name = mimeType.split('/')[1] ?? mimeType
  return name.toUpperCase()
}

export class StatsSampler {
  private previous: Sample | null = null

  /** `direction` decide se lemos outbound-rtp (host) ou inbound-rtp (espectador). */
  constructor(private readonly direction: 'send' | 'receive') {}

  async read(pc: RTCPeerConnection): Promise<StreamStats> {
    const report = await pc.getStats()
    const stats = emptyStats()

    const rtpType = this.direction === 'send' ? 'outbound-rtp' : 'inbound-rtp'
    const byId = new Map<string, RawStat>()
    report.forEach((s) => byId.set((s as RawStat & { id: string }).id, s as RawStat))

    let videoBytes = 0
    let audioBytes = 0
    let videoTs = 0
    let audioTs = 0
    let videoRtp: RawStat | null = null
    let remoteInbound: RawStat | null = null

    report.forEach((raw) => {
      const s = raw as RawStat
      const kind = s.kind ?? s.mediaType

      if (s.type === rtpType) {
        const bytes = (this.direction === 'send' ? s.bytesSent : s.bytesReceived) ?? 0
        if (kind === 'video') {
          videoBytes += bytes
          videoTs = Math.max(videoTs, s.timestamp)
          videoRtp = s
        } else if (kind === 'audio') {
          audioBytes += bytes
          audioTs = Math.max(audioTs, s.timestamp)
        }
      }

      // O RTT confiavel vem do par de candidatos ICE em uso.
      if (s.type === 'candidate-pair' && s.nominated && s.state === 'succeeded') {
        if (typeof s.currentRoundTripTime === 'number') {
          stats.rttMs = Math.round(s.currentRoundTripTime * 1000)
        }
      }

      // No host, a perda so e conhecida pelo relatorio que o espectador devolve.
      if (this.direction === 'send' && s.type === 'remote-inbound-rtp' && kind === 'video') {
        remoteInbound = s
      }
    })

    const rtp = videoRtp as RawStat | null
    const remote = remoteInbound as RawStat | null

    let curLost = 0
    let curTotal = 0

    // Fora do forEach: a ordem das entradas do relatorio nao e garantida, e o
    // calculo de perda precisa do outbound-rtp e do remote-inbound-rtp juntos.
    if (remote) {
      if (stats.rttMs === null && typeof remote.roundTripTime === 'number') {
        stats.rttMs = Math.round(remote.roundTripTime * 1000)
      }
      if (typeof remote.jitter === 'number') stats.jitterMs = Math.round(remote.jitter * 1000)
      const lost = remote.packetsLost ?? 0
      const sent = rtp?.packetsSent ?? 0
      curLost = lost
      curTotal = lost + sent
    }

    if (rtp) {
      stats.fps = Math.round(rtp.framesPerSecond ?? 0)
      stats.width = rtp.frameWidth ?? 0
      stats.height = rtp.frameHeight ?? 0
      stats.codec = shortCodec(rtp.codecId ? byId.get(rtp.codecId)?.mimeType : undefined)

      if (this.direction === 'send') {
        const reason = rtp.qualityLimitationReason
        stats.limitedBy =
          reason === 'cpu' || reason === 'bandwidth' || reason === 'other' ? reason : 'none'
      } else {
        stats.framesDropped = rtp.framesDropped ?? 0
        if (typeof rtp.jitter === 'number') stats.jitterMs = Math.round(rtp.jitter * 1000)
        const lost = rtp.packetsLost ?? 0
        const received = rtp.packetsReceived ?? 0
        curLost = lost
        curTotal = lost + received
      }
    }

    const sample: Sample = {
      videoTimestamp: videoTs,
      audioTimestamp: audioTs,
      videoBytes,
      audioBytes,
      packetsLost: curLost,
      packetsTotal: curTotal
    }
    if (this.previous) {
      if (sample.videoTimestamp > this.previous.videoTimestamp) {
        const sec = (sample.videoTimestamp - this.previous.videoTimestamp) / 1000
        stats.bitrateKbps = Math.max(
          0,
          Math.round(((sample.videoBytes - this.previous.videoBytes) * 8) / sec / 1000)
        )
      }
      if (sample.audioTimestamp > this.previous.audioTimestamp) {
        const sec = (sample.audioTimestamp - this.previous.audioTimestamp) / 1000
        stats.audioBitrateKbps = Math.max(
          0,
          Math.round(((sample.audioBytes - this.previous.audioBytes) * 8) / sec / 1000)
        )
      }
      const dLost = Math.max(0, sample.packetsLost - this.previous.packetsLost)
      const dTotal = Math.max(0, sample.packetsTotal - this.previous.packetsTotal)
      if (dTotal > 0) {
        stats.lossPct = Math.min(100, Math.max(0, (dLost / dTotal) * 100))
      }
    } else if (curTotal > 0) {
      stats.lossPct = Math.min(100, Math.max(0, (curLost / curTotal) * 100))
    }
    this.previous = sample

    return stats
  }
}

export function formatBitrate(kbps: number): string {
  if (kbps >= 1000) return `${(kbps / 1000).toFixed(1)} Mb/s`
  return `${kbps} kb/s`
}

export const LIMITATION_LABEL: Record<QualityLimitation, string> = {
  none: '',
  cpu: 'Limitado pela CPU',
  bandwidth: 'Limitado pela rede',
  other: 'Limitado'
}
