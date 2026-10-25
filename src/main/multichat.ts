import { randomUUID } from 'node:crypto'
import { net } from 'electron'
import type {
  ChatFragment,
  ChatSettings,
  MultiChatEvent,
  MultiChatMessage,
  MultiChatStatus
} from '@shared/protocol'

/**
 * Multichat: lê o chat do YouTube e da Kick no main (o renderer e os widgets
 * não conseguem: CORS e a CSP bloqueiam). A Twitch fica de fora porque o IRC
 * dela abre direto no navegador e cada lugar já lê sozinho.
 *
 * Nenhum dos dois precisa de login para ler:
 * - YouTube: a mesma API interna que a página do chat ao vivo usa (sem chave de API).
 * - Kick: o mesmo WebSocket (Centrifugo) que o site da Kick usa, com token de convidado.
 */

const UA =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36'

type Emit = (event: MultiChatEvent) => void

interface Reader {
  readonly status: MultiChatStatus
  stop(): void
}

function textOnly(text: string): ChatFragment[] {
  return [{ t: 'text', text }]
}

function merge(frags: ChatFragment[]): ChatFragment[] {
  const out: ChatFragment[] = []
  for (const f of frags) {
    const last = out[out.length - 1]
    if (f.t === 'text' && last?.t === 'text') out[out.length - 1] = { t: 'text', text: last.text + f.text }
    else if (f.t !== 'text' || f.text) out.push(f)
  }
  return out
}

async function getText(url: string): Promise<{ status: number; text: string; url: string }> {
  const res = await net.fetch(url, {
    headers: { 'User-Agent': UA, 'Accept-Language': 'pt-BR,pt;q=0.9,en;q=0.8', Cookie: 'CONSENT=YES+1' }
  })
  return { status: res.status, text: await res.text(), url: res.url }
}

// ------------------------------------------------------------------ YouTube

/** "@kayo", "youtube.com/@kayo", link do canal, link da live ou só o id do vídeo. */
function youtubeLiveUrl(input: string): { videoId: string | null; liveUrl: string | null } {
  const v = input.trim()
  const id = /(?:v=|youtu\.be\/|\/live\/|\/shorts\/)([\w-]{11})/.exec(v)?.[1] ?? (/^[\w-]{11}$/.test(v) ? v : null)
  if (id && !v.startsWith('@')) return { videoId: id, liveUrl: null }
  const handle = /(?:youtube\.com\/)?(@[\w.-]+)/.exec(v)?.[1]
  if (handle) return { videoId: null, liveUrl: `https://www.youtube.com/${handle}/live` }
  const channel = /(UC[\w-]{22})/.exec(v)?.[1]
  if (channel) return { videoId: null, liveUrl: `https://www.youtube.com/channel/${channel}/live` }
  return { videoId: null, liveUrl: `https://www.youtube.com/@${v.replace(/^@/, '')}/live` }
}

interface YtRun {
  text?: string
  emoji?: { emojiId?: string; shortcuts?: string[]; image?: { thumbnails?: Array<{ url: string }> } }
}

interface YtRenderer {
  id?: string
  timestampUsec?: string
  authorName?: { simpleText?: string }
  authorExternalChannelId?: string
  authorBadges?: Array<{ liveChatAuthorBadgeRenderer?: { icon?: { iconType?: string }; tooltip?: string } }>
  message?: { runs?: YtRun[] }
  headerSubtext?: { runs?: YtRun[]; simpleText?: string }
  purchaseAmountText?: { simpleText?: string }
}

function ytRuns(runs: YtRun[] | undefined): ChatFragment[] {
  return (runs ?? []).map((r): ChatFragment => {
    if (r.emoji) {
      const url = r.emoji.image?.thumbnails?.at(-1)?.url
      const name = r.emoji.shortcuts?.[0] ?? r.emoji.emojiId ?? ''
      // Emoji comum (unicode) vem com o próprio caractere no id: fica como texto.
      if (!url || (r.emoji.emojiId && !r.emoji.emojiId.includes('/') && r.emoji.emojiId.length <= 8)) {
        return { t: 'text', text: r.emoji.emojiId ?? name }
      }
      return { t: 'emote', url, name }
    }
    return { t: 'text', text: r.text ?? '' }
  })
}

