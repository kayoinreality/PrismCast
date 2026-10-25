import { useEffect, useState } from 'react'
import type { AppInfo } from '@shared/ipc'
import type { HotkeyBindings, StudioCompositorSettings } from '@shared/protocol'
import { Field, Segmented, Slider, Toggle } from '@/components/ui'
import { useEngineState, useStudio } from '@/studio/StudioContext'

const HOTKEY_LABEL: Record<keyof HotkeyBindings, string> = {
  toggleMicMute: 'Mutar / ligar microfone',
  toggleSystemMute: 'Mutar / ligar áudio do PC',
  togglePrivacyShield: 'Tela de pausa',
  saveReplayClip: 'Salvar replay',
  toggleVerticalPreview: 'Ligar / desligar saída 9:16',
  toggleCamera: 'Ligar / desligar câmera'
}

function prettyAccel(a: string): string {
  return a ? a.replace('CommandOrControl', 'Ctrl').split('+').join(' + ') : 'nenhum'
}

const KEY_NAMES: Record<string, string> = { ' ': 'Space', ArrowUp: 'Up', ArrowDown: 'Down', ArrowLeft: 'Left', ArrowRight: 'Right' }

/** Clique e aperte o atalho novo. Precisa de Ctrl, Alt ou Shift para não brigar com o jogo. */
function HotkeyInput({ value, onChange }: { value: string; onChange: (v: string) => void }): React.JSX.Element {
  const [recording, setRecording] = useState(false)
  return (
    <button
      className={`hotkey-btn ${recording ? 'is-recording' : ''}`}
      onClick={() => setRecording(true)}
      onBlur={() => setRecording(false)}
      onKeyDown={(e) => {
        if (!recording) return
        e.preventDefault()
        if (e.key === 'Escape') return setRecording(false)
        if (e.key === 'Backspace' || e.key === 'Delete') {
          onChange('')
          return setRecording(false)
        }
        if (['Control', 'Shift', 'Alt', 'Meta'].includes(e.key)) return
        if (!e.ctrlKey && !e.altKey && !e.shiftKey) return
        const key = KEY_NAMES[e.key] ?? (e.key.length === 1 ? e.key.toUpperCase() : e.key)
        const parts = [e.ctrlKey && 'CommandOrControl', e.altKey && 'Alt', e.shiftKey && 'Shift', key].filter(Boolean)
        onChange(parts.join('+'))
        setRecording(false)
      }}
      title="Clique e aperte a combinação. Backspace apaga."
    >
      {recording ? 'Aperte o atalho…' : <kbd>{prettyAccel(value)}</kbd>}
    </button>
  )
}

export function SettingsScreen(): React.JSX.Element {
  const { settings, update } = useStudio()
  const state = useEngineState()
  const [info, setInfo] = useState<AppInfo | null>(null)
  const studio = settings.studio

  useEffect(() => {
    void window.api.app.info().then(setInfo)
  }, [])

  const setStudio = (p: Partial<StudioCompositorSettings>): void => update({ studio: { ...studio, ...p } })

  return (
    <div className="page">
      <div className="page-inner">
        <header className="page-header">
          <h1>Ajustes</h1>
        </header>

        <section className="section">
          <div className="section-title">Vídeo</div>
          <div className="panel">
            <Field label="Quadros por segundo" hint="vale para a live e a gravação">
              <Segmented
                value={String(studio.fps) as '30' | '60'}
                onChange={(v) => setStudio({ fps: Number(v) as 30 | 60 })}
                options={[
                  { value: '30', label: '30 fps' },
                  { value: '60', label: '60 fps' }
                ]}
              />
            </Field>
            <Toggle
              label="Saída vertical 9:16"
              hint="Desenha um segundo canvas para TikTok, Shorts e Reels. Liga sozinha se um destino vertical estiver ativo."
              checked={studio.dualOutputEnabled}
              onChange={(v) => setStudio({ dualOutputEnabled: v })}
            />
            <div className="hint">
              Encoder da live:{' '}
              <b>{state.multistream.ffmpegFound ? state.multistream.encoder : 'FFmpeg não encontrado'}</b>
              {state.multistream.encoder === 'libx264' && ' (sem placa de vídeo compatível: usa o processador)'}
            </div>
          </div>
        </section>

        <section className="section">
          <div className="section-title">Áudio</div>
          <div className="panel">
            <span className="hint">
              Microfones, áudio do PC e áudio de cada programa ficam no mixer do Estúdio. Clique no ícone de ajustes
              de uma fonte (ou clique com o botão direito nela) para escolher o dispositivo e mexer nos filtros:
              supressão de ruído, noise gate, compressor, limitador, ganho e monitoramento.
            </span>
          </div>
        </section>

        <section className="section">
          <div className="section-title">Gravação e replay</div>
          <div className="panel">
            <Field label="Duração do replay" hint={`${studio.replayBufferSeconds} segundos`}>
              <Slider
                value={studio.replayBufferSeconds}
                min={10}
                max={120}
                step={5}
                onChange={(v) => {
                  setStudio({ replayBufferSeconds: v })
                  void window.api.recorder.setReplayBuffer(true, v)
                }}
              />
            </Field>
            <span className="hint">
              Gravações e replays vão para a pasta Vídeos\PrismCast em MP4.{' '}
              {state.recorder.replaySavedPath && (
                <button className="link-btn" onClick={() => window.api.app.showInFolder(state.recorder.replaySavedPath!)}>
                  Abrir pasta
                </button>
              )}
            </span>
          </div>
        </section>

        <section className="section">
          <div className="section-title">Atalhos globais</div>
          <div className="panel">
            <div className="hotkeys">
              {(Object.keys(HOTKEY_LABEL) as Array<keyof HotkeyBindings>).map((k) => (
                <div key={k} className="hotkey-row">
                  <span>{HOTKEY_LABEL[k]}</span>
                  <HotkeyInput value={settings.hotkeys[k]} onChange={(v) => update({ hotkeys: { ...settings.hotkeys, [k]: v } })} />
                </div>
              ))}
            </div>
            <span className="hint">Funcionam mesmo com o jogo em tela cheia. Clique num atalho para trocar.</span>
          </div>
        </section>

        {info && (
          <p className="hint">
            {info.name} {info.version} · {info.hostname}
          </p>
        )}
      </div>
    </div>
  )
}
