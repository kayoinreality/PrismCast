import { useEffect, useState } from 'react'
import type { Scene, SceneItem, SourceSettings, SourceTransform } from '@shared/protocol'
import { Button, Field, IconButton, Slider, TextInput, Toggle } from '@/components/ui'
import { IconClose, IconRefresh } from '@/components/Icons'
import { useEngineState, useStudio } from '@/studio/StudioContext'
import { SOURCE_LABEL, widgetParams, widgetUrl } from '@/studio/scenes'
import { CANVAS_H, CANVAS_V } from '@/studio/Compositor'
import { SourceIcon } from './sourceIcons'
import { SourcePickerModal } from './Pickers'
import type { StageView } from './Stage'

function useVideoInputs(): MediaDeviceInfo[] {
  const [list, setList] = useState<MediaDeviceInfo[]>([])
  useEffect(() => {
    const load = (): void => {
      void navigator.mediaDevices.enumerateDevices().then((d) => setList(d.filter((x) => x.kind === 'videoinput')))
    }
    load()
    navigator.mediaDevices.addEventListener('devicechange', load)
    return () => navigator.mediaDevices.removeEventListener('devicechange', load)
  }, [])
  return list
}

/** Campo numérico em pixels do canvas (o usuário pensa em 1920x1080, não em 0..1). */
function PxInput({ label, value, onChange }: { label: string; value: number; onChange: (v: number) => void }): React.JSX.Element {
  const [text, setText] = useState(String(Math.round(value)))
  useEffect(() => setText(String(Math.round(value))), [value])
  return (
    <label className="px-input">
      <span>{label}</span>
      <input
        value={text}
        inputMode="numeric"
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          const n = Number(text)
          if (Number.isFinite(n)) onChange(n)
          else setText(String(Math.round(value)))
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
        }}
      />
    </label>
  )
}

function CaptureSection({ item, onPatch }: { item: SceneItem; onPatch: (p: Partial<SceneItem>) => void }): React.JSX.Element {
  const { engine } = useStudio()
  useEngineState()
  const [picking, setPicking] = useState(false)
  const cap = engine.capture(item)
  const window_ = item.type === 'window_capture'
  return (
    <div className="insp-section">
      <Field label={window_ ? 'Janela' : 'Tela'}>
        <div className="insp-capture">
          <span className="insp-capture-name">{cap?.label || item.settings.captureName || 'Monitor principal'}</span>
          <span className={`insp-state is-${cap?.state ?? 'loading'}`}>
            {cap?.state === 'live' ? 'capturando' : cap?.state === 'loading' ? 'abrindo…' : 'indisponível'}
          </span>
        </div>
      </Field>
      <div className="row" style={{ gap: 'var(--sp-2)' }}>
        <Button size="sm" onClick={() => setPicking(true)}>
          Trocar {window_ ? 'janela' : 'tela'}
        </Button>
        {cap && cap.state !== 'live' && (
          <Button size="sm" variant="ghost" icon={<IconRefresh size={14} />} onClick={() => engine.retryCapture(item)}>
            Tentar de novo
          </Button>
        )}
      </div>
      <SourcePickerModal
        open={picking}
        kind={window_ ? 'window' : 'screen'}
        onClose={() => setPicking(false)}
        onPick={(src) => {
          setPicking(false)
          onPatch({
            type: src.kind === 'screen' ? 'screen_capture' : 'window_capture',
            name: item.name === item.settings.captureName || !item.settings.captureName ? src.name : item.name,
            settings: { captureId: src.id, captureName: src.name }
          })
        }}
      />
    </div>
  )
}

