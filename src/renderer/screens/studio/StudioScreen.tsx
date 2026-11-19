import { useCallback, useEffect, useRef, useState } from 'react'
import type { SceneItem, StudioScenesState } from '@shared/protocol'
import { useStudio } from '@/studio/StudioContext'
import { editingScene, removeItem, selectScene, updateItem } from '@/studio/scenes'
import { ChatPanel } from '@/chat/ChatPanel'
import { Button } from '@/components/ui'
import { IconChat, IconSliders } from '@/components/Icons'
import { Stage, type StageView } from './Stage'
import { ScenesDock } from './ScenesDock'
import { SourcesDock } from './SourcesDock'
import { MixerDock } from './MixerDock'
import { Inspector } from './Inspector'
import { ControlBar } from './ControlBar'
import './Studio.css'

const DOCK_KEY = 'prismcast.dockHeight'

function readDockHeight(): number {
  try {
    return Number(localStorage.getItem(DOCK_KEY)) || 230
  } catch {
    return 230
  }
}

/** Puxador entre o palco e os docks: o tamanho fica salvo e usa `--dock-h`. */
function useDockResize(): [number, (e: React.PointerEvent) => void] {
  const [height, setHeight] = useState(readDockHeight)
  const cleanupRef = useRef<(() => void) | null>(null)

  useEffect(() => {
    return () => {
      cleanupRef.current?.()
    }
  }, [])

  const onDown = (e: React.PointerEvent): void => {
    cleanupRef.current?.()
    const startY = e.clientY
    const startH = height

    const move = (ev: PointerEvent): void => {
      const max = Math.round(window.innerHeight * 0.55)
      setHeight(Math.max(160, Math.min(max, startH - (ev.clientY - startY))))
    }

    const up = (): void => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      window.removeEventListener('pointercancel', up)
      cleanupRef.current = null
      setHeight((h) => {
        try {
          localStorage.setItem(DOCK_KEY, String(h))
        } catch {
          /* sem armazenamento */
        }
        return h
      })
    }

    cleanupRef.current = up
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
    window.addEventListener('pointercancel', up)
  }

  return [height, onDown]
}

export function StudioScreen({ onOpenDestinations }: { onOpenDestinations: () => void }): React.JSX.Element {
  const { settings, update, toast, pushUndoSnapshot, undo, redo } = useStudio()
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [view, setView] = useState<StageView>('horizontal')
  const [side, setSide] = useState<'chat' | 'props'>('chat')
  const [popout, setPopout] = useState(false)
  const [dockHeight, onSplitDown] = useDockResize()

  const scenes = settings.scenes
  const scene = editingScene(scenes)
  const selected = scene?.items.find((it) => it.id === selectedId) ?? null
  const vertical = settings.studio.dualOutputEnabled

  // Limpa selectedId órfão caso a fonte tenha sido removida por Undo/Redo ou troca de cena.
  useEffect(() => {
    if (selectedId && !selected) {
      setSelectedId(null)
    }
  }, [selectedId, selected])

  useEffect(() => {
    if (!vertical && view === 'vertical') setView('horizontal')
  }, [vertical, view])

  useEffect(() => setSide(selectedId ? 'props' : 'chat'), [selectedId])

  useEffect(() => {
    let alive = true
    void window.api.chat
      .isPopoutOpen()
      .then((open) => {
        if (alive) setPopout(open)
      })
      .catch(() => {})
    const off = window.api.chat.onPopoutChange(setPopout)
    return () => {
      alive = false
      off()
    }
  }, [])

  const setScenes = useCallback((next: StudioScenesState) => update({ scenes: next }), [update])

  const patchItem = useCallback(
    (itemId: string, patch: Partial<SceneItem>) => {
      if (!scene) return
      setScenes(updateItem(settings.scenes, scene.id, itemId, patch))
    },
    [scene, settings.scenes, setScenes]
  )

  const removeSelected = useCallback(() => {
    if (!selected || !scene) return
    pushUndoSnapshot()
    setScenes(removeItem(settings.scenes, scene.id, selected.id))
    setSelectedId(null)
    toast(`“${selected.name}” removida`, 'info', { label: 'Desfazer', run: () => undo() })
  }, [selected, scene, settings.scenes, setScenes, toast, pushUndoSnapshot, undo])

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      const target = e.target as HTMLElement | null
      if (target?.closest?.('input, textarea, select, [contenteditable]')) return
      if (document.querySelector('.modal-overlay')) return
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'z') {
        e.preventDefault()
        const done = e.shiftKey ? redo() : undo()
        if (!done) toast(e.shiftKey ? 'Nada para refazer' : 'Nada para desfazer')
        return
      }
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'y') {
        e.preventDefault()
        redo()
        return
      }
      if (e.ctrlKey || e.metaKey || e.altKey) return
      if (e.key === 'Delete' && selected && !selected.locked) removeSelected()
      if (e.key === 'Escape') setSelectedId(null)
      if (/^[1-9]$/.test(e.key)) {
        const targetScene = settings.scenes.scenes[Number(e.key) - 1]
        if (targetScene) {
          setScenes(selectScene(settings.scenes, targetScene.id))
          setSelectedId(null)
        }
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [selected, removeSelected, settings.scenes, setScenes, undo, redo, toast])

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
          onRequestRename={(id) => {
            setSelectedId(id)
            setSide('props')
          }}
        />
        <div className="dock-split" onPointerDown={onSplitDown} title="Arraste para mudar o tamanho" />
        <div className="docks" style={{ ['--dock-h' as string]: `${dockHeight}px` }}>
          <ScenesDock onSelectScene={() => setSelectedId(null)} />
          <SourcesDock scene={scene} selectedId={selectedId} onSelect={setSelectedId} />
          <MixerDock />
        </div>
      </section>
      <aside className="studio-side">
        <div className="side-tabs" role="tablist">
          <button
            role="tab"
            aria-selected={side === 'chat'}
            className={side === 'chat' ? 'is-active' : ''}
            onClick={() => setSide('chat')}
          >
            <IconChat size={14} /> Chat
          </button>
          <button
            role="tab"
            aria-selected={side === 'props'}
            className={side === 'props' ? 'is-active' : ''}
            onClick={() => setSide('props')}
          >
            <IconSliders size={14} /> Propriedades
            {selected && <span className="side-tab-dot" />}
          </button>
        </div>

        {side === 'props' && (
          <Inspector
            scene={scene}
            item={selected}
            view={view}
            onPatch={patchItem}
            onClose={() => setSelectedId(null)}
          />
        )}

        {side === 'chat' && popout && (
          <div className="chat-empty">
            <IconChat size={26} />
            <span>O chat está aberto na janela separada.</span>
            <Button size="sm" onClick={() => window.api.chat.closePopout()}>
              Trazer de volta para cá
            </Button>
          </div>
        )}

        {/* Mantém o ChatPanel montado (oculto em Propriedades) para não perder o texto digitado. */}
        {!popout && (
          <div
            style={{
              display: side === 'chat' ? 'flex' : 'none',
              flexDirection: 'column',
              flex: 1,
              minHeight: 0
            }}
          >
            <ChatPanel settings={settings} update={update} variant="dock" onReloginTwitch={onOpenDestinations} />
          </div>
        )}
      </aside>
      <ControlBar onOpenDestinations={onOpenDestinations} />
    </div>
  )
}
