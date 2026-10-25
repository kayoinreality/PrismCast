import { useEffect, useRef, useState, type ReactNode } from 'react'
import type { AudioApp, AudioChannel, AudioFilters, AudioMonitorMode } from '@shared/protocol'
import { defaultFilters, MIN_DB } from '@shared/audio'
import { Modal } from '@/components/Modal'
import { Button, Field, IconButton, Segmented, Slider, TextInput, Toggle } from '@/components/ui'
import { IconApp, IconRefresh, IconSearch, IconVolume } from '@/components/Icons'
import { useEngineState, useStudio } from '@/studio/StudioContext'
import { KindIcon, Meter, type useMeterLoop } from './MixerDock'

const pct = (db: number): number => Math.max(0, Math.min(100, ((db - MIN_DB) / -MIN_DB) * 100))

/** Nível de entrada (antes dos filtros) com a linha do limiar do noise gate. */
function InputMeter({ id, gateDb }: { id: string; gateDb: number | null }): React.JSX.Element {
  const { engine } = useStudio()
  const fill = useRef<HTMLDivElement>(null)
  const open = useRef<HTMLDivElement>(null)
  useEffect(() => {
    let raf = 0
    const loop = (): void => {
      raf = requestAnimationFrame(loop)
      const db = engine.readLevels()[id]?.inputDb ?? MIN_DB
      if (fill.current) fill.current.style.clipPath = `inset(0 ${100 - pct(db)}% 0 0)`
      if (open.current) open.current.classList.toggle('is-open', gateDb === null || db >= gateDb)
    }
    raf = requestAnimationFrame(loop)
    return () => cancelAnimationFrame(raf)
  }, [engine, id, gateDb])
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
          {value > 0 && unit === 'dB' && label.includes('Ganho') ? '+' : ''}
          {Number.isInteger(step) ? Math.round(value) : value.toFixed(1)} {unit}
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
      {onToggle ? <Toggle label={title} hint={hint} checked={on} onChange={onToggle} /> : (
        <div className="filter-title">
          <span className="toggle-label">{title}</span>
          <span className="toggle-hint">{hint}</span>
        </div>
      )}
      {children && (onToggle ? on : true) && <div className="filter-body">{children}</div>}
    </div>
  )
}

function useInputs(): MediaDeviceInfo[] {
  const [list, setList] = useState<MediaDeviceInfo[]>([])
  useEffect(() => {
    const load = (): void => {
      void navigator.mediaDevices
        .enumerateDevices()
        .then((d) => setList(d.filter((x) => x.kind === 'audioinput' && x.deviceId !== 'default' && x.deviceId !== 'communications')))
    }
    load()
    navigator.mediaDevices.addEventListener('devicechange', load)
    return () => navigator.mediaDevices.removeEventListener('devicechange', load)
  }, [])
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

  const load = (): void => {
    setApps(null)
    void window.api.audio.listApps().then((list) =>
      setApps([...list].sort((a, b) => Number(b.audio) - Number(a.audio) || a.title.localeCompare(b.title)))
    )
  }
  useEffect(() => {
    if (open) {
      setQuery('')
      load()
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
  const { engine } = useStudio()
  useEngineState()
  const inputs = useInputs()
  const [pickingApp, setPickingApp] = useState(false)
  const [confirmRemove, setConfirmRemove] = useState(false)
  const status = engine.audioStatus(ch.id)
  const f = ch.filters
  const mic = ch.kind === 'mic'
  const setF = (p: Partial<AudioFilters>): void => onChange({ filters: { ...f, ...p } })

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
            <Button variant="danger" onClick={onRemove}>
              Remover “{ch.name}”?
            </Button>
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
            <Num label="Corte" value={f.highpassHz || 85} min={40} max={300} unit="Hz" onChange={(v) => setF({ highpassHz: v })} />
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
            <span className="hint">A linha amarela no medidor de entrada é o limiar: fale normal e deixe a fala passar dela.</span>
          </FilterCard>

          <FilterCard
            title="Compressor"
            hint="Segura gritos e aproxima falas baixas"
            on={f.compressor.enabled}
            onToggle={(v) => setF({ compressor: { ...f.compressor, enabled: v } })}
          >
            <div className="filter-grid">
              <Num label="Limiar" value={f.compressor.thresholdDb} min={-50} max={0} unit="dB" onChange={(v) => setF({ compressor: { ...f.compressor, thresholdDb: v } })} />
              <Num label="Razão" value={f.compressor.ratio} min={1.5} max={20} step={0.5} unit=":1" onChange={(v) => setF({ compressor: { ...f.compressor, ratio: v } })} />
              <Num label="Ataque" value={f.compressor.attackMs} min={1} max={100} unit="ms" onChange={(v) => setF({ compressor: { ...f.compressor, attackMs: v } })} />
              <Num label="Liberação" value={f.compressor.releaseMs} min={20} max={1000} step={10} unit="ms" onChange={(v) => setF({ compressor: { ...f.compressor, releaseMs: v } })} />
              <Num label="Ganho de saída" value={f.compressor.makeupDb} min={0} max={24} unit="dB" onChange={(v) => setF({ compressor: { ...f.compressor, makeupDb: v } })} />
            </div>
          </FilterCard>

          <FilterCard title="Ganho" hint="Sobe ou desce a fonte antes do fader (mic baixo demais, jogo alto demais)" on>
            <Num label="Ganho" value={f.gainDb} min={-30} max={30} unit="dB" onChange={(v) => setF({ gainDb: v })} />
          </FilterCard>

          <FilterCard
            title="Limitador"
            hint="Teto de segurança: nada passa disso, não estoura nos ouvidos de ninguém"
            on={f.limiter.enabled}
            onToggle={(v) => setF({ limiter: { ...f.limiter, enabled: v } })}
          >
            <Num label="Teto" value={f.limiter.ceilingDb} min={-12} max={0} step={0.5} unit="dB" onChange={(v) => setF({ limiter: { ...f.limiter, ceilingDb: v } })} />
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
