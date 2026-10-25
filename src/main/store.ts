import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { hostname, userInfo } from 'node:os'
import { dirname, join } from 'node:path'
import { app } from 'electron'
import { defaultSettings, DEFAULT_PRESET_ID } from '@shared/quality'
import type { AppSettings } from '@shared/ipc'
import type {
  AudioMixerSettings,
  ConnectedAccount,
  HotkeyBindings,
  MultistreamTarget,
  PlatformId,
  Scene,
  SceneItem,
  StreamWidget,
  StudioCompositorSettings,
  StudioScenesState,
  UserProfile
} from '@shared/protocol'

let cached: AppSettings | null = null

function file(): string {
  return join(app.getPath('userData'), 'prismcast-settings.json')
}

function friendlyName(): string {
  try {
    const user = userInfo().username
    return user ? `Estúdio de ${user}` : hostname()
  } catch {
    return hostname()
  }
}

export function defaultAudioMixer(): AudioMixerSettings {
  return {
    systemAudioEnabled: true,
    systemVolume: 1,
    systemMuted: false,
    micEnabled: false,
    micDeviceId: null,
    micVolume: 1,
    micMuted: false,
    noiseSuppression: true,
    echoCancellation: true,
    compressorEnabled: true,
    partyVoiceEnabled: true
  }
}

export function defaultStudioSettings(): StudioCompositorSettings {
  return {
    dualOutputEnabled: true,
    verticalLayout: 'split-cam-top',
    webcamEnabled: false,
    webcamDeviceId: null,
    webcamPosition: 'bottom-right',
    webcamScale: 0.24,
    webcamRound: false,
    privacyShieldActive: false,
    privacyShieldTitle: 'VOLTO JÁ',
    privacyShieldSubtitle: 'Transmissão em pausa temporária · PrismCast Studio',
    replayBufferSeconds: 30
  }
}

export function defaultHotkeys(): HotkeyBindings {
  return {
    toggleMicMute: 'CommandOrControl+Shift+M',
    toggleSystemMute: 'CommandOrControl+Shift+D',
    togglePrivacyShield: 'CommandOrControl+Shift+P',
    saveReplayClip: 'CommandOrControl+Shift+S',
    toggleVerticalPreview: 'CommandOrControl+Shift+V'
  }
}

export function defaultMultistreamTargets(): MultistreamTarget[] {
  return [
    {
      id: 'twitch-main',
      provider: 'twitch',
      name: 'Twitch',
      enabled: false,
      rtmpUrl: 'rtmp://live.twitch.tv/app',
      streamKey: '',
      orientation: 'horizontal',
      bitrateKbps: 6000
    },
    {
      id: 'youtube-main',
      provider: 'youtube',
      name: 'YouTube Live',
      enabled: false,
      rtmpUrl: 'rtmp://a.rtmp.youtube.com/live2',
      streamKey: '',
      orientation: 'horizontal',
      bitrateKbps: 9000
    },
    {
      id: 'kick-main',
      provider: 'kick',
      name: 'Kick',
      enabled: false,
      rtmpUrl: 'rtmps://fa723fc1b171.global-contribute.live-video.net/app',
      streamKey: '',
      orientation: 'horizontal',
      bitrateKbps: 8000
    },
    {
      id: 'tiktok-vertical',
      provider: 'tiktok',
      name: 'TikTok Live (9:16)',
      enabled: false,
      rtmpUrl: 'rtmp://push.tiktokv.com/live',
      streamKey: '',
      orientation: 'vertical',
      bitrateKbps: 4500
    }
  ]
}

