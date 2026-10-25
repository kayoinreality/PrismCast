import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import type { AppSettings } from '@shared/ipc'
import type { ChatPlatform, MultiChatStatus, TwitchCapabilities } from '@shared/protocol'
import { Button, IconButton, TextInput, Toggle } from '@/components/ui'
import { IconChat, IconDown, IconPin, IconPopout, IconSend, IconSettings } from '@/components/Icons'
import { chatFor, type ChatLine, type TwitchChat } from './TwitchChat'
import { multiChat } from './MultiChat'
import './ChatPanel.css'

const BADGE_LABEL: Record<string, string> = {
  broadcaster: 'streamer',
  moderator: 'mod',
  vip: 'vip',
  subscriber: 'sub',
  founder: 'fundador',
  partner: 'parceiro',
  member: 'membro',
  verified: 'verificado'
}

const PLATFORM_LABEL: Record<ChatPlatform, string> = { twitch: 'Twitch', youtube: 'YouTube', kick: 'Kick' }

function Line({ line, showTime, showPlatform }: { line: ChatLine; showTime: boolean; showPlatform: boolean }): React.JSX.Element {
  if (line.kind === 'system') {
    return <div className="chat-line is-system">{line.fragments.map((f) => (f.t === 'text' ? f.text : f.name)).join('')}</div>
  }
  const time = new Date(line.ts).toLocaleTimeString('pt-BR', { hour: '2-digit', minute: '2-digit' })
  return (
    <div
      className={`chat-line ${line.kind === 'event' ? 'is-event' : ''} ${line.mention ? 'is-mention' : ''} ${line.deleted ? 'is-deleted' : ''}`}
    >
      {showTime && <span className="chat-time mono">{time}</span>}
      {showPlatform && (
        <span className={`chat-plat plat-${line.platform}`} title={PLATFORM_LABEL[line.platform]}>
          {PLATFORM_LABEL[line.platform].slice(0, 1)}
        </span>
      )}
      {line.badges.map((b) => (
        <span key={b} className={`chat-badge b-${b}`}>
          {BADGE_LABEL[b] ?? b}
        </span>
      ))}
      {line.name && (
        <span className="chat-name" style={{ color: line.color }}>
          {line.name}
          {line.action ? ' ' : ': '}
        </span>
      )}
      <span className={`chat-text ${line.action ? 'is-action' : ''}`}>
        {line.deleted
          ? 'mensagem apagada'
          : line.fragments.map((f, i) =>
              f.t === 'text' ? (
                <span key={i}>{f.text}</span>
              ) : (
                <img
                  key={i}
                  className="chat-emote"
                  alt={f.name}
                  title={f.name}
                  src={f.url}
                />
              )
            )}
      </span>
    </div>
  )
}

const noSubscribe = (): (() => void) => () => {}
const noVersion = (): number => 0

function useChat(chat: TwitchChat | null): TwitchChat | null {
  useSyncExternalStore(chat?.subscribe ?? noSubscribe, chat?.getVersion ?? noVersion)
  return chat
}

function useMultiChat(): ReturnType<typeof multiChat> {
  const store = multiChat()
  useSyncExternalStore(store.subscribe, store.getVersion)
  return store
}

const STATE_LABEL: Record<MultiChatStatus['state'], string> = {
  off: 'desligado',
  connecting: 'conectando…',
  connected: 'ao vivo',
  waiting: 'esperando a live',
  error: 'reconectando…'
}

/** Campo que só grava ao sair dele (ou Enter), para não reconectar a cada letra. */
function OptionInput({
  label,
  value,
  placeholder,
  onCommit
}: {
  label: string
  value: string
  placeholder: string
  onCommit: (v: string) => void
}): React.JSX.Element {
  const [draft, setDraft] = useState(value)
  useEffect(() => setDraft(value), [value])
  return (
    <label className="chat-opt-row">
      <span>{label}</span>
      <TextInput
        value={draft}
        placeholder={placeholder}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => draft.trim() !== value && onCommit(draft.trim())}
        onKeyDown={(e) => e.key === 'Enter' && onCommit(draft.trim())}
      />
    </label>
  )
}

/**
 * Chat da Twitch em tempo real. Mesmo componente no painel do estúdio e na
 * janela separada (para o segundo monitor).
 */
