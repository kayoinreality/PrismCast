import { randomUUID } from 'node:crypto'
import type { IncomingMessage } from 'node:http'
import { WebSocket, WebSocketServer } from 'ws'
import {
  PIN_BLOCK_MS,
  PIN_MAX_ATTEMPTS,
  PROTOCOL_VERSION,
  REJECT_MESSAGES,
  type ChatMessage,
  type ConnectedViewer,
  type HostToViewer,
  type ReactionEvent,
  type RejectReason,
  type SourceKind,
  type ViewerToHost
} from '@shared/protocol'
import type { ViewerConnectOptions, ViewerConnectResult, ViewerClosedInfo } from '@shared/ipc'

const HEARTBEAT_MS = 2000
const HEARTBEAT_TIMEOUT_MS = 8000
const HELLO_TIMEOUT_MS = 5000

function normalizeAddress(raw: string | undefined): string {
  if (!raw) return 'desconhecido'
  return raw.startsWith('::ffff:') ? raw.slice(7) : raw
}

interface ViewerSlot {
  ws: WebSocket
  info: ConnectedViewer
  lastPong: number
  pingSentAt: number
}

export interface HostSessionInfo {
  hostName: string
  sourceName: string
  sourceKind: SourceKind
  pin: string | null
  maxViewers: number
}

export class SignalingServer {
  private wss: WebSocketServer | null = null
  private readonly viewers = new Map<string, ViewerSlot>()
  private readonly attempts = new Map<string, { count: number; blockedUntil: number }>()
  private heartbeat: NodeJS.Timeout | null = null
  private session: HostSessionInfo | null = null

  onViewers: ((viewers: ConnectedViewer[]) => void) | null = null
  onMessage: ((viewerId: string, msg: ViewerToHost) => void) | null = null
  onChat: ((msg: ChatMessage) => void) | null = null
  onReaction: ((reaction: ReactionEvent) => void) | null = null

  get viewerCount(): number {
    return this.viewers.size
  }

  async start(port: number, session: HostSessionInfo): Promise<void> {
    this.session = session
    const wss = new WebSocketServer({ port, host: '0.0.0.0', maxPayload: 256 * 1024 })
    this.wss = wss

    await new Promise<void>((resolve, reject) => {
      wss.once('error', reject)
      wss.once('listening', () => {
        wss.removeListener('error', reject)
        resolve()
      })
    })

    wss.on('error', (err) => console.error('[signaling] erro no servidor:', err.message))
    wss.on('connection', (ws, req) => this.handleConnection(ws, req))

    this.heartbeat = setInterval(() => this.tick(), HEARTBEAT_MS)
  }

  updateSession(patch: Partial<HostSessionInfo>): void {
    if (this.session) {
      this.session = { ...this.session, ...patch }
      if (patch.sourceName && patch.sourceKind) {
        this.broadcast({
          t: 'source-changed',
          sourceName: patch.sourceName,
          sourceKind: patch.sourceKind
        })
      }
    }
  }

