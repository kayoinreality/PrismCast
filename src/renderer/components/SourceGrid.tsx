import type { DesktopSource } from '@shared/ipc'
import { IconMonitor, IconWindow } from './Icons'
import './SourceGrid.css'

export function SourceGrid({
  sources,
  selectedId,
  onSelect,
  onDoubleClick
}: {
  sources: DesktopSource[]
  selectedId: string | null
  onSelect: (source: DesktopSource) => void
  onDoubleClick?: (source: DesktopSource) => void
}): React.JSX.Element {
  if (sources.length === 0) {
    return (
      <div className="empty-state">
        <IconWindow size={30} />
        <span className="empty-title">Nada para mostrar aqui</span>
        <span>Abra a janela que você quer compartilhar e atualize a lista.</span>
      </div>
    )
  }

  return (
    <div className="source-grid">
      {sources.map((source) => (
        <button
          key={source.id}
          type="button"
          className={`source-tile ${source.id === selectedId ? 'is-selected' : ''}`}
          onClick={() => onSelect(source)}
          onDoubleClick={() => {
            onSelect(source)
            onDoubleClick?.(source)
          }}
          title={source.name}
        >
          <span className="source-thumb">
            {source.thumbnail ? (
              <img src={source.thumbnail} alt="" draggable={false} />
            ) : (
              <span className="source-thumb-fallback">
                {source.kind === 'screen' ? <IconMonitor size={26} /> : <IconWindow size={26} />}
              </span>
            )}
          </span>
          <span className="source-meta">
            {source.appIcon ? (
              <img className="source-icon" src={source.appIcon} alt="" draggable={false} />
            ) : source.kind === 'screen' ? (
              <IconMonitor size={15} />
            ) : (
              <IconWindow size={15} />
            )}
            <span className="source-name">{source.name}</span>
          </span>
        </button>
      ))}
    </div>
  )
}
