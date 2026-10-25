import { PRISMCAST_TWITCH_CLIENT_ID } from '@shared/ipc'
import type {
  LiveInfo,
  PlatformAccount,
  TwitchActionResult,
  TwitchCapabilities,
  TwitchCategory
} from '@shared/protocol'
import { getSecret, getSettings, setSecret, updateDestination } from './store'

/**
 * Chamadas à API da Twitch com o login do usuário: título e categoria da live
 * e escrever no chat. O token de acesso vive só na memória do main; o de
 * renovação fica criptografado junto com a chave de transmissão.
 */

export const SCOPE_INFO = 'channel:manage:broadcast'
export const SCOPE_CHAT = 'user:write:chat'

const refreshSecret = (destinationId: string): string => `twitchRefresh:${destinationId}`

let cached: { destinationId: string; token: string; expiresAt: number } | null = null

export function rememberAccessToken(destinationId: string, token: string, expiresIn: number): void {
  cached = { destinationId, token, expiresAt: Date.now() + (expiresIn - 60) * 1000 }
}

function clientId(): string {
  return getSettings().twitchClientId.trim() || PRISMCAST_TWITCH_CLIENT_ID
}

function twitchDestination(): { id: string; account: PlatformAccount } | null {
  const dest = getSettings().destinations.find((d) => d.platform === 'twitch' && d.account)
  return dest?.account ? { id: dest.id, account: dest.account } : null
}

async function accessToken(destinationId: string): Promise<string | null> {
  if (cached && cached.destinationId === destinationId && Date.now() < cached.expiresAt) return cached.token
  const refresh = getSecret(refreshSecret(destinationId))
  if (!refresh) return null
  // Cliente público (device code): renova sem secret.
  const res = await fetch('https://id.twitch.tv/oauth2/token', {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({
      client_id: clientId(),
      grant_type: 'refresh_token',
      refresh_token: refresh
    }).toString()
  })
  if (!res.ok) return null
  const json = (await res.json()) as { access_token: string; refresh_token?: string; expires_in?: number }
  if (json.refresh_token) setSecret(refreshSecret(destinationId), json.refresh_token)
  rememberAccessToken(destinationId, json.access_token, json.expires_in ?? 3600)
  return json.access_token
}

class NeedsLogin extends Error {}

async function helix(path: string, init: RequestInit = {}): Promise<Response> {
  const dest = twitchDestination()
  if (!dest) throw new NeedsLogin('Entre com a Twitch em Destinos.')
  const token = await accessToken(dest.id)
  if (!token) throw new NeedsLogin('O login da Twitch expirou. Entre de novo em Destinos.')
  const res = await fetch(`https://api.twitch.tv/helix/${path}`, {
    ...init,
    headers: {
      Authorization: `Bearer ${token}`,
      'Client-Id': clientId(),
      ...(init.body ? { 'Content-Type': 'application/json' } : {}),
      ...(init.headers ?? {})
    }
  })
  if (res.status === 401) {
    cached = null
    throw new NeedsLogin('O login da Twitch expirou. Entre de novo em Destinos.')
  }
  return res
}

/** Logins da 2.2 não guardavam o id do canal. */
async function broadcasterId(): Promise<string> {
  const dest = twitchDestination()
  if (!dest) throw new NeedsLogin('Entre com a Twitch em Destinos.')
  if (dest.account.id) return dest.account.id
  const res = await helix('users')
  const user = ((await res.json()) as { data: Array<{ id: string }> }).data[0]
  if (!user) throw new Error('A Twitch não devolveu a conta.')
  updateDestination(dest.id, { account: { ...dest.account, id: user.id } })
  return user.id
}

function fail(err: unknown, scopeMissing = false): TwitchActionResult {
  if (err instanceof NeedsLogin) return { ok: false, error: err.message, needsRelogin: true }
  if (scopeMissing) {
    return {
      ok: false,
      error: 'O login atual da Twitch não deu essa permissão. Clique em “Entrar de novo com a Twitch”.',
      needsRelogin: true
    }
  }
  return { ok: false, error: (err as Error).message }
}

export function twitchCapabilities(): TwitchCapabilities {
  const dest = twitchDestination()
  const scopes = dest?.account.scopes ?? []
  return {
    linked: dest !== null,
    login: dest?.account.login ?? null,
    canEditInfo: scopes.includes(SCOPE_INFO),
    canChat: scopes.includes(SCOPE_CHAT)
  }
}

export async function getChannelInfo(): Promise<LiveInfo | null> {
  try {
    const id = await broadcasterId()
    const res = await helix(`channels?broadcaster_id=${encodeURIComponent(id)}`)
    if (!res.ok) return null
    const ch = ((await res.json()) as { data: Array<{ title: string; game_id: string; game_name: string }> }).data[0]
    return ch ? { title: ch.title, categoryId: ch.game_id, categoryName: ch.game_name } : null
  } catch {
    return null
  }
}

export async function updateChannelInfo(info: LiveInfo): Promise<TwitchActionResult> {
  try {
    const id = await broadcasterId()
    const body: Record<string, string> = { title: info.title.slice(0, 140) }
    if (info.categoryId) body.game_id = info.categoryId
    const res = await helix(`channels?broadcaster_id=${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: JSON.stringify(body)
    })
    if (res.status === 204 || res.ok) return { ok: true }
    if (res.status === 403) return fail(null, true)
    const err = (await res.json().catch(() => ({}))) as { message?: string }
    return { ok: false, error: `A Twitch recusou: ${err.message ?? res.status}` }
  } catch (err) {
    return fail(err)
  }
}

export async function searchCategories(query: string): Promise<TwitchCategory[]> {
  const q = query.trim()
  if (!q) return []
  try {
    const res = await helix(`search/categories?first=8&query=${encodeURIComponent(q)}`)
    if (!res.ok) return []
    const data = ((await res.json()) as { data: Array<{ id: string; name: string; box_art_url: string }> }).data
    return data.map((c) => ({
      id: c.id,
      name: c.name,
      boxArtUrl: c.box_art_url.replace('{width}', '52').replace('{height}', '72')
    }))
  } catch {
    return []
  }
}

export async function sendChat(text: string): Promise<TwitchActionResult> {
  const message = text.trim().slice(0, 500)
  if (!message) return { ok: true }
  try {
    const id = await broadcasterId()
    const res = await helix('chat/messages', {
      method: 'POST',
      body: JSON.stringify({ broadcaster_id: id, sender_id: id, message })
    })
    if (res.status === 403 || res.status === 401) return fail(null, true)
    const json = (await res.json().catch(() => ({}))) as {
      data?: Array<{ is_sent: boolean; drop_reason?: { message: string } | null }>
      message?: string
    }
    const sent = json.data?.[0]
    if (res.ok && sent?.is_sent) return { ok: true }
    return { ok: false, error: sent?.drop_reason?.message ?? json.message ?? `A Twitch respondeu ${res.status}.` }
  } catch (err) {
    return fail(err)
  }
}
