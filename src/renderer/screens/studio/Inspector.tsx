import { useEffect, useState } from 'react'
import type { KitParam, Scene, SceneItem, SourceSettings, SourceTransform } from '@shared/protocol'
import { makeChannel } from '@shared/audio'
import { Button, Field, IconButton, Slider, TextInput, Toggle } from '@/components/ui'
import { IconCameraOff, IconClose, IconEdit, IconRefresh } from '@/components/Icons'
import { useEngineState, useStudio } from '@/studio/StudioContext'
import { clearTransformCrop, SOURCE_LABEL, widgetParams, widgetUrl } from '@/studio/scenes'
import { CANVAS_H, CANVAS_V } from '@/studio/Compositor'
import { SourceIcon } from './sourceIcons'
import { SourcePickerModal } from './Pickers'
import { KitTextsModal } from '../KitTextsModal'
import { kitWidgetOf, restartsOnShow } from '@/studio/widgets'
import type { StageView } from './Stage'

function useVideoInputs(enabled: boolean): MediaDeviceInfo[] {
  const [list, setList] = useState<MediaDeviceInfo[]>([])
  useEffect(() => {
    if (!enabled || !navigator.mediaDevices) return
    let alive = true
    const load = (): void => {
      void navigator.mediaDevices
        .enumerateDevices()
        .then((d) => {
          if (alive) setList(d.filter((x) => x.kind === 'videoinput'))
        })
        .catch(() => {})
    }
    load()
    navigator.mediaDevices.addEventListener('devicechange', load)
    return () => {
      alive = false
      navigator.mediaDevices.removeEventListener('devicechange', load)
    }
  }, [enabled])
  return list
}

function validHexColor(raw: string | undefined, fallback = '#000000'): string {
  return raw && /^#[0-9a-fA-F]{6}$/.test(raw.trim()) ? raw.trim() : fallback
}

function formatAccel(raw: string | undefined, fallback: string): string[] {
  return (raw?.trim() || fallback)
    .replace(/CommandOrControl/gi, 'Ctrl')
    .split('+')
    .map((s) => s.trim())
    .filter(Boolean)
}

/** Campo numérico em pixels do canvas (o usuário pensa em 1920x1080, não em 0..1). */
function PxInput({
  label,
  value,
  onChange
}: {
  label: string
  value: number
  onChange: (v: number) => void
}): React.JSX.Element {
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
          if (text.trim() !== '' && Number.isFinite(n)) onChange(n)
          else setText(String(Math.round(value)))
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
        }}
      />
    </label>
  )
}

