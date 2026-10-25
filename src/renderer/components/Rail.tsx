import type { ReactNode } from 'react'
import { IconBroadcast, IconSettings, IconStudio, IconWidget, IconWifi } from './Icons'
import './Rail.css'

export type Page = 'studio' | 'widgets' | 'destinations' | 'lan' | 'lan-watch' | 'settings'

const ITEMS: Array<{ id: Page; label: string; icon: ReactNode }> = [
  { id: 'studio', label: 'Estúdio', icon: <IconStudio size={20} /> },
  { id: 'widgets', label: 'Widgets', icon: <IconWidget size={20} /> },
  { id: 'destinations', label: 'Destinos', icon: <IconBroadcast size={20} /> },
  { id: 'lan', label: 'Rede local', icon: <IconWifi size={20} /> }
]

export function Rail({
  page,
  onChange,
  live,
  lan
}: {
  page: Page
  onChange: (p: Page) => void
  live: boolean
  lan: boolean
}): React.JSX.Element {
  const current = page === 'lan-watch' ? 'lan' : page
  const button = (id: Page, label: string, icon: ReactNode, dot = false): React.JSX.Element => (
    <button
      key={id}
      className={`rail-btn ${current === id ? 'is-active' : ''}`}
      onClick={() => onChange(id)}
      aria-current={current === id ? 'page' : undefined}
    >
      <span className="rail-icon">
        {icon}
        {dot && <span className="rail-dot" />}
      </span>
      <span className="rail-label">{label}</span>
    </button>
  )

  return (
    <nav className="rail" aria-label="Seções">
      {ITEMS.map((it) =>
        button(it.id, it.label, it.icon, (it.id === 'destinations' && live) || (it.id === 'lan' && lan))
      )}
      <div className="rail-spacer" />
      {button('settings', 'Ajustes', <IconSettings size={20} />)}
    </nav>
  )
}