function ytBadges(r: YtRenderer): string[] {
  const out: string[] = []
  for (const b of r.authorBadges ?? []) {
    const badge = b.liveChatAuthorBadgeRenderer
    const icon = badge?.icon?.iconType
    if (icon === 'OWNER') out.push('broadcaster')
    else if (icon === 'MODERATOR') out.push('moderator')
    else if (icon === 'VERIFIED') out.push('verified')
    else if (!icon && badge?.tooltip) out.push('member')
  }
  return out
}

class YouTubeReader implements Reader {
  status: MultiChatStatus
  private stopped = false
  private timer: NodeJS.Timeout | null = null
  private apiKey = ''
  private clientVersion = '2.20250101.00.00'
  private continuation = ''
  private failures = 0

  constructor(
    private readonly channel: string,
    private readonly emit: Emit,
    private readonly onStatus: () => void
  ) {
    this.status = { platform: 'youtube', state: 'connecting', channel, message: null }
    void this.start()
  }

  private set(state: MultiChatStatus['state'], message: string | null = null): void {
    this.status = { ...this.status, state, message }
    this.onStatus()
  }

  private later(ms: number, fn: () => Promise<void>): void {
    if (this.stopped) return
    if (this.timer) clearTimeout(this.timer)
    this.timer = setTimeout(() => void fn(), ms)
  }

