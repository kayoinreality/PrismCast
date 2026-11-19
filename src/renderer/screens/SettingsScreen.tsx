import { useEffect, useMemo, useRef, useState } from 'react'
import type { AppInfo } from '@shared/ipc'
import type { HotkeyBindings, StudioCompositorSettings } from '@shared/protocol'
import { Button, Field, Segmented, Slider, TextInput, Toggle } from '@/components/ui'
import { useEngineState, useStudio } from '@/studio/StudioContext'

const HOTKEY_LABEL: Record<keyof HotkeyBindings, string> = {
  toggleMicMute: 'Mutar / ligar microfone',
  toggleSystemMute: 'Mutar / ligar áudio do PC',
  togglePrivacyShield: 'Tela de pausa',
  saveReplayClip: 'Salvar replay',
  toggleVerticalPreview: 'Ligar / desligar saída 9:16',
  toggleCamera: 'Ligar / desligar câmera'
}

const DEFAULT_HOTKEYS: HotkeyBindings = {
  toggleMicMute: 'CommandOrControl+Shift+M',
  toggleSystemMute: 'CommandOrControl+Shift+D',
  togglePrivacyShield: 'CommandOrControl+Shift+P',
  saveReplayClip: 'CommandOrControl+Shift+S',
  toggleVerticalPreview: 'CommandOrControl+Shift+V',
  toggleCamera: 'CommandOrControl+Shift+C'
}

function prettyAccel(a: string): string {
  return a ? a.replace('CommandOrControl', 'Ctrl').split('+').join(' + ') : 'nenhum'
}

function normalizeCanonical(a: string): string {
  return a
    .trim()
    .replace(/^Ctrl\+/i, 'CommandOrControl+')
    .toLowerCase()
}

/** Mapeamento baseado em `e.code` (evita `'Dead'` ou símbolos errados em teclados ABNT2). */
function keyFromEvent(e: React.KeyboardEvent): string | null {
  const code = e.code
  if (code.startsWith('Key') && code.length === 4) return code.slice(3).toUpperCase()
  if (code.startsWith('Digit') && code.length === 6) return code.slice(5)
  if (code.startsWith('Numpad') && code.length === 7) return code
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code
  const map: Record<string, string> = {
    Space: 'Space',
    ArrowUp: 'Up',
    ArrowDown: 'Down',
    ArrowLeft: 'Left',
    ArrowRight: 'Right',
    Comma: 'Comma',
    Period: 'Period',
    Slash: 'Slash',
    Minus: 'Minus',
    Equal: 'Equal',
    BracketLeft: 'BracketLeft',
    BracketRight: 'BracketRight',
    Backslash: 'Backslash',
    Semicolon: 'Semicolon',
    Quote: 'Quote',
    Backquote: 'Backquote',
    Insert: 'Insert',
    Home: 'Home',
    End: 'End',
    PageUp: 'PageUp',
    PageDown: 'PageDown',
    Pause: 'Pause'
  }
  if (map[code]) return map[code]
  if (e.key.length === 1 && /[a-z0-9]/i.test(e.key)) return e.key.toUpperCase()
  return null
}

/** Clique e aperte o atalho novo. Precisa de Ctrl, Alt ou Shift para não brigar com o jogo. */
function HotkeyInput({
  value,
  conflict,
  onChange
}: {
  value: string
  conflict?: boolean
  onChange: (v: string) => void
}): React.JSX.Element {
  const [recording, setRecording] = useState(false)
  return (
    <button
      type="button"
      className={`hotkey-btn ${recording ? 'is-recording' : ''} ${conflict ? 'is-conflict' : ''}`}
      style={conflict ? { color: 'var(--danger)', boxShadow: 'inset 0 0 0 1px rgba(255, 92, 114, 0.5)' } : undefined}
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
        const key = keyFromEvent(e)
        if (!key) return
        const parts = [e.ctrlKey && 'CommandOrControl', e.altKey && 'Alt', e.shiftKey && 'Shift', key].filter(Boolean)
        onChange(parts.join('+'))
        setRecording(false)
      }}
      title={conflict ? 'Atalho duplicado com outra ação! Clique para trocar.' : 'Clique e aperte a combinação. Backspace apaga.'}
    >
      {recording ? 'Aperte o atalho…' : <kbd>{prettyAccel(value)}</kbd>}
    </button>
  )
}

