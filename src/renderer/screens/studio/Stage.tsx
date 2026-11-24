import { useEffect, useRef, useState } from 'react'
import type { Scene, SceneItem, SourceTransform } from '@shared/protocol'
import { Button, Segmented } from '@/components/ui'
import { IconCameraOff, IconSend } from '@/components/Icons'
import { useEngineState, useStudio } from '@/studio/StudioContext'
import {
  clearTransformCrop,
  moveItem,
  removeItem,
  setStudioMode,
  transition
} from '@/studio/scenes'
import { hiddenWithoutCamera } from '@/studio/widgets'

export type StageView = 'horizontal' | 'vertical'

/** Põe um canvas do compositor dentro do layout (o mesmo elemento que vai ao ar). */
function CanvasHost({ canvas }: { canvas: HTMLCanvasElement }): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const host = ref.current
    if (!host) return
    canvas.className = 'stage-canvas'
    host.appendChild(canvas)
    return () => {
      if (canvas.parentElement === host) host.removeChild(canvas)
    }
  }, [canvas])
  return <div ref={ref} className="canvas-host" />
}

const MEDIA = new Set(['screen_capture', 'window_capture', 'camera_input', 'image_source', 'browser_source'])
const SNAP = 0.012

function snapWithGuide(v: number, targets: Array<{ pos: number; guide: number }>): { value: number; guide: number | null } {
  for (const t of targets) {
    if (Math.abs(v - t.pos) < SNAP) return { value: t.pos, guide: t.guide }
  }
  return { value: v, guide: null }
}

type Corner = 'nw' | 'ne' | 'sw' | 'se'

type Drag =
  | { kind: 'move'; id: string; startX: number; startY: number; orig: SourceTransform }
  | { kind: 'resize'; id: string; corner: Corner; startX: number; startY: number; orig: SourceTransform; cropMode: boolean }

interface ContextMenuState {
  itemId: string
  x: number
  y: number
}

