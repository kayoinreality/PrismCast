import { useState } from 'react'
import type { TransitionType } from '@shared/protocol'
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
  const { settings, update, toast, pushUndoSnapshot, undo } = useStudio()
  const live = useEngineState().live
  const st = settings.scenes
  const current = editingScene(st)
  const [renaming, setRenaming] = useState<string | null>(null)
  const [dragId, setDragId] = useState<string | null>(null)
  const [dropTarget, setDropTarget] = useState<{ id: string; pos: 'top' | 'bottom' } | null>(null)

  const set = (next: typeof st): void => update({ scenes: next })

  const add = (): void => {
    const mic = settings.audioMixer.channels.find((c) => c.kind === 'mic')
    const next = createScene(st, `Cena ${st.scenes.length + 1}`, mic?.id)
    set(next)
    setRenaming(next.scenes[next.scenes.length - 1].id)
    onSelectScene()
  }

  const reorderScene = (fromId: string, toId: string, pos: 'top' | 'bottom'): void => {
    if (fromId === toId) return
    const fromIdx = st.scenes.findIndex((s) => s.id === fromId)
    const toIdx = st.scenes.findIndex((s) => s.id === toId)
    if (fromIdx < 0 || toIdx < 0) return
    const nextScenes = [...st.scenes]
    const [moved] = nextScenes.splice(fromIdx, 1)
    let insertIdx = nextScenes.findIndex((s) => s.id === toId)
    if (pos === 'bottom') insertIdx += 1
    nextScenes.splice(insertIdx, 0, moved)
    set({ ...st, scenes: nextScenes })
  }

  return (
    <div className="dock">
      <div className="dock-head">
        <span>Cenas</span>
        <div className="row" style={{ gap: 4 }}>
          <select
            className="dock-select"
            value={st.transitionType === 'fade_to_black' ? 'fade-to-black' : st.transitionType}
            onChange={(e) => set({ ...st, transitionType: e.target.value as TransitionType })}
            title="Transição ao trocar de cena"
          >
            <option value="fade">Esmaecer</option>
            <option value="cut">Corte</option>
            <option value="fade-to-black">Escurecer</option>
            <option value="slide">Deslizar</option>
          </select>
          {st.transitionType !== 'cut' && (
            <select
              className="dock-select"
              value={st.transitionDurationMs || 300}
              onChange={(e) => set({ ...st, transitionDurationMs: Number(e.target.value) || 300 })}
              title="Duração da transição"
            >
              <option value={150}>150ms</option>
              <option value={300}>300ms</option>
              <option value={500}>500ms</option>
              <option value={800}>800ms</option>
            </select>
          )}
        </div>
      </div>
      <div className="dock-list">
        {st.scenes.map((sc, i) => {
          const isLive = Boolean(live)
          const isProgram = sc.id === st.activeSceneId
          const onAir = isLive && isProgram
          const inPreview = st.studioMode && sc.id === st.previewSceneId
          const dragClass =
            dropTarget?.id === sc.id
              ? dropTarget.pos === 'top'
                ? 'drag-over-top'
                : 'drag-over-bottom'
              : ''

          return (
            <div
              key={sc.id}
              draggable={renaming !== sc.id}
              className={`dock-row scene-row ${current?.id === sc.id ? 'is-selected' : ''} ${dragClass}`}
              onClick={() => {
                set(selectScene(st, sc.id))
                onSelectScene()
              }}
              onDoubleClick={() => setRenaming(sc.id)}
              onDragStart={(e) => {
                setDragId(sc.id)
                e.dataTransfer.effectAllowed = 'move'
              }}
              onDragOver={(e) => {
                if (!dragId || dragId === sc.id) return
                e.preventDefault()
                const r = e.currentTarget.getBoundingClientRect()
                const pos = e.clientY - r.top < r.height / 2 ? 'top' : 'bottom'
                setDropTarget({ id: sc.id, pos })
              }}
              onDragLeave={() => {
                if (dropTarget?.id === sc.id) setDropTarget(null)
              }}
              onDrop={(e) => {
                e.preventDefault()
                if (dragId && dropTarget) {
                  reorderScene(dragId, sc.id, dropTarget.pos)
                }
                setDragId(null)
                setDropTarget(null)
              }}
              onDragEnd={() => {
                setDragId(null)
                setDropTarget(null)
              }}
              title={
                i < 9
                  ? `Tecla ${i + 1} troca para esta cena · arraste para reordenar · duplo clique renomeia`
                  : 'Arraste para reordenar · duplo clique renomeia'
              }
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
              {!onAir && isProgram && st.studioMode && <span className="tag">programa</span>}
              {inPreview && !onAir && (!isProgram || !st.studioMode) && <span className="tag">prévia</span>}
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
        <IconButton
          label="Subir"
          disabled={!current || st.scenes[0]?.id === current.id}
          onClick={() => current && set(moveScene(st, current.id, -1))}
        >
          <IconUp size={16} />
        </IconButton>
        <IconButton
          label="Descer"
          disabled={!current || st.scenes[st.scenes.length - 1]?.id === current.id}
          onClick={() => current && set(moveScene(st, current.id, 1))}
        >
          <IconDown size={16} />
        </IconButton>
        <div className="spacer" />
        <IconButton
          label="Apagar cena"
          variant="danger"
          disabled={st.scenes.length <= 1 || !current || (live && current.id === st.activeSceneId)}
          onClick={() => {
            if (!current) return
            pushUndoSnapshot()
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