export function defaultWidgets(): StreamWidget[] {
  return [
    {
      id: 'widget-streamelements',
      name: 'StreamElements Overlay',
      kind: 'streamelements',
      enabled: false,
      url: 'https://streamelements.com/overlay/SEU_OVERLAY_ID/SEU_TOKEN',
      primaryText: 'StreamElements AlertBox & Widgets',
      secondaryText: 'Ativo em 16:9 e 9:16',
      accentColor: '#6366f1',
      x: 0.03,
      y: 0.04,
      width: 0.36,
      height: 0.14,
      verticalX: 0.06,
      verticalY: 0.06,
      showOnHorizontal: true,
      showOnVertical: true,
      opacity: 1
    },
    {
      id: 'widget-goal',
      name: 'Meta de Seguidores / Sub Goal',
      kind: 'goal-bar',
      enabled: false,
      url: '',
      primaryText: 'META DA LIVE · 140 / 200',
      secondaryText: '70% alcançado',
      accentColor: '#10b981',
      x: 0.03,
      y: 0.88,
      width: 0.3,
      height: 0.08,
      verticalX: 0.08,
      verticalY: 0.86,
      showOnHorizontal: true,
      showOnVertical: true,
      opacity: 0.95
    },
    {
      id: 'widget-lower-third',
      name: 'Lower Third / Tarja do Streamer',
      kind: 'lower-third',
      enabled: false,
      url: '',
      primaryText: 'AO VIVO NO PRISMCAST STUDIO',
      secondaryText: 'Twitch · YouTube · Kick · TikTok · LAN',
      accentColor: '#8b5cf6',
      x: 0.03,
      y: 0.77,
      width: 0.34,
      height: 0.09,
      verticalX: 0.06,
      verticalY: 0.75,
      showOnHorizontal: true,
      showOnVertical: true,
      opacity: 0.95
    }
  ]
}

