import { useEffect, useState } from 'react'
import { PLATFORM_INFO } from '@shared/ipc'
import type { Destination, PlatformId, TwitchLoginState } from '@shared/protocol'
import { Button, Field, TextInput, Toggle } from '@/components/ui'
import { IconCheck, IconExternal, IconKey, IconPlus, IconTrash } from '@/components/Icons'
import { useEngineState, useStudio } from '@/studio/StudioContext'
import { newId } from '@/studio/scenes'
import './DestinationsScreen.css'

const PANEL_OF: Record<PlatformId, string> = {
  twitch: 'da Twitch',
  youtube: 'do YouTube',
  kick: 'do Kick',
  tiktok: 'do TikTok',
  custom: 'do servidor'
}

function KeyForm({ dest }: { dest: Destination }): React.JSX.Element {
  const { toast } = useStudio()
  const [key, setKey] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const info = PLATFORM_INFO[dest.platform]

  const save = async (): Promise<void> => {
    setSaving(true)
    const res = await window.api.destinations.setKey(dest.id, key)
    setSaving(false)
    if (res.ok) {
      setKey('')
      setError(null)
      toast(`Chave de ${dest.name} salva e destino ligado`, 'good')
    } else {
      setError(res.error)
    }
  }

  return (
    <div className="dest-keyform">
      <Field label={dest.hasKey ? 'Trocar a chave de transmissão' : 'Chave de transmissão'} hint={info.keyHint}>
        <div className="row" style={{ gap: 'var(--sp-2)' }}>
          <TextInput
            type="password"
            value={key}
            onChange={(e) => {
              setKey(e.target.value)
              setError(null)
            }}
            onKeyDown={(e) => e.key === 'Enter' && key.trim() && void save()}
            placeholder={dest.hasKey ? '•••••••••••• (salva)' : 'Cole aqui'}
            autoComplete="off"
            spellCheck={false}
          />
          <Button variant="primary" onClick={() => void save()} disabled={!key.trim()} loading={saving}>
            Salvar
          </Button>
        </div>
      </Field>
      {error && <span className="error-text">{error}</span>}
      {info.keyUrl && (
        <button className="link-btn dest-link" onClick={() => window.api.app.openExternal(info.keyUrl)}>
          Abrir o painel {PANEL_OF[dest.platform]} para copiar a chave <IconExternal size={12} />
        </button>
      )}
    </div>
  )
}

function TwitchLogin({ dest, state }: { dest: Destination; state: TwitchLoginState }): React.JSX.Element {
  const { settings, update } = useStudio()
  const [clientId, setClientId] = useState(settings.twitchClientId)
  const [setup, setSetup] = useState(false)
  const configured = settings.twitchClientId.trim().length > 0

  if (state.step === 'code') {
    return (
      <div className="twitch-code">
        <span className="hint">Confirme no navegador que abrimos (twitch.tv/activate) com o código:</span>
        <span className="twitch-code-value mono selectable">{state.userCode}</span>
        <div className="row" style={{ gap: 'var(--sp-2)' }}>
          <Button size="sm" onClick={() => window.api.app.openExternal(state.verificationUri)} icon={<IconExternal size={13} />}>
            Abrir de novo
          </Button>
          <Button size="sm" variant="ghost" onClick={() => window.api.destinations.twitchCancel()}>
            Cancelar
          </Button>
        </div>
      </div>
    )
  }

  return (
    <div className="twitch-login">
      <div className="row" style={{ gap: 'var(--sp-2)', flexWrap: 'wrap' }}>
        <Button
          className="btn-twitch"
          disabled={!configured}
          onClick={() => void window.api.destinations.twitchLogin(dest.id)}
          title={configured ? '' : 'Configure o Client ID primeiro'}
        >
          {dest.account ? 'Entrar de novo com a Twitch' : 'Entrar com a Twitch'}
        </Button>
        <button className="link-btn" onClick={() => setSetup((s) => !s)}>
          {configured ? 'Client ID configurado' : 'Configurar login (1 minuto)'}
        </button>
      </div>
      {state.step === 'error' && <span className="error-text">{state.message}</span>}
      {setup && (
        <div className="twitch-setup">
          <ol>
            <li>
              Abra{' '}
              <button className="link-btn" onClick={() => window.api.app.openExternal('https://dev.twitch.tv/console/apps/create')}>
                dev.twitch.tv/console/apps
              </button>{' '}
              e registre um app.
            </li>
            <li>
              Nome: qualquer um (ex. “PrismCast do Kayo”). URL de redirecionamento: <code>https://localhost</code>{' '}
              (clique em Add; o login não usa esse endereço, mas a Twitch exige um).
            </li>
            <li>
              Categoria: <b>Broadcaster Suite</b>. Tipo de cliente: <b>Público</b>.
            </li>
            <li>Copie o Client ID e cole abaixo.</li>
          </ol>
          <div className="row" style={{ gap: 'var(--sp-2)' }}>
            <TextInput value={clientId} onChange={(e) => setClientId(e.target.value)} placeholder="Client ID" spellCheck={false} />
            <Button
              onClick={() => {
                update({ twitchClientId: clientId.trim() })
                setSetup(false)
              }}
              disabled={clientId.trim() === settings.twitchClientId}
            >
              Salvar
            </Button>
          </div>
          <span className="hint">
            O login usa só a permissão de ler a sua chave de transmissão. Ela fica criptografada neste PC.
          </span>
        </div>
      )}
    </div>
  )
}

