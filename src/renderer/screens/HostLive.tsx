import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { AppSettings, DesktopSource } from '@shared/ipc'
import {
  MAX_VIEWERS,
  type MediaDeviceEntry,
  type MultistreamStatus,
  type PlatformId,
  type RecorderStatus,
  type SceneItem,
  type SourceKind,
  type SourceType,
  type VerticalLayoutPreset
} from '@shared/protocol'
import { Badge, Button, IconButton, Segmented, Toggle } from '@/components/ui'
import { Modal } from '@/components/Modal'
import { SourceGrid } from '@/components/SourceGrid'
import { QualityPanel } from '@/components/QualityPanel'
import {
  IconBroadcast,
  IconCamera,
  IconChat,
  IconCopy,
  IconCut,
  IconDisc,
  IconDown,
  IconEye,
  IconEyeOff,
  IconGlobe,
  IconLayers,
  IconLock,
  IconMic,
  IconMicOff,
  IconPlus,
  IconRefresh,
  IconSettings,
  IconSmartphone,
  IconStop,
  IconTrash,
  IconUp,
  IconUser,
  IconUsers,
  IconVolume,
  IconVolumeOff
} from '@/components/Icons'
import type { HostBroadcast } from '@/hooks/useHostBroadcast'
import { AudioMixerEngine, type AudioLevels } from '@/studio/AudioMixerEngine'
import { DualCanvasCompositor } from '@/studio/DualCanvasCompositor'
import './HostLive.css'

type StudioTab = 'room' | 'scenes' | 'multistream' | 'quality'

const VERTICAL_PRESET_OPTIONS: { value: VerticalLayoutPreset; label: string }[] = [
  { value: 'split-cam-top', label: 'Split Cam + Jogo' },
  { value: 'blurred-backdrop', label: 'Fundo Desfocado' },
  { value: 'center-crop', label: 'Corte Central' },
  { value: 'pip-vertical', label: 'PiP Vertical' }
]

function getSourceIcon(type: SourceType): string {
  switch (type) {
    case 'screen_capture':
      return '🖥️'
    case 'window_capture':
      return '🪟'
    case 'camera_input':
      return '📷'
    case 'audio_input':
      return '🎙️'
    case 'audio_output':
      return '🔊'
    case 'browser_source':
      return '🌐'
    case 'color_source':
      return '🎨'
    case 'text_source':
      return '📝'
    case 'image_source':
      return '🖼️'
    default:
      return '📦'
  }
}

