import { useEffect, useRef, useState, type ReactNode } from 'react'
import type {
  AudioApp,
  AudioChannel,
  AudioFilters,
  AudioMonitorMode,
  AudioPluginInfo,
  AudioPluginSlot
} from '@shared/protocol'
import { defaultFilters, MIN_DB } from '@shared/audio'
import { Modal } from '@/components/Modal'
import { Badge, Button, Field, IconButton, Segmented, Slider, TextInput, Toggle } from '@/components/ui'
import {
  IconApp,
  IconDown,
  IconExternal,
  IconPlus,
  IconRefresh,
  IconSearch,
  IconTrash,
  IconUp,
  IconVolume
} from '@/components/Icons'
import { useEngineState, useStudio } from '@/studio/StudioContext'
import { studioStore } from '@/studio/StudioStore'
import { KindIcon, Meter, type useMeterLoop } from './MixerDock'

const pct = (db: number): number => Math.max(0, Math.min(100, ((db - MIN_DB) / -MIN_DB) * 100))

/** Nível de entrada (antes dos filtros) com a linha do limiar do noise gate (throttled 33ms + gateDbRef). */
function InputMeter({ id, gateDb }: { id: string; gateDb: number | null }): React.JSX.Element {
  const { engine } = useStudio()
  const fill = useRef<HTMLDivElement>(null)
  const open = useRef<HTMLDivElement>(null)
  const gateDbRef = useRef(gateDb)
  gateDbRef.current = gateDb

  useEffect(() => {
    let raf = 0
    let last = 0
    const loop = (t: number): void => {
      raf = requestAnimationFrame(loop)
      if (t - last < 33) return
      last = t
      const db = studioStore.audioLevelsRef[id]?.inputDb ?? engine.readLevels()[id]?.inputDb ?? MIN_DB
      if (fill.current) fill.current.style.clipPath = `inset(0 ${100 - pct(db)}% 0 0)`
      const g = gateDbRef.current
      if (open.current) open.current.classList.toggle('is-open', g === null || db >= g)
    }
    raf = requestAnimationFrame(loop)
    return () => cancelAnimationFrame(raf)
  }, [engine, id])

  return (
    <div className="input-meter">
      <div className="vu is-input">
        <div className="vu-fill" ref={fill} />
        {gateDb !== null && <div className="vu-threshold" style={{ left: `${pct(gateDb)}%` }} />}
      </div>
      {gateDb !== null && (
        <span className="gate-led" ref={open}>
          gate
        </span>
      )}
    </div>
  )
}

function Num({
  label,
  value,
  min,
  max,
  step = 1,
  unit,
  onChange,
  disabled
}: {
  label: string
  value: number
  min: number
  max: number
  step?: number
  unit: string
  onChange: (v: number) => void
  disabled?: boolean
}): React.JSX.Element {
  return (
    <div className={`num-slider ${disabled ? 'is-disabled' : ''}`}>
      <div className="num-slider-head">
        <span>{label}</span>
        <span className="mono">
          {value > 0 && unit === 'dB' && (label.includes('Ganho') || label.includes('Graves') || label.includes('Médios') || label.includes('Agudos'))
            ? '+'
            : ''}
          {Number.isInteger(step) ? Math.round(value) : step < 0.1 ? value.toFixed(2) : value.toFixed(1)}
          {unit ? ` ${unit}` : ''}
        </span>
      </div>
      <Slider value={value} min={min} max={max} step={step} onChange={onChange} disabled={disabled} />
    </div>
  )
}

function FilterCard({
  title,
  hint,
  on,
  onToggle,
  children
}: {
  title: string
  hint: string
  on: boolean
  onToggle?: (v: boolean) => void
  children?: ReactNode
}): React.JSX.Element {
  return (
    <div className={`filter-card ${on ? 'is-on' : ''}`}>
      {onToggle ? (
        <Toggle label={title} hint={hint} checked={on} onChange={onToggle} />
      ) : (
        <div className="filter-title">
          <span className="toggle-label">{title}</span>
          <span className="toggle-hint">{hint}</span>
        </div>
      )}
      {children && (onToggle ? on : true) && <div className="filter-body">{children}</div>}
    </div>
  )
}