function CaptureSection({
  item,
  onPatch
}: {
  item: SceneItem
  onPatch: (p: Partial<SceneItem>) => void
}): React.JSX.Element {
  const { engine, settings, update, toast } = useStudio()
  useEngineState()
  const [picking, setPicking] = useState(false)
  const [linking, setLinking] = useState(false)
  const cap = engine.capture(item)
  const window_ = item.type === 'window_capture'
  const mixer = settings.audioMixer
  const audioCh = mixer.channels.find((c) => c.id === item.settings.audioChannelId) ?? null

  const toggleAudio = async (on: boolean): Promise<void> => {
    if (!on) {
      if (audioCh) {
        const usedElsewhere = settings.scenes.scenes.some((sc) =>
          sc.items.some((it) => it.id !== item.id && it.settings.audioChannelId === audioCh.id)
        )
        if (!usedElsewhere) {
          update({ audioMixer: { ...mixer, channels: mixer.channels.filter((c) => c.id !== audioCh.id) } })
        }
      }
      onPatch({ settings: { audioChannelId: undefined } })
      return
    }
    const id = item.settings.captureId
    if (!id?.startsWith('window:')) return
    setLinking(true)
    try {
      const app = await window.api.audio.appForWindow(id)
      if (!app) {
        toast('Não achei o programa dessa janela. Ela está aberta?', 'bad')
        return
      }
      const existing = mixer.channels.find(
        (c) => c.kind === 'app' && c.appExe?.toLowerCase() === app.exe.toLowerCase()
      )
      const ch = existing ?? makeChannel('app', { name: item.name.slice(0, 40), appExe: app.exe })
      if (!existing) update({ audioMixer: { ...mixer, channels: [...mixer.channels, ch] } })
      onPatch({ settings: { audioChannelId: ch.id } })
      toast(
        mixer.channels.some((c) => c.kind === 'desktop' && !c.muted)
          ? `Áudio de ${app.exe} no mixer. Com “Áudio do PC” ligado ele sai em dobro: mute um dos dois.`
          : `Áudio de ${app.exe} no mixer`,
        'good'
      )
    } catch {
      toast('Falha ao localizar o áudio desta janela.', 'bad')
    } finally {
      setLinking(false)
    }
  }

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
      {window_ && (
        <Toggle
          label="Capturar o áudio desta janela"
          hint={
            audioCh
              ? `Canal “${audioCh.name}” no mixer (${audioCh.appExe})`
              : 'Só o som deste programa, num canal próprio do mixer'
          }
          checked={audioCh !== null}
          disabled={linking}
          onChange={(v) => void toggleAudio(v)}
        />
      )}
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

/** Um parâmetro do widget com o controle certo (lista, número, liga/desliga, texto). */
function ParamControl({
  param,
  value,
  onChange
}: {
  param: KitParam
  value: string | undefined
  onChange: (v: string) => void
}): React.JSX.Element {
  const [text, setText] = useState(value ?? '')
  useEffect(() => setText(value ?? ''), [value])

  switch (param.type) {
    case 'toggle': {
      const on = value === undefined || value === '' ? param.defaultOn : value === param.on
      return (
        <Toggle
          label={param.label}
          hint={param.hint}
          checked={on}
          onChange={(v) => onChange(v ? param.on : param.off)}
        />
      )
    }
    case 'select':
      return (
        <Field label={param.label} hint={param.hint}>
          <select className="select" value={value ?? ''} onChange={(e) => onChange(e.target.value)}>
            {param.options.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </Field>
      )
    case 'number': {
      const parsed = Number(text || param.placeholder)
      const current = Number.isFinite(parsed) ? parsed : 0
      const clamp = (n: number): string =>
        String(Math.max(param.min ?? 0, Math.min(param.max ?? 9999, Number.isFinite(n) ? n : 0)))
      return (
        <Field label={param.label} hint={param.unit}>
          <div className="stepper">
            <button aria-label="Menos" onClick={() => onChange(clamp(current - 1))}>
              –
            </button>
            <input
              value={text}
              inputMode="numeric"
              placeholder={param.placeholder}
              onChange={(e) => setText(e.target.value.replace(/[^0-9.]/g, ''))}
              onBlur={() => {
                if (text !== (value ?? '')) {
                  const n = Number(text)
                  onChange(text && Number.isFinite(n) ? clamp(n) : '')
                }
              }}
              onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
            />
            <button aria-label="Mais" onClick={() => onChange(clamp(current + 1))}>
              +
            </button>
          </div>
        </Field>
      )
    }
    default:
      return (
        <Field label={param.label} hint={param.hint}>
          <TextInput
            value={text}
            placeholder={param.placeholder}
            onChange={(e) => setText(e.target.value)}
            onBlur={() => text !== (value ?? '') && onChange(text)}
            onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
          />
        </Field>
      )
  }
}

function MicSection({
  item,
  onPatch
}: {
  item: SceneItem
  onPatch: (p: Partial<SceneItem>) => void
}): React.JSX.Element {
  const { settings } = useStudio()
  const mics = settings.audioMixer.channels.filter((c) => c.kind === 'mic')
  const current = mics.find((c) => c.id === item.settings.audioChannelId)
  return (
    <div className="insp-section">
      <Field label="Microfone do mixer">
        <select
          className="select"
          value={current?.id ?? ''}
          onChange={(e) => {
            const ch = mics.find((c) => c.id === e.target.value)
            if (ch) onPatch({ settings: { audioChannelId: ch.id }, name: ch.name })
          }}
        >
          {!current && <option value="">Escolha um microfone</option>}
          {mics.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>
      </Field>
      <Toggle
        label="Ligado nesta cena"
        hint={
          item.visible
            ? 'O som vai para a live enquanto esta cena estiver no ar'
            : 'Desligado: a cena fica sem este microfone'
        }
        checked={item.visible}
        onChange={(v) => onPatch({ visible: v })}
      />
      <span className="hint">
        Microfone só toca nas cenas que têm esta fonte. Volume, mudo e filtros ficam no mixer (Áudio).
      </span>
    </div>
  )
}

function WidgetSection({
  item,
  onPatch
}: {
  item: SceneItem
  onPatch: (p: Partial<SceneItem>) => void
}): React.JSX.Element {
  const { kit, engine } = useStudio()
  const s = item.settings
  const known = kitWidgetOf(item, kit)
  const params = widgetParams(s.url ?? '')
  const [url, setUrl] = useState(s.url ?? '')
  const [advanced, setAdvanced] = useState(!known)
  const [texts, setTexts] = useState(false)
  useEffect(() => setUrl(s.url ?? ''), [s.url])

  const setParam = (key: string, v: string): void => {
    onPatch({ settings: { url: widgetUrl(s.url ?? '', { ...params, [key]: v }) } })
  }

  return (
    <div className="insp-section">
      {known && (
        <>
          <div className="insp-sub">Opções do widget</div>
          {known.params.map((p) => (
            <ParamControl key={p.key} param={p} value={params[p.key]} onChange={(v) => setParam(p.key, v)} />
          ))}
          <Button size="sm" icon={<IconEdit size={14} />} onClick={() => setTexts(true)}>
            Textos do kit (jogo, recados, redes…)
          </Button>
        </>
      )}
      <Toggle
        label="Reiniciar quando a cena entrar no ar"
        hint="Contagem regressiva e cronômetro começam do zero toda vez"
        checked={restartsOnShow(item, kit)}
        onChange={(v) => onPatch({ settings: { restartOnShow: v } })}
      />
      <div className="row" style={{ gap: 'var(--sp-2)', alignItems: 'center' }}>
        <Button size="sm" icon={<IconRefresh size={14} />} onClick={() => engine.reloadBrowserSource(item)}>
          Recarregar agora
        </Button>
        <button className="link-btn insp-more" onClick={() => setAdvanced((a) => !a)}>
          {advanced ? 'Menos opções' : 'Endereço e tamanho'}
        </button>
      </div>
      {advanced && (
        <>
          <Field label="Endereço">
            <TextInput
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              onBlur={() => url !== s.url && onPatch({ settings: { url: url.trim() } })}
              className="mono-input"
            />
          </Field>
          <div className="insp-row">
            <PxInput
              label="Largura da página"
              value={s.width ?? 1920}
              onChange={(v) => onPatch({ settings: { width: Math.max(50, Math.min(3840, v)) } })}
            />
            <PxInput
              label="Altura"
              value={s.height ?? 1080}
              onChange={(v) => onPatch({ settings: { height: Math.max(50, Math.min(2160, v)) } })}
            />
          </div>
        </>
      )}
      <KitTextsModal open={texts} onClose={() => setTexts(false)} />
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
  const isCam = item?.type === 'camera_input'
  const cams = useVideoInputs(Boolean(isCam))
  const { settings } = useStudio()
  const cameraOn = settings.studio.cameraEnabled
  const hk = settings.hotkeys

  if (!scene || !item) {
    const renderKeys = (keys: string[]): React.JSX.Element => (
      <>
        {keys.map((k, idx) => (
          <span key={idx}>
            {idx > 0 && '+'}
            <kbd>{k}</kbd>
          </span>
        ))}
      </>
    )
    return (
      <div className="inspector is-empty">
        <div className="insp-empty">
          <span className="insp-empty-title">Propriedades</span>
          <span className="hint">
            Selecione uma fonte no palco ou na lista de fontes para ajustar posição, tamanho, recorte e opções.
          </span>
          <div className="insp-shortcuts">
            <span>{renderKeys(formatAccel(hk?.togglePrivacyShield, 'Ctrl+Shift+P'))} Tela de pausa</span>
            <span>{renderKeys(formatAccel(hk?.toggleCamera, 'Ctrl+Shift+C'))} Ligar / desligar câmera</span>
            <span>
              <kbd>1</kbd>…<kbd>9</kbd> Trocar de cena
            </span>
            <span>
              <kbd>Ctrl</kbd>+<kbd>Z</kbd> Desfazer
            </span>
            <span>{renderKeys(formatAccel(hk?.saveReplayClip, 'Ctrl+Shift+S'))} Salvar replay</span>
            <span>{renderKeys(formatAccel(hk?.toggleMicMute, 'Ctrl+Shift+M'))} Mutar microfone</span>
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
  const band = (y: number, h: number): void => setTf({ x: 0, y, width: 1, height: h })

  const cropTopPct = Math.round((tf.cropTop ?? 0) * 100)
  const cropBottomPct = Math.round((tf.cropBottom ?? 0) * 100)
  const cropLeftPct = Math.round((tf.cropLeft ?? 0) * 100)
  const cropRightPct = Math.round((tf.cropRight ?? 0) * 100)
  const hasCrop = cropTopPct > 0 || cropBottomPct > 0 || cropLeftPct > 0 || cropRightPct > 0
  const opacityPct = Math.round((tf.opacity ?? 1) * 100)
  const canCropOrFlip =
    item.type === 'screen_capture' ||
    item.type === 'window_capture' ||
    item.type === 'camera_input' ||
    item.type === 'image_source' ||
    item.type === 'browser_source'

  return (
    <div className="inspector" key={item.id}>
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
            {!cameraOn && (
              <div className="insp-note">
                <IconCameraOff size={14} /> A câmera está desligada no estúdio: esta fonte não aparece na live.
              </div>
            )}
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
            <Toggle
              label="Espelhar horizontalmente"
              hint="Inverte esquerda/direita como um espelho"
              checked={Boolean(tf.flipX)}
              onChange={(v) => setTf({ flipX: v })}
            />
            <Toggle
              label="Cantos arredondados"
              checked={Boolean(item.settings.rounded)}
              onChange={(v) => setS({ rounded: v })}
            />
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
            <Toggle
              label="Cantos arredondados"
              checked={Boolean(item.settings.rounded)}
              onChange={(v) => setS({ rounded: v })}
            />
          </div>
        )}

        {item.type === 'color_source' && (
          <div className="insp-section">
            <Field label="Cor">
              <div className="row" style={{ gap: 'var(--sp-2)' }}>
                <input
                  type="color"
                  className="color-input"
                  value={validHexColor(item.settings.color, '#000000')}
                  onChange={(e) => setS({ color: e.target.value })}
                />
                <TextInput
                  value={item.settings.color ?? ''}
                  onChange={(e) => setS({ color: e.target.value })}
                  className="mono-input"
                />
              </div>
            </Field>
          </div>
        )}

        {item.type === 'audio_input' && <MicSection item={item} onPatch={patch} />}

        {item.type === 'text_source' && (
          <div className="insp-section">
            <Field label="Texto">
              <TextInput value={item.settings.text ?? ''} onChange={(e) => setS({ text: e.target.value })} />
            </Field>
            <Field label="Tamanho" hint={`${item.settings.fontSize ?? 48}px`}>
              <Slider
                value={item.settings.fontSize ?? 48}
                min={16}
                max={200}
                onChange={(v) => setS({ fontSize: v })}
              />
            </Field>
            <Field label="Cor">
              <input
                type="color"
                className="color-input"
                value={validHexColor(item.settings.color, '#ffffff')}
                onChange={(e) => setS({ color: e.target.value })}
              />
            </Field>
          </div>
        )}

        {item.type !== 'audio_input' && (
          <>
            <div className="insp-section">
              <div className="insp-sub">Posição no {vertical ? '9:16' : '16:9'}</div>
              <div className="insp-row">
                <PxInput label="X" value={tf.x * canvas.width} onChange={(v) => setTf({ x: v / canvas.width })} />
                <PxInput label="Y" value={tf.y * canvas.height} onChange={(v) => setTf({ y: v / canvas.height })} />
              </div>
              <div className="insp-row">
                <PxInput
                  label="Largura"
                  value={tf.width * canvas.width}
                  onChange={(v) => setTf({ width: Math.max(10, v) / canvas.width })}
                />
                <PxInput
                  label="Altura"
                  value={tf.height * canvas.height}
                  onChange={(v) => setTf({ height: Math.max(10, v) / canvas.height })}
                />
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

            {canCropOrFlip && (
              <div className="insp-section">
                <div className="row" style={{ justifyContent: 'space-between', alignItems: 'center' }}>
                  <div className="insp-sub">Recorte (%) · Alt + arrastar no palco</div>
                  {hasCrop && (
                    <button className="link-btn" onClick={() => patch({ [tfKey]: clearTransformCrop(tf) })}>
                      Zerar
                    </button>
                  )}
                </div>
                <div className="insp-row">
                  <PxInput
                    label="Topo %"
                    value={cropTopPct}
                    onChange={(v) => setTf({ cropTop: Math.max(0, Math.min(85, v)) / 100 })}
                  />
                  <PxInput
                    label="Base %"
                    value={cropBottomPct}
                    onChange={(v) => setTf({ cropBottom: Math.max(0, Math.min(85, v)) / 100 })}
                  />
                </div>
                <div className="insp-row">
                  <PxInput
                    label="Esq %"
                    value={cropLeftPct}
                    onChange={(v) => setTf({ cropLeft: Math.max(0, Math.min(85, v)) / 100 })}
                  />
                  <PxInput
                    label="Dir %"
                    value={cropRightPct}
                    onChange={(v) => setTf({ cropRight: Math.max(0, Math.min(85, v)) / 100 })}
                  />
                </div>
              </div>
            )}

            <div className="insp-section">
              <Field label="Opacidade" hint={`${opacityPct}%`}>
                <Slider
                  value={opacityPct}
                  min={5}
                  max={100}
                  step={1}
                  onChange={(v) => setTf({ opacity: v / 100 })}
                />
              </Field>
              {canCropOrFlip && item.type !== 'camera_input' && (
                <Toggle
                  label="Espelhar horizontalmente"
                  checked={Boolean(tf.flipX)}
                  onChange={(v) => setTf({ flipX: v })}
                />
              )}
              <Toggle
                label="Aparece no 16:9"
                checked={item.showOnHorizontal}
                onChange={(v) => patch({ showOnHorizontal: v })}
              />
              <Toggle
                label="Aparece no 9:16"
                checked={item.showOnVertical}
                onChange={(v) => patch({ showOnVertical: v })}
              />
              <Toggle label="Travar posição" checked={item.locked} onChange={(v) => patch({ locked: v })} />
            </div>
          </>
        )}
      </div>
    </div>
  )
}
