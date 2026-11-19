import type { VideoCodec } from '@shared/quality'

/**
 * Ajustes de SDP e de codec.
 *
 * O Opus do WebRTC vem mono e em ~64kbps por padrao, o que e razoavel para
 * uma chamada de voz e ruim para a trilha de um filme. Os parametros so entram
 * pelo `a=fmtp`, entao nao ha como pedir isso pela API — tem que ser no texto.
 */

const OPUS_RTPMAP = /a=rtpmap:(\d+) opus\/48000\/2/i

export function preferOpusStereo(sdp: string, bitrateBps = 192_000): string {
  const match = sdp.match(OPUS_RTPMAP)
  if (!match) return sdp

  const pt = match[1]
  const params = [
    'stereo=1',
    'sprop-stereo=1',
    `maxaveragebitrate=${bitrateBps}`,
    'useinbandfec=1'
  ]

  const lines = sdp.split(/\r\n|\n/)
  const fmtpIndex = lines.findIndex((l) => l.startsWith(`a=fmtp:${pt} `))

  if (fmtpIndex >= 0) {
    const existing = lines[fmtpIndex].slice(`a=fmtp:${pt} `.length)
    const map = new Map<string, string>()
    const order: string[] = []
    for (const kv of existing.split(';')) {
      const trimmed = kv.trim()
      if (!trimmed) continue
      const eq = trimmed.indexOf('=')
      const k = (eq < 0 ? trimmed : trimmed.slice(0, eq)).trim()
      const v = eq < 0 ? '' : trimmed.slice(eq + 1).trim()
      if (!map.has(k)) order.push(k)
      map.set(k, v)
    }
    // Sobrescreve chaves existentes para que stereo/bitrate reflitam o pedido (RFC 7587).
    for (const p of params) {
      const [k, v] = p.split('=')
      if (!map.has(k)) order.push(k)
      map.set(k, v)
    }
    lines[fmtpIndex] = `a=fmtp:${pt} ${order.map((k) => (map.get(k) ? `${k}=${map.get(k)}` : k)).join(';')}`
  } else {
    const rtpmapIndex = lines.findIndex((l) => l.startsWith(`a=rtpmap:${pt} `))
    if (rtpmapIndex < 0) return sdp
    lines.splice(rtpmapIndex + 1, 0, `a=fmtp:${pt} ${params.join(';')}`)
  }

  // O SDP precisa voltar com CRLF: alguns parsers rejeitam LF sozinho.
  return lines.join('\r\n')
}

const MIME_BY_CODEC: Record<Exclude<VideoCodec, 'auto'>, string> = {
  h264: 'video/H264',
  vp9: 'video/VP9',
  av1: 'video/AV1'
}

/**
 * Reordena os codecs do transceiver deixando o escolhido na frente.
 *
 * Os outros continuam na lista: se a outra ponta nao suportar o preferido, a
 * negociacao ainda fecha em vez de falhar. 'auto' resolve para H.264, que e o
 * unico com encode por hardware garantido em NVIDIA, Intel e AMD no Windows —
 * e isso e o que decide se da para jogar enquanto transmite.
 */
export function applyCodecPreference(
  transceiver: RTCRtpTransceiver,
  codec: VideoCodec
): void {
  const capabilities = RTCRtpSender.getCapabilities('video')
  if (!capabilities?.codecs?.length) return

  const wanted = MIME_BY_CODEC[codec === 'auto' ? 'h264' : codec].toLowerCase()
  const codecs = capabilities.codecs
  const preferred: (typeof codecs)[number][] = []
  const rest: (typeof codecs)[number][] = []

  // Agrupa cada codec com seu video/rtx imediatamente seguinte e prioriza High/Main em H.264.
  const h264Rank = (fmtp: string | undefined): number => {
    const s = (fmtp ?? '').toLowerCase()
    if (s.includes('profile-level-id=6400')) return 0 // High
    if (s.includes('profile-level-id=4d00')) return 1 // Main
    if (s.includes('profile-level-id=42e0')) return 2 // Constrained Baseline
    return 3
  }

  const preferredGroups: Array<{ rank: number; items: (typeof codecs)[number][] }> = []
  for (let i = 0; i < codecs.length; i++) {
    const c = codecs[i]
    const next = codecs[i + 1]
    const hasRtx = next && next.mimeType.toLowerCase() === 'video/rtx'
    const pair = hasRtx ? [c, next] : [c]
    if (hasRtx) i++

    if (c.mimeType.toLowerCase() === wanted) {
      preferredGroups.push({ rank: wanted === 'video/h264' ? h264Rank(c.sdpFmtpLine) : 0, items: pair })
    } else {
      rest.push(...pair)
    }
  }

  if (preferredGroups.length === 0) return
  preferredGroups.sort((a, b) => a.rank - b.rank)
  for (const g of preferredGroups) preferred.push(...g.items)

  try {
    transceiver.setCodecPreferences([...preferred, ...rest])
  } catch (err) {
    // Navegador sem suporte ao codec pedido: seguir com a ordem padrao.
    console.warn('[rtc] setCodecPreferences falhou:', err)
  }
}