function useInputs(enabled: boolean): MediaDeviceInfo[] {
  const [list, setList] = useState<MediaDeviceInfo[]>([])
  useEffect(() => {
    if (!enabled || !navigator.mediaDevices) return
    let alive = true
    const load = (): void => {
      void navigator.mediaDevices
        .enumerateDevices()
        .then((d) => {
          if (alive) {
            setList(
              d.filter(
                (x) => x.kind === 'audioinput' && x.deviceId !== 'default' && x.deviceId !== 'communications'
              )
            )
          }
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

function useOutputs(enabled: boolean): MediaDeviceInfo[] {
  const [list, setList] = useState<MediaDeviceInfo[]>([])
  useEffect(() => {
    if (!enabled || !navigator.mediaDevices) return
    let alive = true
    const load = (): void => {
      void navigator.mediaDevices
        .enumerateDevices()
        .then((d) => {
          if (alive) {
            setList(
              d.filter(
                (x) => x.kind === 'audiooutput' && x.deviceId !== 'default' && x.deviceId !== 'communications'
              )
            )
          }
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

/** Lista de programas abertos para "áudio de um programa". */
export function AppPickerModal({
  open,
  onClose,
  onPick
}: {
  open: boolean
  onClose: () => void
  onPick: (app: AudioApp) => void
}): React.JSX.Element {
  const [apps, setApps] = useState<AudioApp[] | null>(null)
  const [query, setQuery] = useState('')
  const reqIdRef = useRef(0)

  const load = (): void => {
    const reqId = ++reqIdRef.current
    setApps(null)
    void window.api.audio
      .listApps()
      .then((list) => {
        if (reqId !== reqIdRef.current) return
        setApps([...list].sort((a, b) => Number(b.audio) - Number(a.audio) || a.title.localeCompare(b.title)))
      })
      .catch(() => {
        if (reqId === reqIdRef.current) setApps([])
      })
  }

  useEffect(() => {
    if (open) {
      setQuery('')
      load()
    } else {
      reqIdRef.current++
    }
  }, [open])

  const q = query.trim().toLowerCase()
  const shown = (apps ?? []).filter((a) => !q || a.title.toLowerCase().includes(q) || a.exe.toLowerCase().includes(q))

  return (
    <Modal
      open={open}
      onClose={onClose}
      width={560}
      icon={<IconApp size={19} />}
      title="Áudio de um programa"
      description="Pega só o som desse programa, como a “Captura de áudio de aplicativo” do OBS. Se ele fechar, o PrismCast espera ele abrir de novo."
    >
      <div className="picker-bar">
        <div className="search-input">
          <IconSearch size={15} />
          <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Procurar programa" autoFocus />
        </div>
        <IconButton label="Atualizar lista" onClick={load}>
          <IconRefresh size={16} />
        </IconButton>
      </div>
      <div className="app-list">
        {apps === null ? (
          <div className="empty-state">
            <span className="spinner" />
          </div>
        ) : shown.length === 0 ? (
          <div className="hint" style={{ padding: 12 }}>
            Nenhum programa aberto com esse nome. Abra o jogo ou programa e clique em atualizar.
          </div>
        ) : (
          shown.map((a) => (
            <button key={`${a.exe}-${a.pid}`} className="app-row" onClick={() => onPick(a)}>
              <span className="app-row-icon">
                <IconApp size={16} />
              </span>
              <span className="app-row-text">
                <span className="app-row-title">{a.title}</span>
                <span className="app-row-exe mono">{a.exe}</span>
              </span>
              {a.audio && (
                <span className="tag tag-live">
                  <IconVolume size={11} /> tocando som
                </span>
              )}
            </button>
          ))
        )}
      </div>
    </Modal>
  )
}

const MONITOR_OPTIONS: Array<{ value: AudioMonitorMode; label: string; hint: string }> = [
  { value: 'off', label: 'Só na live', hint: 'Você não ouve esta fonte pelo PrismCast' },
  { value: 'monitor', label: 'Live e fone', hint: 'Ouve no fone o que vai para a live' },
  { value: 'monitor-only', label: 'Só no fone', hint: 'Não vai para a live' }
]

function PluginChainCard({
  channelId,
  slots,
  onChangeSlots
}: {
  channelId: string
  slots: AudioPluginSlot[]
  onChangeSlots: (next: AudioPluginSlot[]) => void
}): React.JSX.Element {
  const { toast } = useStudio()
  const [scanning, setScanning] = useState(false)
  const [scanned, setScanned] = useState(false)
  const [discovered, setDiscovered] = useState<AudioPluginInfo[]>([])
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [openingGuiId, setOpeningGuiId] = useState<string | null>(null)

  const scanPlugins = async (): Promise<void> => {
    const api = window.api.plugins
    if (!api) {
      toast('Host de plugins VST3 / CLAP indisponível neste ambiente', 'bad')
      return
    }
    setScanning(true)
    try {
      const list = await api.scan()
      const items = Array.isArray(list) ? list : []
      setDiscovered(items)
      setScanned(true)
      if (items.length === 0) {
        toast('Nenhum plugin VST3 ou CLAP encontrado nas pastas padrão', 'info')
      } else {
        toast(`${items.length} plugin${items.length > 1 ? 's' : ''} VST3/CLAP encontrado${items.length > 1 ? 's' : ''}`, 'good')
      }
    } catch (err) {
      setScanned(true)
      setDiscovered([])
      toast(err instanceof Error ? err.message : 'Falha ao escanear plugins', 'bad')
    } finally {
      setScanning(false)
    }
  }

  const addPlugin = (info: AudioPluginInfo): void => {
    const params: Record<string, number> = {}
    for (const p of info.params ?? []) {
      params[String(p.id)] = p.defaultValue
    }
    const slot: AudioPluginSlot = {
      id: `plug-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`,
      name: info.name,
      vendor: info.vendor,
      format: info.format,
      path: info.path,
      pluginUid: info.pluginUid,
      enabled: true,
      wetDry: 1,
      stateB64: null,
      params
    }
    onChangeSlots([...slots, slot])
    if ((info.params ?? []).length > 0) setExpandedId(slot.id)
  }

  const updateSlot = (slotId: string, patch: Partial<AudioPluginSlot>): void => {
    onChangeSlots(slots.map((s) => (s.id === slotId ? { ...s, ...patch } : s)))
  }

  const moveSlot = (idx: number, delta: -1 | 1): void => {
    const to = idx + delta
    if (to < 0 || to >= slots.length) return
    const next = [...slots]
    const [moved] = next.splice(idx, 1)
    next.splice(to, 0, moved)
    onChangeSlots(next)
  }

  const removeSlot = (slotId: string): void => {
    onChangeSlots(slots.filter((s) => s.id !== slotId))
    if (expandedId === slotId) setExpandedId(null)
  }

  const openGui = async (slot: AudioPluginSlot): Promise<void> => {
    const api = window.api.plugins
    if (!api) return
    setOpeningGuiId(slot.id)
    try {
      const res = await api.openGui(channelId, slot.id)
      if (!res.ok) {
        toast(res.error || `Não foi possível abrir a interface de ${slot.name}`, 'bad')
      }
    } catch (err) {
      toast(err instanceof Error ? err.message : 'Erro ao abrir UI do plugin', 'bad')
    } finally {
      setOpeningGuiId(null)
    }
  }

  const setParam = (slot: AudioPluginSlot, paramId: number, value: number): void => {
    updateSlot(slot.id, {
      params: {
        ...slot.params,
        [String(paramId)]: value
      }
    })
    void window.api.plugins?.setParam(channelId, slot.id, paramId, value).catch(() => {})
  }

  return (
    <FilterCard
      title="Plugins VST3 / CLAP"
      hint="Processadores externos em cadeia por canal (EQ profissional, de-esser, RNNoise, reverb)"
      on={slots.some((s) => s.enabled)}
    >
      <div className="row" style={{ justifyContent: 'space-between', alignItems: 'center', gap: 'var(--sp-2)', flexWrap: 'wrap' }}>
        <span className="hint">
          {slots.length === 0
            ? 'Nenhum plugin ativo neste canal.'
            : `${slots.length} plugin${slots.length > 1 ? 's' : ''} na cadeia (${slots.filter((s) => s.enabled).length} ativo${slots.filter((s) => s.enabled).length === 1 ? '' : 's'})`}
        </span>
        <Button
          size="sm"
          variant="subtle"
          loading={scanning}
          icon={<IconRefresh size={14} />}
          onClick={() => void scanPlugins()}
        >
          Escanear plugins
        </Button>
      </div>

      {slots.length > 0 && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
          {slots.map((slot, idx) => {
            const info = discovered.find(
              (d) => (d.pluginUid && d.pluginUid === slot.pluginUid) || d.path === slot.path
            )
            const paramDefs =
              info && info.params.length > 0
                ? info.params
                : Object.entries(slot.params ?? {}).map(([k, v]) => ({
                    id: Number(k),
                    name: `Parâmetro #${k}`,
                    value: v,
                    min: 0,
                    max: 1,
                    defaultValue: v
                  }))
            const isExpanded = expandedId === slot.id
            const wetPct = Math.round(Math.max(0, Math.min(1, slot.wetDry ?? 1)) * 100)

            return (
              <div
                key={slot.id}
                style={{
                  padding: '10px',
                  borderRadius: 'var(--r-sm)',
                  background: 'var(--bg-elevated)',
                  border: `1px solid ${slot.enabled ? 'var(--accent-line)' : 'var(--border)'}`,
                  display: 'flex',
                  flexDirection: 'column',
                  gap: 'var(--sp-2)',
                  opacity: slot.enabled ? 1 : 0.7
                }}
              >
                <div className="row" style={{ alignItems: 'center', gap: 'var(--sp-2)' }}>
                  <Badge tone={slot.format === 'clap' ? 'live' : 'accent'}>
                    {slot.format.toUpperCase()}
                  </Badge>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontSize: 13, fontWeight: 600, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                      {slot.name}
                    </div>
                    {slot.vendor && (
                      <div className="hint" style={{ fontSize: 11.5 }}>
                        {slot.vendor}
                      </div>
                    )}
                  </div>
                  <Toggle
                    label=""
                    ariaLabel={slot.enabled ? `Ignorar ${slot.name}` : `Ativar ${slot.name}`}
                    checked={slot.enabled}
                    onChange={(v) => updateSlot(slot.id, { enabled: v })}
                  />
                  <IconButton
                    label="Subir na cadeia"
                    disabled={idx === 0}
                    onClick={() => moveSlot(idx, -1)}
                  >
                    <IconUp size={14} />
                  </IconButton>
                  <IconButton
                    label="Descer na cadeia"
                    disabled={idx === slots.length - 1}
                    onClick={() => moveSlot(idx, 1)}
                  >
                    <IconDown size={14} />
                  </IconButton>
                  <IconButton
                    label={`Remover ${slot.name}`}
                    variant="danger"
                    onClick={() => removeSlot(slot.id)}
                  >
                    <IconTrash size={14} />
                  </IconButton>
                </div>

                <Num
                  label="Mix Wet / Dry"
                  value={wetPct}
                  min={0}
                  max={100}
                  step={1}
                  unit="%"
                  disabled={!slot.enabled}
                  onChange={(v) => updateSlot(slot.id, { wetDry: v / 100 })}
                />

                <div className="row" style={{ gap: 'var(--sp-2)', flexWrap: 'wrap' }}>
                  <Button
                    size="sm"
                    variant="subtle"
                    loading={openingGuiId === slot.id}
                    icon={<IconExternal size={13} />}
                    onClick={() => void openGui(slot)}
                  >
                    Abrir UI
                  </Button>
                  {paramDefs.length > 0 && (
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => setExpandedId(isExpanded ? null : slot.id)}
                    >
                      {isExpanded ? 'Ocultar parâmetros' : `Parâmetros (${paramDefs.length})`}
                    </Button>
                  )}
                </div>

                {isExpanded && paramDefs.length > 0 && (
                  <div className="filter-grid" style={{ paddingTop: 'var(--sp-2)', borderTop: '1px solid var(--border)' }}>
                    {paramDefs.map((p) => {
                      const rawVal = slot.params?.[String(p.id)] ?? p.value ?? p.defaultValue
                      const span = Math.max(0.001, p.max - p.min)
                      const step = span <= 2 ? 0.01 : span <= 20 ? 0.1 : 1
                      return (
                        <Num
                          key={p.id}
                          label={p.name}
                          value={rawVal}
                          min={p.min}
                          max={p.max}
                          step={step}
                          unit=""
                          disabled={!slot.enabled}
                          onChange={(v) => setParam(slot, p.id, v)}
                        />
                      )
                    })}
                  </div>
                )}
              </div>
            )
          })}
        </div>
      )}

      {discovered.length > 0 && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          <div className="hint" style={{ fontWeight: 600 }}>
            Plugins descobertos ({discovered.length})
          </div>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 4, maxHeight: 190, overflowY: 'auto' }}>
            {discovered.map((p) => (
              <div
                key={`${p.format}-${p.pluginUid || p.path}`}
                className="row"
                style={{
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  gap: 'var(--sp-2)',
                  padding: '6px 9px',
                  borderRadius: 'var(--r-sm)',
                  background: 'var(--bg)',
                  border: '1px solid var(--border)'
                }}
              >
                <div className="row" style={{ alignItems: 'center', gap: 'var(--sp-2)', minWidth: 0, flex: 1 }}>
                  <Badge tone={p.format === 'clap' ? 'live' : 'accent'}>
                    {p.format.toUpperCase()}
                  </Badge>
                  <div style={{ minWidth: 0 }}>
                    <div style={{ fontSize: 12.5, fontWeight: 560, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                      {p.name}
                    </div>
                    <div className="hint" style={{ fontSize: 11 }}>
                      {p.vendor || 'Fabricante desconhecido'}{p.version ? ` · v${p.version}` : ''}
                    </div>
                  </div>
                </div>
                <Button size="sm" variant="subtle" icon={<IconPlus size={13} />} onClick={() => addPlugin(p)}>
                  Adicionar
                </Button>
              </div>
            ))}
          </div>
        </div>
      )}

      {(discovered.length === 0 && (!scanned || slots.length === 0)) && (
        <div
          className="hint"
          style={{
            padding: '8px 10px',
            borderRadius: 'var(--r-sm)',
            background: 'var(--bg)',
            border: '1px dashed var(--border)'
          }}
        >
          {scanned
            ? 'Nenhum plugin .vst3 ou .clap encontrado. Instale seus plugins nas pastas padrão e clique em Escanear novamente:'
            : 'Clique em “Escanear plugins” para procurar efeitos instalados nas pastas padrão:'}
          <div className="mono" style={{ fontSize: 11, marginTop: 4, color: 'var(--text-muted)' }}>
            Windows: C:\Program Files\Common Files\VST3 · C:\Program Files\Common Files\CLAP
            <br />
            Linux: ~/.vst3, /usr/lib/vst3 · ~/.clap, /usr/lib/clap
          </div>
        </div>
      )}
    </FilterCard>
  )
}

/** Propriedades e filtros de uma fonte de áudio (o "Filtros" do OBS numa tela só). */
export function AudioPropertiesModal({
  channel: ch,
  register,
  onChange,
  onRemove,
  onClose
}: {
  channel: AudioChannel
  register: ReturnType<typeof useMeterLoop>
  onChange: (patch: Partial<AudioChannel>) => void
  onRemove: () => void
  onClose: () => void
}): React.JSX.Element {
  const { engine, settings, update } = useStudio()
  useEngineState()
  const mic = ch.kind === 'mic'
  const inputs = useInputs(mic)
  const outputs = useOutputs(ch.monitor !== 'off')
  const [pickingApp, setPickingApp] = useState(false)
  const [confirmRemove, setConfirmRemove] = useState(false)
  const status = engine.audioStatus(ch.id)
  const f = ch.filters
  const setF = (p: Partial<AudioFilters>): void => onChange({ filters: { ...f, ...p } })

  useEffect(() => {
    setConfirmRemove(false)
  }, [ch.id])

  const micChannels = settings.audioMixer.channels.filter((c) => c.kind === 'mic')

  return (
    <Modal
      open
      onClose={onClose}
      width={620}
      icon={<KindIcon kind={ch.kind} size={19} />}
      title={`Propriedades · ${ch.name}`}
      description="Tudo aqui vale na hora, ao vivo. Fale no microfone enquanto ajusta."
      footer={
        <>
          {confirmRemove ? (
            <div className="row" style={{ gap: 'var(--sp-2)' }}>
              <Button variant="danger" onClick={onRemove}>
                Remover “{ch.name}”?
              </Button>
              <Button variant="ghost" onClick={() => setConfirmRemove(false)}>
                Cancelar
              </Button>
            </div>
          ) : (
            <Button variant="ghost" onClick={() => setConfirmRemove(true)}>
              Remover do mixer
            </Button>
          )}
          <div className="spacer" />
          <Button variant="ghost" onClick={() => onChange({ filters: defaultFilters(ch.kind) })}>
            Filtros padrão
          </Button>
          <Button variant="primary" onClick={onClose}>
            Concluir
          </Button>
        </>
      }
    >
      <div className="audio-props">
        <div className="form-grid">
          <Field label="Nome">
            <TextInput value={ch.name} onChange={(e) => onChange({ name: e.target.value })} />
          </Field>
          {mic && (
            <Field label="Dispositivo">
              <select
                className="select"
                value={ch.deviceId ?? ''}
                onChange={(e) => {
                  const dev = inputs.find((d) => d.deviceId === e.target.value)
                  onChange({
                    deviceId: e.target.value || null,
                    ...(dev && (ch.name === 'Microfone' || inputs.some((d) => d.label === ch.name))
                      ? { name: dev.label.replace(/\s*\([0-9a-f]{4}:[0-9a-f]{4}\)$/i, '') }
                      : {})
                  })
                }}
              >
                <option value="">Padrão do Windows</option>
                {inputs.map((d) => (
                  <option key={d.deviceId} value={d.deviceId}>
                    {d.label || 'Microfone'}
                  </option>
                ))}
                {ch.deviceId && ch.deviceId !== 'default' && !inputs.some((d) => d.deviceId === ch.deviceId) && (
                  <option value={ch.deviceId} disabled>
                    Dispositivo desconectado ({ch.deviceId.slice(0, 8)}…)
                  </option>
                )}
              </select>
            </Field>
          )}
          {ch.kind === 'app' && (
            <Field label="Programa">
              <div className="row" style={{ gap: 'var(--sp-2)' }}>
                <span className="insp-capture" style={{ flex: 1 }}>
                  <span className="insp-capture-name mono">{ch.appExe ?? 'nenhum'}</span>
                </span>
                <Button size="sm" onClick={() => setPickingApp(true)}>
                  Trocar
                </Button>
              </div>
            </Field>
          )}
          {ch.kind === 'desktop' && (
            <Field label="Fonte">
              <span className="hint">Tudo que toca no dispositivo de som padrão do Windows.</span>
            </Field>
          )}
          {ch.kind === 'party_voice' && (
            <Field label="Fonte">
              <span className="hint">Voz recebida dos espectadores conectados na Rede Local (LAN).</span>
            </Field>
          )}
        </div>

        {status.message && (
          <div className="audio-status">
            {status.message}
            {mic && (
              <button className="link-btn" onClick={() => engine.mixer.retry(ch.id)}>
                Tentar de novo
              </button>
            )}
          </div>
        )}

        <div className="audio-meters">
          <div>
            <div className="meter-label">Entrada (antes dos filtros)</div>
            <InputMeter id={ch.id} gateDb={f.gate.enabled ? f.gate.thresholdDb : null} />
          </div>
          <div>
            <div className="meter-label">Saída (o que vai para a live)</div>
            <Meter id={`${ch.id}|props`} register={register} muted={ch.muted} />
          </div>
        </div>

        <div className="section-title">Filtros, na ordem em que o som passa</div>
        <div className="filters">
          {mic && (
            <FilterCard title="Do Windows / navegador" hint="Processamento aplicado ao abrir o microfone" on>
              <Toggle
                label="Supressão de ruído"
                hint="Tira ventoinha, teclado e chiado constante"
                checked={f.noiseSuppression}
                onChange={(v) => setF({ noiseSuppression: v })}
              />
              <Toggle
                label="Cancelamento de eco"
                hint="Ligue se o som do PC sai em caixa de som, não em fone"
                checked={f.echoCancellation}
                onChange={(v) => setF({ echoCancellation: v })}
              />
              <Toggle
                label="Ganho automático"
                hint="Sobe e desce o volume sozinho (pode bombear; prefira o compressor)"
                checked={f.autoGain}
                onChange={(v) => setF({ autoGain: v })}
              />
            </FilterCard>
          )}

          <FilterCard
            title="Filtro passa-alta"
            hint="Corta graves: batida na mesa, vento, ronco elétrico"
            on={f.highpassHz > 0}
            onToggle={(v) => setF({ highpassHz: v ? 85 : 0 })}
          >
            <Num
              label="Corte"
              value={f.highpassHz || 85}
              min={40}
              max={300}
              unit="Hz"
              onChange={(v) => setF({ highpassHz: v })}
            />
          </FilterCard>

          <FilterCard
            title="Equalizador (EQ 3 bandas)"
            hint="Modela o timbre da voz ou limpa frequências emboladas"
            on={Boolean(f.eqEnabled)}
            onToggle={(v) => setF({ eqEnabled: v })}
          >
            <div className="filter-grid">
              <Num
                label="Graves (100 Hz)"
                value={f.eqLowDb ?? 0}
                min={-12}
                max={12}
                step={0.5}
                unit="dB"
                onChange={(v) => setF({ eqLowDb: v })}
              />
              <Num
                label="Médios (1 kHz)"
                value={f.eqMidDb ?? 0}
                min={-12}
                max={12}
                step={0.5}
                unit="dB"
                onChange={(v) => setF({ eqMidDb: v })}
              />
              <Num
                label="Agudos (8 kHz)"
                value={f.eqHighDb ?? 0}
                min={-12}
                max={12}
                step={0.5}
                unit="dB"
                onChange={(v) => setF({ eqHighDb: v })}
              />
            </div>
          </FilterCard>

          <FilterCard
            title="Noise gate"
            hint="Fecha a fonte quando o som fica abaixo do limiar (some a respiração e o teclado)"
            on={f.gate.enabled}
            onToggle={(v) => setF({ gate: { ...f.gate, enabled: v } })}
          >
            <Num
              label="Limiar"
              value={f.gate.thresholdDb}
              min={-80}
              max={-10}
              unit="dB"
              onChange={(v) => setF({ gate: { ...f.gate, thresholdDb: v } })}
            />
            <Num
              label="Tempo para fechar"
              value={f.gate.releaseMs}
              min={30}
              max={1000}
              step={10}
              unit="ms"
              onChange={(v) => setF({ gate: { ...f.gate, releaseMs: v } })}
            />
            <span className="hint">
              A linha amarela no medidor de entrada é o limiar: fale normal e deixe a fala passar dela.
            </span>
          </FilterCard>

          <FilterCard
            title="Compressor"
            hint="Segura gritos e aproxima falas baixas"
            on={f.compressor.enabled}
            onToggle={(v) => setF({ compressor: { ...f.compressor, enabled: v } })}
          >
            <div className="filter-grid">
              <Num
                label="Limiar"
                value={f.compressor.thresholdDb}
                min={-50}
                max={0}
                unit="dB"
                onChange={(v) => setF({ compressor: { ...f.compressor, thresholdDb: v } })}
              />
              <Num
                label="Razão"
                value={f.compressor.ratio}
                min={1.5}
                max={20}
                step={0.5}
                unit=":1"
                onChange={(v) => setF({ compressor: { ...f.compressor, ratio: v } })}
              />
              <Num
                label="Ataque"
                value={f.compressor.attackMs}
                min={1}
                max={100}
                unit="ms"
                onChange={(v) => setF({ compressor: { ...f.compressor, attackMs: v } })}
              />
              <Num
                label="Liberação"
                value={f.compressor.releaseMs}
                min={20}
                max={1000}
                step={10}
                unit="ms"
                onChange={(v) => setF({ compressor: { ...f.compressor, releaseMs: v } })}
              />
              <Num
                label="Ganho de saída"
                value={f.compressor.makeupDb}
                min={0}
                max={24}
                unit="dB"
                onChange={(v) => setF({ compressor: { ...f.compressor, makeupDb: v } })}
              />
            </div>
          </FilterCard>

          {!mic && (
            <FilterCard
              title="Audio Ducking (abaixar quando falar)"
              hint="Reduz automaticamente o volume do jogo/música enquanto você fala no microfone"
              on={Boolean(f.duckingEnabled)}
              onToggle={(v) =>
                setF({
                  duckingEnabled: v,
                  duckingSourceId: f.duckingSourceId ?? micChannels[0]?.id ?? null,
                  duckingAmountDb: f.duckingAmountDb ?? -12,
                  duckingReleaseMs: f.duckingReleaseMs ?? 350
                })
              }
            >
              {micChannels.length > 1 && (
                <Field label="Microfone que comanda">
                  <select
                    className="select"
                    value={f.duckingSourceId ?? micChannels[0]?.id ?? ''}
                    onChange={(e) => setF({ duckingSourceId: e.target.value || null })}
                  >
                    {micChannels.map((m) => (
                      <option key={m.id} value={m.id}>
                        {m.name}
                      </option>
                    ))}
                  </select>
                </Field>
              )}
              <div className="filter-grid">
                <Num
                  label="Atenuação"
                  value={f.duckingAmountDb ?? -12}
                  min={-36}
                  max={-2}
                  step={1}
                  unit="dB"
                  onChange={(v) => setF({ duckingAmountDb: v })}
                />
                <Num
                  label="Tempo de retorno"
                  value={f.duckingReleaseMs ?? 350}
                  min={100}
                  max={2500}
                  step={50}
                  unit="ms"
                  onChange={(v) => setF({ duckingReleaseMs: v })}
                />
              </div>
            </FilterCard>
          )}

          <FilterCard
            title="Ganho"
            hint="Sobe ou desce a fonte antes do fader (mic baixo demais, jogo alto demais)"
            on
          >
            <Num label="Ganho" value={f.gainDb} min={-24} max={24} unit="dB" onChange={(v) => setF({ gainDb: v })} />
          </FilterCard>

          <PluginChainCard
            channelId={ch.id}
            slots={f.plugins ?? []}
            onChangeSlots={(plugins) => setF({ plugins })}
          />

          <FilterCard
            title="Limitador"
            hint="Teto de segurança: nada passa disso, não estoura nos ouvidos de ninguém"
            on={f.limiter.enabled}
            onToggle={(v) => setF({ limiter: { ...f.limiter, enabled: v } })}
          >
            <Num
              label="Teto"
              value={f.limiter.ceilingDb}
              min={-12}
              max={0}
              step={0.5}
              unit="dB"
              onChange={(v) => setF({ limiter: { ...f.limiter, ceilingDb: v } })}
            />
          </FilterCard>
        </div>

        <div className="section-title">Monitoramento e sincronia</div>
        <Segmented
          size="sm"
          value={ch.monitor}
          onChange={(v) => onChange({ monitor: v })}
          options={MONITOR_OPTIONS.map((o) => ({ value: o.value, label: o.label, hint: o.hint }))}
        />
        <span className="hint">{MONITOR_OPTIONS.find((o) => o.value === ch.monitor)?.hint}. Use fone para não dar eco.</span>

        {ch.monitor !== 'off' && (
          <Field
            label="Saída de monitoramento (fone)"
            hint="Escolha seu fone para não duplicar no Áudio do PC"
          >
            <select
              className="select"
              value={settings.audioMixer.monitorDeviceId ?? ''}
              onChange={(e) =>
                update({
                  audioMixer: {
                    ...settings.audioMixer,
                    monitorDeviceId: e.target.value || null
                  }
                })
              }
            >
              <option value="">Saída padrão do sistema</option>
              {outputs.map((d) => (
                <option key={d.deviceId} value={d.deviceId}>
                  {d.label || 'Saída de áudio'}
                </option>
              ))}
            </select>
          </Field>
        )}

        <Num
          label="Atraso (para casar com a câmera)"
          value={ch.syncOffsetMs}
          min={0}
          max={1000}
          step={10}
          unit="ms"
          onChange={(v) => onChange({ syncOffsetMs: v })}
        />
      </div>

      <AppPickerModal
        open={pickingApp}
        onClose={() => setPickingApp(false)}
        onPick={(app) => {
          setPickingApp(false)
          onChange({ appExe: app.exe, name: app.title.slice(0, 40) })
        }}
      />
    </Modal>
  )
}
