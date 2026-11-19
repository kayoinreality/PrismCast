import { useEffect, useState } from 'react'
import { PLATFORM_INFO, PRISMCAST_TWITCH_CLIENT_ID } from '@shared/ipc'
import type { Destination, PlatformId, TwitchLoginState } from '@shared/protocol'
import { Button, Field, TextInput, Toggle } from '@/components/ui'
import { IconAlert, IconCheck, IconExternal, IconKey, IconPlus, IconTrash } from '@/components/Icons'
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
    try {
      const res = await window.api.destinations.setKey(dest.id, key)
      if (res.ok) {
        setKey('')
        setError(null)
        toast(`Chave de ${dest.name} salva e destino ligado`, 'good')
      } else {
        setError(res.error)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Não foi possível salvar a chave.')
    } finally {
      setSaving(false)
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

function TwitchLogin({
  dest,
  state,
  onStartLogin
}: {
  dest: Destination
  state: TwitchLoginState
  onStartLogin: () => void
}): React.JSX.Element {
  const { settings, update } = useStudio()
  const [clientId, setClientId] = useState(settings.twitchClientId)
  const [setup, setSetup] = useState(false)

  useEffect(() => {
    setClientId(settings.twitchClientId)
  }, [settings.twitchClientId])

  const own = settings.twitchClientId.trim()
  const custom = own.length > 0 && own !== PRISMCAST_TWITCH_CLIENT_ID
  const scopes = dest.account?.scopes ?? []
  const outdated = dest.account !== null && !(scopes.includes('channel:manage:broadcast') && scopes.includes('user:write:chat'))

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
        <Button className="btn-twitch" onClick={onStartLogin}>
          {dest.account ? 'Entrar de novo com a Twitch' : 'Entrar com a Twitch'}
        </Button>
        <span className="hint">Pede a chave de transmissão, mudar título/categoria e escrever no chat.</span>
      </div>
      {outdated && (
        <div className="dest-warn">
          Seu login é da versão anterior e só libera a chave. Entre de novo para o PrismCast mudar o título da live e
          escrever no chat por você.
        </div>
      )}
      {state.step === 'error' && <span className="error-text">{state.message}</span>}
      <button className="link-btn twitch-advanced" onClick={() => setSetup((s) => !s)}>
        {custom ? 'Usando o seu próprio Client ID' : 'Avançado: usar outro Client ID'}
      </button>
      {setup && (
        <div className="twitch-setup">
          <span className="hint">
            Opcional. O PrismCast já tem um app registrado na Twitch. Cole aqui só se quiser usar um app seu
            (dev.twitch.tv, tipo Público). Deixe vazio para voltar ao do PrismCast.
          </span>
          <div className="row" style={{ gap: 'var(--sp-2)' }}>
            <TextInput value={clientId} onChange={(e) => setClientId(e.target.value)} placeholder="Client ID do PrismCast" spellCheck={false} />
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
        </div>
      )}
    </div>
  )
}

function DestinationCard({
  dest,
  twitch,
  onStartTwitchLogin
}: {
  dest: Destination
  twitch: TwitchLoginState
  onStartTwitchLogin: () => void
}): React.JSX.Element {
  const { settings, update } = useStudio()
  const state = useEngineState()
  const [advanced, setAdvanced] = useState(PLATFORM_INFO[dest.platform].serverFromDashboard && !dest.server)
  const [confirmRemove, setConfirmRemove] = useState(false)
  const [showKey, setShowKey] = useState(false)
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
          ariaLabel={`Ativar ${dest.name}`}
          checked={dest.enabled}
          disabled={!ready}
          onChange={(v) => patch({ enabled: v })}
        />
      </div>

      {status?.error && <div className="dest-error">{status.error}</div>}

      {dest.platform === 'twitch' && <TwitchLogin dest={dest} state={twitch} onStartLogin={onStartTwitchLogin} />}
      {dest.platform === 'twitch' && dest.account && !showKey ? (
        <button className="link-btn dest-advanced-toggle" onClick={() => setShowKey(true)}>
          Colar a chave manualmente
        </button>
      ) : (
        <KeyForm dest={dest} />
      )}

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
          <div className="row" style={{ gap: 'var(--sp-2)' }}>
            <Button size="sm" variant="ghost" onClick={() => setConfirmRemove(false)}>
              Cancelar
            </Button>
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
          </div>
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
  const state = useEngineState()
  const [twitchStates, setTwitchStates] = useState<Record<string, TwitchLoginState>>({})
  const [activeTwitchDestId, setActiveTwitchDestId] = useState<string | null>(null)
  const [adding, setAdding] = useState(false)

  useEffect(
    () =>
      window.api.destinations.onTwitchLogin((s) => {
        const targetId = activeTwitchDestId ?? settings.destinations.find((d) => d.platform === 'twitch')?.id
        if (targetId) {
          setTwitchStates((prev) => ({ ...prev, [targetId]: s }))
        }
        if (s.step === 'done') {
          toast(`Twitch conectada como ${s.account.displayName}`, 'good')
          setActiveTwitchDestId(null)
        } else if (s.step === 'idle' || s.step === 'error') {
          setActiveTwitchDestId(null)
        }
      }),
    [toast, activeTwitchDestId, settings.destinations]
  )

  const startTwitchLogin = (destId: string): void => {
    setActiveTwitchDestId(destId)
    void window.api.destinations.twitchLogin(destId)
  }

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
            Conecte suas plataformas uma vez. Em cada live você escolhe quais vão: direto na barra de baixo do Estúdio
            ou em “Entrar ao vivo”. O PrismCast manda para todas ao mesmo tempo.{' '}
            {enabled > 0 ? `${enabled} ligado${enabled > 1 ? 's' : ''} agora.` : ''}
          </p>
        </header>

        {!state.multistream.ffmpegFound && (
          <div className="dest-warn row" style={{ justifyContent: 'space-between', alignItems: 'center' }}>
            <span className="row" style={{ gap: 'var(--sp-2)' }}>
              <IconAlert size={16} />
              <span>
                <strong>FFmpeg não encontrado.</strong> Instale o FFmpeg ou configure o executável nas preferências para conseguir transmitir.
              </span>
            </span>
            <Button
              size="sm"
              variant="subtle"
              icon={<IconExternal size={13} />}
              onClick={() => window.api.app.openExternal('https://ffmpeg.org/download.html')}
            >
              Baixar FFmpeg
            </Button>
          </div>
        )}

        <div className="dest-list">
          {settings.destinations.map((d) => (
            <DestinationCard
              key={d.id}
              dest={d}
              twitch={d.platform === 'twitch' ? (twitchStates[d.id] ?? { step: 'idle' }) : { step: 'idle' }}
              onStartTwitchLogin={() => startTwitchLogin(d.id)}
            />
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