export function defaultScenesState(): StudioScenesState {
  const scene1: Scene = {
    id: 'scene-gameplay-cam',
    name: '🎮 Gameplay & Webcam',
    createdAt: Date.now() - 4000,
    items: [
      {
        id: 'item-screen-main',
        sourceId: 'screen-primary',
        name: 'Tela Principal (Captura)',
        type: 'screen_capture',
        visible: true,
        locked: true,
        transform: { x: 0, y: 0, width: 1, height: 1, zIndex: 1 },
        verticalTransform: { x: -0.38, y: 0, width: 1.76, height: 1, zIndex: 1 },
        showOnHorizontal: true,
        showOnVertical: true,
        settings: { displayId: 'primary' }
      },
      {
        id: 'item-webcam-corner',
        sourceId: 'camera-default',
        name: 'Câmera do Streamer (Facecam)',
        type: 'camera_input',
        visible: true,
        locked: false,
        transform: { x: 0.72, y: 0.68, width: 0.26, height: 0.28, zIndex: 10 },
        verticalTransform: { x: 0.05, y: 0.05, width: 0.9, height: 0.28, zIndex: 10 },
        showOnHorizontal: true,
        showOnVertical: true,
        settings: { deviceId: 'default', round: false }
      },
      {
        id: 'item-mic-host',
        sourceId: 'audio-mic-primary',
        name: 'Microfone Principal',
        type: 'audio_input',
        visible: true,
        locked: true,
        transform: { x: 0, y: 0, width: 0, height: 0, zIndex: 0 },
        verticalTransform: { x: 0, y: 0, width: 0, height: 0, zIndex: 0 },
        showOnHorizontal: true,
        showOnVertical: true,
        settings: { deviceId: 'default', volume: 1, muted: false }
      },
      {
        id: 'item-audio-desktop',
        sourceId: 'audio-desktop-primary',
        name: 'Áudio do Sistema (Jogos/PC)',
        type: 'audio_output',
        visible: true,
        locked: true,
        transform: { x: 0, y: 0, width: 0, height: 0, zIndex: 0 },
        verticalTransform: { x: 0, y: 0, width: 0, height: 0, zIndex: 0 },
        showOnHorizontal: true,
        showOnVertical: true,
        settings: { volume: 1, muted: false }
      }
    ]
  }

  const scene2: Scene = {
    id: 'scene-just-chatting',
    name: '💬 Bate-Papo (Just Chatting)',
    createdAt: Date.now() - 3000,
    items: [
      {
        id: 'item-webcam-fullscreen',
        sourceId: 'camera-default',
        name: 'Câmera Principal',
        type: 'camera_input',
        visible: true,
        locked: true,
        transform: { x: 0.04, y: 0.06, width: 0.68, height: 0.88, zIndex: 2 },
        verticalTransform: { x: 0.05, y: 0.05, width: 0.9, height: 0.5, zIndex: 2 },
        showOnHorizontal: true,
        showOnVertical: true,
        settings: { deviceId: 'default' }
      },
      {
        id: 'item-chat-overlay',
        sourceId: 'browser-chat',
        name: 'Chat da Live (Overlay)',
        type: 'browser_source',
        visible: true,
        locked: false,
        transform: { x: 0.74, y: 0.06, width: 0.22, height: 0.88, zIndex: 5 },
        verticalTransform: { x: 0.05, y: 0.58, width: 0.9, height: 0.38, zIndex: 5 },
        showOnHorizontal: true,
        showOnVertical: true,
        settings: { url: 'https://streamelements.com/overlay/chat', fps: 30 }
      },
      {
        id: 'item-mic-chat',
        sourceId: 'audio-mic-primary',
        name: 'Microfone',
        type: 'audio_input',
        visible: true,
        locked: true,
        transform: { x: 0, y: 0, width: 0, height: 0, zIndex: 0 },
        verticalTransform: { x: 0, y: 0, width: 0, height: 0, zIndex: 0 },
        showOnHorizontal: true,
        showOnVertical: true,
        settings: { deviceId: 'default' }
      }
    ]
  }

  const scene3: Scene = {
    id: 'scene-starting',
    name: '⏳ Começando em Breve',
    createdAt: Date.now() - 2000,
    items: [
      {
        id: 'item-bg-starting',
        sourceId: 'color-bg',
        name: 'Fundo Gradiente Escuro',
        type: 'color_source',
        visible: true,
        locked: true,
        transform: { x: 0, y: 0, width: 1, height: 1, zIndex: 1 },
        verticalTransform: { x: 0, y: 0, width: 1, height: 1, zIndex: 1 },
        showOnHorizontal: true,
        showOnVertical: true,
        settings: { color: '#090d16' }
      },
      {
        id: 'item-text-starting',
        sourceId: 'text-starting',
        name: 'Texto: Iniciando Live',
        type: 'text_source',
        visible: true,
        locked: true,
        transform: { x: 0.2, y: 0.42, width: 0.6, height: 0.16, zIndex: 5 },
        verticalTransform: { x: 0.1, y: 0.44, width: 0.8, height: 0.12, zIndex: 5 },
        showOnHorizontal: true,
        showOnVertical: true,
        settings: { text: 'A TRANSMISSÃO COMEÇARÁ EM BREVE', fontSize: 36, color: '#38bdf8' }
      }
    ]
  }

  const scene4: Scene = {
    id: 'scene-brb',
    name: '🛑 Pausa / Volto Já',
    createdAt: Date.now() - 1000,
    items: [
      {
        id: 'item-bg-brb',
        sourceId: 'color-bg-brb',
        name: 'Fundo Escudo',
        type: 'color_source',
        visible: true,
        locked: true,
        transform: { x: 0, y: 0, width: 1, height: 1, zIndex: 1 },
        verticalTransform: { x: 0, y: 0, width: 1, height: 1, zIndex: 1 },
        showOnHorizontal: true,
        showOnVertical: true,
        settings: { color: '#0f172a' }
      },
      {
        id: 'item-text-brb',
        sourceId: 'text-brb',
        name: 'Texto: Volto Já',
        type: 'text_source',
        visible: true,
        locked: true,
        transform: { x: 0.25, y: 0.42, width: 0.5, height: 0.16, zIndex: 5 },
        verticalTransform: { x: 0.1, y: 0.44, width: 0.8, height: 0.12, zIndex: 5 },
        showOnHorizontal: true,
        showOnVertical: true,
        settings: { text: 'VOLTO EM ALGUNS INSTANTES', fontSize: 38, color: '#f59e0b' }
      }
    ]
  }

  const scene5: Scene = {
    id: 'scene-ending',
    name: '🏁 Encerramento',
    createdAt: Date.now(),
    items: [
      {
        id: 'item-bg-ending',
        sourceId: 'color-bg-end',
        name: 'Fundo Escuro',
        type: 'color_source',
        visible: true,
        locked: true,
        transform: { x: 0, y: 0, width: 1, height: 1, zIndex: 1 },
        verticalTransform: { x: 0, y: 0, width: 1, height: 1, zIndex: 1 },
        showOnHorizontal: true,
        showOnVertical: true,
        settings: { color: '#05070c' }
      },
      {
        id: 'item-text-ending',
        sourceId: 'text-ending',
        name: 'Texto Encerramento',
        type: 'text_source',
        visible: true,
        locked: true,
        transform: { x: 0.2, y: 0.42, width: 0.6, height: 0.16, zIndex: 5 },
        verticalTransform: { x: 0.1, y: 0.44, width: 0.8, height: 0.12, zIndex: 5 },
        showOnHorizontal: true,
        showOnVertical: true,
        settings: { text: 'OBRIGADO POR ASSISTIR! ATÉ A PRÓXIMA!', fontSize: 32, color: '#a855f7' }
      }
    ]
  }

  return {
    scenes: [scene1, scene2, scene3, scene4, scene5],
    activeSceneId: scene1.id,
    previewSceneId: scene1.id,
    studioMode: true,
    transitionType: 'cut',
    transitionDurationMs: 300
  }
}

