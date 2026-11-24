import { useEffect, useRef, useState } from 'react'
import { PLATFORM_INFO } from '@shared/ipc'
import type { LiveInfo } from '@shared/protocol'
import { Button, Field, IconButton, TextInput } from '@/components/ui'
import {
  IconAlert,
  IconBroadcast,
  IconCamera,
  IconCameraOff,
  IconDown,
  IconEdit,
  IconEyeOff,
  IconMicOff,
  IconPlus,
  IconRecord,
  IconReplay,
  IconStop
} from '@/components/Icons'
import { formatClock, useNow } from '@/components/LiveStatus'
import { useEngineState, useStudio } from '@/studio/StudioContext'
import {
  studioStore,
  useAbrTelemetrySlice,
  useGpuStatusSlice,
  useTelemetrySlice,
  useVirtualCamSlice
} from '@/studio/StudioStore'
import { GoLiveModal, LiveInfoFields, useTwitchCaps } from './GoLive'

function formatHotkey(accel: string | undefined, fallback: string): string {
  const raw = accel?.trim() || fallback
  return raw.replace(/CommandOrControl/gi, 'Ctrl')
}

/** Fecha um popover ao clicar fora usando ref estável para o callback `close`. */
function useOutside(open: boolean, close: () => void): React.RefObject<HTMLDivElement | null> {
  const ref = useRef<HTMLDivElement>(null)
  const closeRef = useRef(close)
  closeRef.current = close

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      if (!ref.current?.contains(e.target as Node)) closeRef.current()
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') closeRef.current()
    }
    window.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [open])
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
  }, [open, settings.live])

  const save = async (): Promise<void> => {
    update({ live: draft })
    if (state.live) {
      setSaving(true)
      try {
        const err = await engine.pushLiveInfo(draft)
        if (err) toast(err, 'bad')
        else if (caps?.canEditInfo) toast('Título atualizado na Twitch', 'good')
      } finally {
        setSaving(false)
      }
    }
    setOpen(false)
  }

  return (
    <div className="cb-title-wrap" ref={ref}>
      <button
        className={`cb-title ${live.title ? '' : 'is-empty'}`}
        onClick={() => setOpen((o) => !o)}
        title="Título e categoria da live"
      >
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

/** Tela de pausa com os textos editáveis ali mesmo. */
function PauseButton(): React.JSX.Element {
  const { settings, update } = useStudio()
  const studio = settings.studio
  const [open, setOpen] = useState(false)
  const ref = useOutside(open, () => setOpen(false))
  const set = (p: Partial<typeof studio>): void => update({ studio: { ...studio, ...p } })
  const hk = formatHotkey(settings.hotkeys?.togglePrivacyShield, 'Ctrl+Shift+P')

  return (
    <div className="split-btn" ref={ref}>
      <Button
        size="sm"
        variant={studio.privacyShieldActive ? 'danger' : 'ghost'}
        icon={<IconEyeOff size={15} />}
        onClick={() => set({ privacyShieldActive: !studio.privacyShieldActive })}
        title={`Cobre tudo na hora com uma tela de pausa (${hk})`}
      >
        {studio.privacyShieldActive ? 'Pausa ativa' : 'Pausa'}
      </Button>
      <IconButton
        label="Textos da tela de pausa"
        className="split-caret"
        active={open}
        onClick={() => setOpen((o) => !o)}
      >
        <IconDown size={13} />
      </IconButton>
      {open && (
        <div className="popover pause-pop">
          <Field label="Título da tela de pausa">
            <TextInput
              value={studio.privacyShieldTitle}
              onChange={(e) => set({ privacyShieldTitle: e.target.value })}
            />
          </Field>
          <Field label="Subtítulo">
            <TextInput
              value={studio.privacyShieldSubtitle}
              onChange={(e) => set({ privacyShieldSubtitle: e.target.value })}
            />
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
  const telemetry = useTelemetrySlice()
  const gpuStatus = useGpuStatusSlice()
  const abrTelemetry = useAbrTelemetrySlice()
  const vcamStore = useVirtualCamSlice()
  const [confirmStop, setConfirmStop] = useState(false)
  const [preparing, setPreparing] = useState(false)
  const [togglingRec, setTogglingRec] = useState(false)
  const [togglingVcam, setTogglingVcam] = useState(false)
  const [localMutedSpeaking, setLocalMutedSpeaking] = useState(false)
  const now = useNow(state.live)
  const studio = settings.studio

  useEffect(() => {
    if (!state.live) setConfirmStop(false)
  }, [state.live])

  // Detecta fala em microfone mudo via telemetria Rust ou níveis locais do mixer
  useEffect(() => {
    let speakSince = 0
    const timer = window.setInterval(() => {
      if (telemetry.micMutedSpeaking) {
        setLocalMutedSpeaking(true)
        return
      }
      const levels = engine.readLevels()
      const refLevels = studioStore.audioLevelsRef
      const mutedMics = settings.audioMixer.channels.filter((c) => c.kind === 'mic' && c.muted)
      const speaking = mutedMics.some((c) => {
        const refSample = refLevels[c.id]
        if (refSample?.mutedSpeech) return true
        const inputDb = refSample?.inputDb ?? levels[c.id]?.inputDb ?? -60
        return inputDb > -35
      })
      const t = Date.now()
      if (speaking) {
        if (speakSince === 0) speakSince = t
        if (t - speakSince >= 1100) setLocalMutedSpeaking(true)
      } else {
        speakSince = 0
        setLocalMutedSpeaking(false)
      }
    }, 350)
    return () => window.clearInterval(timer)
  }, [engine, settings.audioMixer.channels, telemetry.micMutedSpeaking])

  const connected = settings.destinations.filter((d) => d.hasKey && d.server.trim())

  const stop = async (): Promise<void> => {
    setConfirmStop(false)
    await engine.stopLive()
    toast('Live encerrada')
  }

  const toggleDest = (id: string): void => {
    update({
      destinations: settings.destinations.map((d) => (d.id === id ? { ...d, enabled: !d.enabled } : d))
    })
  }

  const unmuteAllMics = (): void => {
    update({
      audioMixer: {
        ...settings.audioMixer,
        channels: settings.audioMixer.channels.map((c) => (c.kind === 'mic' ? { ...c, muted: false } : c))
      }
    })
    setLocalMutedSpeaking(false)
    toast('Microfone ligado', 'good')
  }

  const encoderName = state.multistream.encoder && state.multistream.encoder !== '…'
    ? state.multistream.encoder
    : telemetry.encoder || 'libx264'
  const isCpuEncoder = telemetry.cpuFallback || encoderName === 'libx264'
  const realFps = telemetry.compositorFps > 0 ? Math.round(telemetry.compositorFps) : studio.fps
  const bitrateKbps = telemetry.encodeBitrateKbps
  const dropped = telemetry.droppedFrames
  const bufferedSec = Math.round(
    telemetry.replayBufferedSeconds > 0
      ? telemetry.replayBufferedSeconds
      : state.recorder.replayBufferedSeconds
  )

  const vcam = state.virtualCamStatus.active ? state.virtualCamStatus : vcamStore
  const vcamActive = Boolean(vcam.active)
  const zeroCopyActive = Boolean(
    telemetry.zeroCopyActive || state.telemetry.zeroCopyActive || gpuStatus.zeroCopyActive
  )
  const abrActive = Boolean(
    telemetry.abrActive || state.telemetry.abrActive || (state.live && abrTelemetry.length > 0)
  )
  const abrTargetKbps =
    telemetry.abrTargetBitrateKbps ??
    state.telemetry.abrTargetBitrateKbps ??
    abrTelemetry.reduce((sum, d) => sum + d.currentBitrateKbps, 0)
  const abrCongestion = Math.max(
    telemetry.abrCongestionLevel ?? state.telemetry.abrCongestionLevel ?? 0,
    abrTelemetry.reduce((m, d) => Math.max(m, d.congestionLevel), 0)
  )
  const abrStateBad =
    abrCongestion >= 0.6 || abrTelemetry.some((d) => d.state === 'congested' || d.state === 'floor')
  const abrStateWarn =
    !abrStateBad && (abrCongestion >= 0.25 || abrTelemetry.some((d) => d.state === 'recovering'))
  const abrColor = abrStateBad ? 'var(--danger)' : abrStateWarn ? 'var(--warning)' : 'var(--live)'
  const abrLabel =
    abrTargetKbps >= 1000
      ? `${(abrTargetKbps / 1000).toFixed(1)} Mb/s`
      : abrTargetKbps > 0
        ? `${abrTargetKbps} kb/s`
        : 'ativo'

  const telemetryTone = !state.multistream.ffmpegFound || dropped > 30
    ? 'is-bad'
    : isCpuEncoder || (state.live && realFps < studio.fps * 0.85)
      ? 'is-warn'
      : ''

  const camHk = formatHotkey(settings.hotkeys?.toggleCamera, 'Ctrl+Shift+C')
  const replayHk = formatHotkey(settings.hotkeys?.saveReplayClip, 'Ctrl+Shift+S')

  return (
    <footer className="controlbar">
      {(telemetry.micMutedSpeaking || localMutedSpeaking) && (
        <div className="cb-mic-alert" role="status">
          <IconMicOff size={14} />
          <span>Seu microfone está mudo — ouvimos atividade na entrada</span>
          <Button size="sm" variant="primary" onClick={unmuteAllMics}>
            Desmutar
          </Button>
        </div>
      )}

      <div className="cb-left">
        <LiveTitle onOpenDestinations={onOpenDestinations} />
        <div className="cb-destinations">
          {!state.multistream.ffmpegFound && (
            <button
              className="cb-telemetry is-bad"
              onClick={onOpenDestinations}
              title="FFmpeg não encontrado — clique para ver como instalar ou configurar o caminho"
            >
              <IconAlert size={13} />
              <span>FFmpeg não encontrado · configurar</span>
            </button>
          )}
          {connected.length === 0 ? (
            <button className="cb-dest is-empty" onClick={onOpenDestinations}>
              <IconPlus size={13} /> Conectar Twitch, YouTube…
            </button>
          ) : (
            connected.map((d) => {
              const st = state.multistream.destinations.find((s) => s.id === d.id)
              const status =
                state.live && d.enabled
                  ? (st?.state ?? 'connecting')
                  : st?.state === 'error'
                    ? 'error'
                    : 'idle'
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

          {state.multistream.ffmpegFound && (
            <span
              className={`cb-telemetry mono ${telemetryTone}`}
              title={
                isCpuEncoder
                  ? `Codificando por software (${encoderName}) · ${realFps}/${studio.fps} fps · quadros perdidos: ${dropped}`
                  : `Encoder de hardware (${encoderName}) · ${realFps}/${studio.fps} fps · quadros perdidos: ${dropped}`
              }
            >
              <span className="cb-telemetry-dot" />
              <span>{isCpuEncoder ? 'CPU (x264)' : encoderName}</span>
              {zeroCopyActive && (
                <span
                  style={{
                    padding: '1px 6px',
                    borderRadius: 4,
                    background: 'var(--live-soft)',
                    color: 'var(--live)',
                    fontWeight: 650,
                    fontSize: 10.5
                  }}
                  title={`Pipeline GPU Zero-Copy ativo (${telemetry.compositorBackend || gpuStatus.compositorBackend})`}
                >
                  GPU Zero-Copy
                </span>
              )}
              <span>· {realFps}/{studio.fps}fps</span>
              {bitrateKbps > 0 && (
                <span>
                  · {bitrateKbps >= 1000 ? `${(bitrateKbps / 1000).toFixed(1)} Mb/s` : `${bitrateKbps} kb/s`}
                </span>
              )}
              {abrActive && (
                <span
                  style={{
                    display: 'inline-flex',
                    alignItems: 'center',
                    gap: 4,
                    color: abrColor,
                    fontWeight: 650
                  }}
                  title={`Bitrate Dinâmico (ABR): alvo ${abrLabel} · congestionamento ${Math.round(abrCongestion * 100)}%`}
                >
                  · ABR {abrLabel}
                </span>
              )}
              {dropped > 0 && <span>· {dropped} drops</span>}
            </span>
          )}
        </div>
      </div>

      <div className="cb-actions">
        <Button
          size="sm"
          variant={vcamActive ? 'live' : 'ghost'}
          icon={<IconCamera size={15} />}
          disabled={togglingVcam}
          onClick={() => {
            if (togglingVcam) return
            setTogglingVcam(true)
            void engine
              .toggleVirtualCam()
              .then((res) => {
                if (!res.ok) toast(res.error ?? 'Não foi possível alternar a Câmera Virtual', 'bad')
                else if (res.active) toast('Câmera Virtual ativada', 'good')
                else toast('Câmera Virtual parada', 'info')
              })
              .finally(() => setTogglingVcam(false))
          }}
          title={
            vcamActive
              ? `Câmera Virtual ativa (${vcam.device || vcam.backend || 'PrismCast'} · ${vcam.width}x${vcam.height}@${vcam.fps}fps) — clique para parar`
              : 'Publica o palco como webcam virtual para Discord, Zoom, Meet e OBS'
          }
        >
          {vcamActive ? 'Cam Virtual · ON' : 'Cam Virtual'}
        </Button>
        <Button
          size="sm"
          variant={studio.cameraEnabled ? 'ghost' : 'danger'}
          icon={studio.cameraEnabled ? <IconCamera size={15} /> : <IconCameraOff size={15} />}
          onClick={() => update({ studio: { ...studio, cameraEnabled: !studio.cameraEnabled } })}
          title={`Sem câmera hoje? Desliga a câmera e esconde as molduras dos overlays (${camHk})`}
        >
          {studio.cameraEnabled ? 'Câmera' : 'Sem câmera'}
        </Button>
        <PauseButton />
        <Button
          size="sm"
          variant="ghost"
          icon={<IconReplay size={15} />}
          onClick={() => void engine.saveReplay().then((r) => toast(r.message, r.ok ? 'good' : 'bad'))}
          title={`Salva os últimos ${studio.replayBufferSeconds}s (${replayHk})`}
        >
          {bufferedSec > 0 && (state.live || state.recorder.recording) ? `Replay · ${bufferedSec}s` : 'Replay'}
        </Button>
        <Button
          size="sm"
          variant={state.recorder.recording ? 'danger' : 'subtle'}
          icon={<IconRecord size={15} />}
          disabled={togglingRec}
          onClick={() => {
            if (togglingRec) return
            setTogglingRec(true)
            const wasRecording = state.recorder.recording
            const path = state.recorder.recordingPath
            void engine
              .toggleRecording()
              .then((err) => {
                if (err) toast(err, 'bad')
                else if (wasRecording) {
                  toast(
                    'Gravação salva',
                    'good',
                    path ? { label: 'Abrir pasta', run: () => window.api.app.showInFolder(path) } : undefined
                  )
                }
              })
              .finally(() => setTogglingRec(false))
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
