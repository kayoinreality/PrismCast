import { existsSync, readFileSync, writeFileSync, mkdirSync, renameSync } from 'node:fs'
import { hostname, userInfo } from 'node:os'
import { dirname, join } from 'node:path'
import { app, safeStorage } from 'electron'
import { defaultSettings, DEFAULT_PRESET_ID } from '@shared/quality'
import { defaultMixer, normalizeMixer } from '@shared/audio'
import { PLATFORM_INFO, PRISMCAST_TWITCH_CLIENT_ID, type AppSettings } from '@shared/ipc'
import type {
  ChatSettings,
  Destination,
  HotkeyBindings,
  LiveInfo,
  PlatformId,
  Scene,
  SceneItem,
  StudioCompositorSettings,
  StudioScenesState
} from '@shared/protocol'

export const SCHEMA_VERSION = 3

let cached: AppSettings | null = null

function file(): string {
  return join(app.getPath('userData'), 'prismcast-settings.json')
}

/** Escrita atômica: um crash no meio não pode deixar o JSON pela metade. */
function writeJson(path: string, data: unknown): void {
  mkdirSync(dirname(path), { recursive: true })
  const tmp = `${path}.tmp`
  writeFileSync(tmp, JSON.stringify(data, null, 2), 'utf8')
  renameSync(tmp, path)
}

function friendlyName(): string {
  try {
    const user = userInfo().username
    return user ? `Estúdio de ${user}` : hostname()
  } catch {
    return hostname()
  }
}

export function defaultStudioSettings(): StudioCompositorSettings {
  return {
    cameraEnabled: true,
    dualOutputEnabled: false,
    fps: 60,
    privacyShieldActive: false,
    privacyShieldTitle: 'VOLTO JÁ',
    privacyShieldSubtitle: 'Só um minutinho',
    replayBufferSeconds: 30
  }
}

export function defaultHotkeys(): HotkeyBindings {
  return {
    toggleMicMute: 'CommandOrControl+Shift+M',
    toggleSystemMute: 'CommandOrControl+Shift+D',
    togglePrivacyShield: 'CommandOrControl+Shift+P',
    saveReplayClip: 'CommandOrControl+Shift+S',
    toggleVerticalPreview: 'CommandOrControl+Shift+V',
    toggleCamera: 'CommandOrControl+Shift+C'
  }
}

export function defaultLive(): LiveInfo {
  return { title: '', categoryId: '', categoryName: '' }
}

export function defaultChat(): ChatSettings {
  return { channel: '', youtube: '', kick: '', fontSize: 14, showTimestamps: false, popoutOnTop: true, popoutBounds: null }
}

export function makeDestination(platform: PlatformId, id = `${platform}-main`): Destination {
  const info = PLATFORM_INFO[platform]
  return {
    id,
    platform,
    name: info.label,
    enabled: false,
    server: info.defaultServer,
    hasKey: false,
    orientation: info.orientation,
    bitrateKbps: info.bitrateKbps,
    account: null
  }
}

export function defaultDestinations(): Destination[] {
  return (['twitch', 'youtube', 'kick', 'tiktok'] as const).map((p) => makeDestination(p))
}

function fullTransform(zIndex: number): SceneItem['transform'] {
  return { x: 0, y: 0, width: 1, height: 1, zIndex }
}

export function defaultScenesState(): StudioScenesState {
  const main: Scene = {
    id: 'scene-main',
    name: 'Cena principal',
    createdAt: Date.now(),
    items: [
      {
        id: 'item-display',
        sourceId: 'display',
        name: 'Tela principal',
        type: 'screen_capture',
        visible: true,
        locked: false,
        transform: fullTransform(1),
        verticalTransform: fullTransform(1),
        showOnHorizontal: true,
        showOnVertical: true,
        settings: {}
      },
      {
        id: 'item-mic',
        sourceId: 'mic',
        name: 'Microfone',
        type: 'audio_input',
        visible: true,
        locked: true,
        transform: fullTransform(2),
        verticalTransform: fullTransform(2),
        showOnHorizontal: true,
        showOnVertical: true,
        settings: { audioChannelId: 'mic' }
      }
    ]
  }
  return {
    scenes: [main],
    activeSceneId: main.id,
    previewSceneId: main.id,
    studioMode: false,
    transitionType: 'fade',
    transitionDurationMs: 300
  }
}

function fresh(): AppSettings {
  const name = friendlyName()
  return {
    schemaVersion: SCHEMA_VERSION,
    hostName: name,
    viewerName: name,
    quality: defaultSettings(),
    presetId: DEFAULT_PRESET_ID,
    requirePin: true,
    preferredInterface: null,
    firewallPromptDismissed: false,
    audioMixer: defaultMixer(),
    studio: defaultStudioSettings(),
    hotkeys: defaultHotkeys(),
    destinations: defaultDestinations(),
    scenes: defaultScenesState(),
    twitchClientId: '',
    kitPath: guessKitPath(),
    live: defaultLive(),
    chat: defaultChat()
  }
}

