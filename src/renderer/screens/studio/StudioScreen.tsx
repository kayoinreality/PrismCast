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
    return Number(localStorage.getItem(DOCK_KEY)) || 250
  } catch {
    return 250
  }
}

/** Puxador entre o palco e os docks: o tamanho fica salvo. */
function useDockResize(): [number, (e: React.PointerEvent) => void] {
  const [height, setHeight] = useState(readDockHeight)
  const start = useRef<{ y: number; h: number } | null>(null)
  const onDown = (e: React.PointerEvent): void => {
    start.current = { y: e.clientY, h: height }
    const move = (ev: PointerEvent): void => {
      if (!start.current) return
      const max = Math.round(window.innerHeight * 0.6)
      setHeight(Math.max(160, Math.min(max, start.current.h - (ev.clientY - start.current.y))))
    }
    const up = (): void => {
      start.current = null
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      setHeight((h) => {
        try {
          localStorage.setItem(DOCK_KEY, String(h))
        } catch {
          /* sem armazenamento: só não lembra */
        }
        return h
      })
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }
  return [height, onDown]
}

export function StudioScreen({ onOpenDestinations }: { onOpenDestinations: () => void }): React.JSX.Element {
  const { settings, update, toast, undo, redo } = useStudio()
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [view, setView] = useState<StageView>('horizontal')
  const [side, setSide] = useState<'chat' | 'props'>('chat')
  const [popout, setPopout] = useState(false)
  const [dockHeight, onSplitDown] = useDockResize()

  const scenes = settings.scenes
  const scene = editingScene(scenes)
  const selected = scene?.items.find((it) => it.id === selectedId) ?? null
  const vertical = settings.studio.dualOutputEnabled

  useEffect(() => {
    if (!vertical && view === 'vertical') setView('horizontal')
  }, [vertical, view])

  // Selecionar uma fonte mostra as propriedades; tirar a seleção volta para o chat.
  useEffect(() => setSide(selectedId ? 'props' : 'chat'), [selectedId])

  useEffect(() => {
    void window.api.chat.isPopoutOpen().then(setPopout)
    return window.api.chat.onPopoutChange(setPopout)
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
    setScenes(removeItem(settings.scenes, scene.id, selected.id))
    setSelectedId(null)
    toast(`“${selected.name}” removida`, 'info', { label: 'Desfazer', run: () => undo() })
  }, [selected, scene, settings.scenes, setScenes, toast, undo])

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
      // 1–9 trocam de cena (na prévia, no modo estúdio), como os números da lista.
      if (/^[1-9]$/.test(e.key)) {
        const target = settings.scenes.scenes[Number(e.key) - 1]
        if (target) {
          setScenes(selectScene(settings.scenes, target.id))
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
        />
        <div className="dock-split" onPointerDown={onSplitDown} title="Arraste para mudar o tamanho" />
        <div className="docks" style={{ height: dockHeight }}>
          <ScenesDock onSelectScene={() => setSelectedId(null)} />
          <SourcesDock scene={scene} selectedId={selectedId} onSelect={setSelectedId} />
          <MixerDock />
        </div>
      </section>
      <aside className="studio-side">
        <div className="side-tabs" role="tablist">
          <button role="tab" aria-selected={side === 'chat'} className={side === 'chat' ? 'is-active' : ''} onClick={() => setSide('chat')}>
            <IconChat size={14} /> Chat
          </button>
          <button role="tab" aria-selected={side === 'props'} className={side === 'props' ? 'is-active' : ''} onClick={() => setSide('props')}>
            <IconSliders size={14} /> Propriedades
            {selected && <span className="side-tab-dot" />}
          </button>
        </div>
        {side === 'props' ? (
          <Inspector scene={scene} item={selected} view={view} onPatch={patchItem} onClose={() => setSelectedId(null)} />
        ) : popout ? (
          <div className="chat-empty">
            <IconChat size={26} />
            <span>O chat está aberto na janela separada.</span>
            <Button size="sm" onClick={() => window.api.chat.closePopout()}>
              Trazer de volta para cá
            </Button>
          </div>
        ) : (
          <ChatPanel settings={settings} update={update} variant="dock" onReloginTwitch={onOpenDestinations} />
        )}
      </aside>
      <ControlBar onOpenDestinations={onOpenDestinations} />
    </div>
  )
}