  private handleConnection(ws: WebSocket, req: IncomingMessage): void {
    const address = normalizeAddress(req.socket.remoteAddress)
    let viewerId: string | null = null

    const reject = (reason: RejectReason, retryAfterMs?: number): void => {
      this.sendRaw(ws, { t: 'rejected', reason, retryAfterMs })
      setTimeout(() => ws.close(), 100)
    }

    const helloTimer = setTimeout(() => {
      if (!viewerId) ws.terminate()
    }, HELLO_TIMEOUT_MS)

    ws.on('message', (data) => {
      let msg: ViewerToHost
      try {
        msg = JSON.parse(String(data))
      } catch {
        return
      }
      if (!msg || typeof msg !== 'object') return

      if (!viewerId) {
        if (msg.t !== 'hello') return
        clearTimeout(helloTimer)

        const session = this.session
        if (!session) return reject('sem-transmissao')
        if (msg.v !== PROTOCOL_VERSION) return reject('versao-incompativel')

        const block = this.attempts.get(address)
        if (block && block.blockedUntil > Date.now()) {
          return reject('bloqueado', block.blockedUntil - Date.now())
        }

        if (session.pin && msg.pin !== session.pin) {
          const next = { count: (block?.count ?? 0) + 1, blockedUntil: 0 }
          if (next.count >= PIN_MAX_ATTEMPTS) {
            next.blockedUntil = Date.now() + PIN_BLOCK_MS
            next.count = 0
            this.attempts.set(address, next)
            return reject('bloqueado', PIN_BLOCK_MS)
          }
          this.attempts.set(address, next)
          return reject('pin-invalido')
        }

        if (this.viewers.size >= session.maxViewers) return reject('sala-cheia')

        this.attempts.delete(address)

        viewerId = randomUUID()
        const name = String(msg.name || 'Espectador').slice(0, 40)
        this.viewers.set(viewerId, {
          ws,
          info: { id: viewerId, name, address, joinedAt: Date.now(), pingMs: null },
          lastPong: Date.now(),
          pingSentAt: 0
        })

        this.sendRaw(ws, {
          t: 'welcome',
          viewerId,
          hostName: session.hostName,
          sourceName: session.sourceName,
          sourceKind: session.sourceKind
        })
        this.emitViewers()
        return
      }

      if (msg.t === 'bye') {
        ws.close()
        return
      }

      const slot = this.viewers.get(viewerId)
      const senderName = slot?.info.name ?? 'Espectador'

      if (msg.t === 'chat') {
        const text = String(msg.text || '').trim().slice(0, 400)
        if (!text) return
        const chatMsg: ChatMessage = {
          id: randomUUID(),
          senderId: viewerId,
          senderName,
          role: 'viewer',
          text,
          timestamp: Date.now()
        }
        this.broadcast({ t: 'chat', message: chatMsg })
        this.onChat?.(chatMsg)
        return
      }

      if (msg.t === 'reaction') {
        const emoji = String(msg.emoji || '🔥').slice(0, 8)
        const reaction: ReactionEvent = {
          id: randomUUID(),
          senderName,
          emoji,
          timestamp: Date.now()
        }
        this.broadcast({ t: 'reaction', reaction })
        this.onReaction?.(reaction)
        return
      }

      this.onMessage?.(viewerId, msg)
    })

    ws.on('pong', () => {
      if (!viewerId) return
      const slot = this.viewers.get(viewerId)
      if (!slot) return
      slot.lastPong = Date.now()
      if (slot.pingSentAt) {
        slot.info = { ...slot.info, pingMs: Date.now() - slot.pingSentAt }
        this.emitViewers()
      }
    })

    const cleanup = (): void => {
      clearTimeout(helloTimer)
      if (viewerId && this.viewers.delete(viewerId)) this.emitViewers()
    }
    ws.on('close', cleanup)
    ws.on('error', cleanup)
  }

  private tick(): void {
    const now = Date.now()
    for (const [id, slot] of this.viewers) {
      if (now - slot.lastPong > HEARTBEAT_TIMEOUT_MS) {
        slot.ws.terminate()
        this.viewers.delete(id)
        this.emitViewers()
        continue
      }
      slot.pingSentAt = now
      try {
        slot.ws.ping()
      } catch {
        /* timeout resolve */
      }
    }
  }

