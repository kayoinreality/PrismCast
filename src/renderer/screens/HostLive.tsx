import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { AppSettings, DesktopSource } from '@shared/ipc'
import {
  MAX_VIEWERS,
  type CanvasOrientation,
  type HotkeyAction,
  type MultistreamStatus,
  type MultistreamTarget,
  type RecorderStatus,
  type SourceKind,
  type StreamWidget,
  type VerticalLayoutPreset
} from '@shared/protocol'
import type { QualitySettings } from '@shared/quality'
import { Badge, Button, IconButton, Segmented, StatPill, Toggle } from '@/components/ui'
import { Modal } from '@/components/Modal'
import { SourceGrid } from '@/components/SourceGrid'
import { QualityPanel } from '@/components/QualityPanel'
import {
  IconAlert,
  IconCamera,
  IconChat,
  IconCheck,
  IconCopy,
  IconDisc,
  IconExit,
  IconEye,
  IconEyeOff,
  IconGlobe,
  IconLayers,
  IconLock,
  IconMic,
  IconMicOff,
  IconRefresh,
  IconSettings,
  IconSmartphone,
  IconStop,
  IconUsers,
  IconVolume,
  IconVolumeOff
} from '@/components/Icons'
import { LIMITATION_LABEL, formatBitrate } from '@/rtc/stats'
import type { HostBroadcast } from '@/hooks/useHostBroadcast'
import { AudioMixerEngine, type AudioLevels } from '@/studio/AudioMixerEngine'
import { DualCanvasCompositor } from '@/studio/DualCanvasCompositor'
import './HostLive.css'

const STATE_TONE = {
  conectando: 'neutral',
  conectado: 'live',
  instavel: 'warning',
  encerrado: 'danger'
} as const

const STATE_LABEL = {
  conectando: 'conectando',
  conectado: 'conectado',
  instavel: 'instável',
  encerrado: 'encerrado'
} as const

type StudioTab = 'room' | 'scenes' | 'multistream' | 'quality'

const QUICK_EMOJIS = ['🔥', '👏', '😂', '❤️', '🚀', '🎉']

const VERTICAL_PRESET_OPTIONS: { value: VerticalLayoutPreset; label: string }[] = [
  { value: 'split-cam-top', label: 'Split Cam + Jogo' },
  { value: 'blurred-backdrop', label: 'Fundo Desfocado' },
  { value: 'center-crop', label: 'Corte Central' },
  { value: 'pip-vertical', label: 'PiP Vertical' }
]

