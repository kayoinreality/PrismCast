import { useEffect, useState } from 'react'
import type { Destination } from '@shared/protocol'
import type { EngineState } from '@/studio/StudioEngine'
import './LiveStatus.css'

export function formatClock(totalSec: number): string {
  const h = Math.floor(totalSec / 3600)
  const m = Math.floor((totalSec % 3600) / 60)
  const s = totalSec % 60
  const pad = (n: number): string => String(n).padStart(2, '0')
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`
}

export function useNow(active: boolean): number {
  const [now, setNow] = useState(Date.now())
  useEffect(() => {
    if (!active) return
    const t = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(t)
  }, [active])
  return now
}

/** Pílula da barra de título: o estado da live sempre visível, em qualquer página. */
export function LiveStatus({
  state,
  destinations,
  lanViewers,
  onClick
}: {
  state: EngineState
  destinations: Destination[]
  lanViewers: number | null
  onClick: () => void
}): React.JSX.Element {
  const now = useNow(state.live || state.recorder.recording)
  const live = state.multistream.destinations.filter((d) => d.state === 'live' || d.state === 'connecting')
  const names = live
    .map((s) => destinations.find((d) => d.id === s.id)?.name)
    .filter(Boolean)
    .join(' · ')

  return (
    <button className={`live-status ${state.live ? 'is-live' : ''}`} onClick={onClick}>
      {state.live ? (
        <>
          <span className="live-status-dot" />
          <span className="live-status-label">AO VIVO</span>
          <span className="mono">{formatClock(Math.floor((now - (state.liveStartedAt ?? now)) / 1000))}</span>
          {names && <span className="live-status-dim">{names}</span>}
        </>
      ) : (
        <span className="live-status-dim">Fora do ar</span>
      )}
      {state.recorder.recording && (
        <span className="live-status-rec">
          <span className="live-status-dot" /> REC {formatClock(state.recorder.recordingDurationSec)}
        </span>
      )}
      {lanViewers !== null && <span className="live-status-dim">LAN · {lanViewers}</span>}
    </button>
  )
}
