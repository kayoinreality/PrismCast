import { shell } from 'electron'
import { CH, type OkResult } from '@shared/ipc'
import type { PlatformAccount, TwitchLoginState } from '@shared/protocol'
import { getSettings, setSecret, streamKeySecret, updateDestination } from './store'
import { getMainWindow } from './window'

/**
 * Contas e chaves de transmissão.
 *
 * Duas formas de conectar um destino:
 *  1. Colar a chave (todas as plataformas). Ela é validada no formato e salva
 *     criptografada. Nada é inventado: sem chave, o destino fica desligado.
 *  2. "Entrar com a Twitch": login OAuth pelo fluxo device code. O usuário confirma
 *     um código em twitch.tv/activate e o app busca nome, avatar e a chave pela API.
 *     Precisa do Client ID de um app registrado em dev.twitch.tv (tipo "Público").
 */

const TWITCH_SCOPES = 'channel:read:stream_key'

function emitTwitch(state: TwitchLoginState): void {
  const win = getMainWindow()
  if (win && !win.isDestroyed()) win.webContents.send(CH.destTwitchState, state)
}

function emitSettings(): void {
  const win = getMainWindow()
  if (win && !win.isDestroyed()) win.webContents.send(CH.settingsChanged, getSettings())
}

const KEY_RULES: Record<string, { test: RegExp; message: string }> = {
  // Formato real: live_<id numérico do canal>_<código>. Barra o formato inventado pela 2.1.
  twitch: {
    test: /^live_\d{3,}_[A-Za-z0-9]{16,}$/,
    message: 'A chave da Twitch tem o formato live_123456789_xxxxxxxx… (copie inteira do painel).'
  },
  youtube: {
    test: /^[a-z0-9]{4}(-[a-z0-9]{4}){3,4}$/i,
    message: 'A chave do YouTube tem o formato xxxx-xxxx-xxxx-xxxx-xxxx.'
  },
  kick: { test: /^sk_[A-Za-z0-9_.-]{10,}$/, message: 'A chave do Kick começa com "sk_".' }
}

export function setStreamKey(destinationId: string, rawKey: string): OkResult {
  const dest = getSettings().destinations.find((d) => d.id === destinationId)
  if (!dest) return { ok: false, error: 'Destino não encontrado.' }
  const key = rawKey.trim()

  if (!key) {
    setSecret(streamKeySecret(dest.id), null)
    updateDestination(dest.id, { hasKey: false, enabled: false })
    emitSettings()
    return { ok: true }
  }
  if (/\s/.test(key)) return { ok: false, error: 'A chave não pode ter espaços.' }
  const rule = KEY_RULES[dest.platform]
  if (rule && !rule.test.test(key)) return { ok: false, error: rule.message }

  setSecret(streamKeySecret(dest.id), key)
  // Colar a chave liga o destino: é o que a pessoa quer em 100% dos casos.
  updateDestination(dest.id, { hasKey: true, enabled: true })
  emitSettings()
  return { ok: true }
}

export function unlinkDestination(destinationId: string): void {
  setSecret(streamKeySecret(destinationId), null)
  setSecret(`twitchRefresh:${destinationId}`, null)
  updateDestination(destinationId, { hasKey: false, enabled: false, account: null })
  emitSettings()
}

// ------------------------------------------------------------ Twitch OAuth

let loginAbort: AbortController | null = null

export function cancelTwitchLogin(): void {
  loginAbort?.abort()
  loginAbort = null
  emitTwitch({ step: 'idle' })
}

async function postForm(url: string, body: Record<string, string>, signal: AbortSignal): Promise<Response> {
  return fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams(body).toString(),
    signal
  })
}

const sleep = (ms: number, signal: AbortSignal): Promise<void> =>
  new Promise((resolve, reject) => {
    const t = setTimeout(resolve, ms)
    signal.addEventListener('abort', () => {
      clearTimeout(t)
      reject(new Error('cancelado'))
    })
  })

