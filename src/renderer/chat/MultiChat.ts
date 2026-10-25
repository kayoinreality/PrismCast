import type { ChatPlatform, MultiChatMessage, MultiChatStatus } from '@shared/protocol'
import { legible, type ChatLine } from './TwitchChat'

/**
 * YouTube e Kick, lidos no main e entregues por IPC. Junta com a Twitch no
 * ChatPanel. Uma instância por janela, viva desde a abertura: trocar de tela
 * não perde mensagens.
 */

const MAX_LINES = 400

function toLine(m: MultiChatMessage, mentionOf: string): ChatLine {
  const text = m.fragments.map((f) => (f.t === 'text' ? f.text : '')).join('')
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
    mention: mentionOf ? text.toLowerCase().includes(`@${mentionOf}`) : false,
    ts: m.ts
  }
}

class MultiChatStore {
  lines: ChatLine[] = []
  status: MultiChatStatus[] = []
  /** nome do streamer, para destacar quem marca @ele */
  mentionOf = ''
  private version = 0
  private readonly listeners = new Set<() => void>()

  constructor() {
    window.api.chat.onMultichat((e) => {
      if (e.t === 'messages') {
        const seen = new Set(this.lines.map((l) => l.id))
        const fresh = e.messages.filter((m) => !seen.has(m.id)).map((m) => toLine(m, this.mentionOf))
        if (fresh.length === 0) return
        this.lines = [...this.lines, ...fresh].slice(-MAX_LINES)
      } else if (e.t === 'delete') {
        this.lines = this.lines.map((l) => (l.id === e.id ? { ...l, deleted: true } : l))
      } else {
        this.status = e.status
        // Canal apagado nas opções: as mensagens dele saem da lista junto.
        const off = new Set<ChatPlatform>(e.status.filter((s) => s.state === 'off').map((s) => s.platform))
        if (off.size > 0) this.lines = this.lines.filter((l) => !off.has(l.platform))
      }
      this.emit()
    })
    void window.api.chat.multichatStatus().then((s) => {
      this.status = s
      this.emit()
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
}

let store: MultiChatStore | null = null

export function multiChat(): MultiChatStore {
  store ??= new MultiChatStore()
  return store
}
