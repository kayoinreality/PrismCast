import { useState } from 'react'
import type {
  CustomShaderPreset,
  CustomTransitionConfig,
  Scene,
  SceneTransitionOverride,
  StudioScenesState,
  TransitionEasing,
  TransitionType
} from '@shared/protocol'
import { Modal } from '@/components/Modal'
import { Button, Field, IconButton, Segmented, Slider, TextInput, Toggle } from '@/components/ui'
import {
  IconCopy,
  IconDown,
  IconFolder,
  IconPlus,
  IconSliders,
  IconTrash,
  IconUp
} from '@/components/Icons'
import { DEFAULT_CUSTOM_TRANSITION } from '@/studio/Compositor'
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

const TRANSITION_OPTIONS: Array<{ value: TransitionType; label: string }> = [
  { value: 'fade', label: 'Esmaecer' },
  { value: 'cut', label: 'Corte' },
  { value: 'fade-to-black', label: 'Escurecer' },
  { value: 'slide', label: 'Deslizar' },
  { value: 'stinger', label: 'Stinger (vídeo alpha)' },
  { value: 'luma-wipe', label: 'Luma Wipe' },
  { value: 'custom-shader', label: 'Shader custom' }
]

const EASING_OPTIONS: Array<{ value: TransitionEasing; label: string }> = [
  { value: 'linear', label: 'Linear' },
  { value: 'ease-in-out', label: 'Suave' },
  { value: 'cubic-out', label: 'Cúbica' },
  { value: 'expo-out', label: 'Impacto' }
]

const LUMA_PRESETS: Array<{ value: string; label: string }> = [
  { value: 'horizontal-wipe', label: 'Varredura horizontal' },
  { value: 'vertical-wipe', label: 'Varredura vertical' },
  { value: 'radial', label: 'Radial (círculo)' },
  { value: 'diagonal', label: 'Diagonal' },
  { value: 'barn-door', label: 'Portas (centro)' },
  { value: 'custom', label: 'Imagem matte personalizada' }
]

const SHADER_PRESETS: Array<{ value: CustomShaderPreset; label: string }> = [
  { value: 'zoom-blur', label: 'Zoom Blur' },
  { value: 'radial-wipe', label: 'Varredura Radial' },
  { value: 'glitch-cut', label: 'Glitch Cut' },
  { value: 'swirl', label: 'Vórtice (Swirl)' },
  { value: 'custom-wgsl', label: 'WGSL personalizado' }
]

const DEFAULT_WGSL_TEMPLATE = `@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
  let a = textureSample(tex_from, samp, uv);
  let b = textureSample(tex_to, samp, uv);
  let wave = smoothstep(u.progress - 0.1, u.progress + 0.1, uv.x + sin(uv.y * 12.0) * 0.04);
  return mix(b, a, wave);
}`

function fileNameOf(path: string | null | undefined): string {
  if (!path) return ''
  return path.split(/[\\/]/).pop() || path
}

function normalizeTransitionType(t: TransitionType): TransitionType {
  return t === 'fade_to_black' ? 'fade-to-black' : t
}

