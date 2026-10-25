import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from 'react'
import type { AppSettings } from '@shared/ipc'
import type { TwitchCapabilities } from '@shared/protocol'
import { Button, IconButton, TextInput, Toggle } from '@/components/ui'
import { IconChat, IconDown, IconPin, IconPopout, IconSend, IconSettings } from '@/components/Icons'
import { chatFor, type ChatLine, type TwitchChat } from './TwitchChat'
import './ChatPanel.css'

const BADGE_LABEL: Record<string, string> = {
  broadcaster: 'streamer',
  moderator: 'mod',
  vip: 'vip',
  subscriber: 'sub',
  founder: 'fundador',
  partner: 'parceiro'
}

function Line({ line, showTime }: { line: ChatLine; showTime: boolean }): React.JSX.Element {
  if (line.kind === 'system') {
    return <div className="chat-line is-system">{line.fragments.map((f) => (f.t === 'text' ? f.text : f.name)).join('')}</div>
  }
  const time = new Date(line.ts).toLocaleTimeString('pt-BR', { hour: '2-digit', minute: '2-digit' })
  return (
    <div
      className={`chat-line ${line.kind === 'event' ? 'is-event' : ''} ${line.mention ? 'is-mention' : ''} ${line.deleted ? 'is-deleted' : ''}`}
    >
      {showTime && <span className="chat-time mono">{time}</span>}
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
                  src={`https://static-cdn.jtvnw.net/emoticons/v2/${f.id}/default/dark/1.0`}
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

  const lines = chat?.lines ?? []
  const stateLabel = !chat ? 'sem canal' : chat.state === 'connected' ? 'ao vivo' : chat.state === 'connecting' ? 'conectando…' : 'reconectando…'

  return (
    <div className={`chat chat-${variant}`} style={{ ['--chat-size' as string]: `${chatSettings.fontSize}px` }}>
      <div className="chat-head">
        <span className={`chat-dot is-${chat?.state ?? 'off'}`} />
        <span className="chat-channel">{channel || 'Chat da Twitch'}</span>
        <span className="chat-state">{stateLabel}</span>
        <div className="spacer" />
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
          <label className="chat-opt-row">
            <span>Canal</span>
            <TextInput
              value={channelDraft}
              placeholder={twitch?.account?.login ?? 'nome do canal'}
              onChange={(e) => setChannelDraft(e.target.value)}
              onBlur={() => setChat({ channel: channelDraft.trim() })}
              onKeyDown={(e) => e.key === 'Enter' && setChat({ channel: channelDraft.trim() })}
            />
          </label>
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

      {!channel ? (
        <div className="chat-empty">
          <IconChat size={26} />
          <span>Qual canal você quer acompanhar?</span>
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
          <span className="hint">Entrando com a Twitch em Destinos, ele usa o seu canal sozinho.</span>
        </div>
      ) : (
        <div className="chat-list" ref={listRef} onScroll={onScroll}>
          {lines.length === 0 && <div className="chat-line is-system">Esperando mensagens…</div>}
          {lines.map((l) => (
            <Line key={l.id} line={l} showTime={chatSettings.showTimestamps} />
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
                placeholder={`Mensagem como ${caps.login}`}
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
