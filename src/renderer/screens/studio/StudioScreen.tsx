import { useCallback, useEffect, useState } from 'react'
import type { SceneItem, StudioScenesState } from '@shared/protocol'
import { useStudio } from '@/studio/StudioContext'
import { editingScene, removeItem, updateItem } from '@/studio/scenes'
import { Stage, type StageView } from './Stage'
import { ScenesDock } from './ScenesDock'
import { SourcesDock } from './SourcesDock'
import { MixerDock } from './MixerDock'
import { Inspector } from './Inspector'
import { ControlBar } from './ControlBar'
import './Studio.css'

export function StudioScreen({ onOpenDestinations }: { onOpenDestinations: () => void }): React.JSX.Element {
  const { settings, update } = useStudio()
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [view, setView] = useState<StageView>('horizontal')

  const scenes = settings.scenes
  const scene = editingScene(scenes)
  const selected = scene?.items.find((it) => it.id === selectedId) ?? null
  const vertical = settings.studio.dualOutputEnabled

  useEffect(() => {
    if (!vertical && view === 'vertical') setView('horizontal')
  }, [vertical, view])

  const setScenes = useCallback((next: StudioScenesState) => update({ scenes: next }), [update])

  const patchItem = useCallback(
    (itemId: string, patch: Partial<SceneItem>) => {
      if (!scene) return
      setScenes(updateItem(settings.scenes, scene.id, itemId, patch))
    },
    [scene, settings.scenes, setScenes]
  )

  // Delete apaga a fonte selecionada (fora de campos de texto).
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      const target = e.target as HTMLElement
      if (target.closest('input, textarea, select, [contenteditable]')) return
      if (e.key === 'Delete' && selected && scene && !selected.locked) {
        setScenes(removeItem(settings.scenes, scene.id, selected.id))
        setSelectedId(null)
      }
      if (e.key === 'Escape') setSelectedId(null)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [selected, scene, settings.scenes, setScenes])

  return (
    <div className="studio">
      <section className="studio-main">
        <Stage
          scene={scene}
          view={view}
          onViewChange={setView}
          selectedId={selectedId}
          onSelect={setSelectedId}
          onPatchItem={patchItem}
        />
        <div className="docks">
          <ScenesDock onSelectScene={() => setSelectedId(null)} />
          <SourcesDock scene={scene} selectedId={selectedId} onSelect={setSelectedId} />
          <MixerDock />
        </div>
      </section>
      <aside className="studio-side">
        <Inspector scene={scene} item={selected} view={view} onPatch={patchItem} onClose={() => setSelectedId(null)} />
      </aside>
      <ControlBar onOpenDestinations={onOpenDestinations} />
    </div>
  )
}
