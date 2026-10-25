/**
 * Processadores de áudio que rodam na thread de áudio (AudioWorklet).
 *
 *  pcm-player  toca o PCM que chega do prismcast-audio.exe (áudio de um app só)
 *  noise-gate  fecha o microfone quando o som fica abaixo do limiar
 *
 * Este arquivo é carregado com `?worker&url`: vira um script separado, porque o
 * AudioWorklet não enxerga nada do resto da interface.
 */

declare const sampleRate: number
declare function registerProcessor(name: string, ctor: unknown): void
declare class AudioWorkletProcessor {
  readonly port: MessagePort
}
type ParamMap = Record<string, Float32Array>

/** Atraso que o player tenta manter: segura picos do IPC sem atrasar a voz. */
const TARGET_FRAMES = Math.round(0.04 * 48000)
const MAX_FRAMES = Math.round(0.25 * 48000)

class PcmPlayer extends AudioWorkletProcessor {
  private buf = new Float32Array(48000 * 2)
  private read = 0
  private write = 0
  private primed = false

  constructor() {
    super()
    this.port.onmessage = (e: MessageEvent<Float32Array>) => this.push(e.data)
  }

  private get frames(): number {
    return ((this.write - this.read + this.buf.length) % this.buf.length) / 2
  }

  private push(pcm: Float32Array): void {
    const cap = this.buf.length
    for (let i = 0; i < pcm.length; i++) {
      this.buf[this.write] = pcm[i]
      this.write = (this.write + 1) % cap
    }
    // Ficou atrasado (a interface travou um instante): pula para perto do fim.
    if (this.frames > MAX_FRAMES) {
      this.read = (this.write - TARGET_FRAMES * 2 + cap) % cap
    }
  }

  process(_inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    const out = outputs[0]
    const left = out[0]
    const right = out[1] ?? out[0]
    const n = left.length
    if (!this.primed && this.frames >= TARGET_FRAMES) this.primed = true
    if (!this.primed || this.frames < n) {
      this.primed = false
      left.fill(0)
      right.fill(0)
      return true
    }
    const cap = this.buf.length
    for (let i = 0; i < n; i++) {
      left[i] = this.buf[this.read]
      right[i] = this.buf[(this.read + 1) % cap]
      this.read = (this.read + 2) % cap
    }
    return true
  }
}

class NoiseGate extends AudioWorkletProcessor {
  static get parameterDescriptors(): Array<{ name: string; defaultValue: number; minValue: number; maxValue: number }> {
    return [
      { name: 'thresholdDb', defaultValue: -100, minValue: -100, maxValue: 0 },
      { name: 'releaseMs', defaultValue: 150, minValue: 10, maxValue: 2000 }
    ]
  }

  private env = 0
  private gain = 0
  private holdLeft = 0

  process(inputs: Float32Array[][], outputs: Float32Array[][], params: ParamMap): boolean {
    const input = inputs[0]
    const output = outputs[0]
    if (!input || input.length === 0) return true
    const threshold = Math.pow(10, params.thresholdDb[0] / 20)
    const attack = 1 - Math.exp(-1 / (0.004 * sampleRate))
    const release = 1 - Math.exp(-1 / ((params.releaseMs[0] / 1000) * sampleRate))
    const envFall = 1 - Math.exp(-1 / (0.02 * sampleRate))
    const hold = Math.round(0.08 * sampleRate)
    const n = input[0].length
    for (let i = 0; i < n; i++) {
      let peak = 0
      for (let c = 0; c < input.length; c++) peak = Math.max(peak, Math.abs(input[c][i]))
      this.env = peak > this.env ? peak : this.env + (peak - this.env) * envFall
      if (this.env >= threshold) {
        this.holdLeft = hold
        this.gain += (1 - this.gain) * attack
      } else if (this.holdLeft > 0) {
        this.holdLeft--
      } else {
        this.gain += (0 - this.gain) * release
      }
      for (let c = 0; c < output.length; c++) output[c][i] = (input[c] ?? input[0])[i] * this.gain
    }
    return true
  }
}

registerProcessor('pcm-player', PcmPlayer)
registerProcessor('noise-gate', NoiseGate)

export {}
