import { useEffect, useState, type ReactNode } from 'react'
import { IconBroadcast, IconClose, IconMaximize, IconMinimize, IconRestore } from './Icons'
import './Titlebar.css'

/**
 * Barra de titulo propria do PrismCast Studio (a janela e `frame: false`).
 */
export function Titlebar({
  center,
  onRequestClose
}: {
  center?: ReactNode
  onRequestClose?: () => void
}): React.JSX.Element {
  const [maximized, setMaximized] = useState(false)

  useEffect(() => {
    const appApi = window.api.app as typeof window.api.app & {
      isMaximized?: () => Promise<boolean>
    }
    if (typeof appApi.isMaximized === 'function') {
      void appApi.isMaximized().then(setMaximized).catch(() => {})
    }
    return window.api.app.onMaximizeChange(setMaximized)
  }, [])

  return (
    <div className="titlebar">
      <div className="titlebar-brand">
        <IconBroadcast size={16} />
        <span>PrismCast</span>
      </div>

      <div className="titlebar-center">{center}</div>

      <div className="titlebar-controls">
        <button
          className="win-btn"
          onClick={() => window.api.app.minimize()}
          aria-label="Minimizar"
          title="Minimizar"
        >
          <IconMinimize size={15} />
        </button>
        <button
          className="win-btn"
          onClick={() => window.api.app.toggleMaximize()}
          aria-label={maximized ? 'Restaurar' : 'Maximizar'}
          title={maximized ? 'Restaurar' : 'Maximizar'}
        >
          {maximized ? <IconRestore size={15} /> : <IconMaximize size={14} />}
        </button>
        <button
          className="win-btn win-close"
          onClick={() => (onRequestClose ? onRequestClose() : window.api.app.close())}
          aria-label="Fechar"
          title="Fechar"
        >
          <IconClose size={15} />
        </button>
      </div>
    </div>
  )
}