/** Camada de edição por cima do canvas: selecionar, arrastar, recortar (Alt) e redimensionar fontes. */
function StageEditor({
  scene,
  vertical,
  selectedId,
  onSelect,
  onPatchItem,
  onRequestRename
}: {
  scene: Scene
  vertical: boolean
  selectedId: string | null
  onSelect: (id: string | null) => void
  onPatchItem: (id: string, patch: Partial<SceneItem>) => void
  onRequestRename?: (id: string) => void
}): React.JSX.Element {
  const { engine, settings, update, toast, kit, pushUndoSnapshot, undo } = useStudio()
  useEngineState()
  const ref = useRef<HTMLDivElement>(null)
  const cameraOn = settings.studio.cameraEnabled
  const [drag, setDrag] = useState<Drag | null>(null)
  const [localTf, setLocalTf] = useState<{ id: string; tf: SourceTransform } | null>(null)
  const [snapGuides, setSnapGuides] = useState<{ x: number | null; y: number | null }>({ x: null, y: null })
  const [hoverId, setHoverId] = useState<string | null>(null)
  const [ctxMenu, setCtxMenu] = useState<ContextMenuState | null>(null)
  const lastCommitRef = useRef<number>(0)

  const key = vertical ? 'verticalTransform' : 'transform'
  const tfOf = (it: SceneItem): SourceTransform => {
    if (localTf && localTf.id === it.id) return localTf.tf
    return vertical ? it.verticalTransform : it.transform
  }

  const shown = scene.items.filter(
    (it) =>
      it.visible &&
      it.type !== 'audio_input' &&
      (vertical ? it.showOnVertical : it.showOnHorizontal) &&
      (cameraOn || !hiddenWithoutCamera(it, kit))
  )

  useEffect(() => {
    if (!ctxMenu) return
    const close = (): void => setCtxMenu(null)
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') setCtxMenu(null)
    }
    window.addEventListener('mousedown', close)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', close)
      window.removeEventListener('keydown', onKey)
    }
  }, [ctxMenu])

  const rel = (e: { clientX: number; clientY: number }): { x: number; y: number } => {
    const r = ref.current!.getBoundingClientRect()
    return {
      x: r.width > 0 ? (e.clientX - r.left) / r.width : 0,
      y: r.height > 0 ? (e.clientY - r.top) / r.height : 0
    }
  }

  const hit = (x: number, y: number): SceneItem | null => {
    const under = shown.filter((it) => {
      const tf = tfOf(it)
      return x >= tf.x && x <= tf.x + tf.width && y >= tf.y && y <= tf.y + tf.height
    })
    for (let i = under.length - 1; i >= 0; i--) if (!under[i].locked) return under[i]
    return under[under.length - 1] ?? null
  }

  const onPointerDown = (e: React.PointerEvent): void => {
    if (e.button !== 0) return
    setCtxMenu(null)
    const p = rel(e)
    const corner = (e.target as HTMLElement).dataset.corner as Corner | undefined
    const target = corner ? scene.items.find((it) => it.id === selectedId) : hit(p.x, p.y)
    if (!target) {
      onSelect(null)
      return
    }
    onSelect(target.id)
    if (target.locked) return
    pushUndoSnapshot()
    try {
      ref.current?.setPointerCapture(e.pointerId)
    } catch {
      /* ignore */
    }
    const orig = { ...(vertical ? target.verticalTransform : target.transform) }
    setLocalTf({ id: target.id, tf: orig })
    setDrag(
      corner
        ? { kind: 'resize', id: target.id, corner, startX: p.x, startY: p.y, orig, cropMode: e.altKey }
        : { kind: 'move', id: target.id, startX: p.x, startY: p.y, orig }
    )
  }

  const onPointerMove = (e: React.PointerEvent): void => {
    const p = rel(e)
    if (!drag) {
      setHoverId(hit(p.x, p.y)?.id ?? null)
      return
    }
    const item = scene.items.find((it) => it.id === drag.id)
    if (!item) return
    const dx = p.x - drag.startX
    const dy = p.y - drag.startY
    const o = drag.orig
    let next: SourceTransform
    let guideX: number | null = null
    let guideY: number | null = null

    if (drag.kind === 'move') {
      const sx = snapWithGuide(o.x + dx, [
        { pos: 0, guide: 0 },
        { pos: 0.5 - o.width / 2, guide: 0.5 },
        { pos: 1 - o.width, guide: 1 }
      ])
      const sy = snapWithGuide(o.y + dy, [
        { pos: 0, guide: 0 },
        { pos: 0.5 - o.height / 2, guide: 0.5 },
        { pos: 1 - o.height, guide: 1 }
      ])
      guideX = sx.guide
      guideY = sy.guide
      next = { ...o, x: sx.value, y: sy.value }
    } else if (drag.cropMode || e.altKey) {
      // Alt + arraste nas alças ajusta o recorte (crop) proporcionalmente
      let cropLeft = o.cropLeft ?? 0
      let cropRight = o.cropRight ?? 0
      let cropTop = o.cropTop ?? 0
      let cropBottom = o.cropBottom ?? 0
      const w = Math.max(0.05, o.width)
      const h = Math.max(0.05, o.height)

      if (drag.corner.includes('w')) {
        cropLeft = Math.max(0, Math.min(0.85 - cropRight, cropLeft + dx / w))
      }
      if (drag.corner.includes('e')) {
        cropRight = Math.max(0, Math.min(0.85 - cropLeft, cropRight - dx / w))
      }
      if (drag.corner.includes('n')) {
        cropTop = Math.max(0, Math.min(0.85 - cropBottom, cropTop + dy / h))
      }
      if (drag.corner.includes('s')) {
        cropBottom = Math.max(0, Math.min(0.85 - cropTop, cropBottom - dy / h))
      }
      next = {
        ...o,
        cropLeft: Math.round(cropLeft * 1000) / 1000,
        cropRight: Math.round(cropRight * 1000) / 1000,
        cropTop: Math.round(cropTop * 1000) / 1000,
        cropBottom: Math.round(cropBottom * 1000) / 1000
      }
    } else {
      const free = e.shiftKey || !MEDIA.has(item.type)
      let { x, y, width: w, height: h } = o
      const right = o.x + o.width
      const bottom = o.y + o.height

      if (drag.corner.includes('e')) {
        const sn = snapWithGuide(right + dx, [
          { pos: 0.5, guide: 0.5 },
          { pos: 1, guide: 1 }
        ])
        guideX = sn.guide
        w = sn.value - x
      }
      if (drag.corner.includes('s')) {
        const sn = snapWithGuide(bottom + dy, [
          { pos: 0.5, guide: 0.5 },
          { pos: 1, guide: 1 }
        ])
        guideY = sn.guide
        h = sn.value - y
      }
      if (drag.corner.includes('w')) {
        const sn = snapWithGuide(o.x + dx, [
          { pos: 0, guide: 0 },
          { pos: 0.5, guide: 0.5 }
        ])
        guideX = sn.guide
        x = Math.min(right - 0.02, sn.value)
        w = right - x
      }
      if (drag.corner.includes('n')) {
        const sn = snapWithGuide(o.y + dy, [
          { pos: 0, guide: 0 },
          { pos: 0.5, guide: 0.5 }
        ])
        guideY = sn.guide
        y = Math.min(bottom - 0.02, sn.value)
        h = bottom - y
      }

      w = Math.max(0.02, w)
      h = Math.max(0.02, h)

      if (!free) {
        const ratio = o.width > 0 ? o.height / o.width : 1
        if (Math.abs(dy) > Math.abs(dx) && ratio > 0) {
          w = Math.max(0.02, h / ratio)
          h = w * ratio
        } else {
          h = Math.max(0.02, w * ratio)
          w = ratio > 0 ? h / ratio : w
        }
        if (drag.corner.includes('w')) x = right - w
        if (drag.corner.includes('n')) y = bottom - h
      }
      next = { ...o, x, y, width: Math.max(0.02, w), height: Math.max(0.02, h) }
    }

    setSnapGuides({ x: guideX, y: guideY })
    setLocalTf({ id: item.id, tf: next })

    // Atualiza o compositor em no máximo ~25 Hz durante o arraste, commitando o valor final no pointerup.
    const now = performance.now()
    if (now - lastCommitRef.current >= 40) {
      lastCommitRef.current = now
      onPatchItem(item.id, { [key]: next })
    }
  }

  const endDrag = (e: React.PointerEvent): void => {
    if (drag) {
      try {
        if (ref.current?.hasPointerCapture(e.pointerId)) {
          ref.current.releasePointerCapture(e.pointerId)
        }
      } catch {
        /* ignore */
      }
      if (localTf && localTf.id === drag.id) {
        onPatchItem(drag.id, { [key]: localTf.tf })
      }
    }
    setDrag(null)
    setLocalTf(null)
    setSnapGuides({ x: null, y: null })
  }

  const onContextMenu = (e: React.MouseEvent): void => {
    e.preventDefault()
    const p = rel(e)
    const target = hit(p.x, p.y) ?? scene.items.find((it) => it.id === selectedId) ?? null
    if (!target) return
    onSelect(target.id)
    const x = Math.min(window.innerWidth - 220, Math.max(8, e.clientX))
    const y = Math.min(window.innerHeight - 260, Math.max(8, e.clientY))
    setCtxMenu({ itemId: target.id, x, y })
  }

  const onKeyDown = (e: React.KeyboardEvent): void => {
    const item = scene.items.find((it) => it.id === selectedId)
    if (!item || item.locked) return
    const step = e.shiftKey ? 0.01 : 0.002
    const moves: Record<string, [number, number]> = {
      ArrowLeft: [-step, 0],
      ArrowRight: [step, 0],
      ArrowUp: [0, -step],
      ArrowDown: [0, step]
    }
    const m = moves[e.key]
    if (!m) return
    e.preventDefault()
    const tf = tfOf(item)
    onPatchItem(item.id, { [key]: { ...tf, x: tf.x + m[0], y: tf.y + m[1] } })
  }

  const box = (tf: SourceTransform): React.CSSProperties => ({
    left: `${tf.x * 100}%`,
    top: `${tf.y * 100}%`,
    width: `${tf.width * 100}%`,
    height: `${tf.height * 100}%`
  })

  const selected = shown.find((it) => it.id === selectedId)
  const hover = drag ? null : shown.find((it) => it.id === hoverId && it.id !== selectedId)
  const selectedTf = selected ? tfOf(selected) : null
  const isCropping = Boolean(
    (drag?.kind === 'resize' && drag.cropMode) ||
      (selectedTf &&
        ((selectedTf.cropTop ?? 0) > 0 ||
          (selectedTf.cropBottom ?? 0) > 0 ||
          (selectedTf.cropLeft ?? 0) > 0 ||
          (selectedTf.cropRight ?? 0) > 0))
  )

  const ctxItem = ctxMenu ? scene.items.find((it) => it.id === ctxMenu.itemId) ?? null : null
  const ctxItemIdx = ctxItem ? scene.items.findIndex((it) => it.id === ctxItem.id) : -1

  return (
    <div
      ref={ref}
      className={`stage-editor ${drag ? 'is-dragging' : ''}`}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onPointerLeave={() => setHoverId(null)}
      onContextMenu={onContextMenu}
      onKeyDown={onKeyDown}
    >
      {snapGuides.x !== null && <div className="stage-snap-x" style={{ left: `${snapGuides.x * 100}%` }} />}
      {snapGuides.y !== null && <div className="stage-snap-y" style={{ top: `${snapGuides.y * 100}%` }} />}

      {shown.map((it) => {
        const cap = engine.capture(it)
        if (!cap || cap.state === 'live') return null
        const isDisplay = it.type === 'screen_capture' || it.type === 'window_capture'
        const msg =
          cap.state === 'loading'
            ? isDisplay
              ? 'Aguardando você escolher a tela…'
              : 'Abrindo captura…'
            : cap.state === 'idle'
              ? it.type === 'window_capture'
                ? 'Captura de janela pronta'
                : 'Captura de tela pronta'
              : it.type === 'camera_input'
                ? 'Câmera indisponível'
                : it.type === 'window_capture'
                  ? 'Janela fechada — abra o programa ou selecione outra'
                  : 'Captura de tela interrompida'
        const actionLabel =
          cap.state === 'idle'
            ? it.type === 'window_capture'
              ? 'Selecionar janela'
              : 'Selecionar tela'
            : 'Reconectar'
        return (
          <div key={`m-${it.id}`} className="stage-missing" style={box(tfOf(it))}>
            <div className="stage-missing-card">
              <span>{msg}</span>
              {cap.state !== 'loading' && (
                <Button
                  size="sm"
                  variant={cap.state === 'idle' ? 'primary' : 'subtle'}
                  onPointerDown={(e) => e.stopPropagation()}
                  onClick={(e) => {
                    e.stopPropagation()
                    void engine.startCapture(it)
                  }}
                >
                  {actionLabel}
                </Button>
              )}
            </div>
          </div>
        )
      })}
      {hover && (
        <div className="stage-hover" style={box(tfOf(hover))}>
          <span className={`stage-tag ${tfOf(hover).y < 0.04 ? 'is-inside' : ''}`}>{hover.name}</span>
        </div>
      )}
      {selected && selectedTf && (
        <div
          className={`stage-selection ${selected.locked ? 'is-locked' : ''} ${isCropping ? 'stage-crop-active' : ''}`}
          style={box(selectedTf)}
        >
          <span className={`stage-tag ${selectedTf.y < 0.04 ? 'is-inside' : ''}`}>
            {selected.name}
            {selected.locked ? ' · travada' : isCropping ? ' · recorte' : ''}
          </span>
          {!selected.locked &&
            (['nw', 'ne', 'sw', 'se'] as const).map((c) => (
              <span key={c} className={`stage-handle h-${c}`} data-corner={c} />
            ))}
        </div>
      )}

      {ctxMenu && ctxItem && (
        <div
          className="studio-ctx-menu"
          style={{ left: ctxMenu.x, top: ctxMenu.y }}
          onMouseDown={(e) => e.stopPropagation()}
          onPointerDown={(e) => e.stopPropagation()}
        >
          <button
            className="studio-ctx-item"
            disabled={ctxItemIdx >= scene.items.length - 1}
            onClick={() => {
              update({ scenes: moveItem(settings.scenes, scene.id, ctxItem.id, 1) })
              setCtxMenu(null)
            }}
          >
            <span>Trazer para frente</span>
          </button>
          <button
            className="studio-ctx-item"
            disabled={ctxItemIdx <= 0}
            onClick={() => {
              update({ scenes: moveItem(settings.scenes, scene.id, ctxItem.id, -1) })
              setCtxMenu(null)
            }}
          >
            <span>Mandar para trás</span>
          </button>
          <div className="studio-ctx-sep" />
          <button
            className="studio-ctx-item"
            disabled={ctxItem.locked}
            onClick={() => {
              const cur = vertical ? ctxItem.verticalTransform : ctxItem.transform
              onPatchItem(ctxItem.id, { [key]: { ...cur, x: 0, y: 0, width: 1, height: 1 } })
              setCtxMenu(null)
            }}
          >
            <span>Tela cheia</span>
          </button>
          <button
            className="studio-ctx-item"
            disabled={ctxItem.locked}
            onClick={() => {
              const cur = vertical ? ctxItem.verticalTransform : ctxItem.transform
              onPatchItem(ctxItem.id, {
                [key]: { ...cur, x: (1 - cur.width) / 2, y: (1 - cur.height) / 2 }
              })
              setCtxMenu(null)
            }}
          >
            <span>Centralizar</span>
          </button>
          <button
            className="studio-ctx-item"
            disabled={ctxItem.locked}
            onClick={() => {
              const cur = vertical ? ctxItem.verticalTransform : ctxItem.transform
              onPatchItem(ctxItem.id, { [key]: clearTransformCrop(cur) })
              setCtxMenu(null)
            }}
          >
            <span>Zerar recorte</span>
          </button>
          <div className="studio-ctx-sep" />
          <button
            className="studio-ctx-item"
            onClick={() => {
              onPatchItem(ctxItem.id, { locked: !ctxItem.locked })
              setCtxMenu(null)
            }}
          >
            <span>{ctxItem.locked ? 'Destravar posição' : 'Travar posição'}</span>
          </button>
          {onRequestRename && (
            <button
              className="studio-ctx-item"
              onClick={() => {
                onRequestRename(ctxItem.id)
                setCtxMenu(null)
              }}
            >
              <span>Renomear</span>
            </button>
          )}
          <button
            className="studio-ctx-item is-danger"
            onClick={() => {
              pushUndoSnapshot()
              update({ scenes: removeItem(settings.scenes, scene.id, ctxItem.id) })
              onSelect(null)
              setCtxMenu(null)
              toast(`“${ctxItem.name}” removida`, 'info', { label: 'Desfazer', run: () => undo() })
            }}
          >
            <span>Remover fonte</span>
          </button>
        </div>
      )}
    </div>
  )
}

