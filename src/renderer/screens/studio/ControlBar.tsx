import { useEffect, useRef, useState } from 'react'
import { PLATFORM_INFO } from '@shared/ipc'
import type { LiveInfo } from '@shared/protocol'
import { Button, Field, IconButton, TextInput } from '@/components/ui'
import {
  IconBroadcast,
  IconCamera,
  IconCameraOff,
  IconDown,
  IconEdit,
  IconEyeOff,
  IconPlus,
  IconRecord,
  IconReplay,
  IconStop
} from '@/components/Icons'
import { formatClock, useNow } from '@/components/LiveStatus'
import { useEngineState, useStudio } from '@/studio/StudioContext'
import { GoLiveModal, LiveInfoFields, useTwitchCaps } from './GoLive'

/** Fecha um popover ao clicar fora. */
function useOutside(open: boolean, close: () => void): React.RefObject<HTMLDivElement | null> {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      if (!ref.current?.contains(e.target as Node)) close()
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') close()
    }
    window.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [open, close])
  return ref
}

/** Título e categoria na própria barra: clique e edite, inclusive no meio da live. */
function LiveTitle({ onOpenDestinations }: { onOpenDestinations: () => void }): React.JSX.Element {
  const { engine, settings, update, toast } = useStudio()
  const state = useEngineState()
  const caps = useTwitchCaps()
  const [open, setOpen] = useState(false)
  const [draft, setDraft] = useState<LiveInfo>(settings.live)
  const [saving, setSaving] = useState(false)
  const ref = useOutside(open, () => setOpen(false))
  const live = settings.live

  useEffect(() => {
    if (open) setDraft(settings.live)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  const save = async (): Promise<void> => {
    update({ live: draft })
    if (state.live) {
      setSaving(true)
      const err = await engine.pushLiveInfo(draft)
      setSaving(false)
      if (err) toast(err, 'bad')
      else if (caps?.canEditInfo) toast('Título atualizado na Twitch', 'good')
    }
    setOpen(false)
  }

  return (
    <div className="cb-title-wrap" ref={ref}>
      <button className={`cb-title ${live.title ? '' : 'is-empty'}`} onClick={() => setOpen((o) => !o)} title="Título e categoria da live">
        <span className="cb-title-text">{live.title || 'Dê um título para a live'}</span>
        <span className="cb-title-cat">
          {live.categoryName || 'sem categoria'}
          <IconEdit size={12} />
        </span>
      </button>
      {open && (
        <div className="popover cb-title-pop">
          <LiveInfoFields
            info={draft}
            onChange={setDraft}
            caps={caps}
            onRelogin={() => {
              setOpen(false)
              onOpenDestinations()
            }}
          />
          <div className="row" style={{ justifyContent: 'flex-end', gap: 'var(--sp-2)' }}>
            <Button size="sm" variant="ghost" onClick={() => setOpen(false)}>
              Cancelar
            </Button>
            <Button size="sm" variant="primary" loading={saving} onClick={() => void save()}>
              {state.live && caps?.canEditInfo ? 'Atualizar na Twitch' : 'Salvar'}
            </Button>
          </div>
        </div>
      )}
    </div>
  )
}

/** Tela de pausa com os textos editáveis ali mesmo (antes só em Ajustes). */
function PauseButton(): React.JSX.Element {
  const { settings, update } = useStudio()
  const studio = settings.studio
  const [open, setOpen] = useState(false)
  const ref = useOutside(open, () => setOpen(false))
  const set = (p: Partial<typeof studio>): void => update({ studio: { ...studio, ...p } })
  return (
    <div className="split-btn" ref={ref}>
      <Button
        size="sm"
        variant={studio.privacyShieldActive ? 'danger' : 'ghost'}
        icon={<IconEyeOff size={15} />}
        onClick={() => set({ privacyShieldActive: !studio.privacyShieldActive })}
        title="Cobre tudo na hora com uma tela de pausa (Ctrl+Shift+P)"
      >
        {studio.privacyShieldActive ? 'Pausa ativa' : 'Pausa'}
      </Button>
      <IconButton label="Textos da tela de pausa" className="split-caret" active={open} onClick={() => setOpen((o) => !o)}>
        <IconDown size={13} />
      </IconButton>
      {open && (
        <div className="popover pause-pop">
          <Field label="Título da tela de pausa">
            <TextInput value={studio.privacyShieldTitle} onChange={(e) => set({ privacyShieldTitle: e.target.value })} />
          </Field>
          <Field label="Subtítulo">
            <TextInput value={studio.privacyShieldSubtitle} onChange={(e) => set({ privacyShieldSubtitle: e.target.value })} />
          </Field>
          <span className="hint">Para pausas longas, use a cena “Volto já” do kit.</span>
        </div>
      )}
    </div>
  )
}

export function ControlBar({ onOpenDestinations }: { onOpenDestinations: () => void }): React.JSX.Element {
  const { engine, settings, update, toast } = useStudio()
  const state = useEngineState()
  const [confirmStop, setConfirmStop] = useState(false)
  const [preparing, setPreparing] = useState(false)
  const now = useNow(state.live)
  const studio = settings.studio

  const connected = settings.destinations.filter((d) => d.hasKey && d.server.trim())

  const stop = async (): Promise<void> => {
    setConfirmStop(false)
    await engine.stopLive()
    toast('Live encerrada')
  }

  /** Liga/desliga um destino só para esta live, sem abrir a página de Destinos. */
  const toggleDest = (id: string): void => {
    update({ destinations: settings.destinations.map((d) => (d.id === id ? { ...d, enabled: !d.enabled } : d)) })
  }

  return (
    <footer className="controlbar">
      <div className="cb-left">
        <LiveTitle onOpenDestinations={onOpenDestinations} />
        <div className="cb-destinations">
          {connected.length === 0 ? (
            <button className="cb-dest is-empty" onClick={onOpenDestinations}>
              <IconPlus size={13} /> Conectar Twitch, YouTube…
            </button>
          ) : (
            connected.map((d) => {
              const st = state.multistream.destinations.find((s) => s.id === d.id)
              const status = state.live && d.enabled ? (st?.state ?? 'connecting') : st?.state === 'error' ? 'error' : 'idle'
              return (
                <button
                  key={d.id}
                  className={`cb-dest is-${status} ${d.enabled ? 'is-on' : 'is-off'}`}
                  onClick={() => (state.live ? onOpenDestinations() : toggleDest(d.id))}
                  title={
                    st?.error ??
                    (state.live
                      ? `${PLATFORM_INFO[d.platform].label} · ${d.enabled ? 'transmitindo' : 'fora desta live'}`
                      : d.enabled
                        ? 'Vai nesta live — clique para tirar'
                        : 'Fora desta live — clique para incluir')
                  }
                  disabled={state.live && !d.enabled}
                >
                  <span className={`cb-dot plat-${d.platform}`} />
                  {d.account?.displayName ?? d.name}
                  {status === 'connecting' && <span className="dim"> · conectando</span>}
                  {status === 'error' && <span className="cb-err"> · erro</span>}
                </button>
              )
            })
          )}
          <span className="cb-encoder dim mono" title="Encoder usado pelo FFmpeg">
            {state.multistream.ffmpegFound ? state.multistream.encoder : 'sem FFmpeg'} · {studio.fps}fps
          </span>
        </div>
      </div>

      <div className="cb-actions">
        <Button
          size="sm"
          variant={studio.cameraEnabled ? 'ghost' : 'danger'}
          icon={studio.cameraEnabled ? <IconCamera size={15} /> : <IconCameraOff size={15} />}
          onClick={() => update({ studio: { ...studio, cameraEnabled: !studio.cameraEnabled } })}
          title="Sem câmera hoje? Desliga a câmera e esconde as molduras dos overlays (Ctrl+Shift+C)"
        >
          {studio.cameraEnabled ? 'Câmera' : 'Sem câmera'}
        </Button>
        <PauseButton />
        <Button
          size="sm"
          variant="ghost"
          icon={<IconReplay size={15} />}
          onClick={() => void engine.saveReplay().then((r) => toast(r.message, r.ok ? 'good' : 'bad'))}
          title={`Salva os últimos ${studio.replayBufferSeconds}s (Ctrl+Shift+S)`}
        >
          Replay
        </Button>
        <Button
          size="sm"
          variant={state.recorder.recording ? 'danger' : 'subtle'}
          icon={<IconRecord size={15} />}
          onClick={() => {
            const wasRecording = state.recorder.recording
            const path = state.recorder.recordingPath
            void engine.toggleRecording().then((err) => {
              if (err) toast(err, 'bad')
              else if (wasRecording)
                toast('Gravação salva em Vídeos\\PrismCast', 'good', path ? { label: 'Abrir pasta', run: () => window.api.app.showInFolder(path) } : undefined)
            })
          }}
        >
          {state.recorder.recording ? `Parar ${formatClock(state.recorder.recordingDurationSec)}` : 'Gravar'}
        </Button>

        {state.live ? (
          confirmStop ? (
            <div className="cb-confirm">
              <span>Encerrar a live?</span>
              <Button size="md" variant="danger" icon={<IconStop size={15} />} onClick={() => void stop()}>
                Encerrar
              </Button>
              <Button size="md" variant="ghost" onClick={() => setConfirmStop(false)}>
                Continuar
              </Button>
            </div>
          ) : (
            <button className="golive is-live" onClick={() => setConfirmStop(true)}>
              <span className="golive-dot" />
              <span>Ao vivo</span>
              <span className="mono">{formatClock(Math.floor((now - (state.liveStartedAt ?? now)) / 1000))}</span>
            </button>
          )
        ) : (
          <button className="golive" onClick={() => setPreparing(true)} disabled={state.goingLive}>
            {state.goingLive ? <span className="spinner" /> : <IconBroadcast size={17} />}
            <span>{state.goingLive ? 'Conectando…' : 'Entrar ao vivo'}</span>
          </button>
        )}
      </div>
      <GoLiveModal open={preparing} onClose={() => setPreparing(false)} onOpenDestinations={onOpenDestinations} />
    </footer>
  )
}