  private async findVideo(): Promise<string | null> {
    const { videoId, liveUrl } = youtubeLiveUrl(this.channel)
    if (videoId) return videoId
    const page = await getText(liveUrl!)
    if (page.status === 404) throw new Error('Canal do YouTube não encontrado.')
    // Com live no ar, /live mostra a página do vídeo; sem live, a do canal.
    const canonical = /<link rel="canonical" href="[^"]*[?&]v=([\w-]{11})/.exec(page.text)?.[1]
    if (!canonical) return null
    // Live no ar ou agendada (o chat de uma agendada já abre antes).
    return /"(?:isLiveNow|isUpcoming)"\s*:\s*true/.test(page.text) ? canonical : null
  }

  private async start(): Promise<void> {
    if (this.stopped) return
    this.set('connecting')
    try {
      const videoId = await this.findVideo()
      if (this.stopped) return
      if (!videoId) {
        this.set('waiting', 'O canal não está ao vivo no YouTube agora.')
        this.later(60_000, () => this.start())
        return
      }
      const page = await getText(`https://www.youtube.com/live_chat?is_popout=1&v=${videoId}`)
      if (this.stopped) return
      this.apiKey = /"INNERTUBE_API_KEY"\s*:\s*"([^"]+)"/.exec(page.text)?.[1] ?? ''
      this.clientVersion = /"INNERTUBE_CLIENT_VERSION"\s*:\s*"([^"]+)"/.exec(page.text)?.[1] ?? this.clientVersion
      // O menu "Chat principal / Chat ao vivo": o último é o ao vivo (todas as
      // mensagens, sem o filtro). Sem o menu, a continuação da própria página.
      const menu = [...page.text.matchAll(/"reloadContinuationData"\s*:\s*\{\s*"continuation"\s*:\s*"([^"]+)"/g)]
      const first = /"continuation"\s*:\s*"([^"]+)"/.exec(page.text)?.[1]
      this.continuation = menu.at(-1)?.[1] ?? first ?? ''
      if (!this.apiKey || !this.continuation) {
        this.set('waiting', 'Esta live não tem chat (ou ele está desligado).')
        this.later(60_000, () => this.start())
        return
      }
      this.failures = 0
      this.set('connected')
      await this.poll()
    } catch (err) {
      this.retry(err as Error)
    }
  }

  private retry(err: Error): void {
    this.failures++
    this.set('error', err.message || 'Falha ao ler o chat do YouTube.')
    this.later(Math.min(60_000, 5000 * this.failures), () => this.start())
  }

  private async poll(): Promise<void> {
    if (this.stopped) return
    try {
      const res = await net.fetch(
        `https://www.youtube.com/youtubei/v1/live_chat/get_live_chat?key=${encodeURIComponent(this.apiKey)}&prettyPrint=false`,
        {
          method: 'POST',
          headers: { 'User-Agent': UA, 'Content-Type': 'application/json' },
          body: JSON.stringify({
            context: { client: { clientName: 'WEB', clientVersion: this.clientVersion, hl: 'pt', gl: 'BR' } },
            continuation: this.continuation
          })
        }
      )
      if (this.stopped) return
      if (!res.ok) throw new Error(`YouTube respondeu ${res.status}`)
      const data = (await res.json()) as {
        continuationContents?: {
          liveChatContinuation?: {
            actions?: Array<Record<string, unknown>>
            continuations?: Array<Record<string, { continuation?: string; timeoutMs?: number }>>
          }
        }
      }
      const chat = data.continuationContents?.liveChatContinuation
      if (!chat) {
        // A live acabou: volta a esperar a próxima.
        this.set('waiting', 'A live do YouTube terminou.')
        this.later(30_000, () => this.start())
        return
      }
      this.handle(chat.actions ?? [])
      const next = Object.values(chat.continuations?.[0] ?? {})[0]
      if (next?.continuation) this.continuation = next.continuation
      this.failures = 0
      if (this.status.state !== 'connected') this.set('connected')
      this.later(Math.max(1000, Math.min(8000, next?.timeoutMs ?? 3000)), () => this.poll())
    } catch (err) {
      this.retry(err as Error)
    }
  }

  private handle(actions: Array<Record<string, unknown>>): void {
    const messages: MultiChatMessage[] = []
    for (const action of actions) {
      const del = action.markChatItemAsDeletedAction as { targetItemId?: string } | undefined
      if (del?.targetItemId) {
        this.emit({ t: 'delete', platform: 'youtube', id: `yt-${del.targetItemId}` })
        continue
      }
      const item = (action.addChatItemAction as { item?: Record<string, YtRenderer> } | undefined)?.item
      if (!item) continue
      const [type, r] = Object.entries(item)[0] ?? []
      if (!r?.id) continue
      const base = {
        id: `yt-${r.id}`,
        platform: 'youtube' as const,
        user: r.authorExternalChannelId ?? r.authorName?.simpleText ?? '',
        name: (r.authorName?.simpleText ?? '').replace(/^@/, ''),
        color: '',
        badges: ytBadges(r),
        ts: r.timestampUsec ? Math.round(Number(r.timestampUsec) / 1000) : Date.now()
      }
      if (type === 'liveChatTextMessageRenderer') {
        messages.push({ ...base, kind: 'msg', fragments: merge(ytRuns(r.message?.runs)) })
      } else if (type === 'liveChatPaidMessageRenderer' || type === 'liveChatPaidStickerRenderer') {
        const amount = r.purchaseAmountText?.simpleText ?? ''
        messages.push({
          ...base,
          kind: 'event',
          fragments: merge([{ t: 'text', text: `Super Chat ${amount}${r.message?.runs ? ' — ' : ''}` }, ...ytRuns(r.message?.runs)])
        })
      } else if (type === 'liveChatMembershipItemRenderer') {
        const head = r.headerSubtext?.simpleText ?? ytRuns(r.headerSubtext?.runs).map((f) => (f.t === 'text' ? f.text : f.name)).join('')
        messages.push({
          ...base,
          kind: 'event',
          fragments: merge([{ t: 'text', text: head || 'Novo membro' }, ...(r.message?.runs ? [{ t: 'text' as const, text: ' — ' }, ...ytRuns(r.message.runs)] : [])])
        })
      }
    }
    if (messages.length > 0) this.emit({ t: 'messages', messages })
  }

  stop(): void {
    this.stopped = true
    if (this.timer) clearTimeout(this.timer)
  }
}

