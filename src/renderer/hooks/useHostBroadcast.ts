import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type {
  ChatMessage,
  ConnectedViewer,
  ReactionEvent,
  SourceKind
} from '@shared/protocol'
import type { HostStartOptions, HostStartResult } from '@shared/ipc'
import type { QualitySettings } from '@shared/quality'
import { HostSession, type PeerState } from '@/rtc/HostSession'
import type { StreamStats } from '@/rtc/stats'

export interface HostInfo {
  sessionId: string
  ip: string
  port: number
  pin: string | null
}

export interface HostBroadcast {
  info: HostInfo | null
  viewers: ConnectedViewer[]
  states: Record<string, PeerState>
  stats: Map<string, StreamStats>
  chatMessages: ChatMessage[]
  reactions: ReactionEvent[]
  start(
    stream: MediaStream,
    settings: QualitySettings,
    opts: HostStartOptions
  ): Promise<HostStartResult>
  stop(): Promise<void>
  updateSettings(settings: QualitySettings): void
  kick(viewerId: string): void
  sendChat(text: string, hostName?: string): void
  sendReaction(emoji: string, hostName?: string): void
  replaceVideoTrack(track: MediaStreamTrack): Promise<void>
  replaceAudioTrack(track: MediaStreamTrack | null): Promise<void>
  setPartyVoiceEnabled(enabled: boolean): void
  notifySourceChanged(sourceId: string, sourceName: string, sourceKind: SourceKind): void
}