export function defaultUserProfile(): UserProfile {
  const name = friendlyName().replace('Estúdio de ', '') || 'Streamer'
  return {
    isLoggedIn: true,
    id: `prism_${Math.random().toString(36).slice(2, 9)}`,
    name,
    email: `${name.toLowerCase().replace(/[^a-z0-9]/g, '') || 'streamer'}@prismcast.local`,
    avatarUrl: '',
    plan: 'studio',
    accounts: {
      twitch: {
        platform: 'twitch',
        username: '',
        displayName: 'Twitch',
        avatarUrl: 'https://static-cdn.jtvnw.net/jtv_user_pictures/twitch-profile_image-70x70.png',
        connected: false,
        rtmpUrl: 'rtmp://live.twitch.tv/app',
        streamKey: ''
      },
      youtube: {
        platform: 'youtube',
        username: '',
        displayName: 'YouTube Live',
        avatarUrl: 'https://www.gstatic.com/youtube/img/branding/favicon/favicon_144x144.png',
        connected: false,
        rtmpUrl: 'rtmp://a.rtmp.youtube.com/live2',
        streamKey: ''
      },
      kick: {
        platform: 'kick',
        username: '',
        displayName: 'Kick',
        avatarUrl: '',
        connected: false,
        rtmpUrl: 'rtmps://fa723fc1b171.global-contribute.live-video.net/app',
        streamKey: ''
      },
      tiktok: {
        platform: 'tiktok',
        username: '',
        displayName: 'TikTok Live',
        avatarUrl: '',
        connected: false,
        rtmpUrl: 'rtmp://push.tiktokv.com/live',
        streamKey: ''
      },
      prismcast: {
        platform: 'prismcast',
        username: name,
        displayName: 'PrismCast Cloud ID',
        avatarUrl: '',
        connected: true,
        rtmpUrl: 'rtmp://ingest.prismcast.tv/live',
        streamKey: 'pk_live_local_studio'
      }
    }
  }
}

function fresh(): AppSettings {
  const name = friendlyName()
  return {
    hostName: name,
    viewerName: name,
    quality: defaultSettings(),
    presetId: DEFAULT_PRESET_ID,
    requirePin: true,
    preferredInterface: null,
    firewallPromptDismissed: false,
    audioMixer: defaultAudioMixer(),
    studio: defaultStudioSettings(),
    hotkeys: defaultHotkeys(),
    multistreamTargets: defaultMultistreamTargets(),
    widgets: defaultWidgets(),
    scenes: defaultScenesState(),
    profile: defaultUserProfile()
  }
}

function normalize(raw: unknown): AppSettings {
  const base = fresh()
  if (!raw || typeof raw !== 'object') return base
  const input = raw as Partial<AppSettings>
  return {
    ...base,
    ...input,
    quality: { ...base.quality, ...(input.quality ?? {}) },
    audioMixer: { ...base.audioMixer, ...(input.audioMixer ?? {}) },
    studio: { ...base.studio, ...(input.studio ?? {}) },
    hotkeys: { ...base.hotkeys, ...(input.hotkeys ?? {}) },
    multistreamTargets: Array.isArray(input.multistreamTargets)
      ? input.multistreamTargets
      : base.multistreamTargets,
    widgets: Array.isArray(input.widgets) ? input.widgets : base.widgets,
    scenes: input.scenes && Array.isArray(input.scenes.scenes) && input.scenes.scenes.length > 0
      ? input.scenes
      : base.scenes,
    profile: input.profile && input.profile.accounts ? input.profile : base.profile
  }
}