// --------------------------------------------------------------------- Kick

/**
 * A Kick saiu do Pusher: o chat agora vem do Centrifugo deles. O site pede um
 * token de convidado e o endereço do servidor à API web e assina a sala
 * "chatrooms.<id>.v2". As mensagens continuam no mesmo formato de antes.
 */
const KICK_API = 'https://web.kick.com/api/v1/realtime'
const KICK_REALTIME = 'wss://realtime.us-east-1.platform.kick.com/connection/websocket'
const KICK_HEADERS = {
  'User-Agent': UA,
  Accept: 'application/json',
  'Content-Type': 'application/json',
  Origin: 'https://kick.com',
  Referer: 'https://kick.com/'
}

async function kickPost<T>(path: string, body: unknown): Promise<T | null> {
  const res = await net.fetch(`${KICK_API}${path}`, { method: 'POST', headers: KICK_HEADERS, body: JSON.stringify(body) })
  if (!res.ok) return null
  try {
    return ((await res.json()) as { data?: T }).data ?? null
  } catch {
    return null
  }
}

function kickSlug(input: string): string {
  const v = input.trim()
  return (/kick\.com\/([\w-]+)/i.exec(v)?.[1] ?? v.replace(/^@/, '')).toLowerCase()
}

/** "[emote:37226:KEKW]" vira imagem. */
function kickFragments(content: string): ChatFragment[] {
  const out: ChatFragment[] = []
  let last = 0
  for (const m of content.matchAll(/\[emote:(\d+):([^\]]*)\]/g)) {
    if (m.index! > last) out.push({ t: 'text', text: content.slice(last, m.index) })
    out.push({ t: 'emote', url: `https://files.kick.com/emotes/${m[1]}/fullsize`, name: m[2] })
    last = m.index! + m[0].length
  }
  if (last < content.length) out.push({ t: 'text', text: content.slice(last) })
  return merge(out)
}

const KICK_BADGES: Record<string, string> = {
  broadcaster: 'broadcaster',
  moderator: 'moderator',
  vip: 'vip',
  og: 'vip',
  subscriber: 'subscriber',
  founder: 'founder',
  verified: 'verified'
}

class KickReader implements Reader {
  status: MultiChatStatus
  private stopped = false
  private ws: WebSocket | null = null
  private timer: NodeJS.Timeout | null = null
  private refreshTimer: NodeJS.Timeout | null = null
  private failures = 0
  private nextId = 1
  private readonly slug: string
  private readonly clientId = randomUUID()

  constructor(
    channel: string,
    private readonly emit: Emit,
    private readonly onStatus: () => void
  ) {
    this.slug = kickSlug(channel)
    this.status = { platform: 'kick', state: 'connecting', channel: this.slug, message: null }
    void this.start()
  }

  private set(state: MultiChatStatus['state'], message: string | null = null): void {
    this.status = { ...this.status, state, message }
    this.onStatus()
  }

  private retry(message: string): void {
    if (this.stopped) return
    this.failures++
    this.set('error', message)
    this.clearTimers()
    this.timer = setTimeout(() => void this.start(), Math.min(60_000, 3000 * this.failures))
  }

  private clearTimers(): void {
    if (this.timer) clearTimeout(this.timer)
    if (this.refreshTimer) clearTimeout(this.refreshTimer)
    this.timer = null
    this.refreshTimer = null
  }