  private sendRaw(ws: WebSocket, msg: HostToViewer): void {
    if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg))
  }

  private emitViewers(): void {
    this.onViewers?.([...this.viewers.values()].map((v) => v.info))
  }

  send(viewerId: string, msg: HostToViewer): void {
    const slot = this.viewers.get(viewerId)
    if (slot) this.sendRaw(slot.ws, msg)
  }

  broadcast(msg: HostToViewer): void {
    for (const slot of this.viewers.values()) this.sendRaw(slot.ws, msg)
  }

  kick(viewerId: string): void {
    const slot = this.viewers.get(viewerId)
    if (!slot) return
    this.sendRaw(slot.ws, { t: 'kicked' })
    setTimeout(() => slot.ws.close(), 100)
  }

  async stop(): Promise<void> {
    if (this.heartbeat) {
      clearInterval(this.heartbeat)
      this.heartbeat = null
    }
    this.broadcast({ t: 'ended' })
    await new Promise((r) => setTimeout(r, 120))
    for (const slot of this.viewers.values()) slot.ws.terminate()
    this.viewers.clear()
    this.attempts.clear()
    this.session = null

    const wss = this.wss
    this.wss = null
    if (wss) await new Promise<void>((resolve) => wss.close(() => resolve()))
    this.emitViewers()
  }
}

export class SignalingClient {
  private ws: WebSocket | null = null
  private closedInfo: ViewerClosedInfo | null = null

  onMessage: ((msg: HostToViewer) => void) | null = null
  onClosed: ((info: ViewerClosedInfo) => void) | null = null

  connect(opts: ViewerConnectOptions): Promise<ViewerConnectResult> {
    this.disconnect()

    return new Promise<ViewerConnectResult>((resolve) => {
      let settled = false
      const ws = new WebSocket(`ws://${opts.ip}:${opts.port}`, { handshakeTimeout: 6000 })
      this.ws = ws

      const finish = (result: ViewerConnectResult): void => {
        if (settled) return
        settled = true
        if (!result.ok) {
          this.ws = null
          ws.close()
        }
        resolve(result)
      }

      const timeout = setTimeout(() => {
        finish({ ok: false, reason: 'rede', message: 'O host não respondeu a tempo.' })
      }, 8000)

      ws.on('open', () => {
        const hello: ViewerToHost = {
          t: 'hello',
          v: PROTOCOL_VERSION,
          name: opts.name,
          pin: opts.pin
        }
        ws.send(JSON.stringify(hello))
      })

      ws.on('message', (data) => {
        let msg: HostToViewer
        try {
          msg = JSON.parse(String(data))
        } catch {
          return
        }

        if (!settled) {
          clearTimeout(timeout)
          if (msg.t === 'welcome') {
            finish({
              ok: true,
              hostName: msg.hostName,
              sourceName: msg.sourceName,
              sourceKind: msg.sourceKind,
              viewerId: msg.viewerId
            })
            return
          }
          if (msg.t === 'rejected') {
            finish({
              ok: false,
              reason: msg.reason,
              message: REJECT_MESSAGES[msg.reason],
              retryAfterMs: msg.retryAfterMs
            })
            return
          }
          return
        }

        if (msg.t === 'kicked') {
          this.closedInfo = { reason: 'kicked', message: 'O host removeu você da transmissão.' }
        } else if (msg.t === 'ended') {
          this.closedInfo = { reason: 'ended', message: 'O host encerrou a transmissão.' }
        }
        this.onMessage?.(msg)
      })

      ws.on('error', () => {
        clearTimeout(timeout)
        finish({
          ok: false,
          reason: 'rede',
          message: 'Não foi possível conectar. O host ainda está transmitindo?'
        })
      })

      ws.on('close', () => {
        clearTimeout(timeout)
        if (this.ws !== ws) return
        this.ws = null
        const info = this.closedInfo ?? {
          reason: 'rede' as const,
          message: 'A conexão com o host caiu.'
        }
        this.closedInfo = null
        if (settled) this.onClosed?.(info)
      })
    })
  }

  send(msg: ViewerToHost): void {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(msg))
  }

  disconnect(): void {
    const ws = this.ws
    if (!ws) return
    this.ws = null
    this.closedInfo = null
    try {
      if (ws.readyState === WebSocket.OPEN) {
        const bye: ViewerToHost = { t: 'bye' }
        ws.send(JSON.stringify(bye))
      }
      ws.close()
    } catch {
      /* ja fechado */
    }
  }
}
