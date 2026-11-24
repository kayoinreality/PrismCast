import { useEffect, useMemo, useRef, useState } from 'react'
import type { AppInfo } from '@shared/ipc'
import {
  OBS_WS_DEFAULT_PORT,
  type CanvasOrientation,
  type DynamicBitrateSettings,
  type ExternalControlSettings,
  type GpuCaptureBackendKind,
  type GpuPipelineSettings,
  type HotkeyBindings,
  type StudioCompositorSettings,
  type VirtualCameraDevice,
  type VirtualCameraSettings
} from '@shared/protocol'
import { Badge, Button, Field, Segmented, Slider, StatPill, TextInput, Toggle } from '@/components/ui'
import { IconCopy, IconEye, IconEyeOff, IconKey, IconRefresh } from '@/components/Icons'
import { useEngineState, useStudio } from '@/studio/StudioContext'
import {
  studioStore,
  useAbrTelemetrySlice,
  useExternalControlSlice,
  useGpuStatusSlice,
  useVirtualCamSlice
} from '@/studio/StudioStore'

const DEFAULT_GPU_PIPELINE: GpuPipelineSettings = {
  zeroCopyEnabled: true,
  preferWgpu: true,
  captureBackend: 'auto',
  nativeEncoderPipe: true
}

const DEFAULT_ABR_SETTINGS: DynamicBitrateSettings = {
  enabled: true,
  minBitrateKbps: 1500,
  stepDownRatio: 0.75,
  stepUpKbps: 250,
  stabilityWindowSecs: 10,
  usePersistentRtmp: true
}

const DEFAULT_VIRTUAL_CAM_SETTINGS: VirtualCameraSettings = {
  enabled: false,
  orientation: 'horizontal',
  fps: 30,
  deviceName: 'PrismCast Virtual Camera',
  linuxDevicePath: null
}

const DEFAULT_EXTERNAL_CONTROL: ExternalControlSettings = {
  obsWsEnabled: false,
  obsWsPort: OBS_WS_DEFAULT_PORT,
  obsWsHasPassword: false,
  obsWsAllowLan: false,
  rpcExternalEnabled: false,
  allowBrowserOrigins: []
}

