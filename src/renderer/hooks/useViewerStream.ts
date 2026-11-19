import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { ChatMessage, ReactionEvent, SourceKind } from '@shared/protocol'
import type { ViewerClosedInfo, ViewerConnectResult } from '@shared/ipc'
import { ViewerSession, type ViewerState } from '@/rtc/ViewerSession'
import { emptyStats, type StreamStats } from '@/rtc/stats'

export interface ConnectTarget {
  ip: string
  port: number
  name: string
}

export interface ViewerConnection {
  hostName: string
  sourceName: string
  sourceKind: SourceKind
}

export interface ViewerStream {
  stream: MediaStream | null
  state: ViewerState
  stats: StreamStats
  connection: ViewerConnection | null
  closed: ViewerClosedInfo | null
  chatMessages: ChatMessage[]
  reactions: ReactionEvent[]
  partyVoiceActive: boolean
  connect(target: ConnectTarget, pin: string | null, name: string): Promise<ViewerConnectResult>
  disconnect(): void
  dismissClosed(): void
  sendChat(text: string): void
  sendReaction(emoji: string): void
  togglePartyVoice(): Promise<boolean>
}

export function useViewerStream(): ViewerStream {
  const sessionRef = useRef<ViewerSession | null>(null)
  const connectGenRef = useRef(0)
  const partyVoiceLockRef = useRef(false)
  const reactionTimers = useRef<Set<number>>(new Set())
  const mountedRef = useRef(true)

  const [stream, setStream] = useState<MediaStream | null>(null)
  const [state, setState] = useState<ViewerState>('encerrado')
  const [stats, setStats] = useState<StreamStats>(emptyStats())
  const [connection, setConnection] = useState<ViewerConnection | null>(null)
  const [closed, setClosed] = useState<ViewerClosedInfo | null>(null)
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([])
  const [reactions, setReactions] = useState<ReactionEvent[]>([])
  const [partyVoiceActive, setPartyVoiceActive] = useState(false)

  const clearReactionTimers = useCallback((): void => {
    for (const id of reactionTimers.current) window.clearTimeout(id)
    reactionTimers.current.clear()
  }, [])

  useEffect(() => {
    mountedRef.current = true
    const offMessage = window.api.viewer.onMessage((msg) => {
      void sessionRef.current?.handleMessage(msg)
    })
    const offClosed = window.api.viewer.onClosed((info) => {
      connectGenRef.current += 1
      clearReactionTimers()
      sessionRef.current?.close()
      sessionRef.current = null
      setStream(null)
      setState('encerrado')
      setConnection(null)
      setPartyVoiceActive(false)
      setClosed(info)
    })
    return () => {
      mountedRef.current = false
      offMessage()
      offClosed()
      clearReactionTimers()
      sessionRef.current?.close()
      sessionRef.current = null
    }
  }, [clearReactionTimers])

  useEffect(() => {
    if (!connection) return
    let cancelled = false
    const timer = window.setInterval(() => {
      const session = sessionRef.current
      if (!session) return
      void session.readStats().then((nextStats) => {
        if (!cancelled && sessionRef.current === session) {
          setStats(nextStats)
        }
      })
    }, 1000)
    return () => {
      cancelled = true
      window.clearInterval(timer)
    }
  }, [connection])

  const connect = useCallback(
    async (
      target: ConnectTarget,
      pin: string | null,
      name: string
    ): Promise<ViewerConnectResult> => {
      const gen = ++connectGenRef.current
      clearReactionTimers()
      sessionRef.current?.close()
      sessionRef.current = null
      setClosed(null)
      setStats(emptyStats())
      setChatMessages([])
      setReactions([])
      setPartyVoiceActive(false)

      const session = new ViewerSession((msg) => window.api.viewer.send(msg))
      session.onStream = (s) => {
        if (connectGenRef.current === gen) setStream(s)
      }
      session.onStateChange = (st) => {
        if (connectGenRef.current === gen) setState(st)
      }
      session.onChat = (chat) => {
        if (connectGenRef.current === gen) {
          setChatMessages((prev) => [...prev.slice(-99), chat])
        }
      }
      session.onReaction = (reaction) => {
        if (connectGenRef.current !== gen) return
        setReactions((prev) => [...prev.slice(-24), reaction])
        const t = window.setTimeout(() => {
          reactionTimers.current.delete(t)
          if (mountedRef.current && connectGenRef.current === gen) {
            setReactions((prev) => prev.filter((r) => r.id !== reaction.id))
          }
        }, 3600)
        reactionTimers.current.add(t)
      }
      session.onSourceChanged = (sourceName, sourceKind) => {
        if (connectGenRef.current === gen) {
          setConnection((prev) => (prev ? { ...prev, sourceName, sourceKind } : prev))
        }
      }

      sessionRef.current = session
      setState('conectando')

      try {
        const result = await window.api.viewer.connect({
          ip: target.ip,
          port: target.port,
          pin,
          name
        })

        if (connectGenRef.current !== gen) {
          session.close()
          return { ok: false, reason: 'rede', message: 'Conexão substituída por outra tentativa.' }
        }

        if (result.ok) {
          setConnection({
            hostName: result.hostName,
            sourceName: result.sourceName,
            sourceKind: result.sourceKind
          })
        } else {
          session.close()
          sessionRef.current = null
          setState('encerrado')
        }
        return result
      } catch (err) {
        if (connectGenRef.current === gen) {
          session.close()
          sessionRef.current = null
          setState('encerrado')
        }
        return {
          ok: false,
          reason: 'rede',
          message: err instanceof Error ? err.message : 'Falha ao conectar.'
        }
      }
    },
    [clearReactionTimers]
  )

  const disconnect = useCallback((): void => {
    connectGenRef.current += 1
    clearReactionTimers()
    window.api.viewer.disconnect()
    sessionRef.current?.close()
    sessionRef.current = null
    setStream(null)
    setConnection(null)
    setState('encerrado')
    setStats(emptyStats())
    setChatMessages([])
    setReactions([])
    setPartyVoiceActive(false)
    setClosed(null)
  }, [clearReactionTimers])

  const dismissClosed = useCallback((): void => setClosed(null), [])

  const sendChat = useCallback((text: string): void => {
    const trimmed = text.trim()
    if (!trimmed) return
    window.api.viewer.send({ t: 'chat', text: trimmed })
  }, [])

  const sendReaction = useCallback((emoji: string): void => {
    window.api.viewer.send({ t: 'reaction', emoji })
  }, [])

  const togglePartyVoice = useCallback(async (): Promise<boolean> => {
    const session = sessionRef.current
    if (!session || partyVoiceLockRef.current) return false
    partyVoiceLockRef.current = true
    try {
      const next = !partyVoiceActive
      const ok = await session.setPartyVoiceActive(next)
      if (sessionRef.current === session) {
        setPartyVoiceActive(ok)
      }
      return ok
    } finally {
      partyVoiceLockRef.current = false
    }
  }, [partyVoiceActive])

  return useMemo(
    () => ({
      stream,
      state,
      stats,
      connection,
      closed,
      chatMessages,
      reactions,
      partyVoiceActive,
      connect,
      disconnect,
      dismissClosed,
      sendChat,
      sendReaction,
      togglePartyVoice
    }),
    [
      stream,
      state,
      stats,
      connection,
      closed,
      chatMessages,
      reactions,
      partyVoiceActive,
      connect,
      disconnect,
      dismissClosed,
      sendChat,
      sendReaction,
      togglePartyVoice
    ]
  )
}
