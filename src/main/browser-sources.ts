import { BrowserWindow } from 'electron'
import { CH } from '@shared/ipc'
import type { BrowserSourceFrame, BrowserSourceSpec } from '@shared/protocol'
import { getMainWindow } from './window'

/**
 * Browser sources (o "Navegador" do OBS).
 *
 * Cada widget roda numa janela Chromium offscreen e transparente. O evento
 * `paint` entrega só o retângulo que mudou; esse pedaço vai para o renderer,
 * que mantém um canvas por fonte e desenha esse canvas no compositor. Assim o
 * widget entra no vídeo da live e na gravação, não só na pré-visualização.
 *
 * Fontes fora das cenas em uso continuam vivas por um tempo para trocar de
 * cena e voltar não reiniciar o chat nem a contagem regressiva.
 */

interface Rect {
  x: number
  y: number
  width: number
  height: number
}

interface Entry {
  spec: BrowserSourceSpec
  win: BrowserWindow
  unusedSince: number | null
  /** último quadro pintado e a área suja acumulada desde o último envio */
  latest: Electron.NativeImage | null
  dirty: Rect | null
  /** um quadro a caminho do renderer: o próximo só sai depois do "ack" */
  inFlight: boolean
  sentAt: number
}

function union(a: Rect | null, b: Rect): Rect {
  if (!a) return { ...b }
  const x = Math.min(a.x, b.x)
  const y = Math.min(a.y, b.y)
  return {
    x,
    y,
    width: Math.max(a.x + a.width, b.x + b.width) - x,
    height: Math.max(a.y + a.height, b.y + b.height) - y
  }
}

const KEEP_ALIVE_MS = 60_000
const entries = new Map<string, Entry>()
let sweepTimer: NodeJS.Timeout | null = null

function allowedUrl(url: string): boolean {
  return /^(https?|file):\/\//i.test(url)
}

function sendFrame(frame: BrowserSourceFrame): void {
  const win = getMainWindow()
  if (win && !win.isDestroyed()) win.webContents.send(CH.bsFrame, frame)
}

function create(spec: BrowserSourceSpec): Entry {
  const win = new BrowserWindow({
    show: false,
    width: spec.width,
    height: spec.height,
    frame: false,
    transparent: true,
    backgroundColor: '#00000000',
    webPreferences: {
      offscreen: { deviceScaleFactor: 1 },
      // Sessão própria: a CSP e o handler de captura de tela do app não valem
      // para os widgets (que têm scripts inline e de outros domínios).
      partition: 'persist:prismcast-widgets',
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      backgroundThrottling: false,
      autoplayPolicy: 'no-user-gesture-required'
    }
  })
  win.webContents.setFrameRate(Math.max(1, Math.min(60, spec.fps)))
  win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }))
  win.webContents.on('will-navigate', (event, url) => {
    if (url !== win.webContents.getURL()) event.preventDefault()
  })

  const entry: Entry = { spec, win, unusedSince: null, latest: null, dirty: null, inFlight: false, sentAt: 0 }

  // Widgets animados (céu estrelado em tela cheia) pintam 8 MB por quadro. Sem
  // controle de fluxo, os quadros se acumulam na fila do IPC mais rápido do que
  // a interface desenha e a memória explode. Então: guarda o último quadro e a
  // área suja somada, e só manda de novo quando o renderer confirma o anterior.
  win.webContents.on('paint', (_event, dirty, image) => {
    entry.latest = image
    entry.dirty = union(entry.dirty, dirty)
    // Ack perdido (a interface recarregou no meio): não trava a fonte para sempre.
    if (entry.inFlight && Date.now() - entry.sentAt > 1000) entry.inFlight = false
    if (!entry.inFlight) flush(entry)
  })

  // Uma página nova começa transparente: força um quadro inteiro quando terminar de carregar.
  win.webContents.on('did-finish-load', () => win.webContents.invalidate())

  if (allowedUrl(spec.url)) void win.loadURL(spec.url).catch(() => {})
  return entry
}

function flush(entry: Entry): void {
  const image = entry.latest
  const dirty = entry.dirty
  if (!image || !dirty) return
  entry.dirty = null
  const size = image.getSize()
  const x = Math.max(0, Math.floor(dirty.x))
  const y = Math.max(0, Math.floor(dirty.y))
  const width = Math.min(size.width - x, Math.ceil(dirty.width))
  const height = Math.min(size.height - y, Math.ceil(dirty.height))
  if (width <= 0 || height <= 0) return
  const region =
    x === 0 && y === 0 && width === size.width && height === size.height ? image : image.crop({ x, y, width, height })
  entry.inFlight = true
  entry.sentAt = Date.now()
  sendFrame({
    id: entry.spec.id,
    fullWidth: size.width,
    fullHeight: size.height,
    x,
    y,
    width,
    height,
    data: region.toBitmap()
  })
}

/** O renderer desenhou o quadro: libera o próximo. */
export function ackBrowserSource(id: string): void {
  const entry = entries.get(id)
  if (!entry) return
  entry.inFlight = false
  if (entry.dirty) flush(entry)
}

function destroy(entry: Entry): void {
  if (!entry.win.isDestroyed()) entry.win.destroy()
}

function sweep(): void {
  const now = Date.now()
  for (const [id, entry] of entries) {
    if (entry.unusedSince !== null && now - entry.unusedSince > KEEP_ALIVE_MS) {
      destroy(entry)
      entries.delete(id)
    }
  }
}

/** Recebe a lista de fontes que as cenas em uso precisam e ajusta as janelas. */
export function syncBrowserSources(specs: BrowserSourceSpec[]): void {
  const wanted = new Map(specs.filter((s) => allowedUrl(s.url)).map((s) => [s.id, s]))

  for (const [id, entry] of entries) {
    const spec = wanted.get(id)
    if (!spec) {
      // Fora das cenas em uso: a página continua viva (chat, cronômetro), mas
      // para de pintar e não gasta nada mandando quadros.
      if (entry.unusedSince === null) {
        entry.unusedSince = Date.now()
        entry.win.webContents.stopPainting()
      }
      continue
    }
    if (entry.unusedSince !== null) {
      entry.win.webContents.startPainting()
      entry.win.webContents.invalidate()
    }
    entry.unusedSince = null
    const prev = entry.spec
    if (prev.width !== spec.width || prev.height !== spec.height) {
      entry.win.setContentSize(spec.width, spec.height)
      entry.win.webContents.invalidate()
    }
    if (prev.fps !== spec.fps) entry.win.webContents.setFrameRate(spec.fps)
    if (prev.url !== spec.url) void entry.win.loadURL(spec.url).catch(() => {})
    entry.spec = spec
  }

  for (const [id, spec] of wanted) {
    if (!entries.has(id)) entries.set(id, create(spec))
  }

  sweepTimer ??= setInterval(sweep, 10_000)
}

export function reloadBrowserSource(id: string): void {
  const entry = entries.get(id)
  if (entry && !entry.win.isDestroyed()) entry.win.webContents.reloadIgnoringCache()
}

/** O renderer recarregou (HMR ou F5): reenvia o quadro inteiro de todas. */
export function invalidateAllBrowserSources(): void {
  for (const entry of entries.values()) {
    entry.inFlight = false
    if (!entry.win.isDestroyed()) entry.win.webContents.invalidate()
  }
}

export function disposeBrowserSources(): void {
  if (sweepTimer) clearInterval(sweepTimer)
  sweepTimer = null
  for (const entry of entries.values()) destroy(entry)
  entries.clear()
}