function WidgetSection({ item, onPatch }: { item: SceneItem; onPatch: (p: Partial<SceneItem>) => void }): React.JSX.Element {
  const { kit, engine } = useStudio()
  const s = item.settings
  const known = kit?.widgets.find((w) => w.file === s.kitWidget)
  const [params, setParams] = useState<Record<string, string>>(() => widgetParams(s.url ?? ''))
  const [url, setUrl] = useState(s.url ?? '')
  useEffect(() => {
    setParams(widgetParams(s.url ?? ''))
    setUrl(s.url ?? '')
  }, [s.url])

  const applyParams = (next: Record<string, string>): void => {
    setParams(next)
    onPatch({ settings: { url: widgetUrl(s.url ?? '', next) } })
  }

  return (
    <div className="insp-section">
      {known && known.params.length > 0 && (
        <>
          <div className="insp-sub">Opções do widget</div>
          {known.params.map((p) => (
            <Field key={p.key} label={p.label}>
              <TextInput
                value={params[p.key] ?? ''}
                placeholder={p.placeholder}
                onChange={(e) => setParams({ ...params, [p.key]: e.target.value })}
                onBlur={() => applyParams(params)}
                onKeyDown={(e) => e.key === 'Enter' && applyParams(params)}
              />
            </Field>
          ))}
        </>
      )}
      <Field label="Endereço">
        <TextInput
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onBlur={() => url !== s.url && onPatch({ settings: { url: url.trim() } })}
          className="mono-input"
        />
      </Field>
      <div className="insp-row">
        <PxInput label="Largura da página" value={s.width ?? 1920} onChange={(v) => onPatch({ settings: { width: Math.max(50, Math.min(3840, v)) } })} />
        <PxInput label="Altura" value={s.height ?? 1080} onChange={(v) => onPatch({ settings: { height: Math.max(50, Math.min(2160, v)) } })} />
      </div>
      <Button size="sm" icon={<IconRefresh size={14} />} onClick={() => engine.reloadBrowserSource(item)}>
        Recarregar widget
      </Button>
    </div>
  )
}