function CustomTransitionFields({
  transitionType,
  custom,
  onChange
}: {
  transitionType: TransitionType
  custom: CustomTransitionConfig
  onChange: (patch: Partial<CustomTransitionConfig>) => void
}): React.JSX.Element {
  const { toast } = useStudio()

  const pickStinger = async (): Promise<void> => {
    const picker = window.api.files.pickMedia
    if (!picker) {
      toast('Seletor de vídeo indisponível neste ambiente', 'bad')
      return
    }
    const picked = await picker().catch(() => null)
    if (picked) onChange({ stingerPath: picked })
  }

  const pickLumaMatte = async (): Promise<void> => {
    const picked = await window.api.files.pickImage().catch(() => null)
    if (picked) onChange({ lumaMattePath: picked, lumaPreset: 'custom' })
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
      <Field label="Curva de suavização (Easing)">
        <Segmented
          size="sm"
          value={custom.easing}
          onChange={(v) => onChange({ easing: v })}
          options={EASING_OPTIONS}
        />
      </Field>

      {transitionType === 'stinger' && (
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            gap: 'var(--sp-3)',
            padding: 'var(--sp-3)',
            borderRadius: 'var(--r-md)',
            background: 'var(--surface)',
            border: '1px solid var(--border)'
          }}
        >
          <div className="section-title">Stinger (vídeo com canal alpha)</div>
          <Field label="Arquivo de vídeo (.webm / .mov)" hint={custom.stingerPath ? fileNameOf(custom.stingerPath) : 'Fallback animado'}>
            <div className="row" style={{ gap: 'var(--sp-2)' }}>
              <TextInput
                readOnly
                className="mono-input"
                value={custom.stingerPath ?? ''}
                placeholder="Nenhum vídeo escolhido (usa faixa animada padrão)"
              />
              <Button size="sm" variant="subtle" icon={<IconFolder size={14} />} onClick={() => void pickStinger()}>
                Escolher
              </Button>
              {custom.stingerPath && (
                <Button size="sm" variant="ghost" onClick={() => onChange({ stingerPath: null })}>
                  Limpar
                </Button>
              )}
            </div>
          </Field>

          <Field label="Ponto de troca de cena" hint={`${custom.stingerTransitionPointMs} ms`}>
            <Slider
              value={custom.stingerTransitionPointMs}
              min={0}
              max={5000}
              step={25}
              onChange={(v) => onChange({ stingerTransitionPointMs: v })}
            />
          </Field>

          <Field
            label="Ganho do áudio do stinger"
            hint={custom.stingerAudioGainDb <= -60 ? 'Mudo' : `${custom.stingerAudioGainDb > 0 ? '+' : ''}${custom.stingerAudioGainDb} dB`}
          >
            <Slider
              value={custom.stingerAudioGainDb}
              min={-60}
              max={12}
              step={1}
              onChange={(v) => onChange({ stingerAudioGainDb: v })}
            />
          </Field>
        </div>
      )}

      {transitionType === 'luma-wipe' && (
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            gap: 'var(--sp-3)',
            padding: 'var(--sp-3)',
            borderRadius: 'var(--r-md)',
            background: 'var(--surface)',
            border: '1px solid var(--border)'
          }}
        >
          <div className="section-title">Luma Wipe</div>
          <Field label="Padrão de máscara">
            <select
              className="select"
              value={custom.lumaPreset || 'horizontal-wipe'}
              onChange={(e) => onChange({ lumaPreset: e.target.value })}
            >
              {LUMA_PRESETS.map((p) => (
                <option key={p.value} value={p.value}>
                  {p.label}
                </option>
              ))}
            </select>
          </Field>

          <Field
            label="Imagem matte em tons de cinza (opcional)"
            hint={custom.lumaMattePath ? fileNameOf(custom.lumaMattePath) : 'Opcional'}
          >
            <div className="row" style={{ gap: 'var(--sp-2)' }}>
              <TextInput
                readOnly
                className="mono-input"
                value={custom.lumaMattePath ?? ''}
                placeholder="Selecione um PNG/JPG em escala de cinza"
              />
              <Button size="sm" variant="subtle" icon={<IconFolder size={14} />} onClick={() => void pickLumaMatte()}>
                Escolher
              </Button>
              {custom.lumaMattePath && (
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => onChange({ lumaMattePath: null, lumaPreset: 'horizontal-wipe' })}
                >
                  Limpar
                </Button>
              )}
            </div>
          </Field>

          <Field label="Suavidade da borda" hint={`${Math.round(custom.lumaSoftness * 100)}%`}>
            <Slider
              value={custom.lumaSoftness}
              min={0}
              max={0.5}
              step={0.01}
              onChange={(v) => onChange({ lumaSoftness: v })}
            />
          </Field>

          <Toggle
            label="Inverter máscara"
            hint="Inverte a direção da transição (claro para escuro)"
            checked={custom.lumaInvert}
            onChange={(v) => onChange({ lumaInvert: v })}
          />
        </div>
      )}

      {transitionType === 'custom-shader' && (
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            gap: 'var(--sp-3)',
            padding: 'var(--sp-3)',
            borderRadius: 'var(--r-md)',
            background: 'var(--surface)',
            border: '1px solid var(--border)'
          }}
        >
          <div className="section-title">Shader customizado</div>
          <Field label="Preset de efeito">
            <select
              className="select"
              value={custom.shaderPreset}
              onChange={(e) => onChange({ shaderPreset: e.target.value as CustomShaderPreset })}
            >
              {SHADER_PRESETS.map((p) => (
                <option key={p.value} value={p.value}>
                  {p.label}
                </option>
              ))}
            </select>
          </Field>

          {custom.shaderPreset === 'custom-wgsl' && (
            <Field label="Código WGSL (compositor wgpu)" hint="Preview prismático no Canvas 2D">
              <textarea
                className="text-input mono-input"
                rows={7}
                style={{
                  height: 'auto',
                  padding: '8px 10px',
                  resize: 'vertical',
                  lineHeight: 1.45,
                  fontFamily: 'var(--font-mono)'
                }}
                value={custom.customWgsl ?? ''}
                placeholder={DEFAULT_WGSL_TEMPLATE}
                onChange={(e) => onChange({ customWgsl: e.target.value })}
              />
              {!custom.customWgsl && (
                <div className="row" style={{ justifyContent: 'flex-end' }}>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => onChange({ customWgsl: DEFAULT_WGSL_TEMPLATE })}
                  >
                    Inserir modelo WGSL
                  </Button>
                </div>
              )}
            </Field>
          )}
        </div>
      )}
    </div>
  )
}

