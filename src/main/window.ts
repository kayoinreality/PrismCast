import { join } from 'node:path'
import { BrowserWindow, shell } from 'electron'
import { CH } from '@shared/ipc'

let mainWindow: BrowserWindow | null = null

export function getMainWindow(): BrowserWindow | null {
  return mainWindow
}

export function createWindow(): BrowserWindow {
  const win = new BrowserWindow({
    title: 'PrismCast Studio',
    width: 1380,
    height: 860,
    minWidth: 1060,
    minHeight: 680,
    show: false,
    frame: false,
    // Evita o flash branco entre a janela abrir e o React pintar.
    backgroundColor: '#0d0f14',
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      // Autoplay do video do espectador sem exigir clique.
      autoplayPolicy: 'no-user-gesture-required'
    }
  })
  mainWindow = win

  win.once('ready-to-show', () => win.show())

  const emitMaximize = (): void => {
    if (!win.isDestroyed()) win.webContents.send(CH.appMaximizeChanged, win.isMaximized())
  }
  win.on('maximize', emitMaximize)
  win.on('unmaximize', emitMaximize)
  win.on('closed', () => {
    mainWindow = null
  })

  // O app nunca navega para fora de si mesmo: qualquer link externo vai para o
  // navegador do sistema, e navegacao na propria janela e bloqueada.
  win.webContents.setWindowOpenHandler(({ url }) => {
    void shell.openExternal(url)
    return { action: 'deny' }
  })
  win.webContents.on('will-navigate', (event, url) => {
    const devUrl = process.env['ELECTRON_RENDERER_URL']
    if (devUrl && url.startsWith(devUrl)) return
    event.preventDefault()
  })

  const devUrl = process.env['ELECTRON_RENDERER_URL']
  if (devUrl) {
    void win.loadURL(devUrl)
  } else {
    void win.loadFile(join(__dirname, '../renderer/index.html'))
  }

  return win
}
