import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { hostname, userInfo } from 'node:os'
import { dirname, join } from 'node:path'
import { app } from 'electron'
import { defaultSettings, DEFAULT_PRESET_ID } from '@shared/quality'
import type { AppSettings } from '@shared/ipc'
import type {
  AudioMixerSettings,
  HotkeyBindings,
  MultistreamTarget,
  StreamWidget,
  StudioCompositorSettings
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
    widgets: defaultWidgets()
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
    widgets: Array.isArray(input.widgets) ? input.widgets : base.widgets
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