function TransitionConfigModal({
  open,
  onClose,
  state,
  currentScene,
  onChange
}: {
  open: boolean
  onClose: () => void
  state: StudioScenesState
  currentScene: Scene | null
  onChange: (next: StudioScenesState) => void
}): React.JSX.Element {
  const globalCustom: CustomTransitionConfig = {
    ...DEFAULT_CUSTOM_TRANSITION,
    ...(state.customTransition ?? {})
  }
  const globalType = normalizeTransitionType(state.transitionType)
  const override = currentScene?.transitionOverride ?? null

  const patchGlobalCustom = (patch: Partial<CustomTransitionConfig>): void => {
    onChange({
      ...state,
      customTransition: {
        ...globalCustom,
        ...patch
      }
    })
  }

  const setSceneOverride = (nextOverride: SceneTransitionOverride | null): void => {
    if (!currentScene) return
    onChange({
      ...state,
      scenes: state.scenes.map((sc) =>
        sc.id === currentScene.id ? { ...sc, transitionOverride: nextOverride } : sc
      )
    })
  }

  const overrideCustom: CustomTransitionConfig = {
    ...globalCustom,
    ...(override?.custom ?? {})
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      width={560}
      icon={<IconSliders size={18} />}
      title="Configurar transições"
      description="Personalize a transição global do estúdio ou defina um efeito próprio para a cena selecionada."
      footer={
        <>
          <div className="spacer" />
          <Button variant="primary" onClick={onClose}>
            Concluir
          </Button>
        </>
      }
    >
      <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-4)' }}>
        <div className="form-grid">
          <Field label="Transição global">
            <select
              className="select"
              value={globalType}
              onChange={(e) => onChange({ ...state, transitionType: e.target.value as TransitionType })}
            >
              {TRANSITION_OPTIONS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </Field>

          <Field label="Duração" hint={`${state.transitionDurationMs || 300} ms`}>
            <Slider
              value={state.transitionDurationMs || 300}
              min={80}
              max={3000}
              step={20}
              disabled={globalType === 'cut'}
              onChange={(v) => onChange({ ...state, transitionDurationMs: v })}
            />
          </Field>
        </div>

        <CustomTransitionFields
          transitionType={globalType}
          custom={globalCustom}
          onChange={patchGlobalCustom}
        />

        {currentScene && (
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              gap: 'var(--sp-3)',
              paddingTop: 'var(--sp-3)',
              borderTop: '1px solid var(--border)'
            }}
          >
            <Toggle
              label={`Sobrescrever transição na cena “${currentScene.name}”`}
              hint="Quando você entrar nesta cena, ela usará esta transição própria em vez da global."
              checked={Boolean(override)}
              onChange={(enabled) => {
                if (enabled) {
                  setSceneOverride({
                    transitionType: globalType,
                    durationMs: state.transitionDurationMs || 300,
                    custom: { ...globalCustom }
                  })
                } else {
                  setSceneOverride(null)
                }
              }}
            />

            {override && (
              <div
                style={{
                  display: 'flex',
                  flexDirection: 'column',
                  gap: 'var(--sp-3)',
                  padding: 'var(--sp-3)',
                  borderRadius: 'var(--r-md)',
                  background: 'var(--bg)',
                  border: '1px solid var(--accent-line)'
                }}
              >
                <div className="form-grid">
                  <Field label="Transição desta cena">
                    <select
                      className="select"
                      value={normalizeTransitionType(override.transitionType)}
                      onChange={(e) =>
                        setSceneOverride({
                          ...override,
                          transitionType: e.target.value as TransitionType
                        })
                      }
                    >
                      {TRANSITION_OPTIONS.map((o) => (
                        <option key={o.value} value={o.value}>
                          {o.label}
                        </option>
                      ))}
                    </select>
                  </Field>

                  <Field label="Duração" hint={`${override.durationMs} ms`}>
                    <Slider
                      value={override.durationMs}
                      min={80}
                      max={3000}
                      step={20}
                      disabled={override.transitionType === 'cut'}
                      onChange={(v) => setSceneOverride({ ...override, durationMs: v })}
                    />
                  </Field>
                </div>

                <CustomTransitionFields
                  transitionType={normalizeTransitionType(override.transitionType)}
                  custom={overrideCustom}
                  onChange={(patch) =>
                    setSceneOverride({
                      ...override,
                      custom: { ...overrideCustom, ...patch }
                    })
                  }
                />
              </div>
            )}
          </div>
        )}
      </div>
    </Modal>
  )
}

