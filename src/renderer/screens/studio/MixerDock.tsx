import { useEffect, useRef, useState } from 'react'
import type { AudioChannel, AudioChannelKind } from '@shared/protocol'
import { dbToLinear, linearToDb, makeChannel, MAX_DB, MIN_DB } from '@shared/audio'
import { IconButton } from '@/components/ui'
import {
  IconAlert,
  IconApp,
  IconHeadphones,
  IconMic,
  IconMicOff,
  IconMonitor,
  IconPlus,
  IconSliders,
  IconVolume,
  IconVolumeOff
} from '@/components/Icons'
import { useEngineState, useStudio } from '@/studio/StudioContext'
import { AppPickerModal, AudioPropertiesModal } from './AudioProperties'

/** Fader em posição 0..1 com curva de volume (como o OBS): o meio do curso é onde se mexe. */
const MAX_LIN = dbToLinear(MAX_DB)
export const faderToDb = (pos: number): number => linearToDb(Math.pow(pos, 3) * MAX_LIN)
export const dbToFader = (db: number): number => (db <= MIN_DB ? 0 : Math.cbrt(dbToLinear(db) / MAX_LIN))

export function formatDb(db: number): string {
  if (db <= MIN_DB) return '-∞ dB'
  return `${db > 0 ? '+' : ''}${db.toFixed(1)} dB`
}

const meterPct = (db: number): number => Math.max(0, Math.min(1, (db - MIN_DB) / -MIN_DB))

interface MeterEls {
  fill: HTMLDivElement | null
  peak: HTMLDivElement | null
  hold: number
  holdAt: number
}

/**
 * Medidores atualizados direto no DOM, num laço só: 20 re-renders por segundo
 * do React por canal seriam desperdício.
 */
export function useMeterLoop(): (id: string, kind: 'fill' | 'peak', el: HTMLDivElement | null) => void {
  const { engine } = useStudio()
  const els = useRef(new Map<string, MeterEls>())
  useEffect(() => {
    let raf = 0
    let last = 0
    const loop = (t: number): void => {
      raf = requestAnimationFrame(loop)
      if (t - last < 40) return
      last = t
      const levels = engine.readLevels()
      for (const [key, m] of els.current) {
        // "canal|lugar": o mesmo canal pode ter medidor no dock e na janela de filtros.
        const lv = levels[key.split('|')[0]]
        const db = lv ? lv.peakDb : MIN_DB
        if (m.fill) m.fill.style.clipPath = `inset(0 ${(1 - meterPct(db)) * 100}% 0 0)`
        // Pico segura 1,2 s e depois cai devagar.
        if (db >= m.hold || t - m.holdAt > 1200) {
          m.hold = db >= m.hold ? db : Math.max(db, m.hold - 1.5)
          if (db >= m.hold) m.holdAt = t
        }
        if (m.peak) {
          m.peak.style.left = `${meterPct(m.hold) * 100}%`
          m.peak.classList.toggle('is-clip', m.hold > -1)
          m.peak.style.opacity = m.hold <= MIN_DB + 1 ? '0' : '1'
        }
      }
    }
    raf = requestAnimationFrame(loop)
    return () => cancelAnimationFrame(raf)
  }, [engine])
  // Função estável: ref callback nova a cada render zeraria o pico segurado.
  const [register] = useState(() => (id: string, kind: 'fill' | 'peak', el: HTMLDivElement | null): void => {
    const m = els.current.get(id) ?? { fill: null, peak: null, hold: MIN_DB, holdAt: 0 }
    m[kind] = el
    if (!m.fill && !m.peak) els.current.delete(id)
    else els.current.set(id, m)
  })
  return register
}

export function Meter({
  id,
  register,
  muted
}: {
  id: string
  register: ReturnType<typeof useMeterLoop>
  muted?: boolean
}): React.JSX.Element {
  return (
    <div className={`vu ${muted ? 'is-muted' : ''}`}>
      <div className="vu-fill" ref={(el) => register(id, 'fill', el)} />
      <div className="vu-peak" ref={(el) => register(id, 'peak', el)} />
      <div className="vu-ticks" aria-hidden="true">
        {[-48, -36, -24, -12, -6].map((db) => (
          <i key={db} style={{ left: `${meterPct(db) * 100}%` }} />
        ))}
      </div>
    </div>
  )
}