  private async channel(): Promise<{ channelId: number; chatroomId: number }> {
    for (const url of [`https://kick.com/api/v2/channels/${this.slug}`, `https://kick.com/api/v1/channels/${this.slug}`]) {
      const res = await getText(url)
      if (res.status === 404) throw new Error('Canal da Kick não encontrado.')
      if (res.status !== 200) continue
      try {
        const ch = JSON.parse(res.text) as { id?: number; chatroom?: { id?: number } }
        if (ch.id && ch.chatroom?.id) return { channelId: ch.id, chatroomId: ch.chatroom.id }
      } catch {
        /* página de desafio do Cloudflare em vez de JSON: tenta a outra */
      }
    }
    throw new Error('A Kick não respondeu (proteção do site). Tentando de novo…')
  }

  private async token(): Promise<string> {
    const auth = await kickPost<{ token?: string }>('/auth/connection', { client_id: this.clientId })
    if (!auth?.token) throw new Error('A Kick não liberou o chat agora. Tentando de novo…')
    return auth.token
  }

  /** Endereço do servidor do chat (a Kick diz qual; se não disser, o de sempre). */
  private async realtimeUrl(channelId: number): Promise<string> {
    try {
      const conn = await kickPost<{ connections?: Array<{ provider?: string; credentials?: { url?: string } }> }>(
        `/channels/${channelId}/chat/connection`,
        { client: { id: this.clientId, type: 'web' }, capabilities: { accepted_providers: [{ provider: 'centrifugo' }] } }
      )
      return conn?.connections?.find((c) => c.provider === 'centrifugo')?.credentials?.url ?? KICK_REALTIME
    } catch {
      return KICK_REALTIME
    }
  }

  private async start(): Promise<void> {
    if (this.stopped) return
    this.clearTimers()
    this.set('connecting')
    let room: { channelId: number; chatroomId: number }
    let token: string
    let url: string
    try {
      room = await this.channel()
      ;[token, url] = await Promise.all([this.token(), this.realtimeUrl(room.channelId)])
    } catch (err) {
      this.retry((err as Error).message)
      return
    }
    if (this.stopped) return
    const ws = new WebSocket(url)
    this.ws = ws
    const send = (msg: unknown): void => {
      if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg))
    }
    ws.onopen = () => {
      send({ id: this.nextId++, connect: { token, name: 'js' } })
      send({ id: this.nextId++, subscribe: { channel: `chatrooms.${room.chatroomId}.v2` } })
    }
    ws.onmessage = (e) => {
      // Várias respostas podem vir na mesma mensagem, uma por linha.
      for (const line of String(e.data).split('\n')) {
        if (!line) continue
        // Ping do servidor é "{}": responde igual ou ele derruba a conexão.
        if (line === '{}') {
          ws.send('{}')
          continue
        }
        let msg: {
          error?: { message?: string }
          connect?: { ttl?: number; expires?: boolean }
          subscribe?: unknown
          push?: { pub?: { data?: { event?: string; data?: unknown } } }
        }
        try {
          msg = JSON.parse(line) as typeof msg
        } catch {
          continue
        }
        if (msg.error) {
          ws.close()
          this.retry(`Kick: ${msg.error.message ?? 'erro no chat'}`)
          return
        }
        if (msg.connect?.expires && msg.connect.ttl) this.scheduleRefresh(ws, msg.connect.ttl)
        if (msg.subscribe) {
          this.failures = 0
          this.set('connected')
        }
        const pub = msg.push?.pub?.data
        if (pub?.event) this.onEvent(pub.event, pub.data)
      }
    }
    ws.onclose = () => {
      if (this.ws !== ws || this.stopped) return
      this.retry('Conexão com a Kick caiu. Reconectando…')
    }
    ws.onerror = () => {
      /* o onclose cuida */
    }
  }

  /** O token de convidado vence (45 min): renova antes, sem derrubar o chat. */
  private scheduleRefresh(ws: WebSocket, ttlSec: number): void {
    if (this.refreshTimer) clearTimeout(this.refreshTimer)
    this.refreshTimer = setTimeout(
      () => {
        void this.token()
          .then((token) => {
            if (this.ws === ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ id: this.nextId++, refresh: { token } }))
          })
          .catch(() => ws.close())
      },
      Math.max(10, ttlSec - 120) * 1000
    )
  }

  private onEvent(event: string, raw: unknown): void {
    let data: Record<string, unknown>
    try {
      data = (typeof raw === 'string' ? JSON.parse(raw) : raw) as Record<string, unknown>
    } catch {
      return
    }
    if (!data) return
    if (event === 'App\\Events\\ChatMessageEvent') this.onMessage(data)
    else if (event === 'App\\Events\\MessageDeletedEvent') {
      const target = (data.message as { id?: string } | undefined)?.id
      if (target) this.emit({ t: 'delete', platform: 'kick', id: `kick-${target}` })
    }
  }

  private onMessage(d: Record<string, unknown>): void {
    const sender = (d.sender ?? {}) as {
      username?: string
      slug?: string
      identity?: { color?: string; badges?: Array<{ type?: string }> }
    }
    const badges = (sender.identity?.badges ?? [])
      .map((b) => KICK_BADGES[b.type ?? ''])
      .filter((b): b is string => Boolean(b))
    const content = String(d.content ?? '')
    this.emit({
      t: 'messages',
      messages: [
        {
          id: `kick-${String(d.id ?? `${Date.now()}-${Math.random()}`)}`,
          platform: 'kick',
          kind: 'msg',
          user: sender.slug ?? (sender.username ?? '').toLowerCase(),
          name: sender.username ?? '',
          color: sender.identity?.color ?? '',
          badges: [...new Set(badges)],
          fragments: content ? kickFragments(content) : textOnly(''),
          ts: d.created_at ? Date.parse(String(d.created_at)) || Date.now() : Date.now()
        }
      ]
    })
  }

  stop(): void {
    this.stopped = true
    this.clearTimers()
    const ws = this.ws
    this.ws = null
    ws?.close()
  }
}

