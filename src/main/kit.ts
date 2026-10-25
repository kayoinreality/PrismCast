import { existsSync, readdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
import { runInNewContext } from 'node:vm'
import { basename, join } from 'node:path'
import { pathToFileURL } from 'node:url'
import type { KitConfig, KitWidget, WidgetKit } from '@shared/protocol'

/**
 * Kit de widgets: uma pasta com overlays HTML no formato do OBS (pasta `obs/`
 * com páginas 1920x1080 que leem parâmetros pela URL). O kayoinreality-kit é o
 * caso principal, mas qualquer pasta com `obs/*.html` funciona.
 */

type Known = Omit<KitWidget, 'file' | 'url'>

const SUMIR: Array<{ value: string; label: string }> = [
  { value: '', label: '1 minuto' },
  { value: '30', label: '30 segundos' },
  { value: '120', label: '2 minutos' },
  { value: '300', label: '5 minutos' },
  { value: '0', label: 'Nunca somem' }
]

const KNOWN: Record<string, Known> = {
  'comecando.html': {
    name: 'Começando',
    description: 'Contagem regressiva; a estrela nasce e o céu clareia até zerar.',
    width: 1920,
    height: 1080,
    role: 'scene',
    restartOnShow: true,
    params: [
      { key: 'min', label: 'Contagem', type: 'number', placeholder: '5', min: 1, max: 120, unit: 'min' },
      { key: 'jogo', label: 'Jogo de hoje', type: 'text', placeholder: 'o jogo dos textos do kit' }
    ]
  },
  'volto-ja.html': {
    name: 'Volto já',
    description: 'Cronômetro de quanto tempo você saiu e recados girando.',
    width: 1920,
    height: 1080,
    role: 'scene',
    restartOnShow: true,
    params: [{ key: 'jogo', label: 'Jogo/assunto', type: 'text', placeholder: 'o jogo dos textos do kit' }]
  },
  'encerrando.html': {
    name: 'Encerrando',
    description: 'A estrela se põe devagar e mostra a próxima live.',
    width: 1920,
    height: 1080,
    role: 'scene',
    restartOnShow: true,
    params: [
      { key: 'agenda', label: 'Próxima live', type: 'text', placeholder: 'a agenda dos textos do kit' },
      { key: 'msg', label: 'Recado final', type: 'text', placeholder: 'se ficou até aqui, você é estrela' }
    ]
  },
  'conversando.html': {
    name: 'Só conversando',
    description: 'Moldura grande da câmera com o chat ao lado. A câmera vai por cima.',
    width: 1920,
    height: 1080,
    role: 'scene',
    camera: { param: 'cam', off: '0' },
    params: [
      {
        key: 'cam',
        label: 'Moldura da câmera',
        hint: 'Desligada, o espaço vira um cartaz com o seu nome',
        type: 'toggle',
        on: '',
        off: '0',
        defaultOn: true
      },
      { key: 'sumir', label: 'Mensagens do chat somem depois de', type: 'select', options: SUMIR },
      { key: 'max', label: 'Máximo de mensagens do chat', type: 'number', placeholder: '10', min: 1, max: 40 }
    ]
  },
  'gameplay.html': {
    name: 'Overlay de gameplay',
    description: 'Transparente: moldura da câmera e barra de recados por cima do jogo.',
    width: 1920,
    height: 1080,
    role: 'overlay',
    camera: { param: 'cam', off: 'none' },
    params: [
      {
        key: 'cam',
        label: 'Moldura da câmera',
        type: 'select',
        options: [
          { value: '', label: 'Embaixo à esquerda' },
          { value: 'br', label: 'Embaixo à direita' },
          { value: 'tl', label: 'Em cima à esquerda' },
          { value: 'tr', label: 'Em cima à direita' },
          { value: 'none', label: 'Sem moldura' }
        ]
      },
      { key: 'w', label: 'Largura da câmera', type: 'number', placeholder: '480', min: 160, max: 1280, unit: 'px' },
      { key: 'h', label: 'Altura da câmera', type: 'number', placeholder: '270', min: 90, max: 720, unit: 'px' },
      { key: 'chip', label: 'Barra de recados', type: 'toggle', on: '', off: '0', defaultOn: true }
    ]
  },
  'webcam.html': {
    name: 'Moldura da câmera',
    description: 'Moldura avulsa no tamanho da câmera, para pôr por cima dela.',
    width: 480,
    height: 270,
    role: 'frame',
    camera: 'hide',
    params: [{ key: 'nome', label: 'Etiqueta com o nome', type: 'toggle', on: '', off: '0', defaultOn: true }]
  },
  'chat.html': {
    name: 'Chat da Twitch',
    description: 'Chat no estilo do canal. As mensagens antigas somem sozinhas.',
    width: 520,
    height: 900,
    role: 'chat',
    params: [
      { key: 'canal', label: 'Canal', type: 'text', placeholder: 'o canal dos textos do kit' },
      { key: 'sumir', label: 'Mensagem some depois de', type: 'select', options: SUMIR },
      { key: 'max', label: 'Máximo de mensagens na tela', type: 'number', placeholder: '8', min: 1, max: 40 },
      { key: 'fundo', label: 'Fundo escuro sólido', type: 'toggle', on: '1', off: '', defaultOn: false }
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

// ------------------------------------------------- textos do kit (config.js)

function configFile(path: string): string {
  return join(path, 'src', 'config.js')
}

const KNOWN_KEYS = ['canal', 'nome', 'jogo', 'contagem', 'agenda', 'redes', 'pix', 'recados']

/**
 * O config.js é um script (window.KAYO = {...}). Roda isolado, sem acesso a
 * nada, só para ler o objeto: é um arquivo do próprio kit do usuário.
 */
export function readKitConfig(path: string): KitConfig | null {
  const file = configFile(path)
  if (!existsSync(file)) return null
  try {
    const sandbox: { window: Record<string, unknown> } = { window: {} }
    runInNewContext(readFileSync(file, 'utf8'), sandbox, { timeout: 500 })
    const raw = (Object.values(sandbox.window)[0] ?? {}) as Record<string, unknown>
    const str = (k: string): string => (typeof raw[k] === 'string' ? (raw[k] as string) : '')
    const extra = Object.fromEntries(Object.entries(raw).filter(([k]) => !KNOWN_KEYS.includes(k)))
    return {
      canal: str('canal'),
      nome: str('nome'),
      jogo: str('jogo'),
      contagem: typeof raw.contagem === 'number' ? raw.contagem : 5,
      agenda: str('agenda'),
      pix: str('pix'),
      redes: Array.isArray(raw.redes)
        ? (raw.redes as Array<{ rede?: unknown; handle?: unknown }>).map((r) => ({
            rede: String(r.rede ?? ''),
            handle: String(r.handle ?? '')
          }))
        : [],
      recados: Array.isArray(raw.recados) ? (raw.recados as unknown[]).map(String) : [],
      extra: JSON.parse(JSON.stringify(extra)) as Record<string, unknown>
    }
  } catch (err) {
    console.warn('[kit] config.js ilegível:', (err as Error).message)
    return null
  }
}

/** Nome do objeto global (window.KAYO) para gravar de volta com o mesmo nome. */
function globalName(path: string): string {
  try {
    return /window\.([A-Za-z_$][\w$]*)\s*=/.exec(readFileSync(configFile(path), 'utf8'))?.[1] ?? 'KAYO'
  } catch {
    return 'KAYO'
  }
}

const q = (v: string): string => JSON.stringify(v)

export function writeKitConfig(path: string, c: KitConfig): { ok: true } | { ok: false; error: string } {
  const file = configFile(path)
  if (!existsSync(file)) return { ok: false, error: 'O kit não tem src/config.js.' }
  const extra = Object.entries(c.extra)
    .map(([k, v]) => `\n  ${/^[A-Za-z_$][\w$]*$/.test(k) ? k : q(k)}: ${JSON.stringify(v, null, 2).replace(/\n/g, '\n  ')},\n`)
    .join('')
  const redes = c.redes
    .filter((r) => r.rede.trim() || r.handle.trim())
    .map((r) => `    { rede: ${q(r.rede.trim())}, handle: ${q(r.handle.trim())} },`)
    .join('\n')
  const recados = c.recados
    .filter((r) => r.trim())
    .map((r) => `    ${q(r.trim())},`)
    .join('\n')
  const text = `// Edite aqui (ou no PrismCast: Widgets → Textos do kit) e todas as telas se atualizam sozinhas.
window.${globalName(path)} = {
  canal: ${q(c.canal)},
  nome: ${q(c.nome)},

  // Jogo/assunto padrão das telas "começando" e "volto já" (dá pra trocar pela URL: ?jogo=Dark%20Souls)
  jogo: ${q(c.jogo)},

  // Minutos da contagem regressiva da tela "começando" (ou pela URL: ?min=10)
  contagem: ${Number.isFinite(c.contagem) ? c.contagem : 5},

  // Texto da próxima live na tela de encerramento e na tela offline
  agenda: ${q(c.agenda)},

  redes: [
${redes}
  ],

  pix: ${q(c.pix)},

  // Frases que giram na tela "volto já" e na barra do overlay de gameplay
  recados: [
${recados}
  ],
${extra}};
`
  try {
    // Cópia de segurança da versão escrita à mão, só na primeira vez.
    const backup = `${file}.original`
    if (!existsSync(backup)) writeFileSync(backup, readFileSync(file))
    writeFileSync(`${file}.tmp`, text, 'utf8')
    renameSync(`${file}.tmp`, file)
    return { ok: true }
  } catch (err) {
    return { ok: false, error: (err as Error).message }
  }
}
