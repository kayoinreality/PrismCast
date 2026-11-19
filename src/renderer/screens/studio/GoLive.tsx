import { useEffect, useRef, useState } from 'react'
import { PLATFORM_INFO } from '@shared/ipc'
import type { LiveInfo, TwitchCapabilities, TwitchCategory } from '@shared/protocol'
import { Modal } from '@/components/Modal'
import { Button, Field, TextInput, Toggle } from '@/components/ui'
import {
  IconAlert,
  IconBroadcast,
  IconCamera,
  IconCameraOff,
  IconCheck,
  IconMicOff,
  IconSearch
} from '@/components/Icons'
import { useEngineState, useStudio } from '@/studio/StudioContext'
import { sceneMics, selectScene } from '@/studio/scenes'

/** Busca de categoria da Twitch com lista de sugestões e proteção contra respostas fora de ordem. */
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
  const blurTimer = useRef(0)
  const reqIdRef = useRef(0)

  useEffect(() => setQuery(value.name), [value.name])

  useEffect(() => {
    return () => {
      window.clearTimeout(timer.current)
      window.clearTimeout(blurTimer.current)
      reqIdRef.current++
    }
  }, [])

  const search = (q: string): void => {
    setQuery(q)
    window.clearTimeout(timer.current)
    const reqId = ++reqIdRef.current
    if (!enabled || q.trim().length < 2) {
      setResults([])
      setOpen(false)
      return
    }
    timer.current = window.setTimeout(() => {
      void window.api.twitch
        .searchCategories(q)
        .then((r) => {
          if (reqId !== reqIdRef.current) return
          setResults(r)
          setOpen(r.length > 0)
        })
        .catch(() => {
          if (reqId === reqIdRef.current) {
            setResults([])
            setOpen(false)
          }
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
          onBlur={() => {
            window.clearTimeout(blurTimer.current)
            blurTimer.current = window.setTimeout(() => setOpen(false), 150)
          }}
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
              {c.boxArtUrl && <img src={c.boxArtUrl} alt="" />}
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
    let alive = true
    void window.api.twitch
      .capabilities()
      .then((c) => {
        if (alive) setCaps(c)
      })
      .catch(() => {})
    return () => {
      alive = false
    }
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
      {canEdit && (
        <span className="hint">
          Vai para a Twitch ao entrar ao vivo. YouTube e Kick usam o título do painel deles.
        </span>
      )}
    </>
  )
}

/**
 * "Preparar a live": tudo que se decide antes de cada live numa tela só —
 * título, categoria, para onde vai hoje, com ou sem câmera, cena de abertura e checklist de saúde.
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
  const state = useEngineState()
  const caps = useTwitchCaps()
  const [info, setInfo] = useState<LiveInfo>(settings.live)
  const [enabled, setEnabled] = useState<Set<string>>(new Set())
  const [camera, setCamera] = useState(settings.studio.cameraEnabled)
  const [sceneId, setSceneId] = useState(settings.scenes.activeSceneId)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (!open) return
    setInfo(settings.live)
    setEnabled(
      new Set(
        settings.destinations
          .filter((d) => d.enabled && d.hasKey && d.server.trim())
          .map((d) => d.id)
      )
    )
    setCamera(settings.studio.cameraEnabled)
    setSceneId(settings.scenes.activeSceneId)
    if (!settings.live.title) {
      let alive = true
      void window.api.twitch
        .getChannelInfo()
        .then((ch) => {
          if (alive && ch) setInfo((cur) => (cur.title ? cur : ch))
        })
        .catch(() => {})
      return () => {
        alive = false
      }
    }
    return undefined
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  const ready = settings.destinations.filter((d) => d.hasKey && d.server.trim())
  const notReady = settings.destinations.filter((d) => !(d.hasKey && d.server.trim()))

  const chosenScene =
    settings.scenes.scenes.find((s) => s.id === sceneId) ?? settings.scenes.scenes[0] ?? null
  const visualItemsCount = (chosenScene?.items ?? []).filter(
    (it) => it.visible && it.type !== 'audio_input' && (camera || it.type !== 'camera_input')
  ).length

  const mics = settings.audioMixer.channels.filter((c) => c.kind === 'mic')
  const hasPerSceneMics = settings.scenes.scenes.some((s) => s.items.some((it) => it.type === 'audio_input'))
  const activeMicIds = hasPerSceneMics ? sceneMics(chosenScene, true) : new Set(mics.map((m) => m.id))
  const activeMics = mics.filter((m) => activeMicIds.has(m.id))
  const allMicsMuted = activeMics.length > 0 && activeMics.every((m) => m.muted)
  const micWithError = activeMics.find((m) => engine.audioStatus(m.id).state === 'error')
  const ffmpegOk = state.multistream.ffmpegFound

  const unmuteMics = (): void => {
    update({
      audioMixer: {
        ...settings.audioMixer,
        channels: settings.audioMixer.channels.map((c) => (c.kind === 'mic' ? { ...c, muted: false } : c))
      }
    })
    toast('Microfone desmutado', 'good')
  }

  const start = async (): Promise<void> => {
    if (enabled.size === 0) {
      toast('Escolha pelo menos um destino.', 'bad')
      return
    }
    setBusy(true)
    try {
      const scenes =
        sceneId !== settings.scenes.activeSceneId
          ? selectScene({ ...settings.scenes, studioMode: false }, sceneId)
          : settings.scenes
      const newSettings = {
        live: info,
        destinations: settings.destinations.map((d) => ({ ...d, enabled: enabled.has(d.id) })),
        studio: { ...settings.studio, cameraEnabled: camera },
        scenes: { ...scenes, studioMode: settings.scenes.studioMode }
      }
      update(newSettings)
      engine.apply({ ...settings, ...newSettings })
      await window.api.settings.set(newSettings)
      const infoErr = await engine.pushLiveInfo(info)
      if (infoErr) toast(`Título não foi para a Twitch: ${infoErr}`, 'bad')
      const err = await engine.goLive()
      if (err) {
        toast(err, 'bad')
        return
      }
      const names = settings.destinations
        .filter((d) => enabled.has(d.id))
        .map((d) => d.account?.displayName ?? d.name)
      toast(`Ao vivo em ${names.join(', ')}`, 'good')
      onClose()
    } catch (e) {
      toast((e as Error).message || 'Não foi possível iniciar a transmissão.', 'bad')
    } finally {
      setBusy(false)
    }
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
          <Button
            variant="primary"
            icon={<IconBroadcast size={16} />}
            loading={busy}
            disabled={!ffmpegOk || enabled.size === 0}
            onClick={() => void start()}
          >
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

        <div className="golive-checklist">
          {!ffmpegOk ? (
            <div className="golive-check-row is-bad">
              <span>
                <IconAlert size={13} /> FFmpeg não encontrado no sistema
              </span>
              <button
                className="link-btn"
                onClick={() => {
                  onClose()
                  onOpenDestinations()
                }}
              >
                Ver instruções
              </button>
            </div>
          ) : (
            <div className="golive-check-row is-ok">
              <span>
                <IconCheck size={13} /> Encoder pronto ({state.multistream.encoder || 'H.264'})
              </span>
              <span className="mono">{settings.studio.fps} fps</span>
            </div>
          )}

          {visualItemsCount === 0 ? (
            <div className="golive-check-row is-warn">
              <span>
                <IconAlert size={13} /> A cena “{chosenScene?.name ?? 'atual'}” está sem fontes visíveis (tela preta)
              </span>
            </div>
          ) : (
            <div className="golive-check-row is-ok">
              <span>
                <IconCheck size={13} /> Cena “{chosenScene?.name}” com {visualItemsCount}{' '}
                {visualItemsCount === 1 ? 'fonte visível' : 'fontes visíveis'}
              </span>
            </div>
          )}

          {micWithError ? (
            <div className="golive-check-row is-warn">
              <span>
                <IconAlert size={13} /> {micWithError.name}: {engine.audioStatus(micWithError.id).message ?? 'erro ao abrir'}
              </span>
            </div>
          ) : allMicsMuted ? (
            <div className="golive-check-row is-warn">
              <span>
                <IconMicOff size={13} /> Seu microfone está mudo nesta cena
              </span>
              <button className="link-btn" onClick={unmuteMics}>
                Desmutar
              </button>
            </div>
          ) : activeMics.length === 0 ? (
            <div className="golive-check-row is-warn">
              <span>
                <IconMicOff size={13} /> Esta cena começa sem microfone (bom para “Começando”)
              </span>
            </div>
          ) : (
            <div className="golive-check-row is-ok">
              <span>
                <IconCheck size={13} /> Microfone pronto ({activeMics.map((m) => m.name).join(', ')})
              </span>
            </div>
          )}
        </div>
      </div>
    </Modal>
  )
}
