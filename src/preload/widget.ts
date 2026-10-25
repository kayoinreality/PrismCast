import { contextBridge, ipcRenderer } from 'electron'
import type { WidgetEvent, WidgetLiveState } from '@shared/protocol'

// Iguais a CH.widgetState / CH.widgetEvent. Sem importar o @shared/ipc: com dois
// preloads o rollup separaria um pedaço em comum, e preload em sandbox não faz require.
const WIDGET_STATE = 'widget:state'
const WIDGET_EVENT = 'widget:event'

/**
 * Preload das páginas de widget (fontes de navegador). A página não ganha nada
 * do Node nem do app: só lê o jogo da live e recebe o multichat.
 *
 *   window.prismcast.live          → { title, game } no momento em que a página abriu
 *   window.prismcast.getLive()     → o estado atual
 *   window.prismcast.on(cb)        → { t: 'live' | 'messages' | 'delete' | 'status', ... }
 */

// Síncrono para a página já nascer com o jogo certo (sem piscar o texto padrão do kit).
let live: WidgetLiveState = (ipcRenderer.sendSync(WIDGET_STATE) as WidgetLiveState | null) ?? { title: '', game: '' }
const listeners = new Set<(e: WidgetEvent) => void>()

ipcRenderer.on(WIDGET_EVENT, (_e, event: WidgetEvent) => {
  if (event.t === 'live') live = event.live
  for (const cb of listeners) {
    try {
      cb(event)
    } catch (err) {
      console.error(err)
    }
  }
})

contextBridge.exposeInMainWorld('prismcast', {
  version: 1,
  live,
  getLive: (): WidgetLiveState => live,
  on: (cb: (e: WidgetEvent) => void): void => {
    if (typeof cb === 'function') listeners.add(cb)
  }
})