// --------------------------------------------------------------------- hub

export class MultiChatHub {
  private youtube: { key: string; reader: Reader } | null = null
  private kick: { key: string; reader: Reader } | null = null
  /** junta mensagens que chegam juntas num envio só */
  private pending: MultiChatMessage[] = []
  private flushTimer: NodeJS.Timeout | null = null

  constructor(private readonly send: Emit) {}

  configure(chat: Pick<ChatSettings, 'youtube' | 'kick'>, force = false): void {
    const yt = chat.youtube.trim()
    const kick = chat.kick.trim()
    if (force || this.youtube?.key !== yt) {
      this.youtube?.reader.stop()
      this.youtube = yt ? { key: yt, reader: new YouTubeReader(yt, this.emit, this.statusChanged) } : null
    }
    if (force || this.kick?.key !== kick) {
      this.kick?.reader.stop()
      this.kick = kick ? { key: kick, reader: new KickReader(kick, this.emit, this.statusChanged) } : null
    }
    this.statusChanged()
  }

  status(): MultiChatStatus[] {
    return [
      this.youtube?.reader.status ?? { platform: 'youtube', state: 'off', channel: '', message: null },
      this.kick?.reader.status ?? { platform: 'kick', state: 'off', channel: '', message: null }
    ]
  }

  private readonly statusChanged = (): void => {
    this.send({ t: 'status', status: this.status() })
  }

  private readonly emit: Emit = (event) => {
    if (event.t !== 'messages') {
      this.send(event)
      return
    }
    this.pending.push(...event.messages)
    this.flushTimer ??= setTimeout(() => {
      this.flushTimer = null
      const messages = this.pending
      this.pending = []
      if (messages.length > 0) this.send({ t: 'messages', messages })
    }, 120)
  }

  dispose(): void {
    this.youtube?.reader.stop()
    this.kick?.reader.stop()
    this.youtube = null
    this.kick = null
    if (this.flushTimer) clearTimeout(this.flushTimer)
  }
}