export function Inspector({
  scene,
  item,
  view,
  onPatch,
  onClose
}: {
  scene: Scene | null
  item: SceneItem | null
  view: StageView
  onPatch: (itemId: string, patch: Partial<SceneItem>) => void
  onClose: () => void
}): React.JSX.Element {
  const cams = useVideoInputs()

  if (!scene || !item) {
    return (
      <div className="inspector is-empty">
        <div className="insp-empty">
          <span className="insp-empty-title">Propriedades</span>
          <span className="hint">
            Selecione uma fonte no palco ou na lista de fontes para ajustar posição, tamanho e opções.
          </span>
          <div className="insp-shortcuts">
            <span>
              <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> Volto já
            </span>
            <span>
              <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>S</kbd> Salvar replay
            </span>
            <span>
              <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>M</kbd> Mutar microfone
            </span>
          </div>
        </div>
      </div>
    )
  }

  const vertical = view === 'vertical'
  const canvas = vertical ? CANVAS_V : CANVAS_H
  const tfKey = vertical ? 'verticalTransform' : 'transform'
  const tf = item[tfKey]
  const patch = (p: Partial<SceneItem>): void => onPatch(item.id, p)
  const setTf = (p: Partial<SourceTransform>): void => patch({ [tfKey]: { ...tf, ...p } })
  const setS = (p: SourceSettings): void => patch({ settings: p })

  const fill = (): void => setTf({ x: 0, y: 0, width: 1, height: 1 })
  const center = (): void => setTf({ x: (1 - tf.width) / 2, y: (1 - tf.height) / 2 })
  /** Ajusta a altura para a proporção do conteúdo (16:9) mantendo a largura. */
  const band = (y: number, h: number): void => setTf({ x: 0, y, width: 1, height: h })

  return (
    <div className="inspector">
      <div className="insp-head">
        <span className="insp-icon">
          <SourceIcon type={item.type} size={16} />
        </span>
        <input className="insp-name" value={item.name} onChange={(e) => patch({ name: e.target.value })} />
        <IconButton label="Fechar" onClick={onClose}>
          <IconClose size={15} />
        </IconButton>
      </div>
      <div className="insp-type">{SOURCE_LABEL[item.type]}</div>

      <div className="insp-body">
        {(item.type === 'screen_capture' || item.type === 'window_capture') && (
          <CaptureSection item={item} onPatch={patch} />
        )}

        {item.type === 'camera_input' && (
          <div className="insp-section">
            <Field label="Câmera">
              <select
                className="select"
                value={item.settings.deviceId ?? 'default'}
                onChange={(e) => {
                  const dev = cams.find((c) => c.deviceId === e.target.value)
                  setS({ deviceId: e.target.value, deviceLabel: dev?.label })
                }}
              >
                <option value="default">Câmera padrão do Windows</option>
                {cams
                  .filter((c) => c.deviceId && c.deviceId !== 'default')
                  .map((c) => (
                    <option key={c.deviceId} value={c.deviceId}>
                      {c.label || 'Câmera'}
                    </option>
                  ))}
              </select>
            </Field>
            <Toggle label="Cantos arredondados" checked={Boolean(item.settings.rounded)} onChange={(v) => setS({ rounded: v })} />
          </div>
        )}

        {item.type === 'browser_source' && <WidgetSection item={item} onPatch={patch} />}

        {item.type === 'image_source' && (
          <div className="insp-section">
            <Field label="Arquivo">
              <div className="insp-capture">
                <span className="insp-capture-name">{item.settings.path?.split(/[\\/]/).pop()}</span>
              </div>
            </Field>
            <Button
              size="sm"
              onClick={() => void window.api.files.pickImage().then((path) => path && setS({ path }))}
            >
              Trocar imagem
            </Button>
            <Toggle label="Cantos arredondados" checked={Boolean(item.settings.rounded)} onChange={(v) => setS({ rounded: v })} />
          </div>
        )}

        {item.type === 'color_source' && (
          <div className="insp-section">
            <Field label="Cor">
              <div className="row" style={{ gap: 'var(--sp-2)' }}>
                <input type="color" className="color-input" value={item.settings.color ?? '#000000'} onChange={(e) => setS({ color: e.target.value })} />
                <TextInput value={item.settings.color ?? ''} onChange={(e) => setS({ color: e.target.value })} className="mono-input" />
              </div>
            </Field>
          </div>
        )}

        {item.type === 'text_source' && (
          <div className="insp-section">
            <Field label="Texto">
              <TextInput value={item.settings.text ?? ''} onChange={(e) => setS({ text: e.target.value })} />
            </Field>
            <Field label="Tamanho" hint={`${item.settings.fontSize ?? 48}px`}>
              <Slider value={item.settings.fontSize ?? 48} min={16} max={200} onChange={(v) => setS({ fontSize: v })} />
            </Field>
            <Field label="Cor">
              <input type="color" className="color-input" value={item.settings.color ?? '#ffffff'} onChange={(e) => setS({ color: e.target.value })} />
            </Field>
          </div>
        )}

        <div className="insp-section">
          <div className="insp-sub">Posição no {vertical ? '9:16' : '16:9'}</div>
          <div className="insp-row">
            <PxInput label="X" value={tf.x * canvas.width} onChange={(v) => setTf({ x: v / canvas.width })} />
            <PxInput label="Y" value={tf.y * canvas.height} onChange={(v) => setTf({ y: v / canvas.height })} />
          </div>
          <div className="insp-row">
            <PxInput label="Largura" value={tf.width * canvas.width} onChange={(v) => setTf({ width: Math.max(10, v) / canvas.width })} />
            <PxInput label="Altura" value={tf.height * canvas.height} onChange={(v) => setTf({ height: Math.max(10, v) / canvas.height })} />
          </div>
          <div className="insp-chips">
            <button onClick={fill}>Tela cheia</button>
            <button onClick={center}>Centralizar</button>
            {vertical && (
              <>
                <button onClick={() => band(0, 0.5)}>Metade de cima</button>
                <button onClick={() => band(0.5, 0.5)}>Metade de baixo</button>
                <button onClick={() => band(0.34, 0.316)}>Faixa 16:9</button>
              </>
            )}
          </div>
        </div>

        <div className="insp-section">
          <Toggle label="Aparece no 16:9" checked={item.showOnHorizontal} onChange={(v) => patch({ showOnHorizontal: v })} />
          <Toggle label="Aparece no 9:16" checked={item.showOnVertical} onChange={(v) => patch({ showOnVertical: v })} />
          <Toggle label="Travar posição" checked={item.locked} onChange={(v) => patch({ locked: v })} />
        </div>
      </div>
    </div>
  )
}