export function SettingsScreen(): React.JSX.Element {
  const { settings, update } = useStudio()
  const state = useEngineState()
  const [info, setInfo] = useState<AppInfo | null>(null)
  const [replaySeconds, setReplaySeconds] = useState(settings.studio.replayBufferSeconds)
  const [ffmpegDraft, setFfmpegDraft] = useState(settings.ffmpegPath ?? '')
  const replayTimer = useRef<number>(0)
  const studio = settings.studio

  useEffect(() => {
    let cancelled = false
    void window.api.app
      .info()
      .then((res) => {
        if (!cancelled) setInfo(res)
      })
      .catch(() => {})
    return () => {
      cancelled = true
      window.clearTimeout(replayTimer.current)
    }
  }, [])

  useEffect(() => {
    setReplaySeconds(settings.studio.replayBufferSeconds)
  }, [settings.studio.replayBufferSeconds])

  useEffect(() => {
    setFfmpegDraft(settings.ffmpegPath ?? '')
  }, [settings.ffmpegPath])

  const setStudio = (p: Partial<StudioCompositorSettings>): void => update({ studio: { ...studio, ...p } })

  // Detecta atalhos duplicados em tempo real (sem precisar chamar IPC a cada tecla).
  const duplicatedKeys = useMemo(() => {
    const counts = new Map<string, number>()
    for (const k of Object.keys(HOTKEY_LABEL) as Array<keyof HotkeyBindings>) {
      const norm = normalizeCanonical(settings.hotkeys[k])
      if (!norm) continue
      counts.set(norm, (counts.get(norm) ?? 0) + 1)
    }
    const dups = new Set<keyof HotkeyBindings>()
    for (const k of Object.keys(HOTKEY_LABEL) as Array<keyof HotkeyBindings>) {
      const norm = normalizeCanonical(settings.hotkeys[k])
      if (norm && (counts.get(norm) ?? 0) > 1) dups.add(k)
    }
    return dups
  }, [settings.hotkeys])

  const pickRecordingFolder = async (): Promise<void> => {
    const folder = await window.api.kit.pickFolder()
    if (folder) update({ recordingDir: folder })
  }

  const openRecordingFolder = (): void => {
    const target = state.recorder.replaySavedPath ?? state.recorder.recordingPath ?? settings.recordingDir
    if (target) {
      window.api.app.showInFolder(target)
    }
  }

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
            <Field label="Caminho do FFmpeg (opcional)" hint={state.multistream.ffmpegFound ? 'detectado' : 'não encontrado'}>
              <div className="row" style={{ gap: 'var(--sp-2)' }}>
                <TextInput
                  value={ffmpegDraft}
                  placeholder="Automático (PATH do sistema)"
                  className="mono-input"
                  onChange={(e) => setFfmpegDraft(e.target.value)}
                  onBlur={() => {
                    const trimmed = ffmpegDraft.trim() || null
                    if (trimmed !== (settings.ffmpegPath ?? null)) update({ ffmpegPath: trimmed })
                  }}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      const trimmed = ffmpegDraft.trim() || null
                      update({ ffmpegPath: trimmed })
                    }
                  }}
                />
                {ffmpegDraft && (
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => {
                      setFfmpegDraft('')
                      update({ ffmpegPath: null })
                    }}
                  >
                    Automático
                  </Button>
                )}
              </div>
            </Field>
          </div>
        </section>

        <section className="section">
          <div className="section-title">Áudio</div>
          <div className="panel">
            <span className="hint">
              Microfones, áudio do PC e áudio de cada programa ficam no mixer do Estúdio. Clique no ícone de ajustes
              de uma fonte (ou clique com o botão direito nela) para escolher o dispositivo e mexer nos filtros:
              supressão de ruído, noise gate, compressor, limitador, ganho, equalizador, ducking e monitoramento.
            </span>
          </div>
        </section>

        <section className="section">
          <div className="section-title">Gravação e replay</div>
          <div className="panel">
            <Field label="Duração do replay" hint={`${replaySeconds} segundos`}>
              <Slider
                value={replaySeconds}
                min={10}
                max={120}
                step={5}
                onChange={(v) => {
                  setReplaySeconds(v)
                  setStudio({ replayBufferSeconds: v })
                  window.clearTimeout(replayTimer.current)
                  replayTimer.current = window.setTimeout(() => {
                    void window.api.recorder.setReplayBuffer(state.recorder.replayBufferActive, v)
                  }, 250)
                }}
              />
            </Field>
            <Field label="Pasta das gravações e replays">
              <div className="row" style={{ gap: 'var(--sp-2)', flexWrap: 'wrap' }}>
                <TextInput
                  readOnly
                  value={settings.recordingDir || 'Vídeos\\PrismCast (padrão)'}
                  className="mono-input"
                  style={{ flex: 1, minWidth: 200 }}
                />
                <Button size="sm" onClick={() => void pickRecordingFolder()}>
                  Escolher pasta
                </Button>
                {settings.recordingDir && (
                  <Button size="sm" variant="ghost" onClick={() => update({ recordingDir: null })}>
                    Padrão
                  </Button>
                )}
                <Button
                  size="sm"
                  variant="subtle"
                  disabled={!state.recorder.replaySavedPath && !state.recorder.recordingPath && !settings.recordingDir}
                  onClick={openRecordingFolder}
                >
                  Abrir pasta
                </Button>
              </div>
            </Field>
            <span className="hint">
              Gravações e replays são salvos em MP4 (ou WebM se o FFmpeg não estiver disponível).
            </span>
          </div>
        </section>

        <section className="section">
          <div className="row" style={{ justifyContent: 'space-between', alignItems: 'center' }}>
            <div className="section-title">Atalhos globais</div>
            <Button size="sm" variant="ghost" onClick={() => update({ hotkeys: { ...DEFAULT_HOTKEYS } })}>
              Restaurar padrões
            </Button>
          </div>
          <div className="panel">
            <div className="hotkeys">
              {(Object.keys(HOTKEY_LABEL) as Array<keyof HotkeyBindings>).map((k) => (
                <div key={k} className="hotkey-row">
                  <span>{HOTKEY_LABEL[k]}</span>
                  <HotkeyInput
                    value={settings.hotkeys[k]}
                    conflict={duplicatedKeys.has(k)}
                    onChange={(v) => update({ hotkeys: { ...settings.hotkeys, [k]: v } })}
                  />
                </div>
              ))}
            </div>
            {duplicatedKeys.size > 0 && (
              <span className="error-text">Há atalhos repetidos em vermelho. Troque um deles para evitar conflito.</span>
            )}
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
