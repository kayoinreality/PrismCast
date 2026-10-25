import { useState } from 'react'
import { PLATFORM_INFO } from '@shared/ipc'
import { Button } from '@/components/ui'
import { IconBroadcast, IconEyeOff, IconRecord, IconReplay, IconStop } from '@/components/Icons'
import { formatClock, useNow } from '@/components/LiveStatus'
import { useEngineState, useStudio } from '@/studio/StudioContext'

export function ControlBar({ onOpenDestinations }: { onOpenDestinations: () => void }): React.JSX.Element {
  const { engine, settings, update, toast } = useStudio()
  const state = useEngineState()
  const [confirmStop, setConfirmStop] = useState(false)
  const now = useNow(state.live)
  const studio = settings.studio

  const enabled = settings.destinations.filter((d) => d.enabled)

  const goLive = async (): Promise<void> => {
    if (enabled.length === 0) {
      toast('Conecte pelo menos uma plataforma antes de entrar ao vivo.', 'bad')
      onOpenDestinations()
      return
    }
    const err = await engine.goLive()
    if (err) toast(err, 'bad')
    else toast(`Ao vivo em ${enabled.map((d) => d.name).join(', ')}`, 'good')
  }

  const stop = async (): Promise<void> => {
    setConfirmStop(false)
    await engine.stopLive()
    toast('Live encerrada')
  }

  return (
    <footer className="controlbar">
      <div className="cb-destinations">
        {enabled.length === 0 ? (
          <button className="cb-dest is-empty" onClick={onOpenDestinations}>
            Nenhum destino conectado · <span className="link-btn">Conectar Twitch, YouTube…</span>
          </button>
        ) : (
          enabled.map((d) => {
            const st = state.multistream.destinations.find((s) => s.id === d.id)
            const status = state.live ? (st?.state ?? 'connecting') : st?.state === 'error' ? 'error' : 'idle'
            return (
              <button
                key={d.id}
                className={`cb-dest is-${status}`}
                onClick={onOpenDestinations}
                title={st?.error ?? `${PLATFORM_INFO[d.platform].label} · ${d.orientation === 'vertical' ? '9:16' : '16:9'}`}
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

      <div className="cb-actions">
        <Button
          size="sm"
          variant={studio.privacyShieldActive ? 'danger' : 'ghost'}
          icon={<IconEyeOff size={15} />}
          onClick={() => update({ studio: { ...studio, privacyShieldActive: !studio.privacyShieldActive } })}
          title="Esconde tudo atrás de uma tela de pausa (Ctrl+Shift+P)"
        >
          {studio.privacyShieldActive ? 'Tela de pausa ativa' : 'Tela de pausa'}
        </Button>
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
          onClick={() =>
            void engine.toggleRecording().then((err) => {
              if (err) toast(err, 'bad')
              else if (state.recorder.recording) toast('Gravação salva em Vídeos\\PrismCast', 'good')
            })
          }
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
          <button className="golive" onClick={() => void goLive()} disabled={state.goingLive}>
            {state.goingLive ? <span className="spinner" /> : <IconBroadcast size={17} />}
            <span>{state.goingLive ? 'Conectando…' : 'Entrar ao vivo'}</span>
          </button>
        )}
      </div>
    </footer>
  )
}
