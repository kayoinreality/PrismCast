import { join } from 'node:path'
import { BrowserWindow, screen, shell } from 'electron'
import { CH } from '@shared/ipc'
import { getSettings, setSettings } from './store'
import { getMainWindow } from './window'

/**
 * Chat da Twitch numa janela própria, para deixar no segundo monitor ou por
 * cima do jogo. É a mesma interface (renderer) aberta em modo "#chat".
 * Posição e tamanho ficam salvos.
 */

let chatWindow: BrowserWindow | null = null

function notify(open: boolean): void {
  const main = getMainWindow()
  if (main && !main.isDestroyed()) main.webContents.send(CH.chatPopoutChanged, open)
}

/** A janela salva pode estar num monitor que foi desligado. */
function visibleBounds(b: { x: number; y: number; width: number; height: number } | null) {
  if (!b) return null
  const onScreen = screen.getAllDisplays().some((d) => {
    const a = d.workArea
    return b.x < a.x + a.width - 80 && b.x + b.width > a.x + 80 && b.y >= a.y - 10 && b.y < a.y + a.height - 80
  })
  return onScreen ? b : null
}

export function isChatPopoutOpen(): boolean {
  return chatWindow !== null && !chatWindow.isDestroyed()
}

export function openChatPopout(): void {
  if (isChatPopoutOpen()) {
    chatWindow!.show()
    chatWindow!.focus()
    return
  }
  const settings = getSettings()
  const bounds = visibleBounds(settings.chat.popoutBounds)
  const win = new BrowserWindow({
    title: 'Chat · PrismCast',
    width: bounds?.width ?? 380,
    height: bounds?.height ?? 680,
    ...(bounds ? { x: bounds.x, y: bounds.y } : {}),
    minWidth: 260,
    minHeight: 320,
    show: false,
    frame: false,
    backgroundColor: '#0d0f14',
    alwaysOnTop: settings.chat.popoutOnTop,
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false
    }
  })
  chatWindow = win
  win.once('ready-to-show', () => win.show())

  let saveTimer: NodeJS.Timeout | null = null
  const saveBounds = (): void => {
    if (saveTimer) clearTimeout(saveTimer)
    saveTimer = setTimeout(() => {
      if (win.isDestroyed()) return
      setSettings({ chat: { ...getSettings().chat, popoutBounds: win.getBounds() } })
    }, 400)
  }
  win.on('move', saveBounds)
  win.on('resize', saveBounds)
  win.on('closed', () => {
    if (chatWindow === win) chatWindow = null
    notify(false)
  })

  win.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https:\/\//i.test(url)) void shell.openExternal(url)
    return { action: 'deny' }
  })
  win.webContents.on('will-navigate', (event) => event.preventDefault())

  const devUrl = process.env['ELECTRON_RENDERER_URL']
  if (devUrl) void win.loadURL(`${devUrl}#chat`)
  else void win.loadFile(join(__dirname, '../renderer/index.html'), { hash: 'chat' })
  notify(true)
}

export function closeChatPopout(): void {
  if (isChatPopoutOpen()) chatWindow!.close()
}

export function setChatPopoutOnTop(onTop: boolean): void {
  if (isChatPopoutOpen()) chatWindow!.setAlwaysOnTop(onTop)
}

/** Botões da barra de título da janela do chat vêm pelos mesmos canais da principal. */
export function chatWindowFor(sender: Electron.WebContents): BrowserWindow | null {
  return isChatPopoutOpen() && chatWindow!.webContents === sender ? chatWindow : null
}
