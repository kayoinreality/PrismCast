import type { ChatFragment, ChatPlatform, MultiChatMessage, MultiChatStatus } from '@shared/protocol'
import { ensureBridge } from '@/bridge'

export type Fragment = ChatFragment

export interface ChatLine {
  id: string
  platform: ChatPlatform
  kind: 'msg' | 'event' | 'system'
  user: string
  name: string
  color: string
  badges: string[]
  fragments: Fragment[]
  action: boolean
  mention: boolean
  ts: number
  deleted?: boolean
}

export type ChatState = 'connecting' | 'connected' | 'error'

export const PALETTE = ['#ff7a90', '#ffb547', '#7fd1ff', '#b7a6ff', '#7ee0b5', '#ff9fc6', '#f3eee4', '#9ad0ff']

/** Cores escuras ficam ilegíveis no tema escuro: clareia para contraste visual. */
export function legible(hex: string | undefined, name: string): string {
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

/**
 * Fachada fina sobre o hub `src-rust/multichat.rs` (via `window.api.chat`).
 * Toda conexão IRC/YouTube/Kick, parsing de tags e emotes 7TV/BTTV/FFZ roda em Rust.
 */
export class TwitchChat {
  lines: ChatLine[] = []
  state: ChatState = 'connecting'
  private version = 0
  private readonly listeners = new Set<() => void>()
  private off: (() => void) | null = null

  constructor(readonly channel: string) {
    const api = ensureBridge()
    this.off = api.chat.onMultichat((e) => {
      if (e.t === 'status') {
        const tw = e.status.find((s) => s.platform === 'twitch')
        if (!tw) return
        this.state = tw.state === 'connected' ? 'connected' : tw.state === 'error' ? 'error' : 'connecting'
        this.emit()
      } else if (e.t === 'messages') {
        const twMsgs = e.messages.filter((m) => m.platform === 'twitch').map((m) => toChatLine(m, this.channel))
        if (twMsgs.length === 0) return
        const seen = new Set(this.lines.map((l) => `${l.platform}:${l.id}`))
        const fresh = twMsgs.filter((l) => !seen.has(`${l.platform}:${l.id}`))
        if (fresh.length === 0) return
        this.lines = [...this.lines, ...fresh].slice(-400)
        this.emit()
      } else if (e.t === 'delete' && e.platform === 'twitch') {
        this.lines = this.lines.map((l) => (l.id === e.id ? { ...l, deleted: true } : l))
        this.emit()
      }
    })
  }

  subscribe = (cb: () => void): (() => void) => {
    this.listeners.add(cb)
    return () => this.listeners.delete(cb)
  }

  getVersion = (): number => this.version

  private emit(): void {
    this.version++
    for (const cb of this.listeners) cb()
  }

  dispose(): void {
    this.off?.()
    this.off = null
    this.listeners.clear()
  }
}

export function toChatLine(m: MultiChatMessage, mentionOf: string): ChatLine {
  const text = m.fragments.map((f) => (f.t === 'text' ? f.text : '')).join('')
  const target = mentionOf.trim().toLowerCase()
  const escaped = target.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const mention = target ? new RegExp(`(^|\\W)@${escaped}(\\b|$)`, 'i').test(text) : false
  return {
    id: m.id,
    platform: m.platform,
    kind: m.kind,
    user: m.user,
    name: m.name,
    color: legible(m.color || undefined, m.name),
    badges: m.badges,
    fragments: m.fragments,
    action: false,
    mention,
    ts: m.ts
  }
}

let shared: TwitchChat | null = null

export function chatFor(channel: string): TwitchChat {
  const ch = channel.trim().toLowerCase().replace(/^#/, '')
  if (shared && shared.channel === ch) return shared
  shared?.dispose()
  shared = new TwitchChat(ch)
  return shared
}

export type { MultiChatStatus }