/** O kit costuma ficar na raiz do mesmo disco do app; se existir, já vem configurado. */
function guessKitPath(): string | null {
  const candidates = [join(dirname(app.getAppPath()), 'kayoinreality-kit'), 'E:\\kayoinreality-kit']
  return candidates.find((p) => existsSync(join(p, 'obs'))) ?? null
}

// ------------------------------------------------------------- segredos

/**
 * Chaves de transmissão e tokens ficam fora do JSON de preferências, criptografados
 * com o DPAPI do Windows (safeStorage). Nunca saem do processo main.
 */
type SecretsFile = Record<string, string>
let secrets: SecretsFile | null = null

function secretsFile(): string {
  return join(app.getPath('userData'), 'prismcast-secrets.json')
}

function loadSecrets(): SecretsFile {
  if (secrets) return secrets
  try {
    secrets = JSON.parse(readFileSync(secretsFile(), 'utf8')) as SecretsFile
  } catch {
    secrets = {}
  }
  return secrets
}

export function getSecret(name: string): string | null {
  const raw = loadSecrets()[name]
  if (!raw) return null
  try {
    if (raw.startsWith('enc:')) {
      return safeStorage.decryptString(Buffer.from(raw.slice(4), 'base64'))
    }
    return raw.startsWith('raw:') ? raw.slice(4) : null
  } catch {
    return null
  }
}

export function setSecret(name: string, value: string | null): void {
  const all = { ...loadSecrets() }
  if (!value) {
    delete all[name]
  } else if (safeStorage.isEncryptionAvailable()) {
    all[name] = `enc:${safeStorage.encryptString(value).toString('base64')}`
  } else {
    all[name] = `raw:${value}`
  }
  secrets = all
  try {
    writeJson(secretsFile(), all)
  } catch (err) {
    console.warn('[store] nao foi possivel salvar os segredos:', (err as Error).message)
  }
}

export const streamKeySecret = (destinationId: string): string => `streamKey:${destinationId}`

// ------------------------------------------------------------- migração

/** Chaves que a versão 2.1 inventava ao clicar "Conectar" sem chave. Nunca funcionaram. */
function isFakeKey(key: string): boolean {
  return /^live_(twitch|youtube|kick|tiktok|prismcast)_[a-z0-9]{4,10}$/.test(key) || key.startsWith('pk_live_local')
}

const DEMO_SCENE_IDS = new Set([
  'scene-gameplay-cam',
  'scene-just-chatting',
  'scene-starting',
  'scene-brb',
  'scene-ending'
])

interface LegacyTarget {
  provider?: string
  rtmpUrl?: string
  streamKey?: string
  enabled?: boolean
}

/** v2.x -> v3: contas falsas viram destinos reais; chaves reais vão para o cofre. */
function migrate(input: Record<string, unknown>, base: AppSettings): Partial<AppSettings> {
  const out: Partial<AppSettings> = {}
  const destinations = defaultDestinations()

  const legacyTargets = Array.isArray(input.multistreamTargets)
    ? (input.multistreamTargets as LegacyTarget[])
    : []
  for (const t of legacyTargets) {
    const dest = destinations.find((d) => d.platform === t.provider)
    const key = (t.streamKey ?? '').trim()
    if (!dest || !key || isFakeKey(key)) continue
    setSecret(streamKeySecret(dest.id), key)
    dest.hasKey = true
    dest.enabled = Boolean(t.enabled)
    if (t.rtmpUrl) dest.server = t.rtmpUrl
  }
  out.destinations = destinations

  // As cenas de demonstração da 2.x eram só placeholders: saem. Cenas criadas pelo usuário ficam.
  const legacyScenes = input.scenes as StudioScenesState | undefined
  const userScenes = (legacyScenes && Array.isArray(legacyScenes.scenes) ? legacyScenes.scenes : [])
    .filter((s) => !DEMO_SCENE_IDS.has(s.id))
    .map((s) => ({
      ...s,
      items: s.items.filter((it) => (it.type as string) !== 'audio_input' && (it.type as string) !== 'audio_output')
    }))
  out.scenes = { ...base.scenes, scenes: [...base.scenes.scenes, ...userScenes] }

  const legacyStudio = (input.studio ?? {}) as Partial<StudioCompositorSettings>
  out.studio = {
    ...base.studio,
    dualOutputEnabled: Boolean(legacyStudio.dualOutputEnabled ?? base.studio.dualOutputEnabled),
    replayBufferSeconds: legacyStudio.replayBufferSeconds ?? base.studio.replayBufferSeconds
  }
  return out
}