export function ChatPanel({
  settings,
  update,
  variant,
  onReloginTwitch
}: {
  settings: AppSettings
  update: (patch: Partial<AppSettings>) => void
  variant: 'dock' | 'window'
  /** leva para Destinos para entrar de novo com a Twitch */
  onReloginTwitch?: () => void
}): React.JSX.Element {
  const twitch = settings.destinations.find((d) => d.platform === 'twitch' && d.account)
  const channel = (settings.chat.channel || twitch?.account?.login || '').trim().toLowerCase()
  const chat = useChat(channel ? chatFor(channel) : null)
  const multi = useMultiChat()
  multi.mentionOf = channel
  const [caps, setCaps] = useState<TwitchCapabilities | null>(null)
  const [draft, setDraft] = useState('')
  const [sending, setSending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [options, setOptions] = useState(false)
  const [channelDraft, setChannelDraft] = useState(settings.chat.channel)
  const [stuck, setStuck] = useState(true)
  const listRef = useRef<HTMLDivElement>(null)
  const chatSettings = settings.chat
  const setChat = (p: Partial<AppSettings['chat']>): void => update({ chat: { ...chatSettings, ...p } })

  useEffect(() => {
    void window.api.twitch.capabilities().then(setCaps)
  }, [twitch?.account?.login, twitch?.account?.scopes?.length])

  // Fica colado no fim enquanto a pessoa não rola para cima para ler.
  useLayoutEffect(() => {
    const el = listRef.current
    if (el && stuck) el.scrollTop = el.scrollHeight
  })

  const onScroll = (): void => {
    const el = listRef.current
    if (!el) return
    setStuck(el.scrollHeight - el.scrollTop - el.clientHeight < 40)
  }

  const send = async (): Promise<void> => {
    const text = draft.trim()
    if (!text) return
    setSending(true)
    const res = await window.api.twitch.sendChat(text)
    setSending(false)
    if (res.ok) {
      setDraft('')
      setError(null)
    } else {
      setError(res.error)
      if (res.needsRelogin) void window.api.twitch.capabilities().then(setCaps)
    }
  }

  const twitchLines = chat?.lines
  const otherLines = multi.lines
  // Twitch + YouTube + Kick numa lista só, pela hora em que cada mensagem foi enviada.
  const lines = useMemo(() => {
    const all = [...(twitchLines ?? []), ...otherLines]
    return otherLines.length === 0 ? all : all.sort((a, b) => a.ts - b.ts)
  }, [twitchLines, otherLines])

  const sources: Array<{ platform: ChatPlatform; label: string; state: MultiChatStatus['state']; message: string | null }> = []
  if (chat) sources.push({ platform: 'twitch', label: channel, state: chat.state, message: null })
  for (const st of multi.status) {
    if (st.state !== 'off') sources.push({ platform: st.platform, label: st.channel, state: st.state, message: st.message })
  }
  const hasAny = sources.length > 0
  const multiOn = sources.some((src) => src.platform !== 'twitch')
  const problems = multi.status.filter((st) => st.message && st.state !== 'connected')

  return (
    <div className={`chat chat-${variant}`} style={{ ['--chat-size' as string]: `${chatSettings.fontSize}px` }}>
      <div className="chat-head">
        <div className="chat-sources">
          {!hasAny && <span className="chat-channel">Chat</span>}
          {sources.map((src) => (
            <span
              key={src.platform}
              className={`chat-src is-${src.state}`}
              title={`${PLATFORM_LABEL[src.platform]} · ${src.label} · ${src.message ?? STATE_LABEL[src.state]}`}
            >
              <span className={`chat-src-dot plat-${src.platform}`} />
              <span className="chat-channel">{sources.length > 1 ? PLATFORM_LABEL[src.platform] : src.label}</span>
              {(sources.length === 1 || src.state !== 'connected') && <span className="chat-state">{STATE_LABEL[src.state]}</span>}
            </span>
          ))}
        </div>
        <IconButton label="Opções do chat" active={options} onClick={() => setOptions((o) => !o)}>
          <IconSettings size={15} />
        </IconButton>
        {variant === 'dock' ? (
          <IconButton label="Abrir em janela separada (outro monitor)" onClick={() => window.api.chat.openPopout()}>
            <IconPopout size={15} />
          </IconButton>
        ) : (
          <IconButton
            label={chatSettings.popoutOnTop ? 'Não ficar por cima' : 'Ficar por cima das outras janelas'}
            active={chatSettings.popoutOnTop}
            onClick={() => setChat({ popoutOnTop: !chatSettings.popoutOnTop })}
          >
            <IconPin size={15} />
          </IconButton>
        )}
      </div>

      {options && (
        <div className="chat-options">
          <OptionInput
            label="Twitch"
            value={chatSettings.channel}
            placeholder={twitch?.account?.login ?? 'nome do canal'}
            onCommit={(v) => setChat({ channel: v })}
          />
          <OptionInput
            label="YouTube"
            value={chatSettings.youtube}
            placeholder="@seucanal ou link da live"
            onCommit={(v) => setChat({ youtube: v })}
          />
          <OptionInput label="Kick" value={chatSettings.kick} placeholder="nome do canal na Kick" onCommit={(v) => setChat({ kick: v })} />
          {problems.length > 0 && (
            <div className="chat-opt-status">
              {problems.map((st) => (
                <span key={st.platform}>
                  <b>{PLATFORM_LABEL[st.platform]}:</b> {st.message}
                </span>
              ))}
              <button className="link-btn" onClick={() => window.api.chat.reconnectMultichat()}>
                Tentar agora
              </button>
            </div>
          )}
          <label className="chat-opt-row">
            <span>Tamanho da letra</span>
            <input
              type="range"
              className="slider"
              min={11}
              max={22}
              value={chatSettings.fontSize}
              onChange={(e) => setChat({ fontSize: Number(e.target.value) })}
            />
          </label>
          <Toggle label="Mostrar horário" checked={chatSettings.showTimestamps} onChange={(v) => setChat({ showTimestamps: v })} />
        </div>
      )}

      {!hasAny ? (
        <div className="chat-empty">
          <IconChat size={26} />
          <span>Qual canal da Twitch você quer acompanhar?</span>
          <div className="row" style={{ gap: 'var(--sp-2)' }}>
            <TextInput
              value={channelDraft}
              placeholder="nome do canal"
              onChange={(e) => setChannelDraft(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && setChat({ channel: channelDraft.trim() })}
            />
            <Button size="sm" variant="primary" onClick={() => setChat({ channel: channelDraft.trim() })} disabled={!channelDraft.trim()}>
              Ler
            </Button>
          </div>
          <span className="hint">
            Entrando com a Twitch em Destinos, ele usa o seu canal sozinho. YouTube e Kick entram nas{' '}
            <button className="link-btn" onClick={() => setOptions(true)}>
              opções do chat
            </button>
            .
          </span>
        </div>
      ) : (
        <div className="chat-list" ref={listRef} onScroll={onScroll}>
          {lines.length === 0 && <div className="chat-line is-system">Esperando mensagens…</div>}
          {lines.map((l) => (
            <Line key={`${l.platform}-${l.id}`} line={l} showTime={chatSettings.showTimestamps} showPlatform={multiOn} />
          ))}
        </div>
      )}
      {!stuck && (
        <button
          className="chat-jump"
          onClick={() => {
            setStuck(true)
            listRef.current?.scrollTo({ top: listRef.current.scrollHeight })
          }}
        >
          <IconDown size={13} /> Mensagens novas
        </button>
      )}

      {channel && (
        <div className="chat-compose">
          {caps?.canChat ? (
            <>
              <input
                className="chat-input"
                value={draft}
                maxLength={500}
                placeholder={multiOn ? `Mensagem na Twitch como ${caps.login}` : `Mensagem como ${caps.login}`}
                onChange={(e) => {
                  setDraft(e.target.value)
                  setError(null)
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !e.shiftKey) {
                    e.preventDefault()
                    void send()
                  }
                }}
              />
              <IconButton label="Enviar" onClick={() => void send()} disabled={sending || !draft.trim()}>
                <IconSend size={15} />
              </IconButton>
            </>
          ) : (
            <span className="chat-readonly">
              {caps?.linked ? (
                <>
                  Só leitura.{' '}
                  {onReloginTwitch ? (
                    <button className="link-btn" onClick={onReloginTwitch}>
                      Entre de novo com a Twitch
                    </button>
                  ) : (
                    'Entre de novo com a Twitch em Destinos'
                  )}{' '}
                  para escrever daqui.
                </>
              ) : (
                <>Só leitura. Entre com a Twitch em Destinos para escrever daqui.</>
              )}
            </span>
          )}
        </div>
      )}
      {error && <div className="chat-error">{error}</div>}
    </div>
  )
}
