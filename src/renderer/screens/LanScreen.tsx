import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import type { FirewallStatus } from '@shared/ipc'
import { MAX_VIEWERS } from '@shared/protocol'
import { Button, Field, TextInput, Toggle } from '@/components/ui'
import { IconClose, IconEye, IconWifi } from '@/components/Icons'
import { QualityPanel } from '@/components/QualityPanel'
import { NetworkPicker } from '@/components/NetworkPicker'
import { FirewallModal } from '@/components/FirewallModal'
import type { HostBroadcast } from '@/hooks/useHostBroadcast'
import { useStudio } from '@/studio/StudioContext'
import './LanScreen.css'

/**
 * Compartilhar na rede local: o que está no ar no estúdio vai por WebRTC para
 * outros PCs da casa (latência de LAN, sem passar pela internet).
 */
export function LanScreen({ broadcast, onWatch }: { broadcast: HostBroadcast; onWatch: () => void }): React.JSX.Element {
  const { engine, settings, update, toast } = useStudio()
  const [starting, setStarting] = useState(false)
  const [chat, setChat] = useState('')
  const [firewall, setFirewall] = useState<FirewallStatus | null>(null)
  const [showFirewall, setShowFirewall] = useState(false)
  const [stuck, setStuck] = useState(true)
  const [unread, setUnread] = useState(0)
  const chatLogRef = useRef<HTMLDivElement>(null)
  const prevMsgCount = useRef(0)

  const info = broadcast.info
  const program = settings.scenes.scenes.find((s) => s.id === settings.scenes.activeSceneId)

  useEffect(() => {
    let cancelled = false
    void window.api.firewall
      .status()
      .then((st) => {
        if (!cancelled) setFirewall(st)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [])

  const start = async (skipFirewallCheck = false): Promise<void> => {
    if (!skipFirewallCheck && firewall && !firewall.ruleExists && !settings.firewallPromptDismissed) {
      setShowFirewall(true)
      return
    }
    setStarting(true)
    try {
      const res = await broadcast.start(engine.lanStream(), settings.quality, {
        sourceId: 'program',
        sourceName: program?.name ?? 'Estúdio',
        sourceKind: 'screen',
        hostName: settings.hostName,
        requirePin: settings.requirePin,
        maxViewers: MAX_VIEWERS
      })
      if (!res.ok) toast(res.error, 'bad')
    } catch (err) {
      toast(err instanceof Error ? err.message : 'Não foi possível compartilhar na rede.', 'bad')
    } finally {
      setStarting(false)
    }
  }

  // Trocar de cena muda o nome que os espectadores veem.
  const notifySourceChanged = broadcast.notifySourceChanged
  useEffect(() => {
    if (info && program) notifySourceChanged('program', program.name, 'screen')
  }, [info?.sessionId, program?.name, notifySourceChanged])

  // Auto-scroll + contador de mensagens não lidas no chat LAN.
  useLayoutEffect(() => {
    const count = broadcast.chatMessages.length
    if (count === 0) {
      prevMsgCount.current = 0
      setUnread(0)
      return
    }
    const delta = Math.max(0, count - prevMsgCount.current)
    prevMsgCount.current = count
    const el = chatLogRef.current
    if (stuck && el) {
      el.scrollTop = el.scrollHeight
      setUnread(0)
    } else if (delta > 0) {
      setUnread((u) => u + delta)
    }
  }, [broadcast.chatMessages, stuck])

  const onChatScroll = (): void => {
    const el = chatLogRef.current
    if (!el) return
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 32
    setStuck(atBottom)
    if (atBottom) setUnread(0)
  }

  return (
    <div className="page">
      <div className="page-inner">
        <header className="page-header lan-header">
          <div>
            <h1>Rede local</h1>
            <p>
              Mostre o que está no ar em outros computadores da sua casa, com atraso de rede local. Útil para um
              segundo PC, a TV ou alguém assistindo junto.
            </p>
          </div>
          <Button variant="ghost" icon={<IconEye size={15} />} onClick={onWatch}>
            Assistir outro PC
          </Button>
        </header>

        {info ? (
          <section className="panel lan-live">
            <div className="lan-live-head">
              <span className="lan-dot" />
              <span className="lan-live-title">Compartilhando “{program?.name}” na rede</span>
              <div className="spacer" />
              <Button variant="danger" size="sm" onClick={() => void broadcast.stop()}>
                Parar
              </Button>
            </div>
            <div className="lan-codes">
              <div>
                <span className="lan-code-label">Endereço</span>
                <span className="lan-code mono selectable">
                  {info.ip}:{info.port}
                </span>
              </div>
              {info.pin && (
                <div>
                  <span className="lan-code-label">Código</span>
                  <span className="lan-code mono selectable">{info.pin}</span>
                </div>
              )}
              <div>
                <span className="lan-code-label">Assistindo</span>
                <span className="lan-code mono">
                  {broadcast.viewers.length}/{MAX_VIEWERS}
                </span>
              </div>
            </div>
            <span className="hint">No outro PC, abra o PrismCast → Rede local → Assistir outro PC.</span>

            {broadcast.viewers.length > 0 && (
              <div className="lan-viewers">
                {broadcast.viewers.map((v) => (
                  <div key={v.id} className="lan-viewer">
                    <span>{v.name}</span>
                    <span className="dim mono">
                      {v.address} · {broadcast.states[v.id] ?? 'conectando'}
                      {v.pingMs !== null ? ` · ${v.pingMs}ms` : ''}
                    </span>
                    <button className="lan-kick" title="Remover" onClick={() => broadcast.kick(v.id)}>
                      <IconClose size={14} />
                    </button>
                  </div>
                ))}
              </div>
            )}

            <div className="lan-chat">
              <div className="lan-chat-log" ref={chatLogRef} onScroll={onChatScroll}>
                {broadcast.chatMessages.length === 0 ? (
                  <span className="hint">Chat da rede local: aparece aqui o que os espectadores mandarem.</span>
                ) : (
                  broadcast.chatMessages.map((m) => (
                    <div key={m.id} className="lan-msg">
                      <b className={m.role === 'host' ? 'is-host' : ''}>{m.senderName}</b> {m.text}
                    </div>
                  ))
                )}
              </div>
              {unread > 0 && (
                <button
                  type="button"
                  className="link-btn"
                  style={{ alignSelf: 'flex-start', fontSize: 12 }}
                  onClick={() => {
                    setStuck(true)
                    setUnread(0)
                    chatLogRef.current?.scrollTo({ top: chatLogRef.current.scrollHeight })
                  }}
                >
                  ↓ {unread} {unread === 1 ? 'mensagem nova' : 'mensagens novas'}
                </button>
              )}
              <form
                className="row"
                style={{ gap: 'var(--sp-2)' }}
                onSubmit={(e) => {
                  e.preventDefault()
                  broadcast.sendChat(chat, settings.hostName)
                  setChat('')
                  setStuck(true)
                }}
              >
                <TextInput value={chat} onChange={(e) => setChat(e.target.value)} placeholder="Mensagem" />
                <Button type="submit" disabled={!chat.trim()}>
                  Enviar
                </Button>
              </form>
            </div>
          </section>
        ) : (
          <section className="panel lan-start">
            <IconWifi size={22} />
            <div className="lan-start-text">
              <span className="lan-live-title">Compartilhar o estúdio na rede</span>
              <span className="hint">Vai o que estiver no ar: “{program?.name}”.</span>
            </div>
            <Button variant="primary" loading={starting} onClick={() => void start(false)}>
              Começar
            </Button>
          </section>
        )}

        <section className="section">
          <div className="section-title">Opções</div>
          <div className="panel">
            <div className="form-grid">
              <Field label="Nome deste PC na rede">
                <TextInput value={settings.hostName} onChange={(e) => update({ hostName: e.target.value })} />
              </Field>
              <Field label="Placa de rede">
                <NetworkPicker value={settings.preferredInterface} onChange={(v) => update({ preferredInterface: v })} />
              </Field>
            </div>
            <Toggle
              label="Pedir código para entrar"
              hint="Recomendado em redes que não são só da sua casa."
              checked={settings.requirePin}
              disabled={info !== null}
              onChange={(v) => update({ requirePin: v })}
            />
            <QualityPanel
              settings={settings.quality}
              audioLocked={info !== null}
              onChange={(quality) => {
                update({ quality })
                broadcast.updateSettings(quality)
              }}
            />
          </div>
        </section>
      </div>

      <FirewallModal
        open={showFirewall}
        status={firewall}
        onDone={() => {
          setShowFirewall(false)
          void window.api.firewall
            .status()
            .then(setFirewall)
            .catch(() => {})
          void start(true)
        }}
        onSkip={() => {
          setShowFirewall(false)
          update({ firewallPromptDismissed: true })
          void start(true)
        }}
      />
    </div>
  )
}
