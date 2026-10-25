import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { basename, join } from 'node:path'
import { pathToFileURL } from 'node:url'
import type { KitWidget, WidgetKit } from '@shared/protocol'

/**
 * Kit de widgets: uma pasta com overlays HTML no formato do OBS (pasta `obs/`
 * com páginas 1920x1080 que leem parâmetros pela URL). O kayoinreality-kit é o
 * caso principal, mas qualquer pasta com `obs/*.html` funciona.
 */

type Known = Omit<KitWidget, 'file' | 'url'>

const KNOWN: Record<string, Known> = {
  'comecando.html': {
    name: 'Começando',
    description: 'Contagem regressiva; a estrela nasce e o céu clareia até zerar.',
    width: 1920,
    height: 1080,
    role: 'scene',
    params: [
      { key: 'min', label: 'Minutos da contagem', placeholder: '5' },
      { key: 'jogo', label: 'Jogo do dia', placeholder: 'genshin impact' }
    ]
  },
  'volto-ja.html': {
    name: 'Volto já',
    description: 'Cronômetro de quanto tempo você saiu e recados girando.',
    width: 1920,
    height: 1080,
    role: 'scene',
    params: [{ key: 'jogo', label: 'Jogo/assunto', placeholder: 'genshin impact' }]
  },
  'encerrando.html': {
    name: 'Encerrando',
    description: 'A estrela se põe devagar e mostra a próxima live.',
    width: 1920,
    height: 1080,
    role: 'scene',
    params: [{ key: 'agenda', label: 'Próxima live', placeholder: 'amanhã 20h' }]
  },
  'conversando.html': {
    name: 'Só conversando',
    description: 'Moldura grande da câmera com o chat ao lado. A câmera vai por cima.',
    width: 1920,
    height: 1080,
    role: 'scene',
    params: []
  },
  'gameplay.html': {
    name: 'Overlay de gameplay',
    description: 'Transparente: moldura da câmera e barra de recados por cima do jogo.',
    width: 1920,
    height: 1080,
    role: 'overlay',
    params: [
      { key: 'cam', label: 'Canto da câmera (bl, br, tl, tr, none)', placeholder: 'bl' },
      { key: 'w', label: 'Largura da câmera', placeholder: '480' },
      { key: 'h', label: 'Altura da câmera', placeholder: '270' },
      { key: 'chip', label: 'Barra de recados (0 esconde)', placeholder: '1' }
    ]
  },
  'webcam.html': {
    name: 'Moldura da câmera',
    description: 'Moldura avulsa no tamanho da câmera, para pôr por cima dela.',
    width: 480,
    height: 270,
    role: 'frame',
    params: [{ key: 'nome', label: 'Etiqueta com nome (0 esconde)', placeholder: '1' }]
  },
  'chat.html': {
    name: 'Chat da Twitch',
    description: 'Chat no estilo do canal. Entra anônimo, só lê, sem login.',
    width: 520,
    height: 900,
    role: 'chat',
    params: [
      { key: 'canal', label: 'Canal', placeholder: 'kayoinreality' },
      { key: 'sumir', label: 'Some depois de (s, 0 = nunca)', placeholder: '0' },
      { key: 'fundo', label: 'Fundo sólido (1 liga)', placeholder: '0' }
    ]
  }
}

/** Ordem em que aparecem na biblioteca: telas cheias primeiro, depois overlays. */
const ORDER = Object.keys(KNOWN)

function titleOf(html: string, file: string): string {
  const m = /<title>([^<]+)<\/title>/i.exec(html)
  return m ? m[1].trim() : basename(file, '.html')
}

export function loadKit(path: string | null): WidgetKit | null {
  if (!path) return null
  const obsDir = join(path, 'obs')
  if (!existsSync(obsDir)) return null

  let files: string[]
  try {
    files = readdirSync(obsDir).filter((f) => f.toLowerCase().endsWith('.html'))
  } catch {
    return null
  }

  const widgets: KitWidget[] = files.map((file) => {
    const full = join(obsDir, file)
    const url = pathToFileURL(full).href
    const known = KNOWN[file]
    if (known) return { file, url, ...known }
    let html = ''
    try {
      html = readFileSync(full, 'utf8')
    } catch {
      /* lista mesmo sem conseguir ler o titulo */
    }
    return {
      file,
      url,
      name: titleOf(html, file),
      description: 'Página HTML do kit.',
      width: 1920,
      height: 1080,
      role: 'overlay',
      params: []
    }
  })

  widgets.sort((a, b) => {
    const ia = ORDER.indexOf(a.file)
    const ib = ORDER.indexOf(b.file)
    return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib) || a.name.localeCompare(b.name)
  })

  return {
    path,
    name: basename(path),
    widgets,
    hasStreamElements: existsSync(join(path, 'streamelements', 'widget.js'))
  }
}
