import { app, BrowserWindow, session } from 'electron'
import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { spawn, type ChildProcess } from 'node:child_process'
import { installDisplayMediaHandler, setOwnWindowTitle } from './capture'
import { registerGlobalHotkeys, registerIpc, shutdown } from './ipc-handlers'
import { createWindow, getMainWindow } from './window'

/**
 * Sem isto o Chromium troca os IPs locais por nomes .local (privacidade do
 * WebRTC na web). Na LAN isso faz o ICE demorar ou nao conectar de jeito
 * nenhum — e aqui os dois lados sao o mesmo app, na mesma rede.
 */
app.commandLine.appendSwitch('disable-features', 'WebRtcHideLocalIpsWithMdns')

// Otimizações de decodificação e aceleração por GPU (baseado no Streamlabs OBS)
app.commandLine.appendSwitch('enable-gpu-rasterization')
app.commandLine.appendSwitch('enable-zero-copy')
app.commandLine.appendSwitch('ignore-gpu-blocklist')
app.commandLine.appendSwitch('enable-hardware-overlays', 'single-fullscreen,underlay')

// Duas instancias na mesma maquina sao uteis para testar host + espectador
// lado a lado, entao nao ha lock de instancia unica.

let rustEngineProc: ChildProcess | null = null

function startRustSidecarIfAvailable(): void {
  const candidates = [
    join(app.getAppPath(), 'target', 'release', 'prismcast.exe'),
    join(app.getAppPath(), 'target', 'debug', 'prismcast.exe')
  ]
  const binPath = candidates.find((p) => existsSync(p))
  if (!binPath) return
  try {
    rustEngineProc = spawn(binPath, [], {
      stdio: ['pipe', 'ignore', 'ignore'],
      windowsHide: true
    })
    rustEngineProc.on('error', () => {
      rustEngineProc = null
    })
  } catch {
    rustEngineProc = null
  }
}

/**
 * CSP por header (mais forte que meta tag) e so no empacotado: em dev o Vite
 * serve modulos e o socket de HMR do proprio localhost, que `script-src 'self'`
 * e `connect-src 'self'` derrubariam.
 *
 * blob:/mediastream: liberam o MediaStream do WebRTC; data: libera as
 * miniaturas e os icones que o desktopCapturer devolve como data URL.
 * frame-src https: http: libera widgets Browser Source (StreamElements, etc.).
 */
function installContentSecurityPolicy(): void {
  if (!app.isPackaged) return
  session.defaultSession.webRequest.onHeadersReceived((details, callback) => {
    callback({
      responseHeaders: {
        ...details.responseHeaders,
        'Content-Security-Policy': [
          [
            "default-src 'self'",
            "script-src 'self'",
            "style-src 'self' 'unsafe-inline'",
            "img-src 'self' data: https:",
            "media-src 'self' blob: mediastream:",
            "connect-src 'self' https: wss:",
            "font-src 'self' data:",
            "object-src 'none'",
            "frame-src 'self' https: http:"
          ].join('; ')
        ]
      }
    })
  })
}

app.whenReady().then(() => {
  installContentSecurityPolicy()
  installDisplayMediaHandler()
  registerIpc()
  registerGlobalHotkeys()
  startRustSidecarIfAvailable()

  const win = createWindow()
  setOwnWindowTitle(win.getTitle())

  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow()
  })
})

app.on('window-all-closed', () => {
  app.quit()
})

let quitting = false
app.on('before-quit', (event) => {
  if (quitting) return
  // Segura a saida so o suficiente para mandar o 'bye' de descoberta e o
  // 'ended' aos espectadores — senao eles ficam vendo um host fantasma.
  event.preventDefault()
  quitting = true
  try {
    rustEngineProc?.kill()
    rustEngineProc = null
  } catch {
    /* ignore */
  }
  void shutdown().finally(() => app.quit())
})

app.on('render-process-gone', (_e, _wc, details) => {
  console.error('[main] renderer caiu:', details.reason)
})

process.on('uncaughtException', (err) => {
  console.error('[main] excecao nao tratada:', err)
  getMainWindow()?.webContents.send('app:error', String(err?.message ?? err))
})
