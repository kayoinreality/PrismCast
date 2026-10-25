import type {
  KitWidget,
  Scene,
  SceneItem,
  SourceSettings,
  SourceTransform,
  SourceType,
  StudioScenesState
} from '@shared/protocol'

/**
 * Operações de cena como funções puras sobre StudioScenesState. A tela chama,
 * recebe o estado novo e grava nas preferências; o motor redesenha.
 *
 * Ordem das fontes: índice 0 é o fundo. O zIndex acompanha o índice.
 */

export function newId(prefix: string): string {
  return `${prefix}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`
}

export const SOURCE_LABEL: Record<SourceType, string> = {
  screen_capture: 'Tela',
  window_capture: 'Janela',
  camera_input: 'Câmera',
  browser_source: 'Widget / Navegador',
  image_source: 'Imagem',
  color_source: 'Cor',
  text_source: 'Texto'
}

/** A cena que o editor mexe: no modo estúdio é a prévia; fora dele, a que está no ar. */
export function editingScene(st: StudioScenesState): Scene | null {
  const id = st.studioMode ? st.previewSceneId : st.activeSceneId
  return st.scenes.find((s) => s.id === id) ?? st.scenes[0] ?? null
}

function withScene(st: StudioScenesState, sceneId: string, fn: (s: Scene) => Scene): StudioScenesState {
  return { ...st, scenes: st.scenes.map((s) => (s.id === sceneId ? normalizeZ(fn(s)) : s)) }
}

function normalizeZ(scene: Scene): Scene {
  return {
    ...scene,
    items: scene.items.map((it, i) => ({
      ...it,
      transform: { ...it.transform, zIndex: i },
      verticalTransform: { ...it.verticalTransform, zIndex: i }
    }))
  }
}

export function createScene(st: StudioScenesState, name: string): StudioScenesState {
  const scene: Scene = { id: newId('scene'), name: name.trim() || `Cena ${st.scenes.length + 1}`, items: [], createdAt: Date.now() }
  return {
    ...st,
    scenes: [...st.scenes, scene],
    ...(st.studioMode ? { previewSceneId: scene.id } : { activeSceneId: scene.id, previewSceneId: scene.id })
  }
}

export function renameScene(st: StudioScenesState, id: string, name: string): StudioScenesState {
  return { ...st, scenes: st.scenes.map((s) => (s.id === id ? { ...s, name: name.trim() || s.name } : s)) }
}

export function duplicateScene(st: StudioScenesState, id: string): StudioScenesState {
  const src = st.scenes.find((s) => s.id === id)
  if (!src) return st
  const copy: Scene = {
    ...src,
    id: newId('scene'),
    name: `${src.name} (cópia)`,
    createdAt: Date.now(),
    items: src.items.map((it) => ({ ...it, id: newId('item') }))
  }
  const idx = st.scenes.findIndex((s) => s.id === id)
  const scenes = [...st.scenes]
  scenes.splice(idx + 1, 0, copy)
  return { ...st, scenes }
}

export function deleteScene(st: StudioScenesState, id: string): StudioScenesState {
  if (st.scenes.length <= 1) return st
  const scenes = st.scenes.filter((s) => s.id !== id)
  const fallback = scenes[0].id
  return {
    ...st,
    scenes,
    activeSceneId: st.activeSceneId === id ? fallback : st.activeSceneId,
    previewSceneId: st.previewSceneId === id ? fallback : st.previewSceneId
  }
}

export function moveScene(st: StudioScenesState, id: string, delta: number): StudioScenesState {
  const idx = st.scenes.findIndex((s) => s.id === id)
  const to = idx + delta
  if (idx < 0 || to < 0 || to >= st.scenes.length) return st
  const scenes = [...st.scenes]
  const [moved] = scenes.splice(idx, 1)
  scenes.splice(to, 0, moved)
  return { ...st, scenes }
}

/** Clique numa cena: no modo estúdio vai para a prévia; fora dele, direto para o ar. */
export function selectScene(st: StudioScenesState, id: string): StudioScenesState {
  return st.studioMode ? { ...st, previewSceneId: id } : { ...st, activeSceneId: id, previewSceneId: id }
}

/** Botão "Transição": a prévia vai para o ar e a cena que estava no ar vira a prévia. */
export function transition(st: StudioScenesState): StudioScenesState {
  return { ...st, activeSceneId: st.previewSceneId, previewSceneId: st.activeSceneId }
}

export function setStudioMode(st: StudioScenesState, on: boolean): StudioScenesState {
  return { ...st, studioMode: on, previewSceneId: st.activeSceneId }
}

// ----------------------------------------------------------------- fontes

const FULL: SourceTransform = { x: 0, y: 0, width: 1, height: 1, zIndex: 0 }

/** Caixa 16:9 centralizada no canvas vertical com a largura toda. */
const VERTICAL_BAND: SourceTransform = { x: 0, y: 0.34, width: 1, height: 0.316, zIndex: 0 }

export function makeItem(
  type: SourceType,
  name: string,
  settings: SourceSettings,
  transform: Partial<SourceTransform> = {},
  verticalTransform?: Partial<SourceTransform>
): SceneItem {
  const tf = { ...FULL, ...transform }
  return {
    id: newId('item'),
    sourceId: newId('src'),
    name,
    type,
    visible: true,
    locked: false,
    transform: tf,
    verticalTransform: { ...(verticalTransform ? { ...FULL, ...verticalTransform } : defaultVertical(type, tf)) },
    showOnHorizontal: true,
    showOnVertical: true,
    settings
  }
}