export function Stage({
  scene,
  view,
  onViewChange,
  selectedId,
  onSelect,
  onPatchItem,
  onRequestRename
}: {
  scene: Scene | null
  view: StageView
  onViewChange: (v: StageView) => void
  selectedId: string | null
  onSelect: (id: string | null) => void
  onPatchItem: (id: string, patch: Partial<SceneItem>) => void
  onRequestRename?: (id: string) => void
}): React.JSX.Element {
  const { engine, settings, update } = useStudio()
  const state = useEngineState()
  const st = settings.scenes
  const cameraOn = settings.studio.cameraEnabled
  const cameraMissing =
    cameraOn &&
    (scene?.items ?? []).some((it) => {
      if (it.type !== 'camera_input' || !it.visible) return false
      const cap = engine.capture(it)
      return cap !== null && (cap.state === 'missing' || cap.state === 'error')
    })
  const studioMode = st.studioMode
  const dual = settings.studio.dualOutputEnabled
  const program = st.scenes.find((s) => s.id === st.activeSceneId)
  const vertical = view === 'vertical'
  const canEditVertical = !studioMode || st.previewSceneId === st.activeSceneId

  const editor = (verticalView: boolean): React.JSX.Element | null =>
    scene && (!verticalView || canEditVertical) ? (
      <StageEditor
        scene={scene}
        vertical={verticalView}
        selectedId={selectedId}
        onSelect={onSelect}
        onPatchItem={onPatchItem}
        onRequestRename={onRequestRename}
      />
    ) : null

  const transitionLabel =
    st.transitionType === 'cut'
      ? 'Corte'
      : st.transitionType === 'fade-to-black' || st.transitionType === 'fade_to_black'
        ? 'Escurecer'
        : st.transitionType === 'slide'
          ? 'Deslizar'
          : 'Esmaecer'

  return (
    <div className="stage">
      <div className="stage-bar">
        <div className="stage-title">
          {studioMode ? 'Prévia' : state.live ? <span className="stage-onair">No ar</span> : 'Cena'}
          <span className="stage-scene">{scene?.name}</span>
          {!cameraOn && (
            <button
              className="stage-chip"
              onClick={() => update({ studio: { ...settings.studio, cameraEnabled: true } })}
              title="Religar a câmera"
            >
              <IconCameraOff size={13} /> sem câmera
            </button>
          )}
          {cameraMissing && (
            <button
              className="stage-chip is-warn"
              onClick={() => update({ studio: { ...settings.studio, cameraEnabled: false } })}
              title="Esconde a câmera e as molduras dos overlays"
            >
              <IconCameraOff size={13} /> Câmera não abriu — desligar câmera
            </button>
          )}
          {scene && scene.items.length === 0 && (
            <span className="stage-empty-hint">Cena vazia — adicione uma fonte em “Fontes”</span>
          )}
        </div>
        <div className="row" style={{ gap: 'var(--sp-2)' }}>
          {dual && (
            <Segmented
              size="sm"
              value={view}
              onChange={onViewChange}
              options={[
                { value: 'horizontal', label: '16:9' },
                { value: 'vertical', label: '9:16' }
              ]}
            />
          )}
          <Button
            size="sm"
            variant={studioMode ? 'primary' : 'ghost'}
            onClick={() => update({ scenes: setStudioMode(st, !studioMode) })}
            title="Edite uma cena na prévia enquanto outra está no ar"
          >
            Modo estúdio
          </Button>
        </div>
      </div>

      <div className={`stage-body ${studioMode && !vertical ? 'is-split' : ''}`}>
        {studioMode && !vertical ? (
          <>
            <div className="stage-pane">
              <div className="canvas-frame is-preview">
                <CanvasHost canvas={engine.compositor.preview} />
                {editor(false)}
              </div>
            </div>
            <div className="stage-transition">
              <Button
                variant="primary"
                size="sm"
                icon={<IconSend size={15} />}
                onClick={() => update({ scenes: transition(st) })}
                disabled={st.previewSceneId === st.activeSceneId}
                title="Mandar a prévia para o ar"
              >
                Transição
              </Button>
              <span className="stage-transition-type">{transitionLabel}</span>
            </div>
            <div className="stage-pane">
              <div className="canvas-frame is-program">
                <CanvasHost canvas={engine.compositor.program} />
                <span className="pane-label">
                  {state.live ? 'No ar' : 'Programa'} · {program?.name}
                </span>
              </div>
            </div>
          </>
        ) : (
          <div className="stage-pane">
            <div className={`canvas-frame ${vertical ? 'is-vertical' : ''}`}>
              <CanvasHost canvas={vertical ? engine.compositor.vertical : engine.compositor.program} />
              {editor(vertical)}
            </div>
          </div>
        )}
        {dual && !vertical && !studioMode && (
          <button className="stage-mini" onClick={() => onViewChange('vertical')} title="Editar o layout 9:16">
            <div className="canvas-frame is-vertical">
              <CanvasHost canvas={engine.compositor.vertical} />
            </div>
            <span>9:16 · editar</span>
          </button>
        )}
      </div>
      <div className="stage-foot hint">
        {selectedId
          ? 'Arraste para mover · cantos redimensionam (Shift solta proporção · Alt recorta) · botão direito abre opções · Delete remove'
          : 'Clique numa fonte do palco para mover, redimensionar, recortar (Alt) e ver as propriedades'}
      </div>
    </div>
  )
}