export function HostLive({
  broadcast,
  stream,
  sourceName,
  settings,
  onSettingsChange,
  onSourceSwitch,
  onStop
}: {
  broadcast: HostBroadcast
  stream: MediaStream | null
  sourceName: string
  settings: AppSettings
  onSettingsChange: (patch: Partial<AppSettings>) => void
  onSourceSwitch: (newStream: MediaStream, newSource: DesktopSource) => void
  onStop: () => void
}): React.JSX.Element {
  const horizVideoRef = useRef<HTMLVideoElement>(null)
  const vertVideoRef = useRef<HTMLVideoElement>(null)
  const applyTimer = useRef<number>(0)

  const mixerRef = useRef<AudioMixerEngine | null>(null)
  const compositorRef = useRef<DualCanvasCompositor | null>(null)

  const [studioTab, setStudioTab] = useState<StudioTab>('room')
  const [copied, setCopied] = useState(false)
  const [copiedAddress, setCopiedAddress] = useState(false)
  const [toast, setToast] = useState<string | null>(null)

  const [levels, setLevels] = useState<AudioLevels>({ system: 0, mic: 0 })

  const [recorderStatus, setRecorderStatus] = useState<RecorderStatus>({
    recording: false,
    recordingPath: null,
    recordingDurationSec: 0,
    replayBufferActive: true,
    replayBufferedSeconds: 0,
    replaySavedPath: null
  })
  const [multistreamStatus, setMultistreamStatus] = useState<MultistreamStatus>({
    active: false,
    encoder: 'libx264',
    horizontalLive: false,
    verticalLive: false,
    targets: []
  })

  const [chatText, setChatText] = useState('')
  const chatEndRef = useRef<HTMLDivElement>(null)

  const [newWidgetName, setNewWidgetName] = useState('')
  const [newWidgetUrl, setNewWidgetUrl] = useState('')

  const [sourceModalOpen, setSourceModalOpen] = useState(false)
  const [availableSources, setAvailableSources] = useState<DesktopSource[]>([])
  const [loadingSources, setLoadingSources] = useState(false)
  const [sourceTab, setSourceTab] = useState<SourceKind>('screen')
  const [selectedNewSource, setSelectedNewSource] = useState<DesktopSource | null>(null)
  const [switchingSource, setSwitchingSource] = useState(false)

  const { info, viewers, states, stats, chatMessages, reactions } = broadcast

  const showToast = useCallback((msg: string): void => {
    setToast(msg)
    window.setTimeout(() => {
      setToast((curr) => (curr === msg ? null : curr))
    }, 4200)
  }, [])

  // 1. Inicializa o AudioMixerEngine e o DualCanvasCompositor na sessao ao vivo
  useEffect(() => {
    const mixer = new AudioMixerEngine(settings.audioMixer)
    const compositor = new DualCanvasCompositor(
      settings.studio,
      settings.widgets,
      mixer.outputTrack
    )
    mixerRef.current = mixer
    compositorRef.current = compositor

    if (stream) {
      mixer.setSystemStream(stream)
      compositor.setSourceStream(stream, sourceName)
    }

    void mixer.applyConfig(settings.audioMixer)

    const compVideoTrack = compositor.outputHorizontalStream.getVideoTracks()[0]
    if (compVideoTrack) {
      void broadcast.replaceVideoTrack(compVideoTrack)
    }
    void broadcast.replaceAudioTrack(mixer.outputTrack)

    if (horizVideoRef.current) {
      horizVideoRef.current.srcObject = compositor.outputHorizontalStream
    }
    if (vertVideoRef.current) {
      vertVideoRef.current.srcObject = compositor.outputVerticalStream
    }

    const levelTimer = window.setInterval(() => {
      setLevels(mixer.readLevels())
    }, 45)

    return () => {
      window.clearInterval(levelTimer)
      compositor.dispose()
      mixer.dispose()
      compositorRef.current = null
      mixerRef.current = null
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    if (vertVideoRef.current && compositorRef.current) {
      vertVideoRef.current.srcObject = compositorRef.current.outputVerticalStream
    }
  }, [settings.studio.dualOutputEnabled])

  useEffect(() => {
    mixerRef.current?.setSystemStream(stream)
    compositorRef.current?.setSourceStream(stream, sourceName)
  }, [stream, sourceName])

  useEffect(() => {
    compositorRef.current?.setMeta({
      hostName: settings.hostName,
      sourceName,
      viewerCount: viewers.length
    })
  }, [settings.hostName, sourceName, viewers.length])

  useEffect(() => {
    void compositorRef.current?.updateStudio(settings.studio)
  }, [settings.studio])

  useEffect(() => {
    void mixerRef.current?.applyConfig(settings.audioMixer)
    broadcast.setPartyVoiceEnabled(settings.audioMixer.partyVoiceEnabled)
  }, [settings.audioMixer, broadcast])

  useEffect(() => {
    compositorRef.current?.updateWidgets(settings.widgets)
  }, [settings.widgets])

  // 2. Polling leve de status de Gravacao/Replay Buffer e Multistream
  useEffect(() => {
    const syncStatuses = (): void => {
      void window.api.recorder.status().then(setRecorderStatus)
      void window.api.multistream.status().then(setMultistreamStatus)
    }
    syncStatuses()
    const timer = window.setInterval(syncStatuses, 1000)
    return () => window.clearInterval(timer)
  }, [])

  // 3. MediaRecorder continuo para Replay Buffer (30s), Gravacao Local e Multistream 16:9 / 9:16
  useEffect(() => {
    const compositor = compositorRef.current
    if (!compositor) return

    const mimeCandidates = [
      'video/webm;codecs=h264,opus',
      'video/webm;codecs=vp8,opus',
      'video/webm'
    ]
    const mimeType = mimeCandidates.find((m) => MediaRecorder.isTypeSupported(m)) ?? ''

    let horizRecorder: MediaRecorder | null = null
    try {
      horizRecorder = new MediaRecorder(compositor.outputHorizontalStream, {
        ...(mimeType ? { mimeType } : {}),
        videoBitsPerSecond: Math.min(8_000_000, settings.quality.maxBitrateKbps * 1000),
        audioBitsPerSecond: 160_000
      })
      horizRecorder.ondataavailable = (ev) => {
        if (!ev.data || ev.data.size === 0) return
        void ev.data.arrayBuffer().then((buf) => {
          const bytes = new Uint8Array(buf)
          window.api.recorder.pushChunk(bytes)
          window.api.multistream.pushChunk('horizontal', bytes)
        })
      }
      horizRecorder.start(1000)
    } catch (err) {
      console.warn('[Studio] MediaRecorder 16:9 indisponivel:', err)
    }

    return () => {
      if (horizRecorder && horizRecorder.state !== 'inactive') {
        try {
          horizRecorder.stop()
        } catch {
          /* ignore */
        }
      }
    }
  }, [settings.quality.maxBitrateKbps])

  useEffect(() => {
    const compositor = compositorRef.current
    const hasVerticalTarget =
      multistreamStatus.active &&
      settings.multistreamTargets.some((t) => t.enabled && t.orientation === 'vertical')

    if (!compositor || !hasVerticalTarget) return

    const mimeCandidates = [
      'video/webm;codecs=h264,opus',
      'video/webm;codecs=vp8,opus',
      'video/webm'
    ]
    const mimeType = mimeCandidates.find((m) => MediaRecorder.isTypeSupported(m)) ?? ''

    let vertRecorder: MediaRecorder | null = null
    try {
      vertRecorder = new MediaRecorder(compositor.outputVerticalStream, {
        ...(mimeType ? { mimeType } : {}),
        videoBitsPerSecond: 4_500_000,
        audioBitsPerSecond: 160_000
      })
      vertRecorder.ondataavailable = (ev) => {
        if (!ev.data || ev.data.size === 0) return
        void ev.data.arrayBuffer().then((buf) => {
          window.api.multistream.pushChunk('vertical', new Uint8Array(buf))
        })
      }
      vertRecorder.start(1000)
    } catch (err) {
      console.warn('[Studio] MediaRecorder 9:16 indisponivel:', err)
    }

    return () => {
      if (vertRecorder && vertRecorder.state !== 'inactive') {
        try {
          vertRecorder.stop()
        } catch {
          /* ignore */
        }
      }
    }
  }, [multistreamStatus.active, settings.multistreamTargets])

  // 4. Atalhos Globais do Windows (Ctrl+Shift+M / D / P / S / V)
  const settingsRef = useRef(settings)
  settingsRef.current = settings

  const handleSaveReplay = useCallback(async (): Promise<void> => {
    const res = await window.api.recorder.saveReplay()
    if (res.ok && res.path) {
      if (res.status) setRecorderStatus(res.status)
      showToast(`Clipe de Replay (30s) salvo em: ${res.path}`)
    } else {
      showToast(res.error ?? 'Aguarde alguns segundos para preencher o Replay Buffer.')
    }
  }, [showToast])

  useEffect(() => {
    const offHotkey = window.api.app.onHotkey((action: HotkeyAction) => {
      const cur = settingsRef.current
      if (action === 'toggle-mic-mute') {
        const micMuted = !cur.audioMixer.micMuted
        onSettingsChange({ audioMixer: { ...cur.audioMixer, micMuted } })
        showToast(micMuted ? 'Microfone silenciado (Ctrl+Shift+M)' : 'Microfone ativado (Ctrl+Shift+M)')
      } else if (action === 'toggle-system-mute') {
        const systemMuted = !cur.audioMixer.systemMuted
        onSettingsChange({ audioMixer: { ...cur.audioMixer, systemMuted } })
        showToast(
          systemMuted
            ? 'Áudio do sistema silenciado (Ctrl+Shift+D)'
            : 'Áudio do sistema ativado (Ctrl+Shift+D)'
        )
      } else if (action === 'toggle-privacy-shield') {
        const privacyShieldActive = !cur.studio.privacyShieldActive
        onSettingsChange({ studio: { ...cur.studio, privacyShieldActive } })
        showToast(
          privacyShieldActive
            ? 'Escudo "Volto Já" ATIVADO (Ctrl+Shift+P)'
            : 'Escudo "Volto Já" DESATIVADO (Ctrl+Shift+P)'
        )
      } else if (action === 'save-replay-clip') {
        void handleSaveReplay()
      } else if (action === 'toggle-vertical-preview') {
        const dualOutputEnabled = !cur.studio.dualOutputEnabled
        onSettingsChange({ studio: { ...cur.studio, dualOutputEnabled } })
        showToast(
          dualOutputEnabled
            ? 'Saída Vertical 9:16 ativada (Ctrl+Shift+V)'
            : 'Saída Vertical 9:16 ocultada (Ctrl+Shift+V)'
        )
      }
    })
    return () => offHotkey()
  }, [onSettingsChange, handleSaveReplay, showToast])

  useEffect(() => {
    chatEndRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [chatMessages.length])

  const applyLive = (next: QualitySettings): void => {
    window.clearTimeout(applyTimer.current)
    applyTimer.current = window.setTimeout(() => {
      compositorRef.current?.setFps(next.fps)
      broadcast.updateSettings(next)
    }, 180)
  }

  useEffect(() => () => window.clearTimeout(applyTimer.current), [])

  const summary = useMemo(() => {
    const all = [...stats.values()]
    if (all.length === 0) {
      return {
        bitrate: 0,
        fps: settings.quality.fps,
        resolution: '1920x1080',
        codec: settings.quality.codec.toUpperCase(),
        loss: 0,
        limited: 'none' as const
      }
    }
    const best = all.reduce((a, b) => (b.fps > a.fps ? b : a))
    return {
      bitrate: all.reduce((sum, s) => sum + s.bitrateKbps + s.audioBitrateKbps, 0),
      fps: best.fps || settings.quality.fps,
      resolution: best.width ? `${best.width}x${best.height}` : '1920x1080',
      codec: best.codec !== '—' ? best.codec : settings.quality.codec.toUpperCase(),
      loss: Math.max(...all.map((s) => s.lossPct)),
      limited: all.find((s) => s.limitedBy !== 'none')?.limitedBy ?? ('none' as const)
    }
  }, [stats, settings.quality.fps, settings.quality.codec])

  const copyCode = async (): Promise<void> => {
    if (!info?.pin) return
    await navigator.clipboard.writeText(info.pin)
    setCopied(true)
    setTimeout(() => setCopied(false), 1800)
  }

  const copyAddress = async (): Promise<void> => {
    if (!info) return
    await navigator.clipboard.writeText(`${info.ip}:${info.port}`)
    setCopiedAddress(true)
    setTimeout(() => setCopiedAddress(false), 1800)
  }

  // Troca de Fonte Ao Vivo
  const openSourceSwitcher = async (): Promise<void> => {
    setSourceModalOpen(true)
    setLoadingSources(true)
    try {
      const list = await window.api.sources.list()
      setAvailableSources(list)
    } finally {
      setLoadingSources(false)
    }
  }

  const confirmSourceSwitch = async (): Promise<void> => {
    if (!selectedNewSource) return
    setSwitchingSource(true)
    try {
      await window.api.sources.select(selectedNewSource.id, settings.quality.audio)
      const newStream = await navigator.mediaDevices.getDisplayMedia({
        video: {
          frameRate: { ideal: settings.quality.fps, max: settings.quality.fps },
          ...(settings.quality.maxHeight ? { height: { max: settings.quality.maxHeight } } : {})
        },
        audio: settings.quality.audio
      })
      onSourceSwitch(newStream, selectedNewSource)
      setSourceModalOpen(false)
      setSelectedNewSource(null)
      showToast(`Fonte alterada ao vivo para: ${selectedNewSource.name}`)
    } catch (err) {
      showToast(`Falha ao trocar de fonte: ${(err as Error).message}`)
    } finally {
      setSwitchingSource(false)
    }
  }

  // Gravacao Local
  const toggleRecording = async (): Promise<void> => {
    if (recorderStatus.recording) {
      const st = await window.api.recorder.stop()
      setRecorderStatus(st)
      showToast(`Gravação finalizada${st.recordingPath ? `: ${st.recordingPath}` : ''}`)
    } else {
      const res = await window.api.recorder.start()
      if (res.status) setRecorderStatus(res.status)
      if (res.ok && res.status?.recordingPath) {
        showToast(`Gravando localmente em: ${res.status.recordingPath}`)
      }
    }
  }

  // Multistreaming
  const toggleMultistream = async (): Promise<void> => {
    if (multistreamStatus.active) {
      const st = await window.api.multistream.stop()
      setMultistreamStatus(st)
      showToast('Multistreaming encerrado.')
    } else {
      const res = await window.api.multistream.start(
        settings.multistreamTargets,
        settings.quality.fps
      )
      if (res.status) setMultistreamStatus(res.status)
      if (!res.ok) {
        showToast(res.error ?? 'Não foi possível iniciar o Multistream.')
      } else {
        showToast(`Multistreaming iniciado via FFmpeg (${res.status?.encoder ?? 'H.264'})!`)
      }
    }
  }

  const updateTarget = (id: string, patch: Partial<MultistreamTarget>): void => {
    const next = settings.multistreamTargets.map((t) => (t.id === id ? { ...t, ...patch } : t))
    onSettingsChange({ multistreamTargets: next })
  }

  const addCustomRtmpTarget = (): void => {
    const id = `custom-${Date.now()}`
    const next: MultistreamTarget[] = [
      ...settings.multistreamTargets,
      {
        id,
        provider: 'custom',
        name: 'Servidor RTMP Personalizado',
        rtmpUrl: 'rtmp://',
        streamKey: '',
        enabled: true,
        orientation: 'horizontal',
        bitrateKbps: 6000
      }
    ]
    onSettingsChange({ multistreamTargets: next })
  }

  const removeCustomTarget = (id: string): void => {
    onSettingsChange({
      multistreamTargets: settings.multistreamTargets.filter((t) => t.id !== id)
    })
  }

  // Widgets & StreamElements
  const updateWidget = (id: string, patch: Partial<StreamWidget>): void => {
    const next = settings.widgets.map((w) => (w.id === id ? { ...w, ...patch } : w))
    onSettingsChange({ widgets: next })
  }

  const addStreamElementsWidget = (): void => {
    const url = newWidgetUrl.trim()
    if (!url) return
    const next: StreamWidget[] = [
      ...settings.widgets,
      {
        id: `se-${Date.now()}`,
        name: newWidgetName.trim() || 'Overlay StreamElements',
        kind: 'streamelements',
        enabled: true,
        url,
        primaryText: newWidgetName.trim() || 'StreamElements Overlay',
        secondaryText: 'Ativo',
        accentColor: '#17d4b2',
        x: 0,
        y: 0,
        width: 1,
        height: 1,
        verticalX: 0,
        verticalY: 0,
        showOnHorizontal: true,
        showOnVertical: true,
        opacity: 1
      }
    ]
    onSettingsChange({ widgets: next })
    setNewWidgetName('')
    setNewWidgetUrl('')
    showToast('Overlay Browser Source / StreamElements adicionado à cena!')
  }

  const removeWidget = (id: string): void => {
    onSettingsChange({
      widgets: settings.widgets.filter((w) => w.id !== id)
    })
  }

  const triggerTestAlert = (): void => {
    compositorRef.current?.triggerAlert(
      'Novo Inscrito · PrismCast Studio!',
      'Kayor acabou de apoiar a transmissão ao vivo!',
      '#17d4b2'
    )
    broadcast.sendReaction('🎉', settings.hostName)
    showToast('Alerta de teste disparado nos canvases 16:9 e 9:16!')
  }

  const handleChatSubmit = (e: React.FormEvent): void => {
    e.preventDefault()
    if (!chatText.trim()) return
    broadcast.sendChat(chatText, settings.hostName)
    setChatText('')
  }

  const browserWidgets = settings.widgets.filter(
    (w) => w.enabled && (w.kind === 'streamelements' || w.kind === 'custom-url') && w.url.trim()
  )

  return (
    <div className="screen fade-in">
      <header className="page-head studio-head">
        <div className="live-dot" aria-hidden="true" />
        <div className="studio-title-wrap">
          <div className="page-title">PrismCast Studio · Ao Vivo</div>
          <div className="page-sub">{sourceName || 'Fonte ativa'}</div>
        </div>

        <div className="studio-quick-actions">
          <Button
            size="sm"
            variant="subtle"
            onClick={() => void openSourceSwitcher()}
            icon={<IconRefresh size={14} />}
          >
            Trocar Fonte
          </Button>

          <Button
            size="sm"
            variant={settings.studio.privacyShieldActive ? 'danger' : 'subtle'}
            onClick={() =>
              onSettingsChange({
                studio: {
                  ...settings.studio,
                  privacyShieldActive: !settings.studio.privacyShieldActive
                }
              })
            }
            icon={
              settings.studio.privacyShieldActive ? (
                <IconEyeOff size={14} />
              ) : (
                <IconEye size={14} />
              )
            }
          >
            {settings.studio.privacyShieldActive ? 'Volto Já: ATIVO' : 'Modo Volto Já'}
          </Button>

          <Button
            size="sm"
            variant={settings.studio.dualOutputEnabled ? 'primary' : 'subtle'}
            onClick={() =>
              onSettingsChange({
                studio: {
                  ...settings.studio,
                  dualOutputEnabled: !settings.studio.dualOutputEnabled
                }
              })
            }
            icon={<IconSmartphone size={14} />}
          >
            9:16 Vertical
          </Button>

          <Button
            size="sm"
            variant={recorderStatus.recording ? 'danger' : 'subtle'}
            onClick={() => void toggleRecording()}
            icon={<IconDisc size={14} />}
          >
            {recorderStatus.recording
              ? `Gravando (${recorderStatus.recordingDurationSec}s)`
              : 'Gravar'}
          </Button>

          <Button
            size="sm"
            variant="subtle"
            onClick={() => void handleSaveReplay()}
            title="Salvar últimos 30 segundos (Ctrl+Shift+S)"
          >
            Salvar Replay ({recorderStatus.replayBufferedSeconds}s)
          </Button>
        </div>

        <div className="spacer" />

        <Badge tone="live" icon={<IconUsers />}>
          {viewers.length}/{MAX_VIEWERS} LAN
        </Badge>
        <Button variant="danger" onClick={onStop} icon={<IconStop size={16} />}>
          Encerrar
        </Button>
      </header>

      {toast && <div className="studio-toast fade-in">{toast}</div>}

      <div className="live-body">
        <section className="live-main">
          {/* Palco Dual Output: 16:9 Horizontal + 9:16 Vertical */}
          <div
            className={`studio-stage ${settings.studio.dualOutputEnabled ? 'has-vertical' : ''}`}
          >
            <div className="live-preview stage-horiz">
              <div className="stage-badge">16:9 HORIZONTAL · 1080p</div>
              <video ref={horizVideoRef} autoPlay muted playsInline />

              {browserWidgets
                .filter((w) => w.showOnHorizontal)
                .map((w) => (
                  <iframe
                    key={w.id}
                    src={w.url}
                    title={w.name}
                    className="studio-widget-iframe"
                    sandbox="allow-scripts allow-same-origin"
                  />
                ))}

              <div className="floating-reactions">
                {reactions.map((r) => (
                  <div key={r.id} className="floating-emoji">
                    <span>{r.emoji}</span>
                    <small>{r.senderName}</small>
                  </div>
                ))}
              </div>
            </div>

            {settings.studio.dualOutputEnabled && (
              <div className="live-preview stage-vert">
                <div className="stage-badge stage-badge-vert">
                  9:16 VERTICAL · TIKTOK / SHORTS
                </div>
                <video ref={vertVideoRef} autoPlay muted playsInline />
                {browserWidgets
                  .filter((w) => w.showOnVertical)
                  .map((w) => (
                    <iframe
                      key={`v-${w.id}`}
                      src={w.url}
                      title={`${w.name} (9:16)`}
                      className="studio-widget-iframe"
                      sandbox="allow-scripts allow-same-origin"
                    />
                  ))}
              </div>
            )}
          </div>

          {/* Mixer de Audio Ao Vivo (Sistema WASAPI + Microfone) */}
          <div className="studio-mixer">
            <div className="mixer-channel">
              <div className="mixer-channel-top">
                <IconButton
                  label={
                    settings.audioMixer.systemMuted
                      ? 'Ativar Áudio do Sistema (Ctrl+Shift+D)'
                      : 'Mutar Áudio do Sistema (Ctrl+Shift+D)'
                  }
                  variant={settings.audioMixer.systemMuted ? 'danger' : 'plain'}
                  onClick={() =>
                    onSettingsChange({
                      audioMixer: {
                        ...settings.audioMixer,
                        systemMuted: !settings.audioMixer.systemMuted
                      }
                    })
                  }
                >
                  {settings.audioMixer.systemMuted ? (
                    <IconVolumeOff size={16} />
                  ) : (
                    <IconVolume size={16} />
                  )}
                </IconButton>
                <span className="mixer-label">Áudio do Sistema</span>
                <span className="mixer-pct mono">
                  {settings.audioMixer.systemMuted
                    ? 'MUDO'
                    : `${Math.round(settings.audioMixer.systemVolume * 100)}%`}
                </span>
              </div>
              <div className="mixer-vu">
                <div
                  className="mixer-vu-fill"
                  style={{ width: `${Math.round(levels.system * 100)}%` }}
                />
              </div>
              <input
                type="range"
                className="slider"
                min={0}
                max={2}
                step={0.02}
                value={settings.audioMixer.systemVolume}
                style={{
                  ['--fill' as string]: `${Math.round((settings.audioMixer.systemVolume / 2) * 100)}%`
                }}
                onChange={(e) =>
                  onSettingsChange({
                    audioMixer: {
                      ...settings.audioMixer,
                      systemVolume: Number(e.target.value),
                      systemMuted: false
                    }
                  })
                }
              />
            </div>

            <div className="mixer-channel">
              <div className="mixer-channel-top">
                <IconButton
                  label={
                    !settings.audioMixer.micEnabled || settings.audioMixer.micMuted
                      ? 'Ativar Microfone (Ctrl+Shift+M)'
                      : 'Mutar Microfone (Ctrl+Shift+M)'
                  }
                  variant={
                    !settings.audioMixer.micEnabled || settings.audioMixer.micMuted
                      ? 'danger'
                      : 'plain'
                  }
                  onClick={() => {
                    if (!settings.audioMixer.micEnabled) {
                      onSettingsChange({
                        audioMixer: {
                          ...settings.audioMixer,
                          micEnabled: true,
                          micMuted: false
                        }
                      })
                    } else {
                      onSettingsChange({
                        audioMixer: {
                          ...settings.audioMixer,
                          micMuted: !settings.audioMixer.micMuted
                        }
                      })
                    }
                  }}
                >
                  {!settings.audioMixer.micEnabled || settings.audioMixer.micMuted ? (
                    <IconMicOff size={16} />
                  ) : (
                    <IconMic size={16} />
                  )}
                </IconButton>
                <span className="mixer-label">Microfone do Host</span>
                <button
                  className={`mixer-chip ${settings.audioMixer.noiseSuppression ? 'is-on' : ''}`}
                  onClick={() =>
                    onSettingsChange({
                      audioMixer: {
                        ...settings.audioMixer,
                        noiseSuppression: !settings.audioMixer.noiseSuppression
                      }
                    })
                  }
                  title="Filtro High-Pass 85Hz + Supressão de Ruído"
                >
                  Anti-Ruído
                </button>
                <button
                  className={`mixer-chip ${settings.audioMixer.compressorEnabled ? 'is-on' : ''}`}
                  onClick={() =>
                    onSettingsChange({
                      audioMixer: {
                        ...settings.audioMixer,
                        compressorEnabled: !settings.audioMixer.compressorEnabled
                      }
                    })
                  }
                  title="Compressor Dinâmico Master"
                >
                  Compressor
                </button>
                <span className="mixer-pct mono">
                  {!settings.audioMixer.micEnabled
                    ? 'OFF'
                    : settings.audioMixer.micMuted
                      ? 'MUDO'
                      : `${Math.round(settings.audioMixer.micVolume * 100)}%`}
                </span>
              </div>
              <div className="mixer-vu">
                <div
                  className="mixer-vu-fill mixer-vu-mic"
                  style={{ width: `${Math.round(levels.mic * 100)}%` }}
                />
              </div>
              <input
                type="range"
                className="slider"
                min={0}
                max={2}
                step={0.02}
                value={settings.audioMixer.micVolume}
                style={{
                  ['--fill' as string]: `${Math.round((settings.audioMixer.micVolume / 2) * 100)}%`
                }}
                onChange={(e) =>
                  onSettingsChange({
                    audioMixer: {
                      ...settings.audioMixer,
                      micEnabled: true,
                      micMuted: false,
                      micVolume: Number(e.target.value)
                    }
                  })
                }
              />
            </div>
          </div>

          <div className="live-stats">
            <StatPill label="Taxa LAN" value={formatBitrate(summary.bitrate)} />
            <StatPill label="FPS" value={String(summary.fps)} />
            <StatPill label="Canvas" value={summary.resolution} />
            <StatPill label="Codec" value={summary.codec} />
            <StatPill
              label="Perda"
              value={`${summary.loss.toFixed(1)}%`}
              tone={summary.loss > 3 ? 'bad' : summary.loss > 0.8 ? 'warn' : 'good'}
            />
          </div>

          {summary.limited !== 'none' && (
            <div className="live-warning">
              <IconAlert size={16} />
              <span>
                {LIMITATION_LABEL[summary.limited]}.{' '}
                {summary.limited === 'cpu'
                  ? 'Baixe o preset ou troque o codec para H.264.'
                  : 'Reduza a taxa máxima ou use cabo em vez de Wi-Fi.'}
              </span>
            </div>
          )}
        </section>

        {/* Barra Lateral do Estudio com 4 Abas */}
        <aside className="live-side">
          <div className="studio-tabs">
            <button
              className={`studio-tab ${studioTab === 'room' ? 'is-active' : ''}`}
              onClick={() => setStudioTab('room')}
            >
              <IconChat size={14} />
              <span>Sala & Chat</span>
            </button>
            <button
              className={`studio-tab ${studioTab === 'scenes' ? 'is-active' : ''}`}
              onClick={() => setStudioTab('scenes')}
            >
              <IconLayers size={14} />
              <span>Cenas & Widgets</span>
            </button>
            <button
              className={`studio-tab ${studioTab === 'multistream' ? 'is-active' : ''}`}
              onClick={() => setStudioTab('multistream')}
            >
              <IconGlobe size={14} />
              <span>Multistream</span>
            </button>
            <button
              className={`studio-tab ${studioTab === 'quality' ? 'is-active' : ''}`}
              onClick={() => setStudioTab('quality')}
            >
              <IconSettings size={14} />
              <span>Qualidade</span>
            </button>
          </div>

          <div className="studio-tab-content screen-scroll">
            {studioTab === 'room' && (
              <>
                {info?.pin ? (
                  <div className="code-card">
                    <span className="code-label">
                      <IconLock size={13} /> Código PIN da Sala LAN
                    </span>
                    <div className="code-digits mono selectable">
                      {info.pin.slice(0, 3)}
                      <span className="code-gap" />
                      {info.pin.slice(3)}
                    </div>
                    <Button
                      size="sm"
                      variant="subtle"
                      onClick={() => void copyCode()}
                      icon={copied ? <IconCheck size={15} /> : <IconCopy size={15} />}
                    >
                      {copied ? 'Copiado' : 'Copiar código'}
                    </Button>
                  </div>
                ) : (
                  <div className="code-card code-card-open">
                    <span className="code-label">Entrada livre na LAN</span>
                    <p className="code-open-text">
                      Qualquer pessoa na rede local pode entrar sem código.
                    </p>
                  </div>
                )}

                <button
                  className="live-address"
                  onClick={() => void copyAddress()}
                  disabled={!info}
                  title="Copiar endereço IP:Porta"
                >
                  <span className="live-address-line">
                    <span className="mono">{info ? `${info.ip}:${info.port}` : '—'}</span>
                    {copiedAddress ? <IconCheck size={13} /> : <IconCopy size={13} />}
                  </span>
                  <span className="live-address-hint">
                    {copiedAddress
                      ? 'endereço copiado'
                      : 'clique para copiar IP direto (VPN / sub-rede)'}
                  </span>
                </button>

                <div className="studio-card">
                  <Toggle
                    checked={settings.audioMixer.partyVoiceEnabled}
                    onChange={(partyVoiceEnabled) =>
                      onSettingsChange({
                        audioMixer: { ...settings.audioMixer, partyVoiceEnabled }
                      })
                    }
                    label="Party Voice (Retorno de Voz na LAN)"
                    hint="Permite que os espectadores ativem o microfone para conversar com você."
                  />
                </div>

                <div className="viewers">
                  <div className="viewers-head">
                    <span className="viewers-title">
                      Espectadores Conectados ({viewers.length})
                    </span>
                  </div>

                  {viewers.length === 0 ? (
                    <div className="viewers-empty">
                      <span className="spinner" />
                      <span>Aguardando espectadores na rede local...</span>
                    </div>
                  ) : (
                    <ul className="viewer-list">
                      {viewers.map((viewer) => {
                        const state = states[viewer.id] ?? 'conectando'
                        return (
                          <li key={viewer.id} className="viewer-row">
                            <div className="viewer-info">
                              <span className="viewer-name">{viewer.name}</span>
                              <span className="viewer-meta mono">
                                {viewer.address}
                                {viewer.pingMs !== null && ` · ${viewer.pingMs}ms`}
                              </span>
                            </div>
                            <Badge tone={STATE_TONE[state]}>{STATE_LABEL[state]}</Badge>
                            <IconButton
                              label={`Remover ${viewer.name}`}
                              variant="danger"
                              onClick={() => broadcast.kick(viewer.id)}
                            >
                              <IconExit size={16} />
                            </IconButton>
                          </li>
                        )
                      })}
                    </ul>
                  )}
                </div>

                <div className="studio-chat-box">
                  <div className="studio-chat-head">
                    <span>Chat da Transmissão & Reações</span>
                    <div className="emoji-bar">
                      {QUICK_EMOJIS.map((em) => (
                        <button
                          key={em}
                          type="button"
                          className="emoji-btn"
                          onClick={() => broadcast.sendReaction(em, settings.hostName)}
                          title={`Enviar ${em}`}
                        >
                          {em}
                        </button>
                      ))}
                    </div>
                  </div>

                  <div className="studio-chat-messages">
                    {chatMessages.length === 0 ? (
                      <div className="studio-chat-empty">
                        Nenhuma mensagem ainda. Mande um oi para a sala!
                      </div>
                    ) : (
                      chatMessages.map((m) => (
                        <div
                          key={m.id}
                          className={`chat-msg ${m.role === 'host' ? 'is-host' : ''}`}
                        >
                          <span className="chat-author">
                            {m.role === 'host' ? `★ ${m.senderName}` : m.senderName}:
                          </span>{' '}
                          <span className="chat-text">{m.text}</span>
                        </div>
                      ))
                    )}
                    <div ref={chatEndRef} />
                  </div>

                  <form className="studio-chat-form" onSubmit={handleChatSubmit}>
                    <input
                      type="text"
                      className="studio-input"
                      placeholder="Enviar mensagem no chat..."
                      value={chatText}
                      onChange={(e) => setChatText(e.target.value)}
                      maxLength={280}
                    />
                    <Button type="submit" size="sm" variant="primary">
                      Enviar
                    </Button>
                  </form>
                </div>
              </>
            )}

            {studioTab === 'scenes' && (
              <>
                <div className="studio-card">
                  <div className="studio-card-title">
                    <IconCamera size={15} />
                    <span>Webcam Picture-in-Picture (PiP)</span>
                  </div>
                  <Toggle
                    checked={settings.studio.webcamEnabled}
                    onChange={(webcamEnabled) =>
                      onSettingsChange({
                        studio: { ...settings.studio, webcamEnabled }
                      })
                    }
                    label="Sobrepor Webcam na Cena"
                    hint="Renderiza sua câmera sobre a tela 16:9 e no layout vertical 9:16."
                  />

                  {settings.studio.webcamEnabled && (
                    <>
                      <div className="studio-field">
                        <span className="studio-field-label">Posição no Canvas 16:9</span>
                        <Segmented
                          value={settings.studio.webcamPosition}
                          onChange={(webcamPosition) =>
                            onSettingsChange({
                              studio: { ...settings.studio, webcamPosition }
                            })
                          }
                          options={[
                            { value: 'bottom-right', label: 'Inf. Dir' },
                            { value: 'bottom-left', label: 'Inf. Esq' },
                            { value: 'top-right', label: 'Sup. Dir' },
                            { value: 'top-left', label: 'Sup. Esq' }
                          ]}
                        />
                      </div>

                      <div className="studio-field">
                        <span className="studio-field-label">
                          Tamanho da Câmera ({Math.round(settings.studio.webcamScale * 100)}%)
                        </span>
                        <input
                          type="range"
                          className="slider"
                          min={0.15}
                          max={0.4}
                          step={0.01}
                          value={settings.studio.webcamScale}
                          style={{
                            ['--fill' as string]: `${Math.round(((settings.studio.webcamScale - 0.15) / 0.25) * 100)}%`
                          }}
                          onChange={(e) =>
                            onSettingsChange({
                              studio: {
                                ...settings.studio,
                                webcamScale: Number(e.target.value)
                              }
                            })
                          }
                        />
                      </div>
                    </>
                  )}
                </div>

                <div className="studio-card">
                  <div className="studio-card-title">
                    <IconSmartphone size={15} />
                    <span>Dual Output Vertical (9:16 TikTok / Shorts)</span>
                  </div>
                  <Toggle
                    checked={settings.studio.dualOutputEnabled}
                    onChange={(dualOutputEnabled) =>
                      onSettingsChange({
                        studio: { ...settings.studio, dualOutputEnabled }
                      })
                    }
                    label="Ativar Canvas Vertical 1080x1920"
                    hint="Composição simultânea otimizada para celulares."
                  />

                  <div className="studio-field">
                    <span className="studio-field-label">Preset de Enquadramento 9:16</span>
                    <div className="preset-grid">
                      {VERTICAL_PRESET_OPTIONS.map((opt) => (
                        <button
                          key={opt.value}
                          type="button"
                          className={`preset-btn ${settings.studio.verticalLayout === opt.value ? 'is-active' : ''}`}
                          onClick={() =>
                            onSettingsChange({
                              studio: { ...settings.studio, verticalLayout: opt.value }
                            })
                          }
                        >
                          {opt.label}
                        </button>
                      ))}
                    </div>
                  </div>
                </div>

                <div className="studio-card">
                  <div className="studio-card-title">
                    <IconLayers size={15} />
                    <span>Widgets Nativos & StreamElements</span>
                  </div>

                  <Button size="sm" variant="primary" block onClick={triggerTestAlert}>
                    Disparar Alerta de Teste no Palco
                  </Button>

                  <div className="widget-list">
                    {settings.widgets.map((w) => (
                      <div key={w.id} className="widget-item">
                        <div className="widget-item-top">
                          <Toggle
                            checked={w.enabled}
                            onChange={(enabled) => updateWidget(w.id, { enabled })}
                            label={w.name}
                          />
                          {(w.kind === 'streamelements' || w.kind === 'custom-url') && (
                            <IconButton
                              label="Remover overlay"
                              variant="danger"
                              onClick={() => removeWidget(w.id)}
                            >
                              <IconExit size={14} />
                            </IconButton>
                          )}
                        </div>

                        {w.enabled && (w.kind === 'goal-bar' || w.kind === 'lower-third') && (
                          <div className="widget-subfields">
                            <input
                              type="text"
                              className="studio-input"
                              placeholder="Texto Principal"
                              value={w.primaryText}
                              onChange={(e) =>
                                updateWidget(w.id, { primaryText: e.target.value })
                              }
                            />
                            <input
                              type="text"
                              className="studio-input"
                              placeholder="Subtítulo / Progresso"
                              value={w.secondaryText}
                              onChange={(e) =>
                                updateWidget(w.id, { secondaryText: e.target.value })
                              }
                            />
                          </div>
                        )}

                        {w.enabled &&
                          (w.kind === 'streamelements' || w.kind === 'custom-url') && (
                            <div className="widget-subfields">
                              <input
                                type="url"
                                className="studio-input mono"
                                placeholder="https://streamelements.com/overlay/..."
                                value={w.url}
                                onChange={(e) => updateWidget(w.id, { url: e.target.value })}
                              />
                            </div>
                          )}
                      </div>
                    ))}
                  </div>

                  <div className="widget-add-box">
                    <span className="studio-field-label">
                      Adicionar Overlay StreamElements / Browser URL
                    </span>
                    <input
                      type="text"
                      className="studio-input"
                      placeholder="Nome (ex: Alertas StreamElements)"
                      value={newWidgetName}
                      onChange={(e) => setNewWidgetName(e.target.value)}
                    />
                    <input
                      type="url"
                      className="studio-input"
                      placeholder="https://streamelements.com/overlay/..."
                      value={newWidgetUrl}
                      onChange={(e) => setNewWidgetUrl(e.target.value)}
                    />
                    <Button
                      size="sm"
                      variant="subtle"
                      block
                      disabled={!newWidgetUrl.trim()}
                      onClick={addStreamElementsWidget}
                    >
                      + Adicionar Browser Source
                    </Button>
                  </div>
                </div>
              </>
            )}

            {studioTab === 'multistream' && (
              <>
                <div className="studio-card">
                  <div className="studio-card-title">
                    <IconGlobe size={15} />
                    <span>Multistreaming RTMP Simultâneo</span>
                  </div>
                  <p className="live-quality-note">
                    Transmita ao mesmo tempo para YouTube, Twitch, Kick (16:9) e TikTok Live (9:16
                    Vertical) usando o motor FFmpeg ({multistreamStatus.encoder}).
                  </p>
                  <Button
                    variant={multistreamStatus.active ? 'danger' : 'primary'}
                    block
                    onClick={() => void toggleMultistream()}
                  >
                    {multistreamStatus.active
                      ? 'Parar Multistreaming'
                      : 'Iniciar Multistreaming'}
                  </Button>
                </div>

                {settings.multistreamTargets.map((target) => {
                  const runtime = multistreamStatus.targets.find((t) => t.id === target.id)
                  return (
                    <div key={target.id} className="studio-card">
                      <div className="widget-item-top">
                        <Toggle
                          checked={target.enabled}
                          onChange={(enabled) => updateTarget(target.id, { enabled })}
                          label={target.name}
                        />
                        <Badge
                          tone={target.orientation === 'vertical' ? 'accent' : 'neutral'}
                        >
                          {target.orientation === 'vertical' ? '9:16' : '16:9'}
                        </Badge>
                        {runtime && (
                          <Badge
                            tone={
                              runtime.state === 'live'
                                ? 'live'
                                : runtime.state === 'error'
                                  ? 'danger'
                                  : 'neutral'
                            }
                          >
                            {runtime.state === 'live'
                              ? `LIVE (${(runtime.bytesSent / (1024 * 1024)).toFixed(1)} MB)`
                              : runtime.state}
                          </Badge>
                        )}
                        {target.provider === 'custom' && (
                          <IconButton
                            label="Remover destino"
                            variant="danger"
                            onClick={() => removeCustomTarget(target.id)}
                          >
                            <IconExit size={14} />
                          </IconButton>
                        )}
                      </div>

                      <div className="widget-subfields">
                        <input
                          type="text"
                          className="studio-input mono"
                          placeholder="rtmp://..."
                          value={target.rtmpUrl}
                          onChange={(e) => updateTarget(target.id, { rtmpUrl: e.target.value })}
                        />
                        <input
                          type="password"
                          className="studio-input mono"
                          placeholder="Chave de Transmissão (Stream Key)"
                          value={target.streamKey}
                          onChange={(e) =>
                            updateTarget(target.id, { streamKey: e.target.value })
                          }
                        />
                        <div className="widget-row-2">
                          <Segmented
                            value={target.orientation}
                            onChange={(orientation: CanvasOrientation) =>
                              updateTarget(target.id, { orientation })
                            }
                            options={[
                              { value: 'horizontal' as CanvasOrientation, label: '16:9 Horiz' },
                              { value: 'vertical' as CanvasOrientation, label: '9:16 Vert' }
                            ]}
                          />
                          <input
                            type="number"
                            className="studio-input mono"
                            placeholder="Bitrate (kbps)"
                            value={target.bitrateKbps}
                            onChange={(e) =>
                              updateTarget(target.id, {
                                bitrateKbps: Number(e.target.value) || 4500
                              })
                            }
                          />
                        </div>
                        {runtime?.lastError && (
                          <span className="multistream-err">{runtime.lastError}</span>
                        )}
                      </div>
                    </div>
                  )
                })}

                <Button size="sm" variant="subtle" block onClick={addCustomRtmpTarget}>
                  + Adicionar Servidor RTMP Personalizado
                </Button>
              </>
            )}

            {studioTab === 'quality' && (
              <div className="live-quality">
                <span className="live-quality-title">Qualidade do Encoder</span>
                <p className="live-quality-note">
                  As mudanças entram em vigor imediatamente sem desconectar os espectadores.
                </p>
                <QualityPanel
                  audioLocked
                  settings={settings.quality}
                  onChange={(next) => {
                    onSettingsChange({ quality: next })
                    applyLive(next)
                  }}
                />
              </div>
            )}
          </div>
        </aside>
      </div>

      {/* Modal de Troca de Fonte Ao Vivo */}
      <Modal
        open={sourceModalOpen}
        width={680}
        title="Trocar Fonte Ao Vivo"
        description="Selecione outra tela ou janela. A transição acontece imediatamente sem derrubar os espectadores nem a gravação."
        footer={
          <>
            <Button variant="ghost" onClick={() => setSourceModalOpen(false)}>
              Cancelar
            </Button>
            <Button
              variant="primary"
              disabled={!selectedNewSource}
              loading={switchingSource}
              onClick={() => void confirmSourceSwitch()}
            >
              Aplicar Nova Fonte
            </Button>
          </>
        }
      >
        <div className="source-switch-modal">
          <div className="source-switch-bar">
            <Segmented
              value={sourceTab}
              onChange={setSourceTab}
              options={[
                { value: 'screen' as SourceKind, label: 'Telas' },
                { value: 'window' as SourceKind, label: 'Janelas' }
              ]}
            />
            <IconButton label="Atualizar" onClick={() => void openSourceSwitcher()}>
              <IconRefresh size={16} />
            </IconButton>
          </div>
          <div className="source-switch-grid screen-scroll">
            {loadingSources ? (
              <div className="empty-state">
                <span className="spinner" />
                <span>Carregando fontes...</span>
              </div>
            ) : (
              <SourceGrid
                sources={availableSources.filter((s) => s.kind === sourceTab)}
                selectedId={selectedNewSource?.id ?? null}
                onSelect={setSelectedNewSource}
              />
            )}
          </div>
        </div>
      </Modal>
    </div>
  )
}
