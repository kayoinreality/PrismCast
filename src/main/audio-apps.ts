import { spawn, execFile, type ChildProcess } from 'node:child_process'
import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { app } from 'electron'
import { CH } from '@shared/ipc'
import type { AudioApp, AudioChannelStatus } from '@shared/protocol'
import { getMainWindow } from './window'

/**
 * Áudio de aplicativo: um `prismcast-audio.exe capture` por canal, cada um
 * mandando PCM pelo stdout. O main só repassa os pedaços para a interface, que
 * toca no mixer (AudioWorklet).
 *
 * Programa fechado não é erro: o canal fica "esperando" e tenta de novo, para
 * o jogo poder abrir depois do PrismCast.
 */

const RETRY_MS = 3000
/** 10 ms de áudio estéreo f32 a 48 kHz: junta antes de mandar, para não lotar o IPC */
const CHUNK_BYTES = 480 * 2 * 4 * 2

export function helperPath(name: string): string | null {
  const base = app.getAppPath().replace(/app\.asar$/, 'app.asar.unpacked')
  const candidates = [join(base, 'target', 'release', name), join(base, 'target', 'debug', name)]
  return candidates.find((p) => existsSync(p)) ?? null
}

interface Running {
  id: string
  exe: string
  proc: ChildProcess | null
  retry: NodeJS.Timeout | null
  status: AudioChannelStatus
  pending: Buffer[]
  pendingBytes: number
}

const running = new Map<string, Running>()

function send(channel: string, ...args: unknown[]): void {
  const win = getMainWindow()
  if (win && !win.isDestroyed()) win.webContents.send(channel, ...args)
}

function emitStatus(): void {
  send(
    CH.audioStatus,
    [...running.values()].map((r) => r.status)
  )
}

function setStatus(r: Running, state: AudioChannelStatus['state'], message: string | null): void {
  if (r.status.state === state && r.status.message === message) return
  r.status = { id: r.id, state, message }
  emitStatus()
}

function flush(r: Running): void {
  if (r.pendingBytes === 0) return
  const buf = Buffer.concat(r.pending, r.pendingBytes)
  r.pending = []
  r.pendingBytes = 0
  // Float32Array precisa de alinhamento de 4 bytes; o Buffer.concat pode não dar.
  const usable = buf.length - (buf.length % 4)
  const copy = new Float32Array(usable / 4)
  new Uint8Array(copy.buffer).set(buf.subarray(0, usable))
  send(CH.audioPcm, r.id, copy)
}

function start(r: Running): void {
  const exe = helperPath('prismcast-audio.exe')
  if (!exe) {
    setStatus(r, 'error', 'Falta o prismcast-audio.exe (rode npm run rust:build).')
    return
  }
  const proc = spawn(exe, ['capture', '--exe', r.exe], { stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true })
  r.proc = proc
  let stderr = ''
  proc.stdout.on('data', (chunk: Buffer) => {
    if (r.status.state !== 'ok') setStatus(r, 'ok', null)
    r.pending.push(chunk)
    r.pendingBytes += chunk.length
    if (r.pendingBytes >= CHUNK_BYTES) flush(r)
  })
  proc.stderr.on('data', (chunk: Buffer) => {
    stderr += chunk.toString()
    if (stderr.includes('ready')) setStatus(r, 'ok', null)
  })
  proc.on('exit', (code) => {
    if (r.proc !== proc) return
    r.proc = null
    if (!running.has(r.id)) return
    if (code === 2 || code === 3) {
      setStatus(r, 'waiting', `${r.exe} não está aberto`)
    } else {
      const err = /error (.+)/.exec(stderr)?.[1]?.trim()
      setStatus(r, 'error', err ? `Não deu para capturar: ${err}` : 'A captura parou; tentando de novo…')
    }
    r.retry = setTimeout(() => {
      r.retry = null
      if (running.get(r.id) === r) start(r)
    }, RETRY_MS)
  })
  proc.on('error', (err) => setStatus(r, 'error', err.message))
}

function stop(r: Running): void {
  if (r.retry) clearTimeout(r.retry)
  r.retry = null
  const proc = r.proc
  r.proc = null
  proc?.kill()
}

/** Garante que exatamente estes canais estejam capturando. */
export function syncAudioApps(channels: Array<{ id: string; exe: string }>): void {
  const wanted = new Map(channels.filter((c) => c.exe).map((c) => [c.id, c.exe]))
  for (const [id, r] of running) {
    if (wanted.get(id) !== r.exe) {
      stop(r)
      running.delete(id)
    }
  }
  for (const [id, exe] of wanted) {
    if (running.has(id)) continue
    const r: Running = {
      id,
      exe,
      proc: null,
      retry: null,
      status: { id, state: 'waiting', message: 'Abrindo…' },
      pending: [],
      pendingBytes: 0
    }
    running.set(id, r)
    start(r)
  }
  emitStatus()
}

export function listAudioApps(): Promise<AudioApp[]> {
  const exe = helperPath('prismcast-audio.exe')
  if (!exe) return Promise.resolve([])
  return new Promise((resolve) => {
    execFile(exe, ['list'], { windowsHide: true, timeout: 5000, maxBuffer: 4 * 1024 * 1024 }, (err, stdout) => {
      if (err) return resolve([])
      try {
        const list = JSON.parse(stdout) as AudioApp[]
        // O próprio PrismCast (janelas de widget, áudio dos alertas) não entra na lista.
        const own = new Set(app.getAppMetrics().map((m) => m.pid))
        // Uma entrada por programa: o áudio é por processo, não por janela.
        const seen = new Set<string>()
        resolve(
          list.filter((a) => {
            if (own.has(a.pid)) return false
            const key = a.exe.toLowerCase()
            if (seen.has(key)) return false
            seen.add(key)
            return true
          })
        )
      } catch {
        resolve([])
      }
    })
  })
}

/** "window:123456:0" -> o programa dono da janela. */
export async function appForWindow(captureId: string): Promise<AudioApp | null> {
  const hwnd = Number(/^window:(\d+):/.exec(captureId)?.[1])
  if (!hwnd) return null
  const exe = helperPath('prismcast-audio.exe')
  if (!exe) return null
  const all = await new Promise<AudioApp[]>((resolve) => {
    execFile(exe, ['list'], { windowsHide: true, timeout: 5000, maxBuffer: 4 * 1024 * 1024 }, (err, stdout) => {
      try {
        resolve(err ? [] : (JSON.parse(stdout) as AudioApp[]))
      } catch {
        resolve([])
      }
    })
  })
  return all.find((a) => a.hwnd === hwnd) ?? null
}

export function disposeAudioApps(): void {
  for (const r of running.values()) stop(r)
  running.clear()
}
