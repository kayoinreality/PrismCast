import { useEffect, useRef, useState } from 'react'
import { PLATFORM_INFO } from '@shared/ipc'
import type { LiveInfo, TwitchCapabilities, TwitchCategory } from '@shared/protocol'
import { Modal } from '@/components/Modal'
import { Button, Field, TextInput, Toggle } from '@/components/ui'
import { IconBroadcast, IconCamera, IconCameraOff, IconSearch } from '@/components/Icons'
import { useStudio } from '@/studio/StudioContext'
import { selectScene } from '@/studio/scenes'

/** Busca de categoria da Twitch com lista de sugestões. */
export function CategoryPicker({
  value,
  onChange,
  enabled
}: {
  value: { id: string; name: string }
  onChange: (c: { id: string; name: string }) => void
  enabled: boolean
}): React.JSX.Element {
  const [query, setQuery] = useState(value.name)
  const [results, setResults] = useState<TwitchCategory[]>([])
  const [open, setOpen] = useState(false)
  const timer = useRef(0)
  useEffect(() => setQuery(value.name), [value.name])

  const search = (q: string): void => {
    setQuery(q)
    window.clearTimeout(timer.current)
    if (!enabled || q.trim().length < 2) {
      setResults([])
      return
    }
    timer.current = window.setTimeout(() => {
      void window.api.twitch.searchCategories(q).then((r) => {
        setResults(r)
        setOpen(true)
      })
    }, 280)
  }

  return (
    <div className="cat-picker">
      <div className="search-input">
        <IconSearch size={15} />
        <input
          value={query}
          placeholder={enabled ? 'Just Chatting, Valorant…' : 'Entre com a Twitch para escolher'}
          disabled={!enabled}
          onChange={(e) => search(e.target.value)}
          onFocus={() => results.length > 0 && setOpen(true)}
          onBlur={() => window.setTimeout(() => setOpen(false), 150)}
        />
      </div>
      {open && results.length > 0 && (
        <div className="cat-results">
          {results.map((c) => (
            <button
              key={c.id}
              className="cat-row"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => {
                onChange({ id: c.id, name: c.name })
                setQuery(c.name)
                setOpen(false)
              }}
            >
              <img src={c.boxArtUrl} alt="" />
              <span>{c.name}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

export function useTwitchCaps(): TwitchCapabilities | null {
  const { settings } = useStudio()
  const [caps, setCaps] = useState<TwitchCapabilities | null>(null)
  const twitch = settings.destinations.find((d) => d.platform === 'twitch' && d.account)?.account
  useEffect(() => {
    void window.api.twitch.capabilities().then(setCaps)
  }, [twitch?.login, twitch?.scopes?.length])
  return caps
}

/** Campos de título e categoria, usados no "Preparar a live" e no editor rápido da barra. */
export function LiveInfoFields({
  info,
  onChange,
  caps,
  onRelogin
}: {
  info: LiveInfo
  onChange: (i: LiveInfo) => void
  caps: TwitchCapabilities | null
  onRelogin: () => void
}): React.JSX.Element {
  const canEdit = Boolean(caps?.linked && caps.canEditInfo)
  return (
    <>
      <Field label="Título da live" hint={`${info.title.length}/140`}>
        <TextInput
          value={info.title}
          maxLength={140}
          placeholder="ex.: zerando o jogo hoje, bora!"
          onChange={(e) => onChange({ ...info, title: e.target.value })}
          autoFocus
        />
      </Field>
      <Field label="Categoria (Twitch)">
        <CategoryPicker
          value={{ id: info.categoryId, name: info.categoryName }}
          enabled={canEdit}
          onChange={(c) => onChange({ ...info, categoryId: c.id, categoryName: c.name })}
        />
      </Field>
      {!canEdit && (
        <span className="hint">
          {caps?.linked ? (
            <>
              Para o PrismCast mudar título e categoria na Twitch,{' '}
              <button className="link-btn" onClick={onRelogin}>
                entre de novo com a Twitch
              </button>{' '}
              (o login antigo só liberava a chave).
            </>
          ) : (
            <>Sem login da Twitch o título fica só aqui; mude no painel de cada plataforma.</>
          )}
        </span>
      )}
      {canEdit && <span className="hint">Vai para a Twitch ao entrar ao vivo. YouTube e Kick usam o título do painel deles.</span>}
    </>
  )
}

/**
 * "Preparar a live": tudo que se decide antes de cada live numa tela só —
 * título, categoria, para onde vai hoje, com ou sem câmera, cena de abertura.
 */
export function GoLiveModal({
  open,
  onClose,
  onOpenDestinations
}: {
  open: boolean
  onClose: () => void
  onOpenDestinations: () => void
}): React.JSX.Element {
  const { engine, settings, update, toast } = useStudio()
  const caps = useTwitchCaps()
  const [info, setInfo] = useState<LiveInfo>(settings.live)
  const [enabled, setEnabled] = useState<Set<string>>(new Set())
  const [camera, setCamera] = useState(settings.studio.cameraEnabled)
  const [sceneId, setSceneId] = useState(settings.scenes.activeSceneId)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (!open) return
    setInfo(settings.live)
    setEnabled(new Set(settings.destinations.filter((d) => d.enabled && d.hasKey).map((d) => d.id)))
    setCamera(settings.studio.cameraEnabled)
    setSceneId(settings.scenes.activeSceneId)
    // Primeira vez: puxa o título que já está na Twitch.
    if (!settings.live.title) {
      void window.api.twitch.getChannelInfo().then((ch) => ch && setInfo((cur) => (cur.title ? cur : ch)))
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  const ready = settings.destinations.filter((d) => d.hasKey && d.server.trim())
  const notReady = settings.destinations.filter((d) => !(d.hasKey && d.server.trim()))

  const start = async (): Promise<void> => {
    if (enabled.size === 0) {
      toast('Escolha pelo menos um destino.', 'bad')
      return
    }
    setBusy(true)
    const scenes = sceneId !== settings.scenes.activeSceneId ? selectScene({ ...settings.scenes, studioMode: false }, sceneId) : settings.scenes
    const newSettings = {
      live: info,
      destinations: settings.destinations.map((d) => ({ ...d, enabled: enabled.has(d.id) })),
      studio: { ...settings.studio, cameraEnabled: camera },
      scenes: { ...scenes, studioMode: settings.scenes.studioMode }
    }
    update(newSettings)
    engine.apply({ ...settings, ...newSettings })
    // A live lê destinos e cena no main: grava já, sem esperar a escrita adiada.
    await window.api.settings.set(newSettings)
    const infoErr = await engine.pushLiveInfo(info)
    if (infoErr) toast(`Título não foi para a Twitch: ${infoErr}`, 'bad')
    const err = await engine.goLive()
    setBusy(false)
    if (err) {
      toast(err, 'bad')
      return
    }
    const names = settings.destinations.filter((d) => enabled.has(d.id)).map((d) => d.account?.displayName ?? d.name)
    toast(`Ao vivo em ${names.join(', ')}`, 'good')
    onClose()
  }

  return (
    <Modal
      open={open}
      onClose={busy ? undefined : onClose}
      width={600}
      icon={<IconBroadcast size={19} />}
      title="Preparar a live"
      description="Confira o que muda de uma live para outra. Tudo fica salvo para a próxima."
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancelar
          </Button>
          <Button variant="primary" icon={<IconBroadcast size={16} />} loading={busy} onClick={() => void start()}>
            Entrar ao vivo{enabled.size > 0 ? ` em ${enabled.size}` : ''}
          </Button>
        </>
      }
    >
      <div className="golive-form">
        <LiveInfoFields
          info={info}
          onChange={setInfo}
          caps={caps}
          onRelogin={() => {
            onClose()
            onOpenDestinations()
          }}
        />

        <div className="field">
          <div className="field-head">
            <span className="field-label">Para onde vai hoje</span>
          </div>
          <div className="golive-dests">
            {ready.map((d) => (
              <label key={d.id} className={`golive-dest ${enabled.has(d.id) ? 'is-on' : ''}`}>
                <input
                  type="checkbox"
                  checked={enabled.has(d.id)}
                  onChange={(e) => {
                    const next = new Set(enabled)
                    if (e.target.checked) next.add(d.id)
                    else next.delete(d.id)
                    setEnabled(next)
                  }}
                />
                <span className={`cb-dot plat-${d.platform}`} />
                <span className="golive-dest-name">{d.account?.displayName ?? d.name}</span>
                <span className="dim">
                  {PLATFORM_INFO[d.platform].label} · {d.orientation === 'vertical' ? '9:16' : '16:9'}
                </span>
              </label>
            ))}
            {ready.length === 0 && <span className="hint">Nenhuma plataforma conectada ainda.</span>}
          </div>
          {notReady.length > 0 && (
            <button
              className="link-btn golive-connect"
              onClick={() => {
                onClose()
                onOpenDestinations()
              }}
            >
              Conectar {notReady.map((d) => d.name).join(', ')}…
            </button>
          )}
        </div>

        <div className="form-grid">
          <Field label="Começar na cena">
            <select className="select" value={sceneId} onChange={(e) => setSceneId(e.target.value)}>
              {settings.scenes.scenes.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
            </select>
          </Field>
          <div className="golive-camera">
            {camera ? <IconCamera size={18} /> : <IconCameraOff size={18} />}
            <Toggle
              label="Com câmera"
              hint={camera ? 'Câmera e molduras aparecem' : 'Câmera e molduras somem dos overlays'}
              checked={camera}
              onChange={setCamera}
            />
          </div>
        </div>
      </div>
    </Modal>
  )
}