export function HostLive({
  broadcast,
  stream,
  sourceName,
  settings,
  onSettingsChange,
  onSourceSwitch,
  onStartBroadcast,
  onStop,
  onOpenViewer
}: {
  broadcast: HostBroadcast
  stream: MediaStream | null
  sourceName: string
  settings: AppSettings
  onSettingsChange: (patch: Partial<AppSettings>) => void
  onSourceSwitch: (newStream: MediaStream, newSource: DesktopSource) => void
  onStartBroadcast?: (stream: MediaStream, source: DesktopSource) => Promise<string | null>
  onStop: () => void
  onOpenViewer?: () => void
}): React.JSX.Element {
  const horizVideoRef = useRef<HTMLVideoElement>(null)
  const vertVideoRef = useRef<HTMLVideoElement>(null)
  const previewHorizVideoRef = useRef<HTMLVideoElement>(null)

  const mixerRef = useRef<AudioMixerEngine | null>(null)
  const compositorRef = useRef<DualCanvasCompositor | null>(null)

  const [studioTab, setStudioTab] = useState<StudioTab>('scenes')
  const [toast, setToast] = useState<string | null>(null)

  const [levels, setLevels] = useState<AudioLevels>({ system: 0, mic: 0 })
  const [selectedSceneItemId, setSelectedSceneItemId] = useState<string | null>(null)
  const [audioDevices, setAudioDevices] = useState<MediaDeviceEntry[]>([])

  // Modais
  const [loginModalOpen, setLoginModalOpen] = useState(false)
  const [addSourceModalOpen, setAddSourceModalOpen] = useState(false)
  const [settingsModalOpen, setSettingsModalOpen] = useState(false)
  const [newScenePromptOpen, setNewScenePromptOpen] = useState(false)
  const [newSceneName, setNewSceneName] = useState('')
  const [editingItem, setEditingItem] = useState<SceneItem | null>(null)

  // Chaves de stream no modal de plataformas
  const [twitchKey, setTwitchKey] = useState(settings.profile?.accounts?.twitch?.streamKey || '')
  const [youtubeKey, setYoutubeKey] = useState(settings.profile?.accounts?.youtube?.streamKey || '')
  const [kickKey, setKickKey] = useState(settings.profile?.accounts?.kick?.streamKey || '')
  const [tiktokKey, setTiktokKey] = useState(settings.profile?.accounts?.tiktok?.streamKey || '')

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

  const [sourceModalOpen, setSourceModalOpen] = useState(false)
  const [availableSources, setAvailableSources] = useState<DesktopSource[]>([])
  const [loadingSources, setLoadingSources] = useState(false)
  const [sourceTab, setSourceTab] = useState<SourceKind>('screen')
  const [selectedNewSource, setSelectedNewSource] = useState<DesktopSource | null>(null)
  const [switchingSource, setSwitchingSource] = useState(false)

  const { info, viewers, chatMessages, reactions } = broadcast
  const isBroadcasting = info !== null

  const showToast = useCallback((msg: string): void => {
    setToast(msg)
    window.setTimeout(() => {
      setToast((curr) => (curr === msg ? null : curr))
    }, 4200)
  }, [])

  // Cenas ativas e em prévia
  const scenesState = settings.scenes
  const currentScene = useMemo(() => {
    const list = scenesState?.scenes || []
    return list.find((s) => s.id === scenesState?.activeSceneId) || list[0] || null
  }, [scenesState])

  const previewScene = useMemo(() => {
    const list = scenesState?.scenes || []
    return list.find((s) => s.id === scenesState?.previewSceneId) || currentScene
  }, [scenesState, currentScene])

  // 1. Inicializa o AudioMixerEngine e o DualCanvasCompositor
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
    if (compVideoTrack && isBroadcasting) {
      void broadcast.replaceVideoTrack(compVideoTrack)
    }
    if (isBroadcasting) {
      void broadcast.replaceAudioTrack(mixer.outputTrack)
    }

    if (horizVideoRef.current) {
      horizVideoRef.current.srcObject = compositor.outputHorizontalStream
    }
    if (vertVideoRef.current) {
      vertVideoRef.current.srcObject = compositor.outputVerticalStream
    }
    if (previewHorizVideoRef.current) {
      previewHorizVideoRef.current.srcObject = compositor.outputHorizontalStream
    }

    const levelTimer = window.setInterval(() => {
      setLevels(mixer.readLevels())
    }, 45)

    void window.api.media.listDevices().then(setAudioDevices)

    return () => {
      window.clearInterval(levelTimer)
      compositor.dispose()
      mixer.dispose()
      compositorRef.current = null
      mixerRef.current = null
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Sincroniza a cena ativa no Compositor
  useEffect(() => {
    const targetScene = scenesState?.studioMode ? previewScene : currentScene
    compositorRef.current?.setActiveScene(targetScene, selectedSceneItemId)
  }, [currentScene, previewScene, scenesState?.studioMode, selectedSceneItemId])

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

  // Polling de status
  useEffect(() => {
    let cancel = false
    const poll = async (): Promise<void> => {
      try {
        const [rec, multi] = await Promise.all([
          window.api.recorder.status(),
          window.api.multistream.status()
        ])
        if (cancel) return
        setRecorderStatus(rec)
        setMultistreamStatus(multi)
      } catch {
        /* ignore */
      }
    }
    void poll()
    const timer = window.setInterval(poll, 1500)
    return () => {
      cancel = true
      window.clearInterval(timer)
    }
  }, [])

  // ----------------------------------------------------------------- Cenas CRUD

  const handleSelectScene = async (sceneId: string) => {
    await window.api.scenes.selectScene(sceneId)
    const next = await window.api.settings.get()
    onSettingsChange(next)
  }

  const handleCreateScene = async () => {
    if (!newSceneName.trim()) return
    const scene = await window.api.scenes.createScene(newSceneName.trim())
    const next = await window.api.settings.get()
    onSettingsChange(next)
    setNewSceneName('')
    setNewScenePromptOpen(false)
    showToast(`Cena "${scene.name}" criada com sucesso!`)
  }

  const handleDeleteScene = async (sceneId: string) => {
    try {
      await window.api.scenes.deleteScene(sceneId)
      const next = await window.api.settings.get()
      onSettingsChange(next)
      showToast('Cena excluída.')
    } catch (err) {
      showToast((err as Error).message)
    }
  }

  const handleDuplicateScene = async (sceneId: string) => {
    const copy = await window.api.scenes.duplicateScene(sceneId)
    const next = await window.api.settings.get()
    onSettingsChange(next)
    showToast(`Cena duplicada como "${copy.name}".`)
  }

  const handleToggleStudioMode = async () => {
    const nextVal = !scenesState?.studioMode
    await window.api.scenes.setStudioMode(nextVal)
    const next = await window.api.settings.get()
    onSettingsChange(next)
    showToast(nextVal ? 'Modo Estúdio ativado: Prévia | Programa' : 'Modo Padrão ativado')
  }

  const handleSetTransition = async (type: 'cut' | 'fade') => {
    await window.api.scenes.transition(type, 300)
    const next = await window.api.settings.get()
    onSettingsChange(next)
    showToast(`Transição definida para: ${type === 'cut' ? 'Corte' : 'Esmaecer'}`)
  }

  const handleExecuteTransition = async () => {
    await window.api.scenes.transition(scenesState?.transitionType || 'cut', 300)
    const next = await window.api.settings.get()
    onSettingsChange(next)
    showToast('Cena em prévia enviada para o programa ao vivo!')
  }

  // ---------------------------------------------------------------- Fontes CRUD

  const handleToggleSourceVisible = async (item: SceneItem) => {
    if (!currentScene) return
    await window.api.scenes.updateItem(currentScene.id, item.id, {
      visible: !item.visible
    })
    const next = await window.api.settings.get()
    onSettingsChange(next)
  }

  const handleToggleSourceLock = async (item: SceneItem) => {
    if (!currentScene) return
    await window.api.scenes.updateItem(currentScene.id, item.id, {
      locked: !item.locked
    })
    const next = await window.api.settings.get()
    onSettingsChange(next)
  }

  const handleRemoveItem = async (itemId: string) => {
    if (!currentScene) return
    await window.api.scenes.removeItem(currentScene.id, itemId)
    const next = await window.api.settings.get()
    onSettingsChange(next)
    setSelectedSceneItemId(null)
    showToast('Fonte removida da cena.')
  }

  const moveSelectedItem = async (delta: number) => {
    if (!currentScene || !selectedSceneItemId) return
    const items = currentScene.items
    const currIdx = items.findIndex((it) => it.id === selectedSceneItemId)
    if (currIdx < 0) return
    const targetIdx = currIdx + delta
    if (targetIdx < 0 || targetIdx >= items.length) return
    await window.api.scenes.reorderItem(currentScene.id, selectedSceneItemId, targetIdx)
    const next = await window.api.settings.get()
    onSettingsChange(next)
  }

  const handleAddSourceType = async (type: SourceType) => {
    setAddSourceModalOpen(false)
    if (!currentScene) return
    const names: Record<SourceType, string> = {
      screen_capture: 'Captura de Monitor',
      window_capture: 'Captura de Janela',
      camera_input: 'Câmera do Streamer',
      audio_input: 'Microfone',
      audio_output: 'Áudio do Sistema',
      browser_source: 'Browser Source / Overlay',
      color_source: 'Fonte de Cor',
      text_source: 'Texto Personalizado',
      image_source: 'Imagem / Logo'
    }

    const newItem = await window.api.scenes.addItem(currentScene.id, {
      sourceId: `source_${Date.now()}`,
      name: names[type] || 'Nova Fonte',
      type,
      visible: true,
      locked: false,
      transform: {
        x: 0.05,
        y: 0.05,
        width: 0.9,
        height: 0.9,
        zIndex: currentScene.items.length + 1
      },
      verticalTransform: {
        x: 0.05,
        y: 0.05,
        width: 0.9,
        height: 0.5,
        zIndex: currentScene.items.length + 1
      },
      showOnHorizontal: true,
      showOnVertical: true,
      settings:
        type === 'text_source'
          ? { text: 'AO VIVO NO PRISMCAST STUDIO', fontSize: 32, color: '#38bdf8' }
          : type === 'color_source'
            ? { color: '#0f172a' }
            : {}
    })

    const next = await window.api.settings.get()
    onSettingsChange(next)
    setSelectedSceneItemId(newItem.id)
    showToast(`Fonte "${newItem.name}" adicionada.`)

    if (type === 'screen_capture' || type === 'window_capture') {
      void openSourceSwitcher()
    }
  }

  const openSourceProperties = () => {
    if (!currentScene || !selectedSceneItemId) return
    const item = currentScene.items.find((it) => it.id === selectedSceneItemId)
    if (item) {
      setEditingItem(item)
    }
  }

  const handleSaveEditingItem = async () => {
    if (!currentScene || !editingItem) return
    await window.api.scenes.updateItem(currentScene.id, editingItem.id, editingItem)
    const next = await window.api.settings.get()
    onSettingsChange(next)
    setEditingItem(null)
    showToast('Propriedades da fonte atualizadas!')
  }

  // ------------------------------------------------------------- Autenticação & Plataformas

  const handleTogglePlatform = async (platform: PlatformId, key: string) => {
    const account = settings.profile?.accounts?.[platform]
    if (account?.connected) {
      await window.api.auth.disconnectPlatform(platform)
      const next = await window.api.settings.get()
      onSettingsChange(next)
      showToast(`${platform.toUpperCase()} desconectado.`)
    } else {
      await window.api.auth.connectPlatform(platform, {
        username: settings.profile?.name,
        streamKey: key.trim() || `live_${platform}_${Math.random().toString(36).slice(2, 8)}`
      })
      const next = await window.api.settings.get()
      onSettingsChange(next)
      showToast(`${platform.toUpperCase()} conectado e sincronizado no Multistreaming!`)
    }
  }

  // ---------------------------------------------------------------- Transmissão & Gravação

  const handleToggleBroadcast = async () => {
    if (isBroadcasting) {
      onStop()
      return
    }

    if (stream && onStartBroadcast) {
      const dummySource: DesktopSource = {
        id: 'primary',
        name: sourceName || 'Tela Principal',
        kind: 'screen',
        thumbnail: '',
        appIcon: null,
        displayId: 'primary'
      }
      await onStartBroadcast(stream, dummySource)
      return
    }

    // Se ainda não escolheu tela, abre o seletor
    void openSourceSwitcher()
  }

  const openSourceSwitcher = async (): Promise<void> => {
    setLoadingSources(true)
    setSourceModalOpen(true)
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

      if (!isBroadcasting && onStartBroadcast) {
        await onStartBroadcast(newStream, selectedNewSource)
      } else {
        showToast(`Fonte alternada para: ${selectedNewSource.name}`)
      }
    } catch (err) {
      showToast(`Não foi possível capturar: ${(err as Error).message}`)
    } finally {
      setSwitchingSource(false)
    }
  }

  const toggleRecording = async (): Promise<void> => {
    if (recorderStatus.recording) {
      const res = await window.api.recorder.stop()
      setRecorderStatus(res)
      showToast('Gravação finalizada e salva!')
    } else {
      const res = await window.api.recorder.start()
      if (res.ok && res.status) {
        setRecorderStatus(res.status)
        showToast('Gravação iniciada (Gravação Local NVENC/libx264)!')
      } else {
        showToast(res.error || 'Erro ao iniciar gravação.')
      }
    }
  }

  const handleSaveReplay = async (): Promise<void> => {
    const res = await window.api.recorder.saveReplay()
    if (res.ok && res.path) {
      showToast(`Replay salvo com sucesso: ${res.path.split(/[\\/]/).pop()}`)
    } else {
      showToast(res.error || 'Erro ao salvar clipe.')
    }
  }

  const connectedPlatformsCount = useMemo(() => {
    if (!settings.profile?.accounts) return 0
    return Object.values(settings.profile.accounts).filter((a) => a.connected).length
  }, [settings.profile?.accounts])

  const browserWidgets = settings.widgets.filter(
    (w) => w.enabled && (w.kind === 'streamelements' || w.kind === 'custom-url') && w.url.trim()
  )

  return (
    <div className="screen fade-in">
      {/* Cabeçalho do Estúdio */}
      <header className="page-head studio-head">
        <div className={isBroadcasting ? 'live-dot' : ''} aria-hidden="true" />
        <div className="studio-title-wrap">
          <div className="page-title">
            PrismCast Studio · {isBroadcasting ? 'Ao Vivo' : 'Preparação de Cenas'}
          </div>
          <div className="page-sub">
            {currentScene ? currentScene.name : sourceName || 'Cena Principal'}
          </div>
        </div>

        <div className="studio-quick-actions">
          <Button
            size="sm"
            variant="subtle"
            onClick={() => void openSourceSwitcher()}
            icon={<IconRefresh size={14} />}
          >
            {stream ? 'Trocar Fonte' : 'Capturar Tela'}
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
            variant="subtle"
            onClick={() => setLoginModalOpen(true)}
            icon={<IconUser size={14} />}
          >
            Contas ({connectedPlatformsCount})
          </Button>

          {onOpenViewer && (
            <Button
              size="sm"
              variant="subtle"
              onClick={onOpenViewer}
              icon={<IconGlobe size={14} />}
            >
              Assistir LAN
            </Button>
          )}
        </div>

        <div className="spacer" />

        {isBroadcasting ? (
          <>
            <Badge tone="live" icon={<IconUsers />}>
              {viewers.length}/{MAX_VIEWERS} LAN
            </Badge>
            <Button variant="danger" onClick={onStop} icon={<IconStop size={16} />}>
              Interromper
            </Button>
          </>
        ) : (
          <Button
            variant="primary"
            onClick={handleToggleBroadcast}
            icon={<IconBroadcast size={16} />}
          >
            Iniciar Transmissão
          </Button>
        )}
      </header>

      {toast && <div className="studio-toast fade-in">{toast}</div>}

      <div className="live-body">
        <section className="live-main">
          {/* Palco Dual Output: 16:9 Horizontal + 9:16 Vertical lado a lado */}
          <div
            className={`studio-stage ${settings.studio.dualOutputEnabled ? 'has-vertical' : ''} ${scenesState?.studioMode ? 'studio-stage-dual-preview' : ''}`}
          >
            {/* Se estiver em Modo Estúdio, exibe a Prévia à esquerda */}
            {scenesState?.studioMode && (
              <div className="live-preview stage-horiz" style={{ borderColor: '#a855f7' }}>
                <div className="stage-badge" style={{ color: '#c084fc' }}>
                  PRÉVIA (EDIÇÃO) · {previewScene?.name}
                </div>
                <video ref={previewHorizVideoRef} autoPlay muted playsInline />
              </div>
            )}

            {/* Divisor de Transição rápida do Modo Estúdio */}
            {scenesState?.studioMode && (
              <div className="studio-transition-divider">
                <Button
                  size="sm"
                  variant="primary"
                  onClick={handleExecuteTransition}
                  title="Enviar Prévia para o Programa"
                >
                  ➔
                </Button>
                <IconButton
                  label="Corte"
                  variant="plain"
                  onClick={() => handleSetTransition('cut')}
                >
                  <IconCut size={14} />
                </IconButton>
              </div>
            )}

            {/* Canvas Principal: 16:9 Programa (Ao Vivo) */}
            <div className="live-preview stage-horiz">
              <div className="stage-badge">
                {scenesState?.studioMode ? 'PROGRAMA (AO VIVO)' : '16:9 HORIZONTAL · 1080p 60FPS'}
              </div>
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

            {/* Canvas Vertical: 9:16 ao lado do 16:9 (TikTok / Shorts / Reels) */}
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

          {/* Barra de Docks OBS Studio (Cenas | Fontes | Mixer | Transições | Controles) */}
          <div className="obs-dock-bar">
            {/* Dock 1: Cenas */}
            <div className="obs-dock">
              <div className="obs-dock-header">
                <span>Cenas</span>
                <Badge tone="neutral">{scenesState?.scenes.length || 0}</Badge>
              </div>
              <div className="obs-dock-content">
                {scenesState?.scenes.map((sc) => {
                  const isLive = sc.id === scenesState.activeSceneId
                  const isPrev = scenesState.studioMode && sc.id === scenesState.previewSceneId
                  return (
                    <button
                      key={sc.id}
                      className={`obs-scene-item ${isLive ? 'is-active' : ''} ${isPrev ? 'is-preview' : ''}`}
                      onClick={() => handleSelectScene(sc.id)}
                    >
                      <span style={{ overflow: 'hidden', textOverflow: 'ellipsis' }}>
                        {sc.name}
                      </span>
                      {isLive && <Badge tone="live">AO VIVO</Badge>}
                      {isPrev && !isLive && <Badge tone="neutral">PRÉVIA</Badge>}
                    </button>
                  )
                })}
              </div>
              <div className="obs-dock-toolbar">
                <IconButton label="Adicionar Cena" onClick={() => setNewScenePromptOpen(true)}>
                  <IconPlus size={14} />
                </IconButton>
                <IconButton
                  label="Remover Cena"
                  onClick={() => currentScene && handleDeleteScene(currentScene.id)}
                >
                  <IconTrash size={14} />
                </IconButton>
                <IconButton
                  label="Duplicar Cena"
                  onClick={() => currentScene && handleDuplicateScene(currentScene.id)}
                >
                  <IconCopy size={14} />
                </IconButton>
              </div>
            </div>

            {/* Dock 2: Fontes */}
            <div className="obs-dock">
              <div className="obs-dock-header">
                <span>Fontes</span>
                <Badge tone="neutral">{currentScene?.items.length || 0}</Badge>
              </div>
              <div className="obs-dock-content">
                {currentScene?.items.map((it) => {
                  const isSelected = selectedSceneItemId === it.id
                  return (
                    <div
                      key={it.id}
                      className={`obs-source-row ${isSelected ? 'is-selected' : ''}`}
                      onClick={() => setSelectedSceneItemId(it.id)}
                    >
                      <span style={{ fontSize: '13px' }}>{getSourceIcon(it.type)}</span>
                      <span className="obs-source-name">{it.name}</span>
                      <IconButton
                        label={it.visible ? 'Ocultar' : 'Exibir'}
                        variant="plain"
                        onClick={(e) => {
                          e.stopPropagation()
                          handleToggleSourceVisible(it)
                        }}
                      >
                        {it.visible ? <IconEye size={13} /> : <IconEyeOff size={13} />}
                      </IconButton>
                      <IconButton
                        label={it.locked ? 'Desbloquear' : 'Bloquear'}
                        variant="plain"
                        onClick={(e) => {
                          e.stopPropagation()
                          handleToggleSourceLock(it)
                        }}
                      >
                        <IconLock size={13} style={{ opacity: it.locked ? 1 : 0.4 }} />
                      </IconButton>
                    </div>
                  )
                })}
              </div>
              <div className="obs-dock-toolbar">
                <IconButton label="Adicionar Fonte" onClick={() => setAddSourceModalOpen(true)}>
                  <IconPlus size={14} />
                </IconButton>
                <IconButton
                  label="Remover Fonte"
                  onClick={() => selectedSceneItemId && handleRemoveItem(selectedSceneItemId)}
                >
                  <IconTrash size={14} />
                </IconButton>
                <IconButton label="Mover para cima" onClick={() => moveSelectedItem(-1)}>
                  <IconUp size={14} />
                </IconButton>
                <IconButton label="Mover para baixo" onClick={() => moveSelectedItem(1)}>
                  <IconDown size={14} />
                </IconButton>
                <IconButton label="Propriedades" onClick={openSourceProperties}>
                  <IconSettings size={14} />
                </IconButton>
              </div>
            </div>

            {/* Dock 3: Mixer de Áudio */}
            <div className="obs-dock">
              <div className="obs-dock-header">
                <span>Mixer de Áudio</span>
              </div>
              <div className="obs-dock-content" style={{ gap: '8px' }}>
                {/* Desktop Audio */}
                <div className="obs-audio-channel">
                  <div className="obs-audio-title-bar">
                    <span>Áudio do Desktop (PC)</span>
                    <span className="mono">
                      {Math.round(settings.audioMixer.systemVolume * 100)}%
                    </span>
                  </div>
                  <div className="obs-audio-meter-wrap">
                    <div
                      className="obs-audio-meter-bar"
                      style={{ width: `${Math.round(levels.system * 100)}%` }}
                    />
                  </div>
                  <div style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
                    <IconButton
                      label={
                        settings.audioMixer.systemMuted
                          ? 'Desmutar Desktop'
                          : 'Mutar Desktop'
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
                        <IconVolumeOff size={14} />
                      ) : (
                        <IconVolume size={14} />
                      )}
                    </IconButton>
                    <input
                      type="range"
                      min={0}
                      max={1.5}
                      step={0.01}
                      value={settings.audioMixer.systemVolume}
                      onChange={(e) =>
                        onSettingsChange({
                          audioMixer: {
                            ...settings.audioMixer,
                            systemVolume: parseFloat(e.target.value)
                          }
                        })
                      }
                      style={{ flex: 1 }}
                    />
                  </div>
                </div>

                {/* Mic Audio */}
                <div className="obs-audio-channel">
                  <div className="obs-audio-title-bar">
                    <span>Microfone / Auxiliar</span>
                    <span className="mono">
                      {Math.round(settings.audioMixer.micVolume * 100)}%
                    </span>
                  </div>
                  <div className="obs-audio-meter-wrap">
                    <div
                      className="obs-audio-meter-bar"
                      style={{ width: `${Math.round(levels.mic * 100)}%` }}
                    />
                  </div>
                  <div style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
                    <IconButton
                      label={
                        settings.audioMixer.micMuted ? 'Desmutar Mic' : 'Mutar Mic'
                      }
                      variant={settings.audioMixer.micMuted ? 'danger' : 'plain'}
                      onClick={() =>
                        onSettingsChange({
                          audioMixer: {
                            ...settings.audioMixer,
                            micMuted: !settings.audioMixer.micMuted
                          }
                        })
                      }
                    >
                      {settings.audioMixer.micMuted ? (
                        <IconMicOff size={14} />
                      ) : (
                        <IconMic size={14} />
                      )}
                    </IconButton>
                    <input
                      type="range"
                      min={0}
                      max={2}
                      step={0.01}
                      value={settings.audioMixer.micVolume}
                      onChange={(e) =>
                        onSettingsChange({
                          audioMixer: {
                            ...settings.audioMixer,
                            micVolume: parseFloat(e.target.value)
                          }
                        })
                      }
                      style={{ flex: 1 }}
                    />
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
                      title="Redução de Ruído (Noise Suppression)"
                    >
                      NR
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
                      title="Compressor de Áudio (Broadcast Dynamics)"
                    >
                      CMP
                    </button>
                  </div>
                </div>
              </div>
            </div>

            {/* Dock 4: Transição de Cena */}
            <div className="obs-dock">
              <div className="obs-dock-header">
                <span>Transição</span>
              </div>
              <div className="obs-dock-content" style={{ justifyContent: 'center', gap: '6px' }}>
                <button
                  className={`preset-btn ${scenesState?.transitionType === 'cut' ? 'is-active' : ''}`}
                  onClick={() => handleSetTransition('cut')}
                >
                  Corte (Cut)
                </button>
                <button
                  className={`preset-btn ${scenesState?.transitionType === 'fade' ? 'is-active' : ''}`}
                  onClick={() => handleSetTransition('fade')}
                >
                  Esmaecer (Fade)
                </button>
                {scenesState?.studioMode && (
                  <Button
                    variant="primary"
                    size="sm"
                    onClick={handleExecuteTransition}
                    style={{ marginTop: '8px' }}
                  >
                    Transição ➔
                  </Button>
                )}
              </div>
            </div>

            {/* Dock 5: Controles */}
            <div className="obs-dock">
              <div className="obs-dock-header">
                <span>Controles</span>
              </div>
              <div className="obs-controls-col">
                <Button
                  variant={isBroadcasting ? 'danger' : 'primary'}
                  size="sm"
                  className={isBroadcasting ? 'obs-btn-danger-live' : 'obs-btn-primary-live'}
                  onClick={handleToggleBroadcast}
                  icon={isBroadcasting ? <IconStop size={14} /> : <IconBroadcast size={14} />}
                >
                  {isBroadcasting ? 'Interromper Live' : 'Iniciar Transmissão'}
                </Button>

                <Button
                  variant={recorderStatus.recording ? 'danger' : 'subtle'}
                  size="sm"
                  onClick={() => void toggleRecording()}
                  icon={<IconDisc size={14} />}
                >
                  {recorderStatus.recording
                    ? `Parar Grav. (${recorderStatus.recordingDurationSec}s)`
                    : 'Iniciar Gravação'}
                </Button>

                <Button
                  variant="subtle"
                  size="sm"
                  onClick={() => void handleSaveReplay()}
                  title="Salvar últimos 30 segundos (Ctrl+Shift+S)"
                >
                  Salvar Replay (30s)
                </Button>

                <Button
                  variant={scenesState?.studioMode ? 'primary' : 'subtle'}
                  size="sm"
                  onClick={handleToggleStudioMode}
                  icon={<IconLayers size={14} />}
                >
                  Modo Estúdio
                </Button>

                <Button
                  variant={settings.studio.privacyShieldActive ? 'danger' : 'subtle'}
                  size="sm"
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
                  Volto Já
                </Button>

                <Button
                  variant="subtle"
                  size="sm"
                  onClick={() => setLoginModalOpen(true)}
                  icon={<IconUser size={14} />}
                >
                  Contas & Login
                </Button>

                <Button
                  variant="subtle"
                  size="sm"
                  onClick={() => setSettingsModalOpen(true)}
                  icon={<IconSettings size={14} />}
                >
                  Configurações
                </Button>
              </div>
            </div>
          </div>
        </section>

        {/* Barra Lateral: Multistreaming & Salas */}
        <aside className="live-side">
          <nav className="studio-tabs">
            <button
              className={`studio-tab ${studioTab === 'scenes' ? 'is-active' : ''}`}
              onClick={() => setStudioTab('scenes')}
            >
              <IconLayers size={15} />
              <span>Layout</span>
            </button>
            <button
              className={`studio-tab ${studioTab === 'multistream' ? 'is-active' : ''}`}
              onClick={() => setStudioTab('multistream')}
            >
              <IconGlobe size={15} />
              <span>Multi</span>
            </button>
            <button
              className={`studio-tab ${studioTab === 'room' ? 'is-active' : ''}`}
              onClick={() => setStudioTab('room')}
            >
              <IconChat size={15} />
              <span>Chat</span>
            </button>
            <button
              className={`studio-tab ${studioTab === 'quality' ? 'is-active' : ''}`}
              onClick={() => setStudioTab('quality')}
            >
              <IconSettings size={15} />
              <span>Qualidade</span>
            </button>
          </nav>

          <div className="studio-tab-content scrollable">
            {/* Aba 1: Layout & Configurações da Cena */}
            {studioTab === 'scenes' && (
              <>
                <div className="studio-card">
                  <div className="studio-card-title">
                    <IconSmartphone size={16} />
                    <span>Preset da Saída 9:16 (TikTok)</span>
                  </div>
                  <div className="preset-grid">
                    {VERTICAL_PRESET_OPTIONS.map((opt) => (
                      <button
                        key={opt.value}
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

                <div className="studio-card">
                  <div className="studio-card-title">
                    <IconCamera size={16} />
                    <span>Câmera do Streamer (Webcam)</span>
                  </div>
                  <div className="studio-field">
                    <Toggle
                      label="Ativar Webcam na Composição"
                      checked={settings.studio.webcamEnabled}
                      onChange={(v) =>
                        onSettingsChange({
                          studio: { ...settings.studio, webcamEnabled: v }
                        })
                      }
                    />
                  </div>
                  {settings.studio.webcamEnabled && (
                    <div className="studio-field">
                      <span className="studio-field-label">Posição no 16:9</span>
                      <Segmented
                        value={settings.studio.webcamPosition}
                        onChange={(v) =>
                          onSettingsChange({
                            studio: {
                              ...settings.studio,
                              webcamPosition: v as any
                            }
                          })
                        }
                        options={[
                          { value: 'bottom-right', label: 'Inf. Dir.' },
                          { value: 'bottom-left', label: 'Inf. Esq.' },
                          { value: 'top-right', label: 'Sup. Dir.' },
                          { value: 'top-left', label: 'Sup. Esq.' }
                        ]}
                      />
                    </div>
                  )}
                </div>
              </>
            )}

            {/* Aba 2: Multistreaming Status */}
            {studioTab === 'multistream' && (
              <div className="studio-card">
                <div className="studio-card-title">
                  <IconGlobe size={16} />
                  <span>Destinos de Multistreaming</span>
                </div>
                <div style={{ fontSize: '11.5px', color: 'var(--text-dim)' }}>
                  Aceleração por Hardware: <strong>{multistreamStatus.encoder}</strong>
                </div>

                <div style={{ display: 'flex', flexDirection: 'column', gap: '8px' }}>
                  {settings.multistreamTargets.map((t) => {
                    const status = multistreamStatus.targets.find((s) => s.id === t.id)
                    return (
                      <div
                        key={t.id}
                        style={{
                          padding: '8px',
                          borderRadius: 'var(--r-md)',
                          background: 'rgba(255, 255, 255, 0.03)',
                          border: '1px solid var(--border)',
                          display: 'flex',
                          alignItems: 'center',
                          justifyContent: 'space-between'
                        }}
                      >
                        <div>
                          <div style={{ fontWeight: 600, fontSize: '12px' }}>{t.name}</div>
                          <div style={{ fontSize: '10.5px', color: 'var(--text-dim)' }}>
                            {t.orientation === 'vertical' ? '9:16 Vertical' : '16:9 Horizontal'} · {t.bitrateKbps} kbps
                          </div>
                        </div>
                        <Badge tone={t.enabled ? 'live' : 'neutral'}>
                          {t.enabled ? (status?.state === 'live' ? 'AO VIVO' : 'ATIVO') : 'DESATIVADO'}
                        </Badge>
                      </div>
                    )
                  })}
                </div>

                <Button
                  size="sm"
                  variant="subtle"
                  onClick={() => setLoginModalOpen(true)}
                  style={{ marginTop: '8px' }}
                >
                  Gerenciar Contas & Chaves
                </Button>
              </div>
            )}

            {/* Aba 3: Chat da Live */}
            {studioTab === 'room' && (
              <div className="studio-card" style={{ flex: 1 }}>
                <div className="studio-card-title">
                  <IconChat size={16} />
                  <span>Chat da Transmissão</span>
                </div>
                <div
                  style={{
                    flex: 1,
                    minHeight: '140px',
                    maxHeight: '260px',
                    overflowY: 'auto',
                    display: 'flex',
                    flexDirection: 'column',
                    gap: '4px'
                  }}
                >
                  {chatMessages.length === 0 ? (
                    <div style={{ fontSize: '11px', color: 'var(--text-dim)', textAlign: 'center', padding: '16px' }}>
                      Nenhuma mensagem no chat ainda.
                    </div>
                  ) : (
                    chatMessages.map((m) => (
                      <div key={m.id} style={{ fontSize: '11.5px' }}>
                        <strong style={{ color: m.role === 'host' ? '#17d4b2' : '#38bdf8' }}>
                          {m.senderName}:{' '}
                        </strong>
                        <span>{m.text}</span>
                      </div>
                    ))
                  )}
                  <div ref={chatEndRef} />
                </div>
                <form
                  onSubmit={(e) => {
                    e.preventDefault()
                    if (!chatText.trim()) return
                    broadcast.sendChat(chatText, settings.hostName)
                    setChatText('')
                  }}
                  style={{ display: 'flex', gap: '6px' }}
                >
                  <input
                    type="text"
                    value={chatText}
                    onChange={(e) => setChatText(e.target.value)}
                    placeholder="Digite uma mensagem..."
                    className="field-input"
                    style={{ flex: 1, fontSize: '11.5px', padding: '4px 8px' }}
                  />
                  <Button size="sm" variant="primary" type="submit">
                    Enviar
                  </Button>
                </form>
              </div>
            )}

            {/* Aba 4: Qualidade */}
            {studioTab === 'quality' && (
              <div className="studio-card">
                <QualityPanel
                  settings={settings.quality}
                  onChange={(quality) => {
                    onSettingsChange({ quality })
                    broadcast.updateSettings(quality)
                  }}
                />
              </div>
            )}
          </div>
        </aside>
      </div>

      {/* ------------------------------------------------ Modais Interativos */}

      {/* Modal 1: Login & Contas Conectadas */}
      <Modal
        open={loginModalOpen}
        onClose={() => setLoginModalOpen(false)}
        title="Contas & Plataformas de Streaming"
        width={620}
        icon={<IconUser size={20} />}
        footer={
          <Button variant="primary" onClick={() => setLoginModalOpen(false)}>
            Concluir
          </Button>
        }
      >
        <div className="platform-cards-list">
          <div className="studio-field-label" style={{ marginBottom: 4 }}>
            Conecte suas plataformas para fazer multicasting simultâneo (Twitch, YouTube, Kick, TikTok, PrismCast ID).
            O PrismCast sincroniza automaticamente as chaves e endpoints RTMP.
          </div>

          {/* PrismCast ID */}
          <div className={`platform-card ${settings.profile?.isLoggedIn ? 'is-connected' : ''}`}>
            <div className="platform-card-header">
              <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <span style={{ fontSize: 20 }}>🔷</span>
                <div>
                  <div className="platform-badge-prismcast">PrismCast Cloud ID</div>
                  <div style={{ fontSize: 11, color: 'var(--text-dim)' }}>
                    {settings.profile?.name} · {settings.profile?.email} · Plano {settings.profile?.plan?.toUpperCase()}
                  </div>
                </div>
              </div>
              <Badge tone="live">AUTENTICADO</Badge>
            </div>
          </div>

          {/* Twitch */}
          <div
            className={`platform-card ${settings.profile?.accounts?.twitch?.connected ? 'is-connected' : ''}`}
          >
            <div className="platform-card-header">
              <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <span style={{ fontSize: 20 }}>🟣</span>
                <div>
                  <div className="platform-badge-twitch">Twitch</div>
                  <div style={{ fontSize: 11, color: 'var(--text-dim)' }}>
                    rtmp://live.twitch.tv/app
                  </div>
                </div>
              </div>
              <Badge tone={settings.profile?.accounts?.twitch?.connected ? 'live' : 'neutral'}>
                {settings.profile?.accounts?.twitch?.connected ? 'CONECTADO' : 'DESCONECTADO'}
              </Badge>
            </div>
            <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <input
                type="password"
                placeholder="Chave de Transmissão (live_...)"
                className="field-input"
                style={{ flex: 1 }}
                value={twitchKey}
                onChange={(e) => setTwitchKey(e.target.value)}
              />
              <Button
                size="sm"
                variant={settings.profile?.accounts?.twitch?.connected ? 'danger' : 'primary'}
                onClick={() => void handleTogglePlatform('twitch', twitchKey)}
              >
                {settings.profile?.accounts?.twitch?.connected ? 'Desconectar' : 'Conectar'}
              </Button>
            </div>
          </div>

          {/* YouTube */}
          <div
            className={`platform-card ${settings.profile?.accounts?.youtube?.connected ? 'is-connected' : ''}`}
          >
            <div className="platform-card-header">
              <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <span style={{ fontSize: 20 }}>🔴</span>
                <div>
                  <div className="platform-badge-youtube">YouTube Live</div>
                  <div style={{ fontSize: 11, color: 'var(--text-dim)' }}>
                    rtmp://a.rtmp.youtube.com/live2
                  </div>
                </div>
              </div>
              <Badge tone={settings.profile?.accounts?.youtube?.connected ? 'live' : 'neutral'}>
                {settings.profile?.accounts?.youtube?.connected ? 'CONECTADO' : 'DESCONECTADO'}
              </Badge>
            </div>
            <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <input
                type="password"
                placeholder="Chave de Transmissão do YouTube"
                className="field-input"
                style={{ flex: 1 }}
                value={youtubeKey}
                onChange={(e) => setYoutubeKey(e.target.value)}
              />
              <Button
                size="sm"
                variant={settings.profile?.accounts?.youtube?.connected ? 'danger' : 'primary'}
                onClick={() => void handleTogglePlatform('youtube', youtubeKey)}
              >
                {settings.profile?.accounts?.youtube?.connected ? 'Desconectar' : 'Conectar'}
              </Button>
            </div>
          </div>

          {/* Kick */}
          <div
            className={`platform-card ${settings.profile?.accounts?.kick?.connected ? 'is-connected' : ''}`}
          >
            <div className="platform-card-header">
              <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <span style={{ fontSize: 20 }}>🟢</span>
                <div>
                  <div className="platform-badge-kick">Kick</div>
                  <div style={{ fontSize: 11, color: 'var(--text-dim)' }}>
                    rtmps://fa723fc1b171.global-contribute.live-video.net/app
                  </div>
                </div>
              </div>
              <Badge tone={settings.profile?.accounts?.kick?.connected ? 'live' : 'neutral'}>
                {settings.profile?.accounts?.kick?.connected ? 'CONECTADO' : 'DESCONECTADO'}
              </Badge>
            </div>
            <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <input
                type="password"
                placeholder="Chave de Stream do Kick"
                className="field-input"
                style={{ flex: 1 }}
                value={kickKey}
                onChange={(e) => setKickKey(e.target.value)}
              />
              <Button
                size="sm"
                variant={settings.profile?.accounts?.kick?.connected ? 'danger' : 'primary'}
                onClick={() => void handleTogglePlatform('kick', kickKey)}
              >
                {settings.profile?.accounts?.kick?.connected ? 'Desconectar' : 'Conectar'}
              </Button>
            </div>
          </div>

          {/* TikTok Live */}
          <div
            className={`platform-card ${settings.profile?.accounts?.tiktok?.connected ? 'is-connected' : ''}`}
          >
            <div className="platform-card-header">
              <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <span style={{ fontSize: 20 }}>⬛</span>
                <div>
                  <div className="platform-badge-tiktok">TikTok Live (Vertical 9:16)</div>
                  <div style={{ fontSize: 11, color: 'var(--text-dim)' }}>
                    rtmp://push.tiktokv.com/live · Alimenta a saída 9:16 nativa
                  </div>
                </div>
              </div>
              <Badge tone={settings.profile?.accounts?.tiktok?.connected ? 'live' : 'neutral'}>
                {settings.profile?.accounts?.tiktok?.connected ? 'CONECTADO' : 'DESCONECTADO'}
              </Badge>
            </div>
            <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <input
                type="password"
                placeholder="Chave de Transmissão do TikTok Live"
                className="field-input"
                style={{ flex: 1 }}
                value={tiktokKey}
                onChange={(e) => setTiktokKey(e.target.value)}
              />
              <Button
                size="sm"
                variant={settings.profile?.accounts?.tiktok?.connected ? 'danger' : 'primary'}
                onClick={() => void handleTogglePlatform('tiktok', tiktokKey)}
              >
                {settings.profile?.accounts?.tiktok?.connected ? 'Desconectar' : 'Conectar'}
              </Button>
            </div>
          </div>
        </div>
      </Modal>

      {/* Modal 2: Adicionar Fonte */}
      <Modal
        open={addSourceModalOpen}
        onClose={() => setAddSourceModalOpen(false)}
        title="Adicionar Fonte à Cena"
        width={560}
        icon={<IconPlus size={20} />}
      >
        <div className="add-source-grid">
          <button
            className="add-source-btn"
            onClick={() => void handleAddSourceType('screen_capture')}
          >
            <span style={{ fontSize: 24 }}>🖥️</span>
            <div>
              <div style={{ fontWeight: 600 }}>Captura de Tela</div>
              <div style={{ fontSize: 11, color: 'var(--text-dim)' }}>Monitor completo (1080p / 4K)</div>
            </div>
          </button>

          <button
            className="add-source-btn"
            onClick={() => void handleAddSourceType('window_capture')}
          >
            <span style={{ fontSize: 24 }}>🪟</span>
            <div>
              <div style={{ fontWeight: 600 }}>Captura de Janela</div>
              <div style={{ fontSize: 11, color: 'var(--text-dim)' }}>Jogo ou aplicativo específico</div>
            </div>
          </button>

          <button
            className="add-source-btn"
            onClick={() => void handleAddSourceType('camera_input')}
          >
            <span style={{ fontSize: 24 }}>📷</span>
            <div>
              <div style={{ fontWeight: 600 }}>Câmera / Webcam</div>
              <div style={{ fontSize: 11, color: 'var(--text-dim)' }}>Facecam com recorte ou PiP</div>
            </div>
          </button>

          <button
            className="add-source-btn"
            onClick={() => void handleAddSourceType('audio_input')}
          >
            <span style={{ fontSize: 24 }}>🎙️</span>
            <div>
              <div style={{ fontWeight: 600 }}>Entrada de Áudio</div>
              <div style={{ fontSize: 11, color: 'var(--text-dim)' }}>Microfone USB ou XLR</div>
            </div>
          </button>

          <button
            className="add-source-btn"
            onClick={() => void handleAddSourceType('audio_output')}
          >
            <span style={{ fontSize: 24 }}>🔊</span>
            <div>
              <div style={{ fontWeight: 600 }}>Saída de Áudio</div>
              <div style={{ fontSize: 11, color: 'var(--text-dim)' }}>Áudio do PC / Jogos / Música</div>
            </div>
          </button>

          <button
            className="add-source-btn"
            onClick={() => void handleAddSourceType('browser_source')}
          >
            <span style={{ fontSize: 24 }}>🌐</span>
            <div>
              <div style={{ fontWeight: 600 }}>Browser Source</div>
              <div style={{ fontSize: 11, color: 'var(--text-dim)' }}>StreamElements, chat, alertas</div>
            </div>
          </button>

          <button
            className="add-source-btn"
            onClick={() => void handleAddSourceType('color_source')}
          >
            <span style={{ fontSize: 24 }}>🎨</span>
            <div>
              <div style={{ fontWeight: 600 }}>Fonte de Cor</div>
              <div style={{ fontSize: 11, color: 'var(--text-dim)' }}>Fundo ou tarja para overlays</div>
            </div>
          </button>

          <button
            className="add-source-btn"
            onClick={() => void handleAddSourceType('text_source')}
          >
            <span style={{ fontSize: 24 }}>📝</span>
            <div>
              <div style={{ fontWeight: 600 }}>Texto (GDI+)</div>
              <div style={{ fontSize: 11, color: 'var(--text-dim)' }}>Títulos, avisos e metas na live</div>
            </div>
          </button>
        </div>
      </Modal>

      {/* Modal 3: Propriedades da Fonte */}
      <Modal
        open={editingItem !== null}
        onClose={() => setEditingItem(null)}
        title={`Propriedades: ${editingItem?.name || 'Fonte'}`}
        width={480}
        icon={<IconSettings size={20} />}
        footer={
          <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end', width: '100%' }}>
            <Button variant="subtle" onClick={() => setEditingItem(null)}>
              Cancelar
            </Button>
            <Button variant="primary" onClick={handleSaveEditingItem}>
              Salvar Propriedades
            </Button>
          </div>
        }
      >
        {editingItem && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: '12px' }}>
            <div className="studio-field">
              <span className="studio-field-label">Nome da Fonte</span>
              <input
                type="text"
                className="field-input"
                value={editingItem.name}
                onChange={(e) => setEditingItem({ ...editingItem, name: e.target.value })}
              />
            </div>

            {editingItem.type === 'text_source' && (
              <div className="studio-field">
                <span className="studio-field-label">Texto a Exibir</span>
                <input
                  type="text"
                  className="field-input"
                  value={editingItem.settings?.text || ''}
                  onChange={(e) =>
                    setEditingItem({
                      ...editingItem,
                      settings: { ...editingItem.settings, text: e.target.value }
                    })
                  }
                />
              </div>
            )}

            {editingItem.type === 'color_source' && (
              <div className="studio-field">
                <span className="studio-field-label">Cor Hexadecimal</span>
                <input
                  type="color"
                  value={editingItem.settings?.color || '#090d16'}
                  onChange={(e) =>
                    setEditingItem({
                      ...editingItem,
                      settings: { ...editingItem.settings, color: e.target.value }
                    })
                  }
                  style={{ width: '100%', height: 38, border: 'none', background: 'transparent' }}
                />
              </div>
            )}

            {editingItem.type === 'browser_source' && (
              <div className="studio-field">
                <span className="studio-field-label">URL do Overlay (StreamElements / Widget)</span>
                <input
                  type="text"
                  className="field-input"
                  value={editingItem.settings?.url || ''}
                  onChange={(e) =>
                    setEditingItem({
                      ...editingItem,
                      settings: { ...editingItem.settings, url: e.target.value }
                    })
                  }
                />
              </div>
            )}

            <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              <Toggle
                label="Exibir no Canvas Horizontal (16:9)"
                checked={editingItem.showOnHorizontal}
                onChange={(v) => setEditingItem({ ...editingItem, showOnHorizontal: v })}
              />
              <Toggle
                label="Exibir no Canvas Vertical (9:16)"
                checked={editingItem.showOnVertical}
                onChange={(v) => setEditingItem({ ...editingItem, showOnVertical: v })}
              />
            </div>
          </div>
        )}
      </Modal>

      {/* Modal 4: Criar Nova Cena */}
      <Modal
        open={newScenePromptOpen}
        onClose={() => setNewScenePromptOpen(false)}
        title="Criar Nova Cena"
        width={420}
        icon={<IconLayers size={20} />}
        footer={
          <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end', width: '100%' }}>
            <Button variant="subtle" onClick={() => setNewScenePromptOpen(false)}>
              Cancelar
            </Button>
            <Button variant="primary" onClick={handleCreateScene}>
              Criar Cena
            </Button>
          </div>
        }
      >
        <div className="studio-field">
          <span className="studio-field-label">Nome da Cena</span>
          <input
            type="text"
            className="field-input"
            placeholder="Ex: Gameplay com Webcam, Intervalo..."
            value={newSceneName}
            onChange={(e) => setNewSceneName(e.target.value)}
            autoFocus
          />
        </div>
      </Modal>

      {/* Modal 5: Trocar / Selecionar Fonte de Tela */}
      <Modal
        open={sourceModalOpen}
        onClose={() => setSourceModalOpen(false)}
        title="Selecionar Tela ou Janela para Transmissão"
        width={680}
        icon={<IconRefresh size={20} />}
        footer={
          <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end', width: '100%' }}>
            <Button variant="subtle" onClick={() => setSourceModalOpen(false)}>
              Cancelar
            </Button>
            <Button
              variant="primary"
              disabled={!selectedNewSource || switchingSource}
              onClick={() => void confirmSourceSwitch()}
            >
              {switchingSource ? 'Capturando...' : 'Confirmar e Capturar'}
            </Button>
          </div>
        }
      >
        <div className="source-switch-modal">
          <div className="source-switch-bar">
            <Segmented
              value={sourceTab}
              onChange={(k) => setSourceTab(k as SourceKind)}
              options={[
                { value: 'screen', label: 'Telas / Monitores' },
                { value: 'window', label: 'Janelas de Aplicativos' }
              ]}
            />
            <IconButton label="Atualizar lista" onClick={() => void openSourceSwitcher()}>
              <IconRefresh size={16} />
            </IconButton>
          </div>

          <div className="source-switch-grid">
            {loadingSources ? (
              <div style={{ padding: '32px', textAlign: 'center' }}>
                <span className="spinner" />
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

      {/* Modal 6: Configurações do Estúdio */}
      <Modal
        open={settingsModalOpen}
        onClose={() => setSettingsModalOpen(false)}
        title="Configurações do PrismCast Studio"
        width={600}
        icon={<IconSettings size={20} />}
        footer={
          <Button variant="primary" onClick={() => setSettingsModalOpen(false)}>
            Fechar
          </Button>
        }
      >
        <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
          <div className="studio-card">
            <div className="studio-card-title">Qualidade & Codificação por Hardware</div>
            <QualityPanel
              settings={settings.quality}
              onChange={(quality) => {
                onSettingsChange({ quality })
                broadcast.updateSettings(quality)
              }}
            />
          </div>

          <div className="studio-card">
            <div className="studio-card-title">Dispositivos de Áudio</div>
            <div className="studio-field">
              <span className="studio-field-label">Dispositivo de Microfone</span>
              <select
                className="field-input"
                value={settings.audioMixer.micDeviceId || 'default'}
                onChange={(e) =>
                  onSettingsChange({
                    audioMixer: {
                      ...settings.audioMixer,
                      micDeviceId: e.target.value === 'default' ? null : e.target.value
                    }
                  })
                }
              >
                <option value="default">Microfone Padrão do Sistema</option>
                {audioDevices
                  .filter((d) => d.kind === 'audioinput' && d.deviceId !== 'default')
                  .map((d) => (
                    <option key={d.deviceId} value={d.deviceId}>
                      {d.label || `Microfone (${d.deviceId.slice(0, 8)})`}
                    </option>
                  ))}
              </select>
            </div>
          </div>
        </div>
      </Modal>
    </div>
  )
}