function normalizeDestinations(list: unknown, base: Destination[]): Destination[] {
  if (!Array.isArray(list)) return base
  return (list as Destination[])
    .filter((d) => d && typeof d.id === 'string' && d.platform in PLATFORM_INFO)
    .map((d) => ({ ...makeDestination(d.platform, d.id), ...d, hasKey: getSecret(streamKeySecret(d.id)) !== null }))
}

function normalize(raw: unknown): AppSettings {
  const base = fresh()
  if (!raw || typeof raw !== 'object') return base
  let input = raw as Record<string, unknown>
  if (typeof input.schemaVersion !== 'number' || input.schemaVersion < SCHEMA_VERSION) {
    input = { ...input, ...migrate(input, base), schemaVersion: SCHEMA_VERSION }
  }
  const typed = input as Partial<AppSettings>
  const mixer = normalizeMixer(typed.audioMixer)
  const micChannel = mixer.channels.find((c) => c.kind === 'mic')
  const scenes: StudioScenesState =
    typed.scenes && Array.isArray(typed.scenes.scenes) && typed.scenes.scenes.length > 0
      ? typed.scenes
      : base.scenes

  if (micChannel && !scenes.scenes.some((s: Scene) => s.items.some((it: SceneItem) => it.type === 'audio_input'))) {
    for (const sc of scenes.scenes) {
      if (!/volto|intervalo|pause|brb/i.test(sc.name) && !sc.items.some((it: SceneItem) => it.type === 'audio_input')) {
        sc.items.push({
          id: `item-mic-${sc.id}`,
          sourceId: micChannel.id,
          name: micChannel.name,
          type: 'audio_input',
          visible: true,
          locked: true,
          transform: fullTransform(sc.items.length + 1),
          verticalTransform: fullTransform(sc.items.length + 1),
          showOnHorizontal: true,
          showOnVertical: true,
          settings: { audioChannelId: micChannel.id }
        })
      }
    }
  }

  return {
    schemaVersion: SCHEMA_VERSION,
    hostName: typed.hostName ?? base.hostName,
    viewerName: typed.viewerName ?? base.viewerName,
    quality: { ...base.quality, ...(typed.quality ?? {}) },
    presetId: typed.presetId ?? base.presetId,
    requirePin: typed.requirePin ?? base.requirePin,
    preferredInterface: typed.preferredInterface ?? null,
    firewallPromptDismissed: typed.firewallPromptDismissed ?? false,
    audioMixer: mixer,
    studio: { ...base.studio, ...(typed.studio ?? {}) },
    hotkeys: { ...base.hotkeys, ...(typed.hotkeys ?? {}) },
    destinations: normalizeDestinations(typed.destinations, base.destinations),
    scenes,
    // Igual ao embutido não é "próprio": guarda vazio para seguir o do app se ele mudar.
    twitchClientId:
      typeof typed.twitchClientId === 'string' && typed.twitchClientId.trim() !== PRISMCAST_TWITCH_CLIENT_ID
        ? typed.twitchClientId.trim()
        : '',
    kitPath: typed.kitPath === undefined ? base.kitPath : typed.kitPath,
    live: { ...base.live, ...(typed.live ?? {}) },
    chat: { ...base.chat, ...(typed.chat ?? {}) }
  }
}

export function getSettings(): AppSettings {
  if (cached) return cached
  let raw: unknown = null
  try {
    raw = JSON.parse(readFileSync(file(), 'utf8'))
  } catch {
    raw = null
  }
  cached = normalize(raw)
  // Persiste já migrado: as chaves falsas somem do disco na primeira abertura.
  if (raw && (raw as { schemaVersion?: number }).schemaVersion !== SCHEMA_VERSION) persist(cached)
  return cached
}

function persist(next: AppSettings): void {
  try {
    writeJson(file(), next)
  } catch (err) {
    console.warn('[store] nao foi possivel salvar as preferencias:', (err as Error).message)
  }
}

/**
 * `hasKey` e `account` são do main: o renderer pode mandar uma cópia velha deles
 * junto com uma mudança de outro campo, e isso não pode apagar uma chave salva.
 */
export function setSettings(patch: Partial<AppSettings>, fromMain = false): AppSettings {
  const current = getSettings()
  let destinations = patch.destinations ?? current.destinations
  if (patch.destinations && !fromMain) {
    destinations = patch.destinations.map((d) => {
      const prev = current.destinations.find((p) => p.id === d.id)
      return {
        ...d,
        hasKey: getSecret(streamKeySecret(d.id)) !== null,
        account: prev?.account ?? null
      }
    })
  }
  const next = normalize({ ...current, ...patch, destinations })
  cached = next
  persist(next)
  return next
}

export function updateDestination(id: string, patch: Partial<Destination>): AppSettings {
  const current = getSettings()
  return setSettings(
    {
      destinations: current.destinations.map((d) => (d.id === id ? { ...d, ...patch } : d))
    },
    true
  )
}