function defaultVertical(type: SourceType, tf: SourceTransform): SourceTransform {
  if (type === 'color_source') return { ...FULL }
  // Coisa que cobre a tela toda no 16:9 vira uma faixa 16:9 no meio do 9:16.
  if (tf.width >= 0.99 && tf.height >= 0.99) return { ...VERTICAL_BAND }
  // Mesma proporção em pixels, com 90% da largura do canvas vertical.
  const aspect = (tf.height * 1080) / (tf.width * 1920)
  const height = Math.min(0.9, (0.9 * 1080 * aspect) / 1920)
  return { x: 0.05, y: 0.05, width: 0.9, height, zIndex: 0 }
}

export function addItem(st: StudioScenesState, sceneId: string, item: SceneItem): StudioScenesState {
  return withScene(st, sceneId, (s) => ({ ...s, items: [...s.items, item] }))
}

export function updateItem(
  st: StudioScenesState,
  sceneId: string,
  itemId: string,
  patch: Partial<SceneItem>
): StudioScenesState {
  return withScene(st, sceneId, (s) => ({
    ...s,
    items: s.items.map((it) => (it.id === itemId ? { ...it, ...patch, settings: { ...it.settings, ...(patch.settings ?? {}) } } : it))
  }))
}

export function removeItem(st: StudioScenesState, sceneId: string, itemId: string): StudioScenesState {
  return withScene(st, sceneId, (s) => ({ ...s, items: s.items.filter((it) => it.id !== itemId) }))
}

/** delta +1 = para frente (mais perto do topo da lista) */
export function moveItem(st: StudioScenesState, sceneId: string, itemId: string, delta: number): StudioScenesState {
  return withScene(st, sceneId, (s) => {
    const idx = s.items.findIndex((it) => it.id === itemId)
    const to = idx + delta
    if (idx < 0 || to < 0 || to >= s.items.length) return s
    const items = [...s.items]
    const [moved] = items.splice(idx, 1)
    items.splice(to, 0, moved)
    return { ...s, items }
  })
}

// ----------------------------------------------------------- kit de widgets

const px = (x: number, y: number, w: number, h: number): Partial<SourceTransform> => ({
  x: x / 1920,
  y: y / 1080,
  width: w / 1920,
  height: h / 1080
})

/** Monta a URL do widget com os parâmetros preenchidos. */
export function widgetUrl(base: string, params: Record<string, string>): string {
  const root = base.split('?')[0]
  const q = Object.entries(params).filter(([, v]) => v.trim() !== '')
  if (q.length === 0) return root
  return `${root}?${q.map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`).join('&')}`
}

export function widgetParams(url: string): Record<string, string> {
  const q = url.split('?')[1]
  if (!q) return {}
  return Object.fromEntries(new URLSearchParams(q))
}

export function kitItem(w: KitWidget, transform?: Partial<SourceTransform>): SceneItem {
  const tf = transform ?? (w.width >= 1920 ? {} : px((1920 - w.width) / 2, (1080 - w.height) / 2, w.width, w.height))
  const item = makeItem(
    'browser_source',
    w.name,
    { url: w.url, width: w.width, height: w.height, fps: 30, kitWidget: w.file },
    tf
  )
  // Tela cheia não se arrasta sem querer; dá para destravar no cadeado.
  return { ...item, locked: w.role === 'scene' || w.role === 'overlay' }
}

/**
 * Cria as cenas do kit já montadas como o LEIA-ME manda: telas cheias para
 * começando/volto já/encerrando, câmera encaixada na moldura do "só conversando"
 * e do overlay de gameplay.
 */
export function buildKitScenes(widgets: KitWidget[]): Scene[] {
  const find = (file: string): KitWidget | undefined => widgets.find((w) => w.file === file)
  const scenes: Scene[] = []
  const scene = (name: string, items: SceneItem[]): void => {
    scenes.push(normalizeZ({ id: newId('scene'), name, createdAt: Date.now(), items }))
  }
  const camera = (tf: Partial<SourceTransform>): SceneItem =>
    makeItem('camera_input', 'Câmera', { deviceId: 'default' }, tf, { x: 0, y: 0, width: 1, height: 0.34, zIndex: 0 })

  const comecando = find('comecando.html')
  if (comecando) scene('Começando', [kitItem(comecando)])

  const gameplay = find('gameplay.html')
  if (gameplay) {
    // Moldura padrão do overlay: canto inferior esquerdo, 480x270, margem 32.
    scene('Gameplay', [
      makeItem('screen_capture', 'Jogo / tela', {}),
      camera(px(32, 1080 - 32 - 270, 480, 270)),
      kitItem(gameplay)
    ])
  }

  const conversando = find('conversando.html')
  if (conversando) {
    scene('Só conversando', [kitItem(conversando), camera(px(80, 140, 1240, 698))])
  }

  const volto = find('volto-ja.html')
  if (volto) scene('Volto já', [kitItem(volto)])

  const encerrando = find('encerrando.html')
  if (encerrando) scene('Encerrando', [kitItem(encerrando)])

  return scenes
}