export function getSettings(): AppSettings {
  if (cached) return cached
  try {
    cached = normalize(JSON.parse(readFileSync(file(), 'utf8')))
  } catch {
    cached = fresh()
  }
  return cached
}

export function setSettings(patch: Partial<AppSettings>): AppSettings {
  const next = normalize({ ...getSettings(), ...patch })
  cached = next
  try {
    mkdirSync(dirname(file()), { recursive: true })
    writeFileSync(file(), JSON.stringify(next, null, 2), 'utf8')
  } catch (err) {
    console.warn('[store] nao foi possivel salvar as preferencias:', (err as Error).message)
  }
  return next
}

// ---------------------------------------------------- Gerenciador de Cenas & Fontes

export function getScenesState(): StudioScenesState {
  return getSettings().scenes
}

export function setScenesState(patch: Partial<StudioScenesState>): StudioScenesState {
  const current = getScenesState()
  const next: StudioScenesState = { ...current, ...patch }
  setSettings({ scenes: next })
  return next
}

export function createScene(name: string): Scene {
  const state = getScenesState()
  const newScene: Scene = {
    id: `scene-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    name: name.trim() || `Nova Cena ${state.scenes.length + 1}`,
    items: [],
    createdAt: Date.now()
  }
  setScenesState({
    scenes: [...state.scenes, newScene],
    activeSceneId: newScene.id,
    previewSceneId: newScene.id
  })
  return newScene
}

export function updateScene(sceneId: string, patch: Partial<Scene>): Scene {
  const state = getScenesState()
  let targetScene: Scene | null = null
  const scenes = state.scenes.map((s) => {
    if (s.id === sceneId) {
      targetScene = { ...s, ...patch }
      return targetScene
    }
    return s
  })
  setScenesState({ scenes })
  if (!targetScene) throw new Error(`Cena ${sceneId} não encontrada`)
  return targetScene
}

export function deleteScene(sceneId: string): void {
  const state = getScenesState()
  if (state.scenes.length <= 1) {
    throw new Error('Não é possível excluir a única cena restante')
  }
  const scenes = state.scenes.filter((s) => s.id !== sceneId)
  const activeSceneId = state.activeSceneId === sceneId ? scenes[0].id : state.activeSceneId
  const previewSceneId = state.previewSceneId === sceneId ? scenes[0].id : state.previewSceneId
  setScenesState({ scenes, activeSceneId, previewSceneId })
}

export function selectScene(sceneId: string): void {
  const state = getScenesState()
  if (!state.scenes.some((s) => s.id === sceneId)) return
  if (state.studioMode) {
    setScenesState({ previewSceneId: sceneId })
  } else {
    setScenesState({ activeSceneId: sceneId, previewSceneId: sceneId })
  }
}

export function duplicateScene(sceneId: string): Scene {
  const state = getScenesState()
  const source = state.scenes.find((s) => s.id === sceneId)
  if (!source) throw new Error(`Cena ${sceneId} não encontrada`)
  const copy: Scene = {
    id: `scene-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    name: `${source.name} (Cópia)`,
    items: source.items.map((it) => ({
      ...it,
      id: `item-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`
    })),
    createdAt: Date.now()
  }
  setScenesState({
    scenes: [...state.scenes, copy],
    previewSceneId: copy.id
  })
  return copy
}

export function setStudioMode(enabled: boolean): boolean {
  setScenesState({ studioMode: enabled })
  return enabled
}

export function transitionScenes(type: 'cut' | 'fade' = 'cut', durationMs = 300): void {
  const state = getScenesState()
  setScenesState({
    activeSceneId: state.previewSceneId,
    transitionType: type,
    transitionDurationMs: durationMs
  })
}

export function addItemToScene(sceneId: string, item: Omit<SceneItem, 'id'>): SceneItem {
  const state = getScenesState()
  const scene = state.scenes.find((s) => s.id === sceneId)
  if (!scene) throw new Error(`Cena ${sceneId} não encontrada`)

  const newItem: SceneItem = {
    ...item,
    id: `item-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`
  }

  const updatedScenes = state.scenes.map((s) => {
    if (s.id === sceneId) {
      return {
        ...s,
        items: [...s.items, newItem]
      }
    }
    return s
  })
  setScenesState({ scenes: updatedScenes })
  return newItem
}