export function ScenesDock({ onSelectScene }: { onSelectScene: () => void }): React.JSX.Element {
  const { settings, update, toast, pushUndoSnapshot, undo } = useStudio()
  const live = useEngineState().live
  const st = settings.scenes
  const current = editingScene(st)
  const [renaming, setRenaming] = useState<string | null>(null)
  const [dragId, setDragId] = useState<string | null>(null)
  const [dropTarget, setDropTarget] = useState<{ id: string; pos: 'top' | 'bottom' } | null>(null)
  const [configOpen, setConfigOpen] = useState(false)

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

  const activeTransitionType = normalizeTransitionType(st.transitionType)
  const hasCustomOverride = Boolean(current?.transitionOverride)

  return (
    <div className="dock">
      <div className="dock-head">
        <span>Cenas</span>
        <div className="row" style={{ gap: 4, alignItems: 'center' }}>
          <select
            className="dock-select"
            value={activeTransitionType}
            onChange={(e) => set({ ...st, transitionType: e.target.value as TransitionType })}
            title="Transição ao trocar de cena"
          >
            {TRANSITION_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          {activeTransitionType !== 'cut' && (
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
              <option value={1200}>1.2s</option>
              <option value={1500}>1.5s</option>
            </select>
          )}
          <IconButton
            label="Configurar transição (Stinger, Luma, Shader, Easing e por cena)"
            active={
              hasCustomOverride ||
              activeTransitionType === 'stinger' ||
              activeTransitionType === 'luma-wipe' ||
              activeTransitionType === 'custom-shader'
            }
            style={{ width: 24, height: 24 }}
            onClick={() => setConfigOpen(true)}
          >
            <IconSliders size={13} />
          </IconButton>
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
              {sc.transitionOverride && (
                <span
                  className="tag"
                  title={`Transição própria: ${TRANSITION_OPTIONS.find((o) => o.value === normalizeTransitionType(sc.transitionOverride!.transitionType))?.label ?? sc.transitionOverride.transitionType} (${sc.transitionOverride.durationMs}ms)`}
                >
                  fx
                </span>
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

      <TransitionConfigModal
        open={configOpen}
        onClose={() => setConfigOpen(false)}
        state={st}
        currentScene={current}
        onChange={set}
      />
    </div>
  )
}
