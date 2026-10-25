import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process'
import type {
  CanvasOrientation,
  Destination,
  DestinationStatus,
  MultistreamStatus
} from '@shared/protocol'
import { getSecret, getSettings, streamKeySecret } from './store'

/**
 * Multistream: um FFmpeg por destino.
 *
 * O renderer grava o canvas (16:9 e, se houver destino vertical, 9:16) com
 * MediaRecorder e manda os pedaços WebM para cá. Cada FFmpeg lê esse WebM do
 * stdin, recodifica em H.264/AAC com GOP de 2s (o que a Twitch exige) e publica
 * em FLV no RTMP do destino. Um destino que cai não derruba os outros.
 */

interface Pipeline {
  dest: Destination
  proc: ChildProcessWithoutNullStreams
  startedAt: number
  /** primeiro byte saiu sem erro: a conexão RTMP abriu */
  live: boolean
  error: string | null
  log: string[]
}

let encoderPromise: Promise<string> | null = null
let ffmpegFound = true

function runFfmpeg(args: string[], timeoutMs: number): Promise<number> {
  return new Promise((resolve) => {
    let done = false
    const proc = spawn('ffmpeg', args, { windowsHide: true, stdio: 'ignore' })
    const timer = setTimeout(() => {
      if (!done) proc.kill()
    }, timeoutMs)
    proc.on('error', () => {
      done = true
      clearTimeout(timer)
      resolve(-1)
    })
    proc.on('exit', (code) => {
      done = true
      clearTimeout(timer)
      resolve(code ?? -1)
    })
  })
}

/**
 * `ffmpeg -encoders` lista NVENC/AMF/QSV mesmo sem a placa correspondente, então
 * cada um é testado codificando um quadro de verdade. Roda uma vez, em segundo plano.
 */
export function detectEncoder(): Promise<string> {
  if (!encoderPromise) {
    encoderPromise = (async () => {
      const probe = await runFfmpeg(['-hide_banner', '-version'], 5000)
      if (probe !== 0) {
        ffmpegFound = false
        return 'nenhum'
      }
      for (const enc of ['h264_nvenc', 'h264_amf', 'h264_qsv']) {
        const code = await runFfmpeg(
          [
            '-hide_banner',
            '-loglevel',
            'error',
            '-f',
            'lavfi',
            '-i',
            'color=c=black:s=320x240:r=30',
            '-frames:v',
            '5',
            '-c:v',
            enc,
            '-f',
            'null',
            '-'
          ],
          8000
        )
        if (code === 0) return enc
      }
      return 'libx264'
    })()
  }
  return encoderPromise
}

function encoderArgs(encoder: string, kbps: number): string[] {
  const rate = ['-b:v', `${kbps}k`, '-maxrate', `${kbps}k`, '-bufsize', `${kbps * 2}k`]
  switch (encoder) {
    case 'h264_nvenc':
      return ['-c:v', encoder, '-preset', 'p5', '-tune', 'll', '-rc', 'cbr', ...rate, '-bf', '2']
    case 'h264_amf':
      return ['-c:v', encoder, '-quality', 'balanced', '-rc', 'cbr', ...rate]
    case 'h264_qsv':
      return ['-c:v', encoder, '-preset', 'veryfast', ...rate]
    default:
      return ['-c:v', 'libx264', '-preset', 'veryfast', '-tune', 'zerolatency', ...rate, '-x264-params', 'nal-hrd=cbr']
  }
}

function endpoint(server: string, key: string): string {
  return `${server.trim().replace(/\/+$/, '')}/${key.trim().replace(/^\/+/, '')}`
}

/** Mensagens do FFmpeg/RTMP traduzidas para algo que dê para agir. */
function explain(log: string[], dest: Destination): string {
  const text = log.join('\n')
  // -10053/-10054 = WSAECONNABORTED/RESET: o servidor derrubou a conexão.
  if (/Input\/output error|Connection reset|Broken pipe|End of file|-1005[34]|Error closing file|av_interleaved_write_frame/i.test(text)) {
    return `${dest.name} fechou a conexão. Quase sempre é chave errada ou expirada: cole a chave de novo.`
  }
  if (/Connection refused|Connection timed out|No route|Failed to resolve|getaddrinfo/i.test(text)) {
    return `Não deu para alcançar o servidor de ${dest.name}. Confira a internet e o endereço do servidor.`
  }
  if (/Protocol not found|rtmps/i.test(text) && dest.server.startsWith('rtmps')) {
    return 'Este FFmpeg não tem suporte a RTMPS. Instale uma versão "full" (ex.: gyan.dev).'
  }
  const last = log.filter((l) => l.trim()).slice(-1)[0]
  return last ? `FFmpeg: ${last.trim()}` : `A transmissão para ${dest.name} parou.`
}

export class MultistreamManager {
  private pipes = new Map<string, Pipeline>()
  private failed = new Map<string, string>()
  private active = false
  private encoder = 'libx264'

  constructor() {
    void detectEncoder().then((e) => (this.encoder = e))
  }

