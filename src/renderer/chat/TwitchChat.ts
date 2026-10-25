/**
 * Leitor do chat da Twitch pelo IRC em WebSocket, entrando anônimo (justinfan):
 * não precisa de login para ler. Escrever vai pela API (main), com o login.
 */

export type Fragment = { t: 'text'; text: string } | { t: 'emote'; id: string; name: string }

export interface ChatLine {
  id: string
  kind: 'msg' | 'event' | 'system'
  user: string
  name: string
  color: string
  badges: string[]
  fragments: Fragment[]
  action: boolean
  /** mensagem que menciona o streamer */
  mention: boolean
  ts: number
  deleted?: boolean
}

export type ChatState = 'connecting' | 'connected' | 'error'

const MAX_LINES = 400
const PALETTE = ['#ff7a90', '#ffb547', '#7fd1ff', '#b7a6ff', '#7ee0b5', '#ff9fc6', '#f3eee4', '#9ad0ff']

/** Cores escuras ficam ilegíveis no tema escuro: clareia. */
function legible(hex: string | undefined, name: string): string {
  if (!hex || !/^#[0-9a-f]{6}$/i.test(hex)) {
    let h = 0
    for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0
    return PALETTE[h % PALETTE.length]
  }
  let [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
  const l = (Math.max(r, g, b) + Math.min(r, g, b)) / 2
  if (l >= 0.55) return hex
  const k = (0.62 - l) / (1 - l)
  ;[r, g, b] = [r, g, b].map((v) => v + (1 - v) * k)
  return '#' + [r, g, b].map((v) => Math.round(v * 255).toString(16).padStart(2, '0')).join('')
}

function unescapeTag(v: string): string {
  return v.replace(/\\s/g, ' ').replace(/\\:/g, ';').replace(/\\\\/g, '\\').replace(/\\r/g, '').replace(/\\n/g, '')
}

interface Parsed {
  tags: Record<string, string>
  prefix: string
  cmd: string
  target: string
  text: string
}

function parse(line: string): Parsed | null {
  const tags: Record<string, string> = {}
  let rest = line
  if (rest.startsWith('@')) {
    const sp = rest.indexOf(' ')
    for (const kv of rest.slice(1, sp).split(';')) {
      const eq = kv.indexOf('=')
      tags[eq < 0 ? kv : kv.slice(0, eq)] = eq < 0 ? '' : unescapeTag(kv.slice(eq + 1))
    }
    rest = rest.slice(sp + 1)
  }
  // :prefixo COMANDO param1 param2 :texto final
  const m = /^(?::(\S+?)(?:!\S+)? )?(\S+)((?: [^:\s]\S*)*)(?: :(.*))?$/.exec(rest)
  if (!m) return null
  const params = m[3].trim().split(' ')
  return { tags, prefix: m[1] ?? '', cmd: m[2], target: params[params.length - 1] ?? '', text: m[4] ?? '' }
}

function fragments(text: string, emotes: string | undefined): Fragment[] {
  const chars = Array.from(text)
  if (!emotes) return [{ t: 'text', text }]
  const spots: Array<{ id: string; a: number; b: number }> = []
  for (const part of emotes.split('/')) {
    const [id, ranges] = part.split(':')
    if (!ranges) continue
    for (const r of ranges.split(',')) {
      const [a, b] = r.split('-').map(Number)
      spots.push({ id, a, b })
    }
  }
  spots.sort((x, y) => x.a - y.a)
  const out: Fragment[] = []
  let i = 0
  for (const s of spots) {
    if (s.a > i) out.push({ t: 'text', text: chars.slice(i, s.a).join('') })
    out.push({ t: 'emote', id: s.id, name: chars.slice(s.a, s.b + 1).join('') })
    i = s.b + 1
  }
  if (i < chars.length) out.push({ t: 'text', text: chars.slice(i).join('') })
  return out
}

function badgesOf(raw: string | undefined): string[] {
  if (!raw) return []
  return raw
    .split(',')
    .map((b) => b.split('/')[0])
    .filter((b) => ['broadcaster', 'moderator', 'vip', 'subscriber', 'founder', 'partner'].includes(b))
}

export class TwitchChat {
  lines: ChatLine[] = []
  state: ChatState = 'connecting'
  private ws: WebSocket | null = null
  private retry = 1000
  private timer: number | null = null
  private closed = false
  private readonly listeners = new Set<() => void>()
  private version = 0

  constructor(readonly channel: string) {
    this.connect()
  }

  subscribe = (cb: () => void): (() => void) => {
    this.listeners.add(cb)
    return () => this.listeners.delete(cb)
  }

  /** muda a cada novidade (para o useSyncExternalStore) */
  getVersion = (): number => this.version

  private emit(): void {
    this.version++
    for (const cb of this.listeners) cb()
  }

  private push(line: ChatLine): void {
    this.lines = [...this.lines.slice(-(MAX_LINES - 1)), line]
    this.emit()
  }

  private system(text: string): void {
    this.push({
      id: `sys-${Date.now()}-${Math.random()}`,
      kind: 'system',
      user: '',
      name: '',
      color: '',
      badges: [],
      fragments: [{ t: 'text', text }],
      action: false,
      mention: false,
      ts: Date.now()
    })
  }

  private connect(): void {
    if (this.closed) return
    this.state = 'connecting'
    this.emit()
    const ws = new WebSocket('wss://irc-ws.chat.twitch.tv:443')
    this.ws = ws
    ws.onopen = () => {
      ws.send('CAP REQ :twitch.tv/tags twitch.tv/commands')
      ws.send('PASS SCHMOOPIIE')
      ws.send(`NICK justinfan${Math.floor(10000 + Math.random() * 80000)}`)
      ws.send(`JOIN #${this.channel}`)
    }
    ws.onmessage = (e) => {
      for (const raw of String(e.data).split('\r\n')) {
        if (raw) this.handle(raw, ws)
      }
    }
    ws.onclose = () => {
      if (this.ws !== ws || this.closed) return
      this.state = 'error'
      this.emit()
      this.timer = window.setTimeout(() => this.connect(), this.retry)
      this.retry = Math.min(this.retry * 2, 30_000)
    }
  }

  private handle(raw: string, ws: WebSocket): void {
    if (raw.startsWith('PING')) {
      ws.send('PONG :tmi.twitch.tv')
      return
    }
    const m = parse(raw)
    if (!m) return
    switch (m.cmd) {
      case 'JOIN':
      case '366':
        if (this.state !== 'connected') {
          this.state = 'connected'
          this.retry = 1000
          this.system(`Conectado ao chat de ${this.channel}`)
        }
        break
      case 'PRIVMSG': {
        let text = m.text
        let action = false
        const am = /^\u0001ACTION (.*)\u0001$/.exec(text)
        if (am) {
          text = am[1]
          action = true
        }
        const name = m.tags['display-name'] || m.prefix
        this.push({
          id: m.tags.id || `${Date.now()}-${Math.random()}`,
          kind: m.tags['msg-id'] === 'highlighted-message' || m.tags.bits ? 'event' : 'msg',
          user: m.prefix.toLowerCase(),
          name,
          color: legible(m.tags.color, name),
          badges: badgesOf(m.tags.badges),
          fragments: fragments(text, m.tags.emotes),
          action,
          mention: text.toLowerCase().includes(`@${this.channel}`),
          ts: Number(m.tags['tmi-sent-ts']) || Date.now()
        })
        break
      }
      case 'USERNOTICE': {
        // sub, resub, presente, raid, anúncio
        const sys = m.tags['system-msg'] || ''
        const name = m.tags['display-name'] || m.tags.login || ''
        this.push({
          id: m.tags.id || `${Date.now()}-${Math.random()}`,
          kind: 'event',
          user: (m.tags.login || '').toLowerCase(),
          name,
          color: legible(m.tags.color, name),
          badges: badgesOf(m.tags.badges),
          fragments: [{ t: 'text', text: sys }, ...(m.text ? [{ t: 'text' as const, text: ` — ${m.text}` }] : [])],
          action: false,
          mention: false,
          ts: Number(m.tags['tmi-sent-ts']) || Date.now()
        })
        break
      }
      case 'CLEARMSG': {
        const target = m.tags['target-msg-id']
        this.lines = this.lines.map((l) => (l.id === target ? { ...l, deleted: true } : l))
        this.emit()
        break
      }
      case 'CLEARCHAT': {
        if (m.text) {
          const user = m.text.toLowerCase()
          this.lines = this.lines.map((l) => (l.user === user ? { ...l, deleted: true } : l))
          this.emit()
        } else {
          this.lines = []
          this.system('Um moderador limpou o chat')
        }
        break
      }
      case 'NOTICE':
        if (m.text) this.system(m.text)
        break
      case 'RECONNECT':
        ws.close()
        break
    }
  }

  dispose(): void {
    this.closed = true
    if (this.timer !== null) window.clearTimeout(this.timer)
    this.ws?.close()
    this.ws = null
    this.listeners.clear()
  }
}

/** Uma conexão por janela; trocar de página não reconecta. */
let shared: TwitchChat | null = null

export function chatFor(channel: string): TwitchChat {
  const ch = channel.trim().toLowerCase().replace(/^#/, '')
  if (shared && shared.channel === ch) return shared
  shared?.dispose()
  shared = new TwitchChat(ch)
  return shared
}
