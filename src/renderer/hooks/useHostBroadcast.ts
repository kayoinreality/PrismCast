import { useCallback, useEffect, useRef, useState } from 'react'
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
  const [info, setInfo] = useState<HostInfo | null>(null)
  const [viewers, setViewers] = useState<ConnectedViewer[]>([])
  const [states, setStates] = useState<Record<string, PeerState>>({})
  const [stats, setStats] = useState<Map<string, StreamStats>>(new Map())
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([])
  const [reactions, setReactions] = useState<ReactionEvent[]>([])

  useEffect(() => {
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
      setTimeout(() => {
        setReactions((prev) => prev.filter((r) => r.id !== reaction.id))
      }, 3600)
    })

    return () => {
      offViewers()
      offMessage()
      offChat()
      offReaction()
    }
  }, [])

  useEffect(() => {
    if (!info) return
    const timer = setInterval(() => {
      void sessionRef.current?.readStats().then(setStats)
    }, 1000)
    return () => clearInterval(timer)
  }, [info])

  const start = useCallback(
    async (
      stream: MediaStream,
      settings: QualitySettings,
      opts: HostStartOptions
    ): Promise<HostStartResult> => {
      const session = new HostSession(stream, settings, (viewerId, msg) =>
        window.api.host.send(viewerId, msg)
      )
      session.onStateChange = (viewerId, state) =>
        setStates((prev) => ({ ...prev, [viewerId]: state }))
      sessionRef.current = session

      const result = await window.api.host.start(opts)
      if (result.ok) {
        setChatMessages([])
        setReactions([])
        setInfo({ sessionId: result.sessionId, ip: result.ip, port: result.port, pin: result.pin })
      } else {
        session.close()
        sessionRef.current = null
      }
      return result
    },
    []
  )

  const stop = useCallback(async (): Promise<void> => {
    await window.api.host.stop()
    sessionRef.current?.close()
    sessionRef.current = null
    setInfo(null)
    setViewers([])
    setStates({})
    setStats(new Map())
    setChatMessages([])
    setReactions([])
  }, [])

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

  const sendReaction = useCallback((emoji: string, hostName = 'Host'): void => {
    const reaction: ReactionEvent = {
      id: `react-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
      senderName: hostName,
      emoji,
      timestamp: Date.now()
    }
    setReactions((prev) => [...prev.slice(-24), reaction])
    setTimeout(() => {
      setReactions((prev) => prev.filter((r) => r.id !== reaction.id))
    }, 3600)
    window.api.host.broadcast({ t: 'reaction', reaction })
  }, [])

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

  return {
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
  }
}
