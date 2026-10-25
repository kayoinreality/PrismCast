import type { KitWidget, SceneItem, WidgetKit } from '@shared/protocol'
import { widgetParams, widgetUrl } from './scenes'

/**
 * Regras dos widgets que dependem do kit: o que muda quando o estúdio está sem
 * câmera e quais páginas recomeçam ao entrar no ar.
 */

export function kitWidgetOf(item: SceneItem, kit: WidgetKit | null): KitWidget | undefined {
  const file = item.settings.kitWidget
  return file ? kit?.widgets.find((w) => w.file === file) : undefined
}

/** A fonte é câmera, ou é a moldura avulsa que só existe por causa dela. */
export function hiddenWithoutCamera(item: SceneItem, kit: WidgetKit | null): boolean {
  if (item.type === 'camera_input') return true
  return kitWidgetOf(item, kit)?.camera === 'hide'
}

/** Endereço que a página realmente abre: sem câmera, o overlay esconde a moldura. */
export function effectiveUrl(item: SceneItem, kit: WidgetKit | null, cameraEnabled: boolean): string {
  const url = item.settings.url ?? ''
  const cam = kitWidgetOf(item, kit)?.camera
  if (cameraEnabled || !cam || cam === 'hide') return url
  return widgetUrl(url, { ...widgetParams(url), [cam.param]: cam.off })
}

export function restartsOnShow(item: SceneItem, kit: WidgetKit | null): boolean {
  if (item.type !== 'browser_source') return false
  return item.settings.restartOnShow ?? kitWidgetOf(item, kit)?.restartOnShow ?? false
}
