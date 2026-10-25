import { useCallback, useEffect, useRef, useState } from 'react'
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
  const [stream, setStream] = useState<MediaStream | null>(null)
  const [state, setState] = useState<ViewerState>('encerrado')
  const [stats, setStats] = useState<StreamStats>(emptyStats())
  const [connection, setConnection] = useState<ViewerConnection | null>(null)
  const [closed, setClosed] = useState<ViewerClosedInfo | null>(null)
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([])
  const [reactions, setReactions] = useState<ReactionEvent[]>([])
  const [partyVoiceActive, setPartyVoiceActive] = useState(false)

  useEffect(() => {
    const offMessage = window.api.viewer.onMessage((msg) => {
      void sessionRef.current?.handleMessage(msg)
    })
    const offClosed = window.api.viewer.onClosed((info) => {
      sessionRef.current?.close()
      sessionRef.current = null
      setStream(null)
      setState('encerrado')
      setConnection(null)
      setPartyVoiceActive(false)
      setClosed(info)
    })
    return () => {
      offMessage()
      offClosed()
    }
  }, [])

  useEffect(() => {
    if (!connection) return
    const timer = setInterval(() => {
      void sessionRef.current?.readStats().then(setStats)
    }, 1000)
    return () => clearInterval(timer)
  }, [connection])

  const connect = useCallback(
    async (
      target: ConnectTarget,
      pin: string | null,
      name: string
    ): Promise<ViewerConnectResult> => {
      sessionRef.current?.close()
      setClosed(null)
      setStats(emptyStats())
      setChatMessages([])
      setReactions([])
      setPartyVoiceActive(false)

      const session = new ViewerSession((msg) => window.api.viewer.send(msg))
      session.onStream = setStream
      session.onStateChange = setState
      session.onChat = (chat) => {
        setChatMessages((prev) => [...prev.slice(-99), chat])
      }
      session.onReaction = (reaction) => {
        setReactions((prev) => [...prev.slice(-24), reaction])
        setTimeout(() => {
          setReactions((prev) => prev.filter((r) => r.id !== reaction.id))
        }, 3600)
      }
      session.onSourceChanged = (sourceName, sourceKind) => {
        setConnection((prev) => (prev ? { ...prev, sourceName, sourceKind } : prev))
      }

      sessionRef.current = session
      setState('conectando')

      const result = await window.api.viewer.connect({
        ip: target.ip,
        port: target.port,
        pin,
        name
      })

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
    },
    []
  )

  const disconnect = useCallback((): void => {
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
  }, [])

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
    if (!session) return false
    const next = !partyVoiceActive
    const ok = await session.setPartyVoiceActive(next)
    setPartyVoiceActive(ok)
    return ok
  }, [partyVoiceActive])

  return {
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
  }
}
