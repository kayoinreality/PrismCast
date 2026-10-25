import { useState } from 'react'
import { IconButton } from '@/components/ui'
import { IconCopy, IconDown, IconPlus, IconTrash, IconUp } from '@/components/Icons'
import { useEngineState, useStudio } from '@/studio/StudioContext'
import {
  createScene,
  deleteScene,
  duplicateScene,
  editingScene,
  moveScene,
  renameScene,
  selectScene
} from '@/studio/scenes'

export function ScenesDock({ onSelectScene }: { onSelectScene: () => void }): React.JSX.Element {
  const { settings, update, toast, undo } = useStudio()
  const live = useEngineState().live
  const st = settings.scenes
  const current = editingScene(st)
  const [renaming, setRenaming] = useState<string | null>(null)

  const set = (next: typeof st): void => update({ scenes: next })

  const add = (): void => {
    const next = createScene(st, `Cena ${st.scenes.length + 1}`)
    set(next)
    setRenaming(next.scenes[next.scenes.length - 1].id)
    onSelectScene()
  }

  return (
    <div className="dock">
      <div className="dock-head">
        <span>Cenas</span>
        <select
          className="dock-select"
          value={st.transitionType}
          onChange={(e) => set({ ...st, transitionType: e.target.value as 'cut' | 'fade' })}
          title="Transição ao trocar de cena"
        >
          <option value="fade">Esmaecer</option>
          <option value="cut">Corte</option>
        </select>
      </div>
      <div className="dock-list">
        {st.scenes.map((sc, i) => {
          const onAir = sc.id === st.activeSceneId
          const inPreview = st.studioMode && sc.id === st.previewSceneId
          return (
            <div
              key={sc.id}
              className={`dock-row scene-row ${current?.id === sc.id ? 'is-selected' : ''}`}
              onClick={() => {
                set(selectScene(st, sc.id))
                onSelectScene()
              }}
              onDoubleClick={() => setRenaming(sc.id)}
              title={i < 9 ? `Tecla ${i + 1} troca para esta cena · duplo clique renomeia` : 'Duplo clique renomeia'}
            >
              <span className="scene-key mono">{i + 1}</span>
              {renaming === sc.id ? (
                <input
                  className="dock-rename"
                  defaultValue={sc.name}
                  autoFocus
                  onFocus={(e) => e.target.select()}
                  onClick={(e) => e.stopPropagation()}
                  onBlur={(e) => {
                    set(renameScene(st, sc.id, e.target.value))
                    setRenaming(null)
                  }}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
                    if (e.key === 'Escape') setRenaming(null)
                  }}
                />
              ) : (
                <span className="dock-name">{sc.name}</span>
              )}
              {onAir && <span className="tag tag-onair">no ar</span>}
              {inPreview && !onAir && <span className="tag">prévia</span>}
            </div>
          )
        })}
      </div>
      <div className="dock-tools">
        <IconButton label="Nova cena" onClick={add}>
          <IconPlus size={16} />
        </IconButton>
        <IconButton label="Duplicar cena" onClick={() => current && set(duplicateScene(st, current.id))}>
          <IconCopy size={15} />
        </IconButton>
        <IconButton label="Subir" onClick={() => current && set(moveScene(st, current.id, -1))}>
          <IconUp size={16} />
        </IconButton>
        <IconButton label="Descer" onClick={() => current && set(moveScene(st, current.id, 1))}>
          <IconDown size={16} />
        </IconButton>
        <div className="spacer" />
        <IconButton
          label="Apagar cena"
          variant="danger"
          disabled={st.scenes.length <= 1 || !current || (live && current.id === st.activeSceneId)}
          onClick={() => {
            if (!current) return
            set(deleteScene(st, current.id))
            toast(`Cena “${current.name}” apagada`, 'info', { label: 'Desfazer', run: () => undo() })
          }}
        >
          <IconTrash size={15} />
        </IconButton>
      </div>
    </div>
  )
}