export function useHostBroadcast(): HostBroadcast {
  const sessionRef = useRef<HostSession | null>(null)
  const reactionTimers = useRef<Set<number>>(new Set())
  const mountedRef = useRef(true)

  const [info, setInfo] = useState<HostInfo | null>(null)
  const [viewers, setViewers] = useState<ConnectedViewer[]>([])
  const [states, setStates] = useState<Record<string, PeerState>>({})
  const [stats, setStats] = useState<Map<string, StreamStats>>(new Map())
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([])
  const [reactions, setReactions] = useState<ReactionEvent[]>([])

  const clearReactionTimers = useCallback((): void => {
    for (const id of reactionTimers.current) window.clearTimeout(id)
    reactionTimers.current.clear()
  }, [])

  const scheduleReactionExpiry = useCallback((reactionId: string): void => {
    const t = window.setTimeout(() => {
      reactionTimers.current.delete(t)
      if (mountedRef.current) {
        setReactions((prev) => prev.filter((r) => r.id !== reactionId))
      }
    }, 3600)
    reactionTimers.current.add(t)
  }, [])

  useEffect(() => {
    mountedRef.current = true
    const offViewers = window.api.host.onViewers((list) => {
      setViewers(list)

      const session = sessionRef.current
      if (!session) return

      const present = new Set(list.map((v) => v.id))

      for (const viewer of list) {
        if (session.getState(viewer.id) === null) {
          void session.addViewer(viewer.id).catch((err) => {
            console.error('[host] nao foi possivel oferecer a transmissao:', err)
          })
        }
      }
      for (const id of session.peerIds()) {
        if (!present.has(id)) session.removeViewer(id)
      }

      setStates((prev) => {
        const next: Record<string, PeerState> = {}
        for (const v of list) next[v.id] = prev[v.id] ?? 'conectando'
        return next
      })
    })

    const offMessage = window.api.host.onMessage((viewerId, msg) => {
      void sessionRef.current?.handleMessage(viewerId, msg)
    })

    const offChat = window.api.host.onChat((chat) => {
      setChatMessages((prev) => [...prev.slice(-99), chat])
    })

    const offReaction = window.api.host.onReaction((reaction) => {
      setReactions((prev) => [...prev.slice(-24), reaction])
      scheduleReactionExpiry(reaction.id)
    })

    return () => {
      mountedRef.current = false
      offViewers()
      offMessage()
      offChat()
      offReaction()
      clearReactionTimers()
      sessionRef.current?.close()
      sessionRef.current = null
    }
  }, [clearReactionTimers, scheduleReactionExpiry])

  useEffect(() => {
    if (!info) return
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
  }, [info])

  const start = useCallback(
    async (
      stream: MediaStream,
      settings: QualitySettings,
      opts: HostStartOptions
    ): Promise<HostStartResult> => {
      sessionRef.current?.close()
      sessionRef.current = null

      const session = new HostSession(stream, settings, (viewerId, msg) =>
        window.api.host.send(viewerId, msg)
      )
      session.onStateChange = (viewerId, state) => {
        if (sessionRef.current === session) {
          setStates((prev) => ({ ...prev, [viewerId]: state }))
        }
      }
      sessionRef.current = session

      try {
        const result = await window.api.host.start(opts)
        if (result.ok) {
          clearReactionTimers()
          setChatMessages([])
          setReactions([])
          setInfo({ sessionId: result.sessionId, ip: result.ip, port: result.port, pin: result.pin })
        } else {
          session.close()
          if (sessionRef.current === session) sessionRef.current = null
        }
        return result
      } catch (err) {
        session.close()
        if (sessionRef.current === session) sessionRef.current = null
        return { ok: false, error: err instanceof Error ? err.message : 'Erro ao iniciar transmissão.' }
      }
    },
    [clearReactionTimers]
  )

  const stop = useCallback(async (): Promise<void> => {
    try {
      await window.api.host.stop()
    } finally {
      clearReactionTimers()
      sessionRef.current?.close()
      sessionRef.current = null
      setInfo(null)
      setViewers([])
      setStates({})
      setStats(new Map())
      setChatMessages([])
      setReactions([])
    }
  }, [clearReactionTimers])

  const updateSettings = useCallback((settings: QualitySettings): void => {
    void sessionRef.current?.updateSettings(settings)
  }, [])

  const kick = useCallback((viewerId: string): void => {
    void window.api.host.kick(viewerId)
  }, [])

  const sendChat = useCallback((text: string, hostName = 'Host'): void => {
    const trimmed = text.trim()
    if (!trimmed) return
    const message: ChatMessage = {
      id: `chat-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
      senderId: 'host',
      senderName: hostName,
      role: 'host',
      text: trimmed,
      timestamp: Date.now()
    }
    setChatMessages((prev) => [...prev.slice(-99), message])
    window.api.host.broadcast({ t: 'chat', message })
  }, [])

  const sendReaction = useCallback(
    (emoji: string, hostName = 'Host'): void => {
      const reaction: ReactionEvent = {
        id: `react-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
        senderName: hostName,
        emoji,
        timestamp: Date.now()
      }
      setReactions((prev) => [...prev.slice(-24), reaction])
      scheduleReactionExpiry(reaction.id)
      window.api.host.broadcast({ t: 'reaction', reaction })
    },
    [scheduleReactionExpiry]
  )

  const replaceVideoTrack = useCallback(async (track: MediaStreamTrack): Promise<void> => {
    await sessionRef.current?.replaceVideoTrack(track)
  }, [])

  const replaceAudioTrack = useCallback(async (track: MediaStreamTrack | null): Promise<void> => {
    await sessionRef.current?.replaceAudioTrack(track)
  }, [])

  const setPartyVoiceEnabled = useCallback((enabled: boolean): void => {
    sessionRef.current?.setPartyVoiceEnabled(enabled)
  }, [])

  const notifySourceChanged = useCallback(
    (_sourceId: string, sourceName: string, sourceKind: SourceKind): void => {
      window.api.host.updateSource(sourceName, sourceKind)
    },
    []
  )

  return useMemo(
    () => ({
      info,
      viewers,
      states,
      stats,
      chatMessages,
      reactions,
      start,
      stop,
      updateSettings,
      kick,
      sendChat,
      sendReaction,
      replaceVideoTrack,
      replaceAudioTrack,
      setPartyVoiceEnabled,
      notifySourceChanged
    }),
    [
      info,
      viewers,
      states,
      stats,
      chatMessages,
      reactions,
      start,
      stop,
      updateSettings,
      kick,
      sendChat,
      sendReaction,
      replaceVideoTrack,
      replaceAudioTrack,
      setPartyVoiceEnabled,
      notifySourceChanged
    ]
  )
}
