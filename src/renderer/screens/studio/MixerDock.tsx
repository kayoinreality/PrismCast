import { useEffect, useRef } from 'react'
import type { AudioMixerSettings } from '@shared/protocol'
import { IconButton, Slider } from '@/components/ui'
import { IconMic, IconMicOff, IconVolume, IconVolumeOff } from '@/components/Icons'
import { useStudio } from '@/studio/StudioContext'

/** Medidor atualizado direto no DOM: 20 re-renders por segundo do React seriam desperdício. */
function useMeters(): { system: React.RefObject<HTMLDivElement | null>; mic: React.RefObject<HTMLDivElement | null> } {
  const { engine } = useStudio()
  const system = useRef<HTMLDivElement>(null)
  const mic = useRef<HTMLDivElement>(null)
  useEffect(() => {
    let raf = 0
    let last = 0
    const loop = (t: number): void => {
      raf = requestAnimationFrame(loop)
      if (t - last < 50) return
      last = t
      const lv = engine.readLevels()
      if (system.current) system.current.style.transform = `scaleX(${lv.system})`
      if (mic.current) mic.current.style.transform = `scaleX(${lv.mic})`
    }
    raf = requestAnimationFrame(loop)
    return () => cancelAnimationFrame(raf)
  }, [engine])
  return { system, mic }
}

function Channel({
  label,
  muted,
  volume,
  max,
  meter,
  onMute,
  onVolume,
  icon
}: {
  label: string
  muted: boolean
  volume: number
  max: number
  meter: React.RefObject<HTMLDivElement | null>
  onMute: () => void
  onVolume: (v: number) => void
  icon: React.ReactNode
}): React.JSX.Element {
  return (
    <div className={`mixer-channel ${muted ? 'is-muted' : ''}`}>
      <div className="mixer-head">
        <span>{label}</span>
        <span className="mono dim">{muted ? 'mudo' : `${Math.round(volume * 100)}%`}</span>
      </div>
      <div className="mixer-meter">
        <div className="mixer-meter-fill" ref={meter} />
      </div>
      <div className="mixer-controls">
        <IconButton label={muted ? 'Ligar' : 'Mutar'} variant={muted ? 'danger' : 'plain'} active={muted} onClick={onMute}>
          {icon}
        </IconButton>
        <Slider value={volume} min={0} max={max} step={0.01} onChange={onVolume} disabled={muted} />
      </div>
    </div>
  )
}

export function MixerDock(): React.JSX.Element {
  const { settings, update } = useStudio()
  const mixer = settings.audioMixer
  const meters = useMeters()
  const set = (patch: Partial<AudioMixerSettings>): void => update({ audioMixer: { ...mixer, ...patch } })

  return (
    <div className="dock">
      <div className="dock-head">
        <span>Áudio</span>
      </div>
      <div className="dock-list mixer">
        <Channel
          label="Áudio do PC"
          muted={mixer.systemMuted || !mixer.systemAudioEnabled}
          volume={mixer.systemVolume}
          max={1.5}
          meter={meters.system}
          icon={mixer.systemMuted ? <IconVolumeOff size={16} /> : <IconVolume size={16} />}
          onMute={() =>
            mixer.systemAudioEnabled ? set({ systemMuted: !mixer.systemMuted }) : set({ systemAudioEnabled: true, systemMuted: false })
          }
          onVolume={(v) => set({ systemVolume: v })}
        />
        {mixer.micEnabled ? (
          <Channel
            label="Microfone"
            muted={mixer.micMuted}
            volume={mixer.micVolume}
            max={2}
            meter={meters.mic}
            icon={mixer.micMuted ? <IconMicOff size={16} /> : <IconMic size={16} />}
            onMute={() => set({ micMuted: !mixer.micMuted })}
            onVolume={(v) => set({ micVolume: v })}
          />
        ) : (
          <button className="mixer-enable" onClick={() => set({ micEnabled: true, micMuted: false })}>
            <IconMic size={16} />
            <span>
              Ligar microfone
              <small>Escolha qual em Ajustes → Áudio</small>
            </span>
          </button>
        )}
      </div>
    </div>
  )
}
