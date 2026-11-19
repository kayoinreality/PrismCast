import { useState } from 'react'
import { Field, Segmented, Slider, Toggle, type SegmentOption } from './ui'
import {
  BITRATE_MAX_KBPS,
  BITRATE_MIN_KBPS,
  BITRATE_STEP_KBPS,
  CODEC_OPTIONS,
  FPS_OPTIONS,
  HEIGHT_OPTIONS,
  PRESETS,
  matchPreset,
  presetById,
  type PresetId,
  type QualitySettings
} from '@shared/quality'
import { formatBitrate } from '@/rtc/stats'
import './QualityPanel.css'

const PRESET_OPTIONS: SegmentOption<PresetId>[] = [
  ...PRESETS.map((p) => ({ value: p.id, label: p.label, hint: p.hint })),
  { value: 'custom' as PresetId, label: 'Personalizado', hint: 'Ajuste cada item na mão' }
]

export function QualityPanel({
  settings,
  onChange,
  disabled = false,
  audioLocked = false
}: {
  settings: QualitySettings
  onChange: (next: QualitySettings) => void
  disabled?: boolean
  /** Durante a transmissao: ligar ou desligar o audio exigiria recapturar a fonte. */
  audioLocked?: boolean
}): React.JSX.Element {
  // "Personalizado" nao tem valores proprios: e o rotulo de quando os controles
  // saem de qualquer preset. Clicar nele so revela os sliders, sem mexer em
  // nada — abrir o painel nao deve, sozinho, mudar a qualidade da transmissao.
  const [forceCustom, setForceCustom] = useState(false)
  const matched = matchPreset(settings)
  const active = forceCustom ? 'custom' : matched
  const showCustom = active === 'custom'

  const set = <K extends keyof QualitySettings>(key: K, value: QualitySettings[K]): void => {
    onChange({ ...settings, [key]: value })
  }

  const choosePreset = (id: PresetId): void => {
    if (disabled) return
    if (id === 'custom') {
      setForceCustom(true)
      return
    }
    setForceCustom(false)
    const preset = presetById(id)
    if (preset) onChange({ ...preset.settings })
  }

  return (
    <div className={`quality-panel ${disabled ? 'is-disabled' : ''}`}>
      <div className="preset-row">
        <Segmented options={PRESET_OPTIONS} value={active} onChange={choosePreset} size="sm" disabled={disabled} />
      </div>

      {!showCustom && (
        <p className="preset-summary">
          {presetById(active)?.hint} · {settings.maxHeight ? `${settings.maxHeight}p` : 'nativa'} ·{' '}
          {settings.fps} fps · até {formatBitrate(settings.maxBitrateKbps)}
        </p>
      )}

      {showCustom && (
        <div className="custom-grid">
          <Field label="Resolução">
            <Segmented
              size="sm"
              disabled={disabled}
              value={String(settings.maxHeight ?? 'native')}
              onChange={(v) => set('maxHeight', v === 'native' ? null : Number(v))}
              options={HEIGHT_OPTIONS.map((h) => ({
                value: String(h.value ?? 'native'),
                label: h.label
              }))}
            />
          </Field>

          <Field label="Quadros por segundo">
            <Segmented
              size="sm"
              disabled={disabled}
              value={String(settings.fps)}
              onChange={(v) => set('fps', Number(v))}
              options={FPS_OPTIONS.map((f) => ({ value: String(f), label: String(f) }))}
            />
          </Field>

          <Field label="Taxa máxima" hint={formatBitrate(settings.maxBitrateKbps)}>
            <Slider
              min={BITRATE_MIN_KBPS}
              max={BITRATE_MAX_KBPS}
              step={BITRATE_STEP_KBPS}
              value={settings.maxBitrateKbps}
              onChange={(v) => set('maxBitrateKbps', v)}
              disabled={disabled}
            />
          </Field>

          <Field
            label="Quando a rede aperta"
            hint={
              settings.degradation === 'maintain-framerate'
                ? 'perde nitidez'
                : settings.degradation === 'maintain-resolution'
                  ? 'perde fluidez'
                  : 'equilibra'
            }
          >
            <Segmented
              size="sm"
              disabled={disabled}
              value={settings.degradation}
              onChange={(v) => set('degradation', v)}
              options={[
                { value: 'maintain-framerate', label: 'Fluidez', hint: 'Ideal para jogo' },
                { value: 'maintain-resolution', label: 'Nitidez', hint: 'Ideal para vídeo e texto' },
                { value: 'balanced', label: 'Equilíbrio' }
              ]}
            />
          </Field>

          <Field
            label="Codec de vídeo"
            hint={CODEC_OPTIONS.find((c) => c.value === settings.codec)?.hint}
          >
            <Segmented
              size="sm"
              disabled={disabled}
              value={settings.codec}
              onChange={(v) => set('codec', v)}
              options={CODEC_OPTIONS.map((c) => ({
                value: c.value,
                label: c.label,
                hint: c.hint
              }))}
            />
          </Field>
        </div>
      )}

      <div className="quality-audio">
        <Toggle
          checked={settings.audio}
          onChange={(v) => set('audio', v)}
          disabled={disabled || audioLocked}
          label="Enviar o áudio do sistema"
          hint={
            audioLocked
              ? 'Só dá para mudar isso antes de começar: a captura precisa ser refeita.'
              : 'Quem assiste ouve o mesmo que sai da sua caixa de som.'
          }
        />
      </div>
    </div>
  )
}
