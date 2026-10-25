import { useEffect, useState } from 'react'
import type { AppInfo } from '@shared/ipc'
import type { AudioMixerSettings, HotkeyBindings, StudioCompositorSettings } from '@shared/protocol'
import { Field, Segmented, Slider, TextInput, Toggle } from '@/components/ui'
import { useEngineState, useStudio } from '@/studio/StudioContext'

const HOTKEY_LABEL: Record<keyof HotkeyBindings, string> = {
  toggleMicMute: 'Mutar / ligar microfone',
  toggleSystemMute: 'Mutar / ligar áudio do PC',
  togglePrivacyShield: 'Tela de pausa',
  saveReplayClip: 'Salvar replay',
  toggleVerticalPreview: 'Ligar / desligar saída 9:16'
}

function prettyAccel(a: string): string {
  return a.replace('CommandOrControl', 'Ctrl').split('+').join(' + ')
}

export function SettingsScreen(): React.JSX.Element {
  const { settings, update } = useStudio()
  const state = useEngineState()
  const [mics, setMics] = useState<MediaDeviceInfo[]>([])
  const [info, setInfo] = useState<AppInfo | null>(null)
  const studio = settings.studio
  const mixer = settings.audioMixer

  useEffect(() => {
    void window.api.app.info().then(setInfo)
    const load = (): void => {
      void navigator.mediaDevices.enumerateDevices().then((d) => setMics(d.filter((x) => x.kind === 'audioinput')))
    }
    load()
    navigator.mediaDevices.addEventListener('devicechange', load)
    return () => navigator.mediaDevices.removeEventListener('devicechange', load)
  }, [])

  const setStudio = (p: Partial<StudioCompositorSettings>): void => update({ studio: { ...studio, ...p } })
  const setMixer = (p: Partial<AudioMixerSettings>): void => update({ audioMixer: { ...mixer, ...p } })

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
            <Field label="Microfone">
              <select
                className="select"
                value={mixer.micDeviceId ?? 'default'}
                onChange={(e) =>
                  setMixer({ micDeviceId: e.target.value === 'default' ? null : e.target.value, micEnabled: true })
                }
              >
                <option value="default">Microfone padrão do Windows</option>
                {mics
                  .filter((m) => m.deviceId && m.deviceId !== 'default' && m.deviceId !== 'communications')
                  .map((m) => (
                    <option key={m.deviceId} value={m.deviceId}>
                      {m.label || 'Microfone'}
                    </option>
                  ))}
              </select>
            </Field>
            <Toggle label="Usar microfone" checked={mixer.micEnabled} onChange={(v) => setMixer({ micEnabled: v })} />
            <Toggle
              label="Redução de ruído"
              hint="Corta ventoinha e chiado (filtro passa-alta + supressão do Chromium)."
              checked={mixer.noiseSuppression}
              onChange={(v) => setMixer({ noiseSuppression: v })}
            />
            <Toggle
              label="Cancelamento de eco"
              hint="Ligue se você usa caixa de som em vez de fone."
              checked={mixer.echoCancellation}
              onChange={(v) => setMixer({ echoCancellation: v })}
            />
            <Toggle
              label="Compressor"
              hint="Segura gritos e sobe falas baixas."
              checked={mixer.compressorEnabled}
              onChange={(v) => setMixer({ compressorEnabled: v })}
            />
            <Toggle
              label="Capturar áudio do PC"
              hint="Som do jogo, música e alertas que tocam no seu PC."
              checked={mixer.systemAudioEnabled}
              onChange={(v) => setMixer({ systemAudioEnabled: v })}
            />
          </div>
        </section>

        <section className="section">
          <div className="section-title">Tela de pausa</div>
          <div className="panel">
            <div className="form-grid">
              <Field label="Título">
                <TextInput value={studio.privacyShieldTitle} onChange={(e) => setStudio({ privacyShieldTitle: e.target.value })} />
              </Field>
              <Field label="Subtítulo">
                <TextInput
                  value={studio.privacyShieldSubtitle}
                  onChange={(e) => setStudio({ privacyShieldSubtitle: e.target.value })}
                />
              </Field>
            </div>
            <span className="hint">
              Botão de pânico: cobre tudo na hora (Ctrl + Shift + P), para digitar senha ou trocar de janela. Para pausas
              longas use a cena “Volto já” do kit.
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
                  <kbd>{prettyAccel(settings.hotkeys[k])}</kbd>
                </div>
              ))}
            </div>
            <span className="hint">Funcionam mesmo com o jogo em tela cheia.</span>
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