export function KindIcon({ kind, size = 15 }: { kind: AudioChannelKind; size?: number }): React.JSX.Element {
  if (kind === 'mic') return <IconMic size={size} />
  if (kind === 'app') return <IconApp size={size} />
  return <IconMonitor size={size} />
}

function Strip({
  ch,
  register,
  onChange,
  onOpen
}: {
  ch: AudioChannel
  register: ReturnType<typeof useMeterLoop>
  onChange: (patch: Partial<AudioChannel>) => void
  onOpen: () => void
}): React.JSX.Element {
  const { engine } = useStudio()
  useEngineState()
  const status = engine.audioStatus(ch.id)
  const problem = status.state === 'error' || status.state === 'waiting' ? status.message : null
  const MuteIcon = ch.kind === 'mic' ? (ch.muted ? IconMicOff : IconMic) : ch.muted ? IconVolumeOff : IconVolume

  return (
    <div
      className={`strip ${ch.muted ? 'is-muted' : ''} ${problem ? 'has-problem' : ''}`}
      onContextMenu={(e) => {
        e.preventDefault()
        onOpen()
      }}
    >
      <div className="strip-head">
        <span className="strip-kind">
          <KindIcon kind={ch.kind} size={14} />
        </span>
        <span className="strip-name" title={ch.appExe ?? ch.name}>
          {ch.name}
        </span>
        {ch.monitor !== 'off' && (
          <span className="strip-flag" title={ch.monitor === 'monitor-only' ? 'Só no fone (não vai para a live)' : 'Você ouve no fone'}>
            <IconHeadphones size={12} />
          </span>
        )}
        <span className="strip-db mono">{ch.muted ? 'mudo' : formatDb(ch.volumeDb)}</span>
        <IconButton label={`Propriedades e filtros de ${ch.name}`} className="strip-gear" onClick={onOpen}>
          <IconSliders size={14} />
        </IconButton>
      </div>
      {problem ? (
        <button className="strip-problem" onClick={onOpen} title={problem}>
          <IconAlert size={12} /> {problem}
        </button>
      ) : (
        <Meter id={ch.id} register={register} muted={ch.muted} />
      )}
      <div className="strip-controls">
        <IconButton
          label={ch.muted ? `Ligar ${ch.name}` : `Mutar ${ch.name}`}
          variant={ch.muted ? 'danger' : 'plain'}
          active={ch.muted}
          onClick={() => onChange({ muted: !ch.muted })}
        >
          <MuteIcon size={15} />
        </IconButton>
        <input
          type="range"
          className="slider fader"
          min={0}
          max={1}
          step={0.001}
          value={dbToFader(ch.volumeDb)}
          style={{ ['--fill' as string]: `${dbToFader(ch.volumeDb) * 100}%` }}
          onChange={(e) => {
            const db = faderToDb(Number(e.target.value))
            // Encaixa em 0 dB: é onde quase todo mundo quer voltar.
            onChange({ volumeDb: Math.abs(db) < 0.6 ? 0 : Math.round(db * 10) / 10 })
          }}
          onDoubleClick={() => onChange({ volumeDb: 0 })}
          title="Duplo clique volta para 0 dB"
          aria-label={`Volume de ${ch.name}`}
        />
      </div>
    </div>
  )
}