function generateStrongPassword(length = 20): string {
  const alphabet = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789-_!@#'
  const bytes = new Uint8Array(length)
  if (typeof crypto !== 'undefined' && crypto.getRandomValues) {
    crypto.getRandomValues(bytes)
  } else {
    for (let i = 0; i < length; i++) bytes[i] = Math.floor(Math.random() * 256)
  }
  let out = ''
  for (let i = 0; i < length; i++) {
    out += alphabet[bytes[i] % alphabet.length]
  }
  return out
}

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
  const { settings, update, toast } = useStudio()
  const state = useEngineState()
  const gpuStatus = useGpuStatusSlice()
  const abrTelemetry = useAbrTelemetrySlice()
  const vcamStatus = useVirtualCamSlice()
  const extStatus = useExternalControlSlice()

  const [info, setInfo] = useState<AppInfo | null>(null)
  const [replaySeconds, setReplaySeconds] = useState(settings.studio.replayBufferSeconds)
  const [ffmpegDraft, setFfmpegDraft] = useState(settings.ffmpegPath ?? '')
  const [vcamDevices, setVcamDevices] = useState<VirtualCameraDevice[]>([])
  const [loadingVcamDevices, setLoadingVcamDevices] = useState(false)
  const [refreshingGpu, setRefreshingGpu] = useState(false)

  const gpu: GpuPipelineSettings = { ...DEFAULT_GPU_PIPELINE, ...(settings.gpuPipeline ?? {}) }
  const abr: DynamicBitrateSettings = { ...DEFAULT_ABR_SETTINGS, ...(settings.dynamicBitrate ?? {}) }
  const vcam: VirtualCameraSettings = { ...DEFAULT_VIRTUAL_CAM_SETTINGS, ...(settings.virtualCamera ?? {}) }
  const ext: ExternalControlSettings = { ...DEFAULT_EXTERNAL_CONTROL, ...(settings.externalControl ?? {}) }

  const [linuxVcamDraft, setLinuxVcamDraft] = useState(vcam.linuxDevicePath ?? '')
  const [obsPortDraft, setObsPortDraft] = useState(String(ext.obsWsPort || OBS_WS_DEFAULT_PORT))
  const [obsPasswordDraft, setObsPasswordDraft] = useState('')
  const [showObsPassword, setShowObsPassword] = useState(false)
  const [savingObsPassword, setSavingObsPassword] = useState(false)
  const [restartingExt, setRestartingExt] = useState(false)

  const replayTimer = useRef<number>(0)
  const studio = settings.studio

  const refreshGpuStatus = async (): Promise<void> => {
    const api = window.api.gpu
    if (!api) return
    setRefreshingGpu(true)
    try {
      const next = await api.status()
      studioStore.setGpuStatus(next)
    } catch {
      /* ignore */
    } finally {
      setRefreshingGpu(false)
    }
  }

  const refreshVcamDevices = async (): Promise<void> => {
    const api = window.api.virtualCam
    if (!api) return
    setLoadingVcamDevices(true)
    try {
      const list = await api.devices()
      setVcamDevices(Array.isArray(list) ? list : [])
    } catch {
      setVcamDevices([])
    } finally {
      setLoadingVcamDevices(false)
    }
  }

  useEffect(() => {
    let cancelled = false
    void window.api.app
      .info()
      .then((res) => {
        if (!cancelled) setInfo(res)
      })
      .catch(() => {})
    void window.api.gpu
      ?.status()
      .then((st) => {
        if (!cancelled) studioStore.setGpuStatus(st)
      })
      .catch(() => {})
    void window.api.externalControl
      ?.status()
      .then((st) => {
        if (!cancelled) studioStore.setExternalControlStatus(st)
      })
      .catch(() => {})
    void window.api.virtualCam
      ?.devices()
      .then((list) => {
        if (!cancelled && Array.isArray(list)) setVcamDevices(list)
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

  useEffect(() => {
    setLinuxVcamDraft(settings.virtualCamera?.linuxDevicePath ?? '')
  }, [settings.virtualCamera?.linuxDevicePath])

  useEffect(() => {
    setObsPortDraft(String(settings.externalControl?.obsWsPort || OBS_WS_DEFAULT_PORT))
  }, [settings.externalControl?.obsWsPort])

  const setStudio = (p: Partial<StudioCompositorSettings>): void => update({ studio: { ...studio, ...p } })
  const setGpu = (p: Partial<GpuPipelineSettings>): void => update({ gpuPipeline: { ...gpu, ...p } })
  const setAbr = (p: Partial<DynamicBitrateSettings>): void => update({ dynamicBitrate: { ...abr, ...p } })
  const setVcam = (p: Partial<VirtualCameraSettings>): void => update({ virtualCamera: { ...vcam, ...p } })
  const setExt = (p: Partial<ExternalControlSettings>): void => update({ externalControl: { ...ext, ...p } })

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

  const saveObsPassword = async (password: string, clearAfter = true, silent = false): Promise<boolean> => {
    const api = window.api.externalControl
    if (!api) {
      toast('Controle externo indisponível neste ambiente', 'bad')
      return false
    }
    setSavingObsPassword(true)
    try {
      const res = await api.setObsPassword(password)
      if (!res.ok) {
        toast(res.error || 'Falha ao salvar a senha do obs-websocket', 'bad')
        return false
      }
      setExt({ obsWsHasPassword: password.trim().length > 0 })
      if (clearAfter) setObsPasswordDraft('')
      if (!silent) {
        toast(
          password.trim().length > 0
            ? 'Senha do obs-websocket salva no cofre seguro'
            : 'Senha do obs-websocket removida',
          'good'
        )
      }
      return true
    } catch (err) {
      toast(err instanceof Error ? err.message : 'Erro ao definir senha', 'bad')
      return false
    } finally {
      setSavingObsPassword(false)
    }
  }

  const generateAndSaveObsPassword = async (): Promise<void> => {
    const generated = generateStrongPassword(20)
    setObsPasswordDraft(generated)
    setShowObsPassword(true)
    const ok = await saveObsPassword(generated, false, true)
    if (!ok) return
    if (navigator.clipboard?.writeText) {
      void navigator.clipboard.writeText(generated).then(
        () => toast('Senha gerada, salva no cofre e copiada', 'good'),
        () => toast('Senha gerada e salva no cofre seguro', 'good')
      )
    } else {
      toast('Senha gerada e salva no cofre seguro', 'good')
    }
  }

  const copyObsWsUrl = (): void => {
    const port = extStatus.obsWsPort || ext.obsWsPort || OBS_WS_DEFAULT_PORT
    const url = `ws://127.0.0.1:${port}`
    if (navigator.clipboard?.writeText) {
      void navigator.clipboard.writeText(url).then(
        () => toast(`URL copiada: ${url}`, 'good'),
        () => toast(`URL: ${url}`, 'info')
      )
    } else {
      toast(`URL: ${url}`, 'info')
    }
  }

  const restartExternalControl = async (): Promise<void> => {
    const api = window.api.externalControl
    if (!api) return
    setRestartingExt(true)
    try {
      const next = await api.restart()
      studioStore.setExternalControlStatus(next)
      if (next.error) toast(next.error, 'bad')
      else toast('Servidor obs-websocket reiniciado', 'good')
    } catch (err) {
      toast(err instanceof Error ? err.message : 'Falha ao reiniciar servidor', 'bad')
    } finally {
      setRestartingExt(false)
    }
  }

  const commitObsPort = (): void => {
    const parsed = Number.parseInt(obsPortDraft, 10)
    const valid = Number.isFinite(parsed) && parsed >= 1024 && parsed <= 65535 ? parsed : OBS_WS_DEFAULT_PORT
    setObsPortDraft(String(valid))
    if (valid !== ext.obsWsPort) setExt({ obsWsPort: valid })
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
          <div className="row" style={{ justifyContent: 'space-between', alignItems: 'center' }}>
            <div className="section-title">Pipeline GPU (Zero-Copy)</div>
            <div className="row" style={{ gap: 'var(--sp-2)', alignItems: 'center' }}>
              <Badge tone={gpuStatus.zeroCopyActive ? 'live' : 'neutral'}>
                {gpuStatus.zeroCopyActive ? 'Zero-Copy ativo' : 'Modo compatibilidade'}
              </Badge>
              <Button
                size="sm"
                variant="ghost"
                loading={refreshingGpu}
                icon={<IconRefresh size={13} />}
                onClick={() => void refreshGpuStatus()}
              >
                Atualizar
              </Button>
            </div>
          </div>
          <div className="panel">
            <Toggle
              label="Pipeline GPU Zero-Copy"
              hint="Mantém os quadros capturados em memória da placa de vídeo até o encoder, reduzindo CPU e latência."
              checked={gpu.zeroCopyEnabled}
              onChange={(v) => setGpu({ zeroCopyEnabled: v })}
            />
            <Toggle
              label="Compositor wgpu (GPU nativo)"
              hint="Usa DirectX 12 / Vulkan / Metal via wgpu para composição e shaders WGSL."
              checked={gpu.preferWgpu}
              onChange={(v) => setGpu({ preferWgpu: v })}
            />
            <Field label="Backend de captura" hint="Automático escolhe DXGI/WGC no Windows e PipeWire no Linux">
              <select
                className="select"
                value={gpu.captureBackend}
                onChange={(e) => setGpu({ captureBackend: e.target.value as GpuCaptureBackendKind })}
              >
                <option value="auto">Automático (recomendado)</option>
                <option value="dxgi-duplication">DXGI Desktop Duplication (Windows)</option>
                <option value="windows-graphics-capture">Windows Graphics Capture (WGC)</option>
                <option value="pipewire-dmabuf">PipeWire DMA-BUF (Linux Wayland)</option>
                <option value="software-fallback">Fallback por software (CPU)</option>
              </select>
            </Field>
            <Toggle
              label="Pipe direto para encoder nativo"
              hint="Alimenta NVENC / AMF / QuickSync / VAAPI diretamente sem passar pelo MediaRecorder do navegador."
              checked={gpu.nativeEncoderPipe}
              onChange={(v) => setGpu({ nativeEncoderPipe: v })}
            />
            <div
              style={{
                display: 'grid',
                gridTemplateColumns: 'repeat(auto-fit, minmax(160px, 1fr))',
                gap: 'var(--sp-2)'
              }}
            >
              <StatPill label="GPU / Adaptador" value={gpuStatus.adapterName || 'Desconhecido'} />
              <StatPill
                label="Compositor"
                value={gpuStatus.compositorBackend || 'canvas-2d'}
                tone={gpuStatus.zeroCopyActive ? 'good' : 'neutral'}
              />
              <StatPill label="Captura" value={gpuStatus.captureBackend || 'software'} />
              <StatPill
                label="Cópias / quadro"
                value={String(gpuStatus.frameCopyCount)}
                tone={gpuStatus.frameCopyCount === 0 ? 'good' : 'warn'}
              />
            </div>
          </div>
        </section>

        <section className="section">
          <div className="section-title">Bitrate Dinâmico (ABR)</div>
          <div className="panel">
            <Toggle
              label="Ajustar bitrate automaticamente em congestionamento"
              hint="Reduz o bitrate de saída em tempo real quando a rede oscila e recupera gradualmente quando estabilizar."
              checked={abr.enabled}
              onChange={(v) => setAbr({ enabled: v })}
            />
            <Field label="Bitrate mínimo (piso de segurança)" hint={`${abr.minBitrateKbps} kbps`}>
              <Slider
                value={abr.minBitrateKbps}
                min={500}
                max={6000}
                step={100}
                disabled={!abr.enabled}
                onChange={(v) => setAbr({ minBitrateKbps: v })}
              />
            </Field>
            <Toggle
              label="RTMP persistente sem desconexão"
              hint="Mantém a sessão RTMP/RTMPS aberta continuamente ao adaptar o bitrate, evitando queda na live."
              checked={abr.usePersistentRtmp}
              disabled={!abr.enabled}
              onChange={(v) => setAbr({ usePersistentRtmp: v })}
            />
            <Field label="Janela de estabilidade para subir bitrate" hint={`${abr.stabilityWindowSecs} segundos`}>
              <Slider
                value={abr.stabilityWindowSecs}
                min={3}
                max={60}
                step={1}
                disabled={!abr.enabled}
                onChange={(v) => setAbr({ stabilityWindowSecs: v })}
              />
            </Field>
            {abrTelemetry.length > 0 && (
              <div className="row" style={{ gap: 'var(--sp-2)', flexWrap: 'wrap' }}>
                {abrTelemetry.map((d) => (
                  <Badge
                    key={d.id}
                    tone={
                      d.state === 'congested' || d.state === 'floor'
                        ? 'danger'
                        : d.state === 'recovering'
                          ? 'warning'
                          : 'live'
                    }
                  >
                    {d.currentBitrateKbps} kbps · {d.state} ({Math.round(d.congestionLevel * 100)}%)
                  </Badge>
                ))}
              </div>
            )}
          </div>
        </section>

        <section className="section">
          <div className="row" style={{ justifyContent: 'space-between', alignItems: 'center' }}>
            <div className="section-title">Câmera Virtual</div>
            <Badge tone={vcamStatus.active ? 'live' : 'neutral'}>
              {vcamStatus.active ? `Ativa · ${vcamStatus.width}x${vcamStatus.height}@${vcamStatus.fps}fps` : 'Parada'}
            </Badge>
          </div>
          <div className="panel">
            <div className="form-grid">
              <Field label="Orientação padrão">
                <Segmented
                  value={vcam.orientation}
                  onChange={(v: CanvasOrientation) => setVcam({ orientation: v })}
                  options={[
                    { value: 'horizontal', label: '16:9 Horizontal' },
                    { value: 'vertical', label: '9:16 Vertical' }
                  ]}
                />
              </Field>
              <Field label="Taxa de quadros (FPS)">
                <Segmented
                  value={String(vcam.fps <= 35 ? 30 : 60) as '30' | '60'}
                  onChange={(v) => setVcam({ fps: Number(v) })}
                  options={[
                    { value: '30', label: '30 fps' },
                    { value: '60', label: '60 fps' }
                  ]}
                />
              </Field>
            </div>

            <Field
              label="Dispositivo Linux v4l2loopback (opcional)"
              hint="Ex.: /dev/video10 — ignorado no Windows (DirectShow / MediaFoundation)"
            >
              <div className="row" style={{ gap: 'var(--sp-2)' }}>
                <TextInput
                  className="mono-input"
                  value={linuxVcamDraft}
                  placeholder="/dev/video10 (automático)"
                  onChange={(e) => setLinuxVcamDraft(e.target.value)}
                  onBlur={() => {
                    const trimmed = linuxVcamDraft.trim() || null
                    if (trimmed !== (vcam.linuxDevicePath ?? null)) setVcam({ linuxDevicePath: trimmed })
                  }}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      const trimmed = linuxVcamDraft.trim() || null
                      setVcam({ linuxDevicePath: trimmed })
                    }
                  }}
                />
                {linuxVcamDraft && (
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => {
                      setLinuxVcamDraft('')
                      setVcam({ linuxDevicePath: null })
                    }}
                  >
                    Automático
                  </Button>
                )}
              </div>
            </Field>

            <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
              <div className="row" style={{ justifyContent: 'space-between', alignItems: 'center' }}>
                <span className="field-label">Dispositivos de câmera virtual detectados</span>
                <Button
                  size="sm"
                  variant="ghost"
                  loading={loadingVcamDevices}
                  icon={<IconRefresh size={13} />}
                  onClick={() => void refreshVcamDevices()}
                >
                  Detectar
                </Button>
              </div>
              {vcamDevices.length === 0 ? (
                <span className="hint">
                  Nenhum dispositivo virtual listado ainda. No Linux carregue o módulo{' '}
                  <span className="mono">v4l2loopback</span>; no Windows o driver virtual é exposto automaticamente.
                </span>
              ) : (
                <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                  {vcamDevices.map((dev) => (
                    <div
                      key={dev.id}
                      className="row"
                      style={{
                        alignItems: 'center',
                        justifyContent: 'space-between',
                        gap: 'var(--sp-2)',
                        padding: '7px 10px',
                        borderRadius: 'var(--r-sm)',
                        background: 'var(--surface)',
                        border: '1px solid var(--border)'
                      }}
                    >
                      <div style={{ minWidth: 0 }}>
                        <div style={{ fontSize: 13, fontWeight: 560 }}>{dev.name}</div>
                        <div className="hint mono" style={{ fontSize: 11.5 }}>
                          {dev.id} · {dev.backend}
                        </div>
                      </div>
                      <div className="row" style={{ gap: 'var(--sp-2)', alignItems: 'center' }}>
                        <Badge tone={dev.available ? 'live' : 'warning'}>
                          {dev.available ? 'Disponível' : 'Indisponível'}
                        </Badge>
                        {dev.id.startsWith('/dev/video') && (
                          <Button
                            size="sm"
                            variant="subtle"
                            onClick={() => {
                              setLinuxVcamDraft(dev.id)
                              setVcam({ linuxDevicePath: dev.id })
                            }}
                          >
                            Usar
                          </Button>
                        )}
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
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
            <div className="section-title">Controladores Externos (Stream Deck · obs-websocket v5)</div>
            <div className="row" style={{ gap: 'var(--sp-2)', alignItems: 'center' }}>
              <Badge tone={extStatus.obsWsRunning ? 'live' : 'neutral'}>
                {extStatus.obsWsRunning
                  ? `Rodando · ${extStatus.obsWsClients} cliente${extStatus.obsWsClients === 1 ? '' : 's'}`
                  : 'Parado'}
              </Badge>
              <Button
                size="sm"
                variant="subtle"
                loading={restartingExt}
                icon={<IconRefresh size={13} />}
                onClick={() => void restartExternalControl()}
              >
                Reiniciar servidor
              </Button>
            </div>
          </div>
          <div className="panel">
            <Toggle
              label="Ativar servidor obs-websocket v5"
              hint="Compatível com Elgato Stream Deck, Touch Portal, Companion e scripts obs-websocket v5."
              checked={ext.obsWsEnabled}
              onChange={(v) => setExt({ obsWsEnabled: v })}
            />

            <div className="form-grid">
              <Field label="Porta do servidor" hint={`Padrão: ${OBS_WS_DEFAULT_PORT}`}>
                <TextInput
                  type="number"
                  min={1024}
                  max={65535}
                  className="mono-input"
                  value={obsPortDraft}
                  onChange={(e) => setObsPortDraft(e.target.value)}
                  onBlur={commitObsPort}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') commitObsPort()
                  }}
                />
              </Field>

              <Field label="Endereço de conexão">
                <div className="row" style={{ gap: 'var(--sp-2)' }}>
                  <TextInput
                    readOnly
                    className="mono-input"
                    value={`ws://127.0.0.1:${extStatus.obsWsPort || ext.obsWsPort || OBS_WS_DEFAULT_PORT}`}
                  />
                  <Button size="sm" variant="subtle" icon={<IconCopy size={13} />} onClick={copyObsWsUrl}>
                    Copiar URL
                  </Button>
                </div>
              </Field>
            </div>

            <Toggle
              label="Permitir conexões da rede local (LAN)"
              hint="Necessário para tablets/celulares com Touch Portal ou Deckboard no mesmo Wi-Fi. Recomenda-se usar senha fixa."
              checked={ext.obsWsAllowLan}
              onChange={(v) => setExt({ obsWsAllowLan: v })}
            />

            <Field
              label="Senha fixa de autenticação"
              hint={ext.obsWsHasPassword ? 'Senha definida no cofre seguro' : 'Sem senha configurada'}
            >
              <div className="row" style={{ gap: 'var(--sp-2)', flexWrap: 'wrap' }}>
                <TextInput
                  type={showObsPassword ? 'text' : 'password'}
                  className="mono-input"
                  style={{ flex: 1, minWidth: 200 }}
                  value={obsPasswordDraft}
                  placeholder={ext.obsWsHasPassword ? '•••••••••••• (digite nova senha para trocar)' : 'Digite ou gere uma senha fixa'}
                  onChange={(e) => setObsPasswordDraft(e.target.value)}
                />
                <Button
                  size="sm"
                  variant="ghost"
                  icon={showObsPassword ? <IconEyeOff size={14} /> : <IconEye size={14} />}
                  onClick={() => setShowObsPassword((v) => !v)}
                >
                  {showObsPassword ? 'Ocultar' : 'Ver'}
                </Button>
                <Button
                  size="sm"
                  variant="primary"
                  loading={savingObsPassword}
                  disabled={!obsPasswordDraft.trim()}
                  onClick={() => void saveObsPassword(obsPasswordDraft.trim(), true)}
                >
                  Salvar senha
                </Button>
                <Button
                  size="sm"
                  variant="subtle"
                  loading={savingObsPassword}
                  icon={<IconKey size={13} />}
                  onClick={() => void generateAndSaveObsPassword()}
                >
                  Gerar senha
                </Button>
                {ext.obsWsHasPassword && (
                  <Button
                    size="sm"
                    variant="danger"
                    loading={savingObsPassword}
                    onClick={() => void saveObsPassword('', true)}
                  >
                    Remover
                  </Button>
                )}
              </div>
              <span className="hint">
                A senha nunca é salva em texto claro no JSON de configurações — ela fica protegida no cofre de credenciais do sistema.
              </span>
            </Field>

            {extStatus.error && <span className="error-text">{extStatus.error}</span>}
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
