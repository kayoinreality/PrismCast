import type { ChatPlatform, MultiChatStatus } from '@shared/protocol'
import { ensureBridge } from '@/bridge'
import { toChatLine, type ChatLine } from './TwitchChat'

const MAX_LINES = 400

/**
 * Fachada fina para eventos YouTube + Kick vindos do hub Rust (`src-rust/multichat.rs`).
 */
class MultiChatStore {
  lines: ChatLine[] = []
  status: MultiChatStatus[] = []
  mentionOf = ''
  private version = 0
  private readonly listeners = new Set<() => void>()

  constructor() {
    const api = ensureBridge()
    api.chat.onMultichat((e) => {
      if (e.t === 'messages') {
        const nonTwitch = e.messages.filter((m) => m.platform !== 'twitch')
        if (nonTwitch.length === 0) return
        const seen = new Set(this.lines.map((l) => `${l.platform}:${l.id}`))
        const fresh = nonTwitch
          .filter((m) => !seen.has(`${m.platform}:${m.id}`))
          .map((m) => toChatLine(m, this.mentionOf))
        if (fresh.length === 0) return
        this.lines = [...this.lines, ...fresh].slice(-MAX_LINES)
      } else if (e.t === 'delete') {
        this.lines = this.lines.map((l) =>
          l.platform === e.platform && l.id === e.id ? { ...l, deleted: true } : l
        )
      } else {
        this.status = e.status.filter((s) => s.platform !== 'twitch')
        const off = new Set<ChatPlatform>(this.status.filter((s) => s.state === 'off').map((s) => s.platform))
        if (off.size > 0) this.lines = this.lines.filter((l) => !off.has(l.platform))
      }
      this.emit()
    })
    void api.chat
      .multichatStatus()
      .then((s) => {
        this.status = s.filter((st) => st.platform !== 'twitch')
        this.emit()
      })
      .catch(() => {})
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
}

let store: MultiChatStore | null = null

export function multiChat(): MultiChatStore {
  store ??= new MultiChatStore()
  return store
}
