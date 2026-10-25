import { useEffect, useRef, useState } from 'react'
import type { Scene, SceneItem, SourceTransform } from '@shared/protocol'
import { Button, Segmented } from '@/components/ui'
import { IconCameraOff, IconSend } from '@/components/Icons'
import { useEngineState, useStudio } from '@/studio/StudioContext'
import { setStudioMode, transition } from '@/studio/scenes'
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

function snap(v: number, targets: number[]): number {
  for (const t of targets) if (Math.abs(v - t) < SNAP) return t
  return v
}

type Corner = 'nw' | 'ne' | 'sw' | 'se'

type Drag =
  | { kind: 'move'; id: string; startX: number; startY: number; orig: SourceTransform }
  | { kind: 'resize'; id: string; corner: Corner; startX: number; startY: number; orig: SourceTransform }

/** Camada de edição por cima do canvas: selecionar, arrastar e redimensionar fontes. */
function StageEditor({
  scene,
  vertical,
  selectedId,
  onSelect,
  onPatchItem
}: {
  scene: Scene
  vertical: boolean
  selectedId: string | null
  onSelect: (id: string | null) => void
  onPatchItem: (id: string, patch: Partial<SceneItem>) => void
}): React.JSX.Element {
  const { engine, settings, kit } = useStudio()
  useEngineState() // redesenha quando uma captura muda de estado
  const ref = useRef<HTMLDivElement>(null)
  const cameraOn = settings.studio.cameraEnabled
  const [drag, setDrag] = useState<Drag | null>(null)
  const [hoverId, setHoverId] = useState<string | null>(null)

  const tfOf = (it: SceneItem): SourceTransform => (vertical ? it.verticalTransform : it.transform)
  const shown = scene.items.filter(
    (it) =>
      it.visible &&
      it.type !== 'audio_input' &&
      (vertical ? it.showOnVertical : it.showOnHorizontal) &&
      (cameraOn || !hiddenWithoutCamera(it, kit))
  )
  const key = vertical ? 'verticalTransform' : 'transform'

  const rel = (e: { clientX: number; clientY: number }): { x: number; y: number } => {
    const r = ref.current!.getBoundingClientRect()
    return { x: (e.clientX - r.left) / r.width, y: (e.clientY - r.top) / r.height }
  }

  /**
   * Fonte sob o ponteiro. Fontes travadas (overlays de tela cheia) ficam por
   * cima de tudo e roubariam o clique: só são escolhidas se não houver uma
   * destravada no mesmo ponto.
   */
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
    const p = rel(e)
    const corner = (e.target as HTMLElement).dataset.corner as Corner | undefined
    const target = corner ? scene.items.find((it) => it.id === selectedId) : hit(p.x, p.y)
    if (!target) {
      onSelect(null)
      return
    }
    onSelect(target.id)
    if (target.locked) return
    ref.current!.setPointerCapture(e.pointerId)
    const orig = { ...tfOf(target) }
    setDrag(
      corner
        ? { kind: 'resize', id: target.id, corner, startX: p.x, startY: p.y, orig }
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

    if (drag.kind === 'move') {
      let x = o.x + dx
      let y = o.y + dy
      // Encaixa bordas e centro nas bordas e no centro do canvas.
      const sx = snap(x, [0, 0.5 - o.width / 2, 1 - o.width])
      const sy = snap(y, [0, 0.5 - o.height / 2, 1 - o.height])
      x = sx
      y = sy
      next = { ...o, x, y }
    } else {
      const free = e.shiftKey || !MEDIA.has(item.type)
      let { x, y, width: w, height: h } = o
      const right = o.x + o.width
      const bottom = o.y + o.height
      if (drag.corner.includes('e')) w = snap(right + dx, [1, 0.5]) - x
      if (drag.corner.includes('s')) h = snap(bottom + dy, [1, 0.5]) - y
      if (drag.corner.includes('w')) {
        x = snap(o.x + dx, [0, 0.5])
        w = right - x
      }
      if (drag.corner.includes('n')) {
        y = snap(o.y + dy, [0, 0.5])
        h = bottom - y
      }
      if (!free) {
        // Mantém a proporção original (em pixels do canvas).
        const ratio = o.height / o.width
        h = w * ratio
        if (drag.corner.includes('n')) y = bottom - h
      }
      next = { ...o, x, y, width: Math.max(0.02, w), height: Math.max(0.02, h) }
    }
    onPatchItem(item.id, { [key]: next })
  }

  const onPointerUp = (e: React.PointerEvent): void => {
    if (drag) ref.current?.releasePointerCapture(e.pointerId)
    setDrag(null)
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

  return (
    <div
      ref={ref}
      className={`stage-editor ${drag ? 'is-dragging' : ''}`}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerLeave={() => setHoverId(null)}
      onKeyDown={onKeyDown}
    >
      {shown.map((it) => {
        const cap = engine.capture(it)
        if (!cap || cap.state === 'live') return null
        const msg =
          cap.state === 'loading'
            ? 'Abrindo captura…'
            : it.type === 'camera_input'
              ? 'Câmera indisponível'
              : it.type === 'window_capture'
                ? 'Janela fechada — abra o programa ou troque a janela'
                : 'Tela indisponível'
        return (
          <div key={`m-${it.id}`} className="stage-missing" style={box(tfOf(it))}>
            <span>{msg}</span>
          </div>
        )
      })}
      {hover && (
        <div className="stage-hover" style={box(tfOf(hover))}>
          <span className="stage-tag">{hover.name}</span>
        </div>
      )}
      {selected && (
        <div className={`stage-selection ${selected.locked ? 'is-locked' : ''}`} style={box(tfOf(selected))}>
          <span className="stage-tag">
            {selected.name}
            {selected.locked ? ' · travada' : ''}
          </span>
          {!selected.locked &&
            (['nw', 'ne', 'sw', 'se'] as const).map((c) => (
              <span key={c} className={`stage-handle h-${c}`} data-corner={c} />
            ))}
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
  onPatchItem
}: {
  scene: Scene | null
  view: StageView
  onViewChange: (v: StageView) => void
  selectedId: string | null
  onSelect: (id: string | null) => void
  onPatchItem: (id: string, patch: Partial<SceneItem>) => void
}): React.JSX.Element {
  const { engine, settings, update } = useStudio()
  const state = useEngineState()
  const st = settings.scenes
  const cameraOn = settings.studio.cameraEnabled
  // Tem câmera na cena mas ela não abre: oferece desligar em vez de mostrar um buraco na live.
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
  // No modo estúdio o 9:16 mostra o programa; só dá para editar quando a prévia é a mesma cena.
  const canEditVertical = !studioMode || st.previewSceneId === st.activeSceneId

  const editor = (verticalView: boolean): React.JSX.Element | null =>
    scene && (!verticalView || canEditVertical) ? (
      <StageEditor
        scene={scene}
        vertical={verticalView}
        selectedId={selectedId}
        onSelect={onSelect}
        onPatchItem={onPatchItem}
      />
    ) : null

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
              <span className="stage-transition-type">{st.transitionType === 'fade' ? 'Esmaecer' : 'Corte'}</span>
            </div>
            <div className="stage-pane">
              <div className="canvas-frame is-program">
                <CanvasHost canvas={engine.compositor.program} />
                <span className="pane-label">{state.live ? 'No ar' : 'Programa'} · {program?.name}</span>
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
          ? 'Arraste para mover · cantos redimensionam (Shift solta a proporção) · setas ajustam fino · Delete remove'
          : 'Clique numa fonte do palco para mover, redimensionar e ver as propriedades'}
      </div>
    </div>
  )
}