function DestinationCard({ dest, twitch }: { dest: Destination; twitch: TwitchLoginState }): React.JSX.Element {
  const { settings, update } = useStudio()
  const state = useEngineState()
  const [advanced, setAdvanced] = useState(PLATFORM_INFO[dest.platform].serverFromDashboard && !dest.server)
  const [confirmRemove, setConfirmRemove] = useState(false)
  const info = PLATFORM_INFO[dest.platform]
  const status = state.multistream.destinations.find((s) => s.id === dest.id)
  const live = state.live && (status?.state === 'live' || status?.state === 'connecting')

  const patch = (p: Partial<Destination>): void =>
    update({ destinations: settings.destinations.map((d) => (d.id === dest.id ? { ...d, ...p } : d)) })

  const ready = dest.hasKey && dest.server.trim().length > 0

  return (
    <div className={`dest-card ${dest.enabled && ready ? 'is-on' : ''}`}>
      <div className="dest-head">
        <span className={`dest-logo plat-${dest.platform}`}>{info.label.slice(0, 1)}</span>
        <div className="dest-title">
          <div className="dest-name">
            {dest.name}
            {live && <span className="tag tag-onair">ao vivo</span>}
          </div>
          <div className="dest-status">
            {dest.account ? (
              <>
                {dest.account.avatarUrl && <img src={dest.account.avatarUrl} alt="" className="dest-avatar" />}
                Conectado como <b>{dest.account.displayName}</b>
              </>
            ) : dest.hasKey ? (
              <>
                <IconCheck size={13} /> Chave salva
              </>
            ) : (
              <span className="dim">Não conectado</span>
            )}
            <span className="dim"> · {dest.orientation === 'vertical' ? '9:16' : '16:9'} · {dest.bitrateKbps} kbps</span>
          </div>
        </div>
        <Toggle
          label=""
          checked={dest.enabled}
          disabled={!ready}
          onChange={(v) => patch({ enabled: v })}
        />
      </div>

      {status?.error && <div className="dest-error">{status.error}</div>}

      {dest.platform === 'twitch' && <TwitchLogin dest={dest} state={twitch} />}
      <KeyForm dest={dest} />

      <button className="link-btn dest-advanced-toggle" onClick={() => setAdvanced((a) => !a)}>
        {advanced ? 'Esconder opções' : 'Servidor e qualidade'}
      </button>
      {advanced && (
        <div className="form-grid">
          <Field label="Servidor (RTMP)" hint={info.serverFromDashboard ? 'copie do painel' : ''}>
            <TextInput
              value={dest.server}
              onChange={(e) => patch({ server: e.target.value.trim() })}
              placeholder="rtmp://…"
              spellCheck={false}
              className="mono-input"
            />
          </Field>
          <Field label="Bitrate do vídeo" hint={`${dest.bitrateKbps} kbps`}>
            <select className="select" value={dest.bitrateKbps} onChange={(e) => patch({ bitrateKbps: Number(e.target.value) })}>
              {[2500, 3500, 4500, 6000, 8000, 9000, 12000].map((b) => (
                <option key={b} value={b}>
                  {b} kbps{b === info.bitrateKbps ? ' (recomendado)' : ''}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Formato">
            <select
              className="select"
              value={dest.orientation}
              onChange={(e) => patch({ orientation: e.target.value as Destination['orientation'] })}
            >
              <option value="horizontal">16:9 horizontal</option>
              <option value="vertical">9:16 vertical</option>
            </select>
          </Field>
          <Field label="Nome">
            <TextInput value={dest.name} onChange={(e) => patch({ name: e.target.value })} />
          </Field>
        </div>
      )}

      <div className="dest-foot">
        {dest.hasKey && (
          <Button size="sm" variant="ghost" icon={<IconKey size={14} />} onClick={() => void window.api.destinations.unlink(dest.id)}>
            {dest.account ? 'Desconectar conta' : 'Apagar chave'}
          </Button>
        )}
        <div className="spacer" />
        {confirmRemove ? (
          <Button
            size="sm"
            variant="danger"
            onClick={() => {
              void window.api.destinations.unlink(dest.id)
              update({ destinations: settings.destinations.filter((d) => d.id !== dest.id) })
            }}
          >
            Remover {dest.name}?
          </Button>
        ) : (
          <Button size="sm" variant="ghost" icon={<IconTrash size={14} />} onClick={() => setConfirmRemove(true)}>
            Remover destino
          </Button>
        )}
      </div>
    </div>
  )
}

export function DestinationsScreen(): React.JSX.Element {
  const { settings, update, toast } = useStudio()
  const [twitch, setTwitch] = useState<TwitchLoginState>({ step: 'idle' })
  const [adding, setAdding] = useState(false)

  useEffect(
    () =>
      window.api.destinations.onTwitchLogin((s) => {
        setTwitch(s)
        if (s.step === 'done') toast(`Twitch conectada como ${s.account.displayName}`, 'good')
      }),
    [toast]
  )

  const add = (platform: PlatformId): void => {
    const info = PLATFORM_INFO[platform]
    const dest: Destination = {
      id: newId(platform),
      platform,
      name: info.label,
      enabled: false,
      server: info.defaultServer,
      hasKey: false,
      orientation: info.orientation,
      bitrateKbps: info.bitrateKbps,
      account: null
    }
    update({ destinations: [...settings.destinations, dest] })
    setAdding(false)
  }

  const enabled = settings.destinations.filter((d) => d.enabled).length

  return (
    <div className="page">
      <div className="page-inner">
        <header className="page-header">
          <h1>Destinos</h1>
          <p>
            Para onde a live vai quando você clica em “Entrar ao vivo”. Ligue quantos quiser: o PrismCast manda para
            todos ao mesmo tempo. {enabled > 0 ? `${enabled} ligado${enabled > 1 ? 's' : ''} agora.` : ''}
          </p>
        </header>

        <div className="dest-list">
          {settings.destinations.map((d) => (
            <DestinationCard key={d.id} dest={d} twitch={d.platform === 'twitch' ? twitch : { step: 'idle' }} />
          ))}
        </div>

        {adding ? (
          <div className="row" style={{ gap: 'var(--sp-2)', flexWrap: 'wrap' }}>
            {(Object.keys(PLATFORM_INFO) as PlatformId[]).map((p) => (
              <Button key={p} size="sm" onClick={() => add(p)}>
                {PLATFORM_INFO[p].label}
              </Button>
            ))}
            <Button size="sm" variant="ghost" onClick={() => setAdding(false)}>
              Cancelar
            </Button>
          </div>
        ) : (
          <Button variant="subtle" icon={<IconPlus size={15} />} onClick={() => setAdding(true)} style={{ alignSelf: 'flex-start' }}>
            Adicionar destino
          </Button>
        )}

        <p className="hint">
          As chaves ficam criptografadas neste computador (proteção do Windows) e nunca aparecem na tela depois de
          salvas. Precisa do FFmpeg instalado para transmitir.
        </p>
      </div>
    </div>
  )
}