export function updateItemInScene(
  sceneId: string,
  itemId: string,
  patch: Partial<SceneItem>
): SceneItem {
  const state = getScenesState()
  let updatedItem: SceneItem | null = null

  const updatedScenes = state.scenes.map((s) => {
    if (s.id === sceneId) {
      const items = s.items.map((it) => {
        if (it.id === itemId) {
          updatedItem = { ...it, ...patch }
          return updatedItem
        }
        return it
      })
      return { ...s, items }
    }
    return s
  })

  if (!updatedItem) throw new Error(`Item ${itemId} não encontrado na cena ${sceneId}`)
  setScenesState({ scenes: updatedScenes })
  return updatedItem
}

export function removeItemFromScene(sceneId: string, itemId: string): void {
  const state = getScenesState()
  const updatedScenes = state.scenes.map((s) => {
    if (s.id === sceneId) {
      return {
        ...s,
        items: s.items.filter((it) => it.id !== itemId)
      }
    }
    return s
  })
  setScenesState({ scenes: updatedScenes })
}

export function reorderItemInScene(sceneId: string, itemId: string, newIndex: number): void {
  const state = getScenesState()
  const updatedScenes = state.scenes.map((s) => {
    if (s.id === sceneId) {
      const items = [...s.items]
      const oldIndex = items.findIndex((it) => it.id === itemId)
      if (oldIndex < 0) return s
      const [moved] = items.splice(oldIndex, 1)
      items.splice(newIndex, 0, moved)
      return { ...s, items }
    }
    return s
  })
  setScenesState({ scenes: updatedScenes })
}

// ------------------------------------------ Gerenciador de Autenticação & Contas

export function getUserProfile(): UserProfile {
  return getSettings().profile
}

export function updateUserProfile(patch: Partial<UserProfile>): UserProfile {
  const current = getUserProfile()
  const next: UserProfile = { ...current, ...patch }
  setSettings({ profile: next })
  return next
}

export function connectPlatform(
  platform: PlatformId,
  credentials?: { username?: string; streamKey?: string; rtmpUrl?: string }
): ConnectedAccount {
  const profile = getUserProfile()
  const account = profile.accounts[platform] || {
    platform,
    username: '',
    displayName: platform,
    avatarUrl: '',
    connected: false,
    rtmpUrl: '',
    streamKey: ''
  }

  const generatedKey = credentials?.streamKey || account.streamKey || `live_${platform}_${Math.random().toString(36).slice(2, 10)}`
  const updatedAccount: ConnectedAccount = {
    ...account,
    username: credentials?.username || account.username || 'Streamer',
    displayName: account.displayName || platform.toUpperCase(),
    connected: true,
    streamKey: generatedKey,
    rtmpUrl: credentials?.rtmpUrl || account.rtmpUrl,
    connectedAt: Date.now()
  }

  const nextAccounts = {
    ...profile.accounts,
    [platform]: updatedAccount
  }

  // Sincroniza automaticamente com multistreamTargets
  const settings = getSettings()
  const targets = settings.multistreamTargets.map((t) => {
    if (t.provider === platform) {
      return {
        ...t,
        enabled: true,
        streamKey: updatedAccount.streamKey,
        rtmpUrl: updatedAccount.rtmpUrl || t.rtmpUrl
      }
    }
    return t
  })

  setSettings({
    profile: { ...profile, accounts: nextAccounts },
    multistreamTargets: targets
  })

  return updatedAccount
}

export function disconnectPlatform(platform: PlatformId): void {
  const profile = getUserProfile()
  const account = profile.accounts[platform]
  if (!account) return

  const updatedAccount: ConnectedAccount = {
    ...account,
    connected: false
  }

  const nextAccounts = {
    ...profile.accounts,
    [platform]: updatedAccount
  }

  const settings = getSettings()
  const targets = settings.multistreamTargets.map((t) => {
    if (t.provider === platform) {
      return { ...t, enabled: false }
    }
    return t
  })

  setSettings({
    profile: { ...profile, accounts: nextAccounts },
    multistreamTargets: targets
  })
}