export async function startTwitchLogin(destinationId: string): Promise<void> {
  loginAbort?.abort()
  const abort = new AbortController()
  loginAbort = abort
  const { signal } = abort

  const clientId = getSettings().twitchClientId.trim()
  if (!clientId) {
    emitTwitch({ step: 'error', message: 'Cole o Client ID do seu app da Twitch antes de entrar.' })
    return
  }

  try {
    const deviceRes = await postForm(
      'https://id.twitch.tv/oauth2/device',
      { client_id: clientId, scopes: TWITCH_SCOPES },
      signal
    )
    if (!deviceRes.ok) {
      throw new Error(
        deviceRes.status === 400
          ? 'A Twitch recusou o Client ID. Confira se o app é do tipo "Público".'
          : `A Twitch respondeu ${deviceRes.status} ao pedir o código.`
      )
    }
    const device = (await deviceRes.json()) as {
      device_code: string
      user_code: string
      verification_uri: string
      expires_in: number
      interval: number
    }
    const expiresAt = Date.now() + device.expires_in * 1000
    emitTwitch({
      step: 'code',
      userCode: device.user_code,
      verificationUri: device.verification_uri,
      expiresAt
    })
    void shell.openExternal(device.verification_uri)

    let interval = Math.max(1, device.interval) * 1000
    let token: { access_token: string; refresh_token?: string } | null = null
    while (!token) {
      if (Date.now() > expiresAt) throw new Error('O código expirou. Tente de novo.')
      await sleep(interval, signal)
      const res = await postForm(
        'https://id.twitch.tv/oauth2/token',
        {
          client_id: clientId,
          scopes: TWITCH_SCOPES,
          device_code: device.device_code,
          grant_type: 'urn:ietf:params:oauth:grant-type:device_code'
        },
        signal
      )
      if (res.ok) {
        token = (await res.json()) as { access_token: string; refresh_token?: string }
        break
      }
      const err = (await res.json().catch(() => ({}))) as { message?: string }
      if (err.message === 'authorization_pending') continue
      if (err.message === 'slow_down') {
        interval += 5000
        continue
      }
      if (err.message === 'access_denied') throw new Error('Você recusou o acesso na Twitch.')
      throw new Error(`A Twitch não liberou o login (${err.message ?? res.status}).`)
    }

    const headers = { Authorization: `Bearer ${token.access_token}`, 'Client-Id': clientId }
    const userRes = await fetch('https://api.twitch.tv/helix/users', { headers, signal })
    if (!userRes.ok) throw new Error(`Não deu para ler sua conta (${userRes.status}).`)
    const user = ((await userRes.json()) as {
      data: Array<{ id: string; login: string; display_name: string; profile_image_url: string }>
    }).data[0]
    if (!user) throw new Error('A Twitch não devolveu os dados da conta.')

    const keyRes = await fetch(
      `https://api.twitch.tv/helix/streams/key?broadcaster_id=${encodeURIComponent(user.id)}`,
      { headers, signal }
    )
    if (!keyRes.ok) throw new Error(`Não deu para buscar a chave de transmissão (${keyRes.status}).`)
    const streamKey = ((await keyRes.json()) as { data: Array<{ stream_key: string }> }).data[0]
      ?.stream_key
    if (!streamKey) throw new Error('A Twitch não devolveu a chave de transmissão.')

    const account: PlatformAccount = {
      login: user.login,
      displayName: user.display_name,
      avatarUrl: user.profile_image_url
    }
    setSecret(streamKeySecret(destinationId), streamKey)
    if (token.refresh_token) setSecret(`twitchRefresh:${destinationId}`, token.refresh_token)
    updateDestination(destinationId, { hasKey: true, enabled: true, account, name: 'Twitch' })
    emitSettings()
    emitTwitch({ step: 'done', account })
  } catch (err) {
    if (signal.aborted) return
    emitTwitch({ step: 'error', message: (err as Error).message })
  } finally {
    if (loginAbort === abort) loginAbort = null
  }
}