export function MixerDock(): React.JSX.Element {
  const { settings, update, toast } = useStudio()
  const mixer = settings.audioMixer
  const register = useMeterLoop()
  const [menu, setMenu] = useState(false)
  const [editing, setEditing] = useState<string | null>(null)
  const [pickingApp, setPickingApp] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!menu) return
    const close = (e: MouseEvent): void => {
      if (!menuRef.current?.contains(e.target as Node)) setMenu(false)
    }
    window.addEventListener('mousedown', close)
    return () => window.removeEventListener('mousedown', close)
  }, [menu])

  const setChannels = (channels: AudioChannel[]): void => update({ audioMixer: { ...mixer, channels } })
  const patch = (id: string, p: Partial<AudioChannel>): void =>
    setChannels(mixer.channels.map((c) => (c.id === id ? { ...c, ...p } : c)))

  const add = (ch: AudioChannel): void => {
    setChannels([...mixer.channels, ch])
    setMenu(false)
    setEditing(ch.id)
  }

  const hasDesktop = mixer.channels.some((c) => c.kind === 'desktop')
  const editingCh = mixer.channels.find((c) => c.id === editing) ?? null

  return (
    <div className="dock">
      <div className="dock-head">
        <span>Áudio</span>
        <span className="dock-sub">clique direito: filtros</span>
      </div>
      <div className="dock-list mixer">
        {mixer.channels.length === 0 && (
          <button className="dock-empty" onClick={() => setMenu(true)}>
            <IconPlus size={15} /> Adicionar fonte de áudio
          </button>
        )}
        {mixer.channels.map((ch) => (
          <Strip key={ch.id} ch={ch} register={register} onChange={(p) => patch(ch.id, p)} onOpen={() => setEditing(ch.id)} />
        ))}
      </div>
      <div className="dock-tools" ref={menuRef}>
        <IconButton label="Adicionar fonte de áudio" active={menu} onClick={() => setMenu((m) => !m)}>
          <IconPlus size={16} />
        </IconButton>
        <span className="dock-tools-hint">Mic, áudio de um programa, PC</span>
        {menu && (
          <div className="add-menu" role="menu">
            <button className="add-menu-item" role="menuitem" onClick={() => add(makeChannel('mic', { name: 'Microfone' }))}>
              <IconMic size={16} />
              <span className="add-menu-text">
                <span>Microfone / entrada</span>
                <small>Outro mic, placa de captura, mesa</small>
              </span>
            </button>
            <button
              className="add-menu-item"
              role="menuitem"
              onClick={() => {
                setMenu(false)
                setPickingApp(true)
              }}
            >
              <IconApp size={16} />
              <span className="add-menu-text">
                <span>Áudio de um programa</span>
                <small>Só o jogo, o Spotify, o Discord…</small>
              </span>
            </button>
            {!hasDesktop && (
              <button
                className="add-menu-item"
                role="menuitem"
                onClick={() => add(makeChannel('desktop', { id: 'desktop' }))}
              >
                <IconMonitor size={16} />
                <span className="add-menu-text">
                  <span>Áudio do PC</span>
                  <small>Tudo que toca no computador</small>
                </span>
              </button>
            )}
          </div>
        )}
      </div>

      <AppPickerModal
        open={pickingApp}
        onClose={() => setPickingApp(false)}
        onPick={(app) => {
          setPickingApp(false)
          if (mixer.channels.some((c) => c.kind === 'app' && c.appExe?.toLowerCase() === app.exe.toLowerCase())) {
            toast(`${app.title} já está no mixer`)
            return
          }
          add(makeChannel('app', { name: app.title.slice(0, 40), appExe: app.exe }))
          if (hasDesktop) {
            toast('Dica: com “Áudio do PC” ligado, este programa vai sair duas vezes. Mute um dos dois.', 'info')
          }
        }}
      />
      {editingCh && (
        <AudioPropertiesModal
          channel={editingCh}
          register={register}
          onChange={(p) => patch(editingCh.id, p)}
          onRemove={() => {
            const removed = editingCh
            const index = mixer.channels.findIndex((c) => c.id === removed.id)
            setEditing(null)
            setChannels(mixer.channels.filter((c) => c.id !== removed.id))
            toast(`“${removed.name}” removido do mixer`, 'info', {
              label: 'Desfazer',
              run: () => {
                const list = [...settings.audioMixer.channels.filter((c) => c.id !== removed.id)]
                list.splice(index, 0, removed)
                setChannels(list)
              }
            })
          }}
          onClose={() => setEditing(null)}
        />
      )}
    </div>
  )
}
