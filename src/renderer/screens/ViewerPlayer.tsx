import { useCallback, useEffect, useRef, useState } from 'react'
import { Badge, Button, IconButton, StatPill } from '@/components/ui'
import {
  IconActivity,
  IconArrowLeft,
  IconChat,
  IconFullscreen,
  IconMic,
  IconMicOff,
  IconPin,
  IconVolume,
  IconVolumeOff
} from '@/components/Icons'
import { formatBitrate } from '@/rtc/stats'
import type { ViewerStream } from '@/hooks/useViewerStream'
import './ViewerPlayer.css'

const IDLE_MS = 2800
const QUICK_EMOJIS = ['🔥', '👏', '😂', '❤️', '🚀', '🎉']

const STATE_LABEL: Record<string, string> = {
  conectando: 'Conectando',
  recebendo: 'Ao vivo',
  instavel: 'Reconectando',
  encerrado: 'Encerrado'
}

export function ViewerPlayer({
  viewer,
  onLeave
}: {
  viewer: ViewerStream
  onLeave: () => void
}): React.JSX.Element {
  const videoRef = useRef<HTMLVideoElement>(null)
  const shellRef = useRef<HTMLDivElement>(null)
  const idleTimer = useRef<number>(0)

  const [controlsVisible, setControlsVisible] = useState(true)
  const [muted, setMuted] = useState(false)
  const [volume, setVolume] = useState(1)
  const [onTop, setOnTop] = useState(false)
  const [showStats, setShowStats] = useState(false)
  const [showChat, setShowChat] = useState(false)
  const [chatText, setChatText] = useState('')
  const [fullscreen, setFullscreen] = useState(false)

  const {
    stream,
    state,
    stats,
    connection,
    chatMessages,
    reactions,
    partyVoiceActive
  } = viewer

  useEffect(() => {
    if (videoRef.current) videoRef.current.srcObject = stream
  }, [stream])

  useEffect(() => {
    const video = videoRef.current
    if (!video) return
    video.muted = muted
    video.volume = volume
  }, [muted, volume])

  const wake = useCallback((): void => {
    setControlsVisible(true)
    window.clearTimeout(idleTimer.current)
    idleTimer.current = window.setTimeout(() => setControlsVisible(false), IDLE_MS)
  }, [])

  useEffect(() => {
    wake()
    return () => window.clearTimeout(idleTimer.current)
  }, [wake])

  const toggleFullscreen = useCallback((): void => {
    if (document.fullscreenElement) void document.exitFullscreen()
    else void shellRef.current?.requestFullscreen()
  }, [])

  useEffect(() => {
    const onChange = (): void => setFullscreen(document.fullscreenElement !== null)
    document.addEventListener('fullscreenchange', onChange)
    return () => document.removeEventListener('fullscreenchange', onChange)
  }, [])

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if ((e.target as HTMLElement)?.tagName === 'INPUT') return
      if (e.key === 'f' || e.key === 'F') toggleFullscreen()
      else if (e.key === 'm' || e.key === 'M') setMuted((m) => !m)
      else if (e.key === 'Escape' && !document.fullscreenElement) onLeave()
      wake()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [toggleFullscreen, onLeave, wake])

  const toggleOnTop = async (): Promise<void> => {
    setOnTop(await window.api.app.setAlwaysOnTop(!onTop))
  }

  const handleChatSubmit = (e: React.FormEvent): void => {
    e.preventDefault()
    if (!chatText.trim()) return
    viewer.sendChat(chatText)
    setChatText('')
  }

  const waiting = !stream || state === 'conectando'

  return (
    <div
      ref={shellRef}
      className={`player ${controlsVisible || showChat ? '' : 'is-idle'}`}
      onMouseMove={wake}
      onDoubleClick={toggleFullscreen}
    >
      <video ref={videoRef} autoPlay playsInline />

      {waiting && (
        <div className="player-waiting">
          <span className="spinner" />
          <span>Negociando a conexão P2P com {connection?.hostName ?? 'o estúdio'}...</span>
        </div>
      )}

      {state === 'instavel' && (
        <div className="player-banner">A conexão oscilou. Tentando recuperar...</div>
      )}

      <div className="player-reactions">
        {reactions.map((r) => (
          <div key={r.id} className="player-reaction-item">
            <span>{r.emoji}</span>
            <small>{r.senderName}</small>
          </div>
        ))}
      </div>

      <div className="player-top">
        <IconButton label="Sair da transmissão" onClick={onLeave}>
          <IconArrowLeft size={19} />
        </IconButton>
        <div className="player-title">
          <span className="player-host">{connection?.hostName ?? '—'}</span>
          <span className="player-source">{connection?.sourceName ?? ''}</span>
        </div>
        <Badge tone={state === 'recebendo' ? 'live' : state === 'instavel' ? 'warning' : 'neutral'}>
          {STATE_LABEL[state] ?? state}
        </Badge>
      </div>

      <div className="player-bottom">
        <div className="player-volume">
          <IconButton
            label={muted ? 'Ativar som' : 'Silenciar'}
            onClick={() => setMuted((m) => !m)}
          >
            {muted ? <IconVolumeOff size={18} /> : <IconVolume size={18} />}
          </IconButton>
          <input
            className="slider volume-slider"
            type="range"
            min={0}
            max={1}
            step={0.01}
            value={muted ? 0 : volume}
            style={{ ['--fill' as string]: `${(muted ? 0 : volume) * 100}%` }}
            onChange={(e) => {
              const v = Number(e.target.value)
              setVolume(v)
              setMuted(v === 0)
            }}
            aria-label="Volume"
          />
        </div>

        <div className="player-quick-emojis">
          {QUICK_EMOJIS.map((em) => (
            <button
              key={em}
              type="button"
              className="player-emoji-btn"
              onClick={() => viewer.sendReaction(em)}
              title={`Reagir com ${em}`}
            >
              {em}
            </button>
          ))}
        </div>

        <div className="spacer" />

        <IconButton
          label={
            partyVoiceActive
              ? 'Desligar Voz na Sala (Party Voice)'
              : 'Falar com o Host (Party Voice)'
          }
          active={partyVoiceActive}
          onClick={() => void viewer.togglePartyVoice()}
        >
          {partyVoiceActive ? <IconMic size={18} /> : <IconMicOff size={18} />}
        </IconButton>

        <IconButton
          label="Chat da Sala"
          active={showChat}
          onClick={() => setShowChat((c) => !c)}
        >
          <IconChat size={18} />
        </IconButton>

        <IconButton
          label="Estatísticas"
          active={showStats}
          onClick={() => setShowStats((s) => !s)}
        >
          <IconActivity size={18} />
        </IconButton>
        <IconButton label="Manter sempre visível" active={onTop} onClick={() => void toggleOnTop()}>
          <IconPin size={18} />
        </IconButton>
        <IconButton
          label={fullscreen ? 'Sair da tela cheia (F)' : 'Tela cheia (F)'}
          onClick={toggleFullscreen}
        >
          <IconFullscreen size={18} />
        </IconButton>
      </div>

      {showChat && (
        <aside className="player-chat-drawer fade-in" onDoubleClick={(e) => e.stopPropagation()}>
          <div className="player-chat-head">
            <span>Chat ao Vivo</span>
            <Badge tone="live">{chatMessages.length}</Badge>
          </div>
          <div className="player-chat-list">
            {chatMessages.length === 0 ? (
              <div className="player-chat-empty">Mande uma mensagem para o host e a sala!</div>
            ) : (
              chatMessages.map((m) => (
                <div key={m.id} className="player-chat-msg">
                  <strong style={{ color: m.role === 'host' ? '#7c6aff' : '#17d4b2' }}>
                    {m.senderName}:
                  </strong>{' '}
                  <span>{m.text}</span>
                </div>
              ))
            )}
          </div>
          <form className="player-chat-form" onSubmit={handleChatSubmit}>
            <input
              type="text"
              className="player-chat-input"
              placeholder="Mensagem..."
              value={chatText}
              onChange={(e) => setChatText(e.target.value)}
              maxLength={280}
            />
            <Button type="submit" size="sm" variant="primary">
              Enviar
            </Button>
          </form>
        </aside>
      )}

      {showStats && (
        <div className="player-stats fade-in">
          <StatPill label="Chegando" value={formatBitrate(stats.bitrateKbps)} />
          <StatPill label="FPS" value={String(stats.fps)} />
          <StatPill
            label="Resolução"
            value={stats.width ? `${stats.width}x${stats.height}` : '—'}
          />
          <StatPill label="Codec" value={stats.codec} />
          <StatPill
            label="Latência"
            value={stats.rttMs === null ? '—' : `${stats.rttMs}ms`}
            tone={stats.rttMs === null ? 'neutral' : stats.rttMs > 120 ? 'warn' : 'good'}
          />
          <StatPill
            label="Perda"
            value={`${stats.lossPct.toFixed(1)}%`}
            tone={stats.lossPct > 3 ? 'bad' : stats.lossPct > 0.8 ? 'warn' : 'good'}
          />
          <StatPill label="Áudio" value={formatBitrate(stats.audioBitrateKbps)} />
          <StatPill label="Descartados" value={String(stats.framesDropped)} />
        </div>
      )}
    </div>
  )
}
