import { useEffect, useState } from 'react'
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
  const info = broadcast.info
  const program = settings.scenes.scenes.find((s) => s.id === settings.scenes.activeSceneId)

  useEffect(() => {
    void window.api.firewall.status().then(setFirewall)
  }, [])

  const start = async (): Promise<void> => {
    if (firewall && !firewall.ruleExists && !settings.firewallPromptDismissed) {
      setShowFirewall(true)
      return
    }
    setStarting(true)
    const res = await broadcast.start(engine.lanStream(), settings.quality, {
      sourceId: 'program',
      sourceName: program?.name ?? 'Estúdio',
      sourceKind: 'screen',
      hostName: settings.hostName,
      requirePin: settings.requirePin,
      maxViewers: MAX_VIEWERS
    })
    setStarting(false)
    if (!res.ok) toast(res.error, 'bad')
  }

  // Trocar de cena muda o nome que os espectadores veem.
  useEffect(() => {
    if (info && program) broadcast.notifySourceChanged('program', program.name, 'screen')
  }, [info, program?.name, broadcast, program])

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
              <div className="lan-chat-log">
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
              <form
                className="row"
                style={{ gap: 'var(--sp-2)' }}
                onSubmit={(e) => {
                  e.preventDefault()
                  broadcast.sendChat(chat, settings.hostName)
                  setChat('')
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
            <Button variant="primary" loading={starting} onClick={() => void start()}>
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
          void window.api.firewall.status().then(setFirewall)
          void start()
        }}
        onSkip={() => {
          setShowFirewall(false)
          update({ firewallPromptDismissed: true })
        }}
      />
    </div>
  )
}