  async start(fps: number): Promise<{ ok: boolean; status: MultistreamStatus; error?: string }> {
    this.stop()
    this.encoder = await detectEncoder()
    if (!ffmpegFound) {
      const installCmd =
        process.platform === 'linux'
          ? 'sudo apt install ffmpeg / sudo pacman -S ffmpeg / sudo dnf install ffmpeg'
          : 'winget install ffmpeg'
      return {
        ok: false,
        status: this.status(),
        error: `FFmpeg não encontrado. Instale (${installCmd}) e reabra o app.`
      }
    }

    const destinations = getSettings().destinations.filter((d) => d.enabled)
    if (destinations.length === 0) {
      return {
        ok: false,
        status: this.status(),
        error: 'Nenhum destino ligado. Conecte a Twitch (ou outra plataforma) em Destinos.'
      }
    }

    this.failed.clear()
    for (const dest of destinations) {
      const key = getSecret(streamKeySecret(dest.id))
      if (!key) {
        this.failed.set(dest.id, `${dest.name} está sem chave de transmissão.`)
        continue
      }
      if (!dest.server.trim()) {
        this.failed.set(dest.id, `${dest.name} está sem endereço de servidor.`)
        continue
      }
      try {
        this.pipes.set(dest.id, this.spawn(dest, key, fps))
      } catch (err) {
        this.failed.set(dest.id, (err as Error).message)
      }
    }

    this.active = this.pipes.size > 0
    const status = this.status()
    if (!this.active) {
      return { ok: false, status, error: [...this.failed.values()][0] ?? 'Nada para transmitir.' }
    }
    return { ok: true, status }
  }

  private spawn(dest: Destination, key: string, fps: number): Pipeline {
    const kbps = Math.min(20000, Math.max(1000, dest.bitrateKbps))
    const gop = String(fps * 2)
    const args = [
      '-hide_banner',
      '-loglevel',
      'warning',
      '-probesize',
      '2M',
      '-analyzeduration',
      '1000000',
      '-fflags',
      '+genpts',
      '-i',
      'pipe:0',
      '-map',
      '0:v:0',
      '-map',
      '0:a:0?',
      ...encoderArgs(this.encoder, kbps),
      '-r',
      String(fps),
      '-g',
      gop,
      '-keyint_min',
      gop,
      '-pix_fmt',
      'yuv420p',
      '-c:a',
      'aac',
      '-b:a',
      '160k',
      '-ar',
      '48000',
      '-ac',
      '2',
      '-f',
      'flv',
      endpoint(dest.server, key)
    ]

    const proc = spawn('ffmpeg', args, { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true })
    const pipe: Pipeline = { dest, proc, startedAt: Date.now(), live: false, error: null, log: [] }

    // stderr TEM que ser lido: se o buffer do pipe enche, o FFmpeg trava esperando.
    proc.stderr.setEncoding('utf8')
    proc.stderr.on('data', (chunk: string) => {
      for (const line of chunk.split(/\r?\n/)) {
        if (!line.trim()) continue
        // Nunca guardar a chave no log que vai para a interface.
        pipe.log.push(line.split(key).join('••••'))
      }
      if (pipe.log.length > 40) pipe.log.splice(0, pipe.log.length - 40)
    })
    proc.stdout.resume()
    proc.stdin.on('error', () => {
      /* tratado no 'exit' */
    })
    proc.on('error', (err) => {
      pipe.error = `Não deu para abrir o FFmpeg: ${err.message}`
    })
    proc.on('exit', () => {
      if (this.pipes.get(dest.id) !== pipe) return
      this.pipes.delete(dest.id)
      this.failed.set(dest.id, pipe.error ?? explain(pipe.log, dest))
      this.active = this.pipes.size > 0
    })
    // Sem erro em 4s depois de receber dados = RTMP aceitou a conexão.
    setTimeout(() => {
      if (this.pipes.get(dest.id) === pipe && !pipe.error) pipe.live = true
    }, 4000)
    return pipe
  }

  pushChunk(orientation: CanvasOrientation, chunk: Uint8Array): void {
    if (!this.active || chunk.byteLength === 0) return
    const buf = Buffer.from(chunk)
    for (const pipe of this.pipes.values()) {
      if (pipe.dest.orientation !== orientation) continue
      if (pipe.proc.killed || !pipe.proc.stdin.writable) continue
      pipe.proc.stdin.write(buf)
    }
  }

  /** Quais canvases o renderer precisa gravar agora. */
  orientations(): CanvasOrientation[] {
    return [...new Set([...this.pipes.values()].map((p) => p.dest.orientation))]
  }

  stop(): MultistreamStatus {
    for (const pipe of this.pipes.values()) {
      try {
        pipe.proc.stdin.end()
        // Dá um instante para o FFmpeg fechar o FLV direito antes de matar.
        setTimeout(() => pipe.proc.kill(), 1500)
      } catch {
        /* ja encerrado */
      }
    }
    this.pipes.clear()
    this.active = false
    return this.status()
  }

  status(): MultistreamStatus {
    const now = Date.now()
    const destinations: DestinationStatus[] = getSettings()
      .destinations.filter((d) => d.enabled || this.pipes.has(d.id) || this.failed.has(d.id))
      .map((d) => {
        const pipe = this.pipes.get(d.id)
        if (pipe) {
          return {
            id: d.id,
            state: pipe.live ? 'live' : 'connecting',
            uptimeSec: Math.floor((now - pipe.startedAt) / 1000),
            error: null
          }
        }
        const error = this.failed.get(d.id) ?? null
        return { id: d.id, state: error ? 'error' : 'idle', uptimeSec: 0, error }
      })
    return { active: this.active, encoder: this.encoder, ffmpegFound, destinations }
  }
}
