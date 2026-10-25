import type { Scene, SceneItem, StudioCompositorSettings, StreamWidget } from '@shared/protocol'

export interface ActiveAlert {
  title: string
  subtitle: string
  accentColor: string
  until: number
}

/**
 * Compositor Dual-Output do PrismCast Studio (inspirado no DualOutputService +
 * ScenesService + WidgetsService do Streamlabs Desktop).
 */
export class DualCanvasCompositor {
  readonly horizontalCanvas: HTMLCanvasElement
  readonly verticalCanvas: HTMLCanvasElement
  private readonly hCtx: CanvasRenderingContext2D
  private readonly vCtx: CanvasRenderingContext2D

  private readonly screenVideo: HTMLVideoElement
  private readonly webcamVideo: HTMLVideoElement
  private webcamStream: MediaStream | null = null
  private currentWebcamDeviceId: string | null = null

  private studio: StudioCompositorSettings
  private widgets: StreamWidget[] = []
  private activeScene: Scene | null = null
  private selectedItemId: string | null = null
  private hostName = 'PrismCast Host'
  private sourceName = 'Tela Principal'
  private viewerCount = 0
  private activeAlert: ActiveAlert | null = null

  private renderTimer: number | null = null
  private fps = 60
  private readonly horizontalStream: MediaStream
  private readonly verticalStream: MediaStream
  private disposed = false

  constructor(
    initialStudio: StudioCompositorSettings,
    initialWidgets: StreamWidget[],
    audioTrack?: MediaStreamTrack
  ) {
    this.studio = { ...initialStudio }
    this.widgets = [...initialWidgets]

    this.horizontalCanvas = document.createElement('canvas')
    this.horizontalCanvas.width = 1920
    this.horizontalCanvas.height = 1080
    this.hCtx = this.horizontalCanvas.getContext('2d', { alpha: false })!

    this.verticalCanvas = document.createElement('canvas')
    this.verticalCanvas.width = 1080
    this.verticalCanvas.height = 1920
    this.vCtx = this.verticalCanvas.getContext('2d', { alpha: false })!

    this.screenVideo = document.createElement('video')
    this.screenVideo.muted = true
    this.screenVideo.playsInline = true
    this.screenVideo.autoplay = true

    this.webcamVideo = document.createElement('video')
    this.webcamVideo.muted = true
    this.webcamVideo.playsInline = true
    this.webcamVideo.autoplay = true

    this.renderTick()

    this.horizontalStream = this.horizontalCanvas.captureStream(this.fps)
    this.verticalStream = this.verticalCanvas.captureStream(this.fps)

    if (audioTrack) {
      this.horizontalStream.addTrack(audioTrack)
      this.verticalStream.addTrack(audioTrack.clone())
    }

    this.startLoop(this.fps)
    if (initialStudio.webcamEnabled) {
      void this.syncWebcam(true, initialStudio.webcamDeviceId)
    }
  }

  get outputHorizontalStream(): MediaStream {
    return this.horizontalStream
  }

  get outputVerticalStream(): MediaStream {
    return this.verticalStream
  }

  setSourceStream(stream: MediaStream | null, sourceName?: string): void {
    if (sourceName) this.sourceName = sourceName
    this.screenVideo.srcObject = stream
    if (stream) {
      void this.screenVideo.play().catch(() => {})
    }
  }

  setMeta(meta: { hostName?: string; sourceName?: string; viewerCount?: number }): void {
    if (meta.hostName !== undefined) this.hostName = meta.hostName
    if (meta.sourceName !== undefined) this.sourceName = meta.sourceName
    if (meta.viewerCount !== undefined) this.viewerCount = meta.viewerCount
  }

  setFps(fps: number): void {
    const clamped = Math.max(15, Math.min(60, fps))
    if (clamped === this.fps) return
    this.fps = clamped
    this.startLoop(this.fps)
  }

  async updateStudio(next: StudioCompositorSettings): Promise<void> {
    const webcamChanged =
      next.webcamEnabled !== this.studio.webcamEnabled ||
      next.webcamDeviceId !== this.studio.webcamDeviceId
    this.studio = { ...next }
    if (webcamChanged) {
      await this.syncWebcam(next.webcamEnabled, next.webcamDeviceId)
    }
  }

  updateWidgets(widgets: StreamWidget[]): void {
    this.widgets = [...widgets]
  }

  setActiveScene(scene: Scene | null, selectedItemId?: string | null): void {
    this.activeScene = scene
    if (selectedItemId !== undefined) {
      this.selectedItemId = selectedItemId
    }
  }

  setSelectedItemId(id: string | null): void {
    this.selectedItemId = id
  }

  triggerAlert(title: string, subtitle: string, accentColor = '#17d4b2'): void {
    this.activeAlert = {
      title,
      subtitle,
      accentColor,
      until: Date.now() + 5000
    }
  }

  private async syncWebcam(enabled: boolean, deviceId: string | null): Promise<void> {
    if (!enabled) {
      this.stopWebcam()
      return
    }
    if (this.webcamStream && this.currentWebcamDeviceId === deviceId) return
    this.stopWebcam()
    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        video: {
          ...(deviceId ? { deviceId: { exact: deviceId } } : {}),
          width: { ideal: 1280 },
          height: { ideal: 720 },
          frameRate: { ideal: 30 }
        },
        audio: false
      })
      if (this.disposed || !this.studio.webcamEnabled) {
        stream.getTracks().forEach((t) => t.stop())
        return
      }
      this.webcamStream = stream
      this.currentWebcamDeviceId = deviceId
      this.webcamVideo.srcObject = stream
      await this.webcamVideo.play().catch(() => {})
    } catch (err) {
      console.warn('[Compositor] Webcam indisponivel:', err)
    }
  }

  private stopWebcam(): void {
    this.webcamVideo.srcObject = null
    if (this.webcamStream) {
      this.webcamStream.getTracks().forEach((t) => t.stop())
      this.webcamStream = null
    }
    this.currentWebcamDeviceId = null
  }

  private startLoop(fps: number): void {
    if (this.renderTimer !== null) {
      window.clearInterval(this.renderTimer)
    }
    const intervalMs = Math.max(16, Math.round(1000 / fps))
    this.renderTimer = window.setInterval(() => this.renderTick(), intervalMs)
  }

  private renderTick(): void {
    if (this.disposed) return
    const now = Date.now()
    if (this.activeAlert && now > this.activeAlert.until) {
      this.activeAlert = null
    }

    this.renderHorizontal(now)
    if (this.studio.dualOutputEnabled) {
      this.renderVertical(now)
    }
  }

  private renderHorizontal(now: number): void {
    const ctx = this.hCtx
    const W = 1920
    const H = 1080

    ctx.fillStyle = '#090b10'
    ctx.fillRect(0, 0, W, H)

    if (this.studio.privacyShieldActive) {
      this.drawPrivacyShield(ctx, W, H, now, false)
      return
    }

    if (this.activeScene && this.activeScene.items.length > 0) {
      const items = [...this.activeScene.items].sort(
        (a, b) => (a.transform.zIndex || 0) - (b.transform.zIndex || 0)
      )
      for (const item of items) {
        if (!item.visible || !item.showOnHorizontal) continue
        this.renderSceneItem(ctx, item, W, H, false)
      }
      this.drawWidgetsOverlay(ctx, W, H, false, now)
      return
    }

    if (this.screenVideo.readyState >= 2 && this.screenVideo.videoWidth > 0) {
      this.drawVideoContain(ctx, this.screenVideo, 0, 0, W, H)
    } else {
      this.drawStandbyPattern(ctx, W, H)
    }

    if (
      this.studio.webcamEnabled &&
      this.webcamVideo.readyState >= 2 &&
      this.webcamVideo.videoWidth > 0
    ) {
      const scale = Math.max(0.15, Math.min(0.42, this.studio.webcamScale || 0.24))
      const pipW = Math.round(W * scale)
      const pipH = Math.round(pipW * (9 / 16))
      const margin = 32
      let x = W - pipW - margin
      let y = H - pipH - margin

      if (this.studio.webcamPosition === 'bottom-left') {
        x = margin
        y = H - pipH - margin
      } else if (this.studio.webcamPosition === 'top-right') {
        x = W - pipW - margin
        y = margin
      } else if (this.studio.webcamPosition === 'top-left') {
        x = margin
        y = margin
      }

      this.drawRoundedVideo(ctx, this.webcamVideo, x, y, pipW, pipH, 18, '#17d4b2')
    }

    this.drawWidgetsOverlay(ctx, W, H, false, now)
  }

  private renderVertical(now: number): void {
    const ctx = this.vCtx
    const W = 1080
    const H = 1920

    ctx.fillStyle = '#090b10'
    ctx.fillRect(0, 0, W, H)

    if (this.studio.privacyShieldActive) {
      this.drawPrivacyShield(ctx, W, H, now, true)
      return
    }

    if (this.activeScene && this.activeScene.items.length > 0) {
      const items = [...this.activeScene.items].sort(
        (a, b) => (a.verticalTransform.zIndex || 0) - (b.verticalTransform.zIndex || 0)
      )
      for (const item of items) {
        if (!item.visible || !item.showOnVertical) continue
        this.renderSceneItem(ctx, item, W, H, true)
      }
      this.drawWidgetsOverlay(ctx, W, H, true, now)
      return
    }

    const hasScreen = this.screenVideo.readyState >= 2 && this.screenVideo.videoWidth > 0
    const hasWebcam =
      this.studio.webcamEnabled &&
      this.webcamVideo.readyState >= 2 &&
      this.webcamVideo.videoWidth > 0

    const preset = this.studio.verticalLayout

    if (preset === 'split-cam-top') {
      const topH = 660
      const botH = H - topH

      if (hasWebcam) {
        this.drawVideoCover(ctx, this.webcamVideo, 0, 0, W, topH)
      } else if (hasScreen) {
        ctx.save()
        ctx.globalAlpha = 0.25
        this.drawVideoCover(ctx, this.screenVideo, 0, 0, W, topH)
        ctx.restore()
        this.drawVerticalTopBanner(ctx, W, topH)
      } else {
        this.drawVerticalTopBanner(ctx, W, topH)
      }

      if (hasScreen) {
        this.drawVideoCover(ctx, this.screenVideo, 0, topH, W, botH)
      }

      const grad = ctx.createLinearGradient(0, topH - 2, W, topH + 2)
      grad.addColorStop(0, '#6c5ce7')
      grad.addColorStop(0.5, '#17d4b2')
      grad.addColorStop(1, '#6c5ce7')
      ctx.fillStyle = grad
      ctx.fillRect(0, topH - 3, W, 6)
    } else if (preset === 'blurred-backdrop') {
      if (hasScreen) {
        ctx.save()
        ctx.globalAlpha = 0.26
        this.drawVideoCover(ctx, this.screenVideo, 0, 0, W, H)
        ctx.restore()

        const fgH = Math.round(W * (9 / 16))
        const fgY = Math.round((H - fgH) / 2) - (hasWebcam ? 140 : 0)
        this.drawRoundedVideo(ctx, this.screenVideo, 24, fgY, W - 48, fgH, 20, '#6c5ce7')
      }
      if (hasWebcam) {
        const camW = 460
        const camH = 260
        this.drawRoundedVideo(
          ctx,
          this.webcamVideo,
          Math.round((W - camW) / 2),
          H - camH - 180,
          camW,
          camH,
          20,
          '#17d4b2'
        )
      }
    } else if (preset === 'center-crop') {
      if (hasScreen) {
        this.drawVideoCover(ctx, this.screenVideo, 0, 0, W, H)
      }
      if (hasWebcam) {
        this.drawRoundedVideo(ctx, this.webcamVideo, W - 360, H - 260, 320, 180, 18, '#17d4b2')
      }
    } else {
      if (hasScreen) {
        this.drawVideoContain(ctx, this.screenVideo, 0, 0, W, H)
      }
      if (hasWebcam) {
        this.drawRoundedVideo(ctx, this.webcamVideo, W - 380, 90, 340, 200, 18, '#17d4b2')
      }
    }

    this.drawWidgetsOverlay(ctx, W, H, true, now)
  }

  private renderSceneItem(
    ctx: CanvasRenderingContext2D,
    item: SceneItem,
    canvasW: number,
    canvasH: number,
    isVertical: boolean
  ): void {
    const tf = isVertical ? item.verticalTransform : item.transform
    const x = Math.round(tf.x * canvasW)
    const y = Math.round(tf.y * canvasH)
    const w = Math.round(tf.width * canvasW)
    const h = Math.round(tf.height * canvasH)

    if (w <= 0 || h <= 0) return

    ctx.save()

    switch (item.type) {
      case 'screen_capture':
      case 'window_capture': {
        if (this.screenVideo.readyState >= 2 && this.screenVideo.videoWidth > 0) {
          this.drawVideoContain(ctx, this.screenVideo, x, y, w, h)
        } else {
          this.drawStandbyPattern(ctx, canvasW, canvasH)
        }
        break
      }
      case 'camera_input': {
        if (
          this.studio.webcamEnabled &&
          this.webcamVideo.readyState >= 2 &&
          this.webcamVideo.videoWidth > 0
        ) {
          this.drawRoundedVideo(ctx, this.webcamVideo, x, y, w, h, 16, '#17d4b2')
        } else {
          ctx.fillStyle = '#1e293b'
          ctx.fillRect(x, y, w, h)
          ctx.fillStyle = '#94a3b8'
          ctx.font = 'bold 22px system-ui, sans-serif'
          ctx.textAlign = 'center'
          ctx.textBaseline = 'middle'
          ctx.fillText('📷 Webcam Standby', x + w / 2, y + h / 2)
        }
        break
      }
      case 'color_source': {
        ctx.fillStyle = (item.settings && item.settings.color) || '#090d16'
        ctx.fillRect(x, y, w, h)
        break
      }
      case 'text_source': {
        const text = (item.settings && item.settings.text) || item.name
        const fontSize = Math.max(16, (item.settings && item.settings.fontSize) || 32)
        const color = (item.settings && item.settings.color) || '#ffffff'
        ctx.fillStyle = color
        ctx.font = `bold ${fontSize}px system-ui, sans-serif`
        ctx.textAlign = 'center'
        ctx.textBaseline = 'middle'
        ctx.shadowColor = 'rgba(0,0,0,0.85)'
        ctx.shadowBlur = 12
        ctx.fillText(text, x + w / 2, y + h / 2)
        break
      }
      case 'browser_source': {
        ctx.fillStyle = 'rgba(15, 23, 42, 0.85)'
        ctx.fillRect(x, y, w, h)
        ctx.strokeStyle = '#6366f1'
        ctx.lineWidth = 2
        ctx.strokeRect(x, y, w, h)
        ctx.fillStyle = '#a5b4fc'
        ctx.font = 'bold 16px system-ui, sans-serif'
        ctx.textAlign = 'center'
        ctx.textBaseline = 'middle'
        ctx.fillText(`🌐 Overlay: ${item.name}`, x + w / 2, y + h / 2)
        break
      }
      default:
        break
    }

    // Highlight para o item selecionado (estilo OBS/Streamlabs Studio Editor)
    if (this.selectedItemId === item.id) {
      ctx.save()
      ctx.strokeStyle = '#38bdf8'
      ctx.lineWidth = 3
      ctx.setLineDash([8, 6])
      ctx.strokeRect(x, y, w, h)

      // Handles nos cantos
      const handleSize = 10
      ctx.fillStyle = '#38bdf8'
      const corners = [
        [x - handleSize / 2, y - handleSize / 2],
        [x + w - handleSize / 2, y - handleSize / 2],
        [x - handleSize / 2, y + h - handleSize / 2],
        [x + w - handleSize / 2, y + h - handleSize / 2]
      ]
      for (const [cx, cy] of corners) {
        ctx.fillRect(cx, cy, handleSize, handleSize)
      }
      ctx.restore()
    }

    ctx.restore()
  }

  private drawVerticalTopBanner(ctx: CanvasRenderingContext2D, w: number, h: number): void {
    const grad = ctx.createLinearGradient(0, 0, w, h)
    grad.addColorStop(0, 'rgba(18, 22, 34, 0.92)')
    grad.addColorStop(1, 'rgba(12, 15, 24, 0.96)')
    ctx.fillStyle = grad
    ctx.fillRect(0, 0, w, h)

    ctx.fillStyle = '#17d4b2'
    ctx.font = '700 26px system-ui, -apple-system, sans-serif'
    ctx.textAlign = 'center'
    ctx.fillText('PRISMCAST STUDIO · 9:16 LIVE', w / 2, h / 2 - 20)

    ctx.fillStyle = '#f3f5f9'
    ctx.font = '700 42px system-ui, -apple-system, sans-serif'
    ctx.fillText(this.hostName, w / 2, h / 2 + 34)

    ctx.fillStyle = '#8b95a7'
    ctx.font = '500 24px system-ui, -apple-system, sans-serif'
    ctx.fillText(this.sourceName, w / 2, h / 2 + 80)
  }

  private drawWidgetsOverlay(
    ctx: CanvasRenderingContext2D,
    W: number,
    H: number,
    isVertical: boolean,
    now: number
  ): void {
    for (const w of this.widgets) {
      if (!w.enabled) continue
      if (isVertical && !w.showOnVertical) continue
      if (!isVertical && !w.showOnHorizontal) continue

      const accent = w.accentColor || '#17d4b2'

      if (w.kind === 'goal-bar') {
        const barW = isVertical ? W - 96 : 520
        const barH = 54
        const x = isVertical ? 48 : 36
        const y = isVertical ? 44 : 32

        ctx.save()
        this.roundRect(ctx, x, y, barW, barH, 14)
        ctx.fillStyle = 'rgba(13, 16, 23, 0.84)'
        ctx.fill()
        ctx.lineWidth = 2
        ctx.strokeStyle = 'rgba(255, 255, 255, 0.12)'
        ctx.stroke()

        this.roundRect(ctx, x + 4, y + 4, Math.round((barW - 8) * 0.65), barH - 8, 10)
        ctx.fillStyle = accent
        ctx.globalAlpha = 0.28
        ctx.fill()
        ctx.globalAlpha = 1

        ctx.fillStyle = '#ffffff'
        ctx.font = '700 20px system-ui, -apple-system, sans-serif'
        ctx.textAlign = 'left'
        ctx.fillText(w.primaryText || w.name || 'Meta da Live', x + 18, y + 34)

        ctx.fillStyle = accent
        ctx.textAlign = 'right'
        ctx.fillText(w.secondaryText || '65%', x + barW - 18, y + 34)
        ctx.restore()
      } else if (w.kind === 'lower-third') {
        const cardW = isVertical ? W - 96 : 560
        const cardH = 84
        const x = isVertical ? 48 : 36
        const y = isVertical ? H - 150 : H - 120

        ctx.save()
        this.roundRect(ctx, x, y, cardW, cardH, 16)
        ctx.fillStyle = 'rgba(13, 16, 23, 0.88)'
        ctx.fill()

        ctx.fillStyle = accent
        this.roundRect(ctx, x, y, 8, cardH, 4)
        ctx.fill()

        ctx.fillStyle = '#ffffff'
        ctx.font = '700 24px system-ui, -apple-system, sans-serif'
        ctx.textAlign = 'left'
        ctx.fillText(w.primaryText || this.hostName, x + 24, y + 36)

        ctx.fillStyle = '#9aa4b8'
        ctx.font = '500 18px system-ui, -apple-system, sans-serif'
        ctx.fillText(w.secondaryText || this.sourceName, x + 24, y + 64)
        ctx.restore()
      } else if (w.kind === 'viewer-counter') {
        const pillW = 210
        const pillH = 44
        const x = isVertical ? W - pillW - 48 : W - pillW - 36
        const y = isVertical ? 114 : 32

        ctx.save()
        this.roundRect(ctx, x, y, pillW, pillH, 22)
        ctx.fillStyle = 'rgba(13, 16, 23, 0.84)'
        ctx.fill()

        ctx.beginPath()
        ctx.arc(x + 24, y + 22, 6, 0, Math.PI * 2)
        ctx.fillStyle = '#17d4b2'
        ctx.fill()

        ctx.fillStyle = '#f1f5f9'
        ctx.font = '600 18px system-ui, -apple-system, sans-serif'
        ctx.textAlign = 'left'
        ctx.fillText(`AO VIVO · ${this.viewerCount}`, x + 40, y + 28)
        ctx.restore()
      }
    }

    if (this.activeAlert) {
      const remaining = this.activeAlert.until - now
      const alpha = remaining < 500 ? Math.max(0, remaining / 500) : 1
      const boxW = isVertical ? W - 120 : 640
      const boxH = 110
      const x = Math.round((W - boxW) / 2)
      const y = isVertical ? 220 : 88

      ctx.save()
      ctx.globalAlpha = alpha
      this.roundRect(ctx, x, y, boxW, boxH, 20)
      ctx.fillStyle = 'rgba(15, 19, 29, 0.94)'
      ctx.fill()
      ctx.lineWidth = 3
      ctx.strokeStyle = this.activeAlert.accentColor
      ctx.stroke()

      ctx.fillStyle = this.activeAlert.accentColor
      ctx.font = '800 24px system-ui, -apple-system, sans-serif'
      ctx.textAlign = 'center'
      ctx.fillText(this.activeAlert.title.toUpperCase(), W / 2, y + 46)

      ctx.fillStyle = '#ffffff'
      ctx.font = '600 22px system-ui, -apple-system, sans-serif'
      ctx.fillText(this.activeAlert.subtitle, W / 2, y + 82)
      ctx.restore()
    }
  }

  private drawPrivacyShield(
    ctx: CanvasRenderingContext2D,
    W: number,
    H: number,
    now: number,
    isVertical: boolean
  ): void {
    const grad = ctx.createRadialGradient(W / 2, H / 2, 60, W / 2, H / 2, Math.max(W, H) * 0.65)
    grad.addColorStop(0, '#161b29')
    grad.addColorStop(1, '#090b10')
    ctx.fillStyle = grad
    ctx.fillRect(0, 0, W, H)

    const pulse = (Math.sin(now / 450) + 1) / 2
    ctx.save()
    ctx.beginPath()
    ctx.arc(W / 2, H / 2 - 40, 88 + pulse * 18, 0, Math.PI * 2)
    ctx.strokeStyle = `rgba(23, 212, 178, ${0.18 + pulse * 0.25})`
    ctx.lineWidth = 4
    ctx.stroke()

    ctx.fillStyle = '#17d4b2'
    ctx.font = `800 ${isVertical ? 54 : 64}px system-ui, -apple-system, sans-serif`
    ctx.textAlign = 'center'
    ctx.fillText(this.studio.privacyShieldTitle || 'VOLTO JÁ', W / 2, H / 2 - 20)

    ctx.fillStyle = '#e2e8f0'
    ctx.font = `600 ${isVertical ? 26 : 30}px system-ui, -apple-system, sans-serif`
    ctx.fillText(
      this.studio.privacyShieldSubtitle || 'Escudo de Privacidade Ativo — Transmissão Pausada',
      W / 2,
      H / 2 + 38
    )

    ctx.fillStyle = '#8b95a7'
    ctx.font = `500 ${isVertical ? 20 : 22}px system-ui, -apple-system, sans-serif`
    ctx.fillText(`${this.hostName} · PrismCast Studio`, W / 2, H / 2 + 82)
    ctx.restore()
  }

  private drawStandbyPattern(ctx: CanvasRenderingContext2D, W: number, H: number): void {
    ctx.save()
    ctx.fillStyle = '#8b95a7'
    ctx.font = '600 28px system-ui, -apple-system, sans-serif'
    ctx.textAlign = 'center'
    ctx.fillText('PrismCast Studio · Aguardando quadro da fonte...', W / 2, H / 2)
    ctx.restore()
  }

  private drawVideoContain(
    ctx: CanvasRenderingContext2D,
    video: HTMLVideoElement,
    x: number,
    y: number,
    w: number,
    h: number
  ): void {
    const vw = video.videoWidth
    const vh = video.videoHeight
    if (!vw || !vh) return
    const scale = Math.min(w / vw, h / vh)
    const dw = Math.round(vw * scale)
    const dh = Math.round(vh * scale)
    const dx = x + Math.round((w - dw) / 2)
    const dy = y + Math.round((h - dh) / 2)
    ctx.drawImage(video, dx, dy, dw, dh)
  }

  private drawVideoCover(
    ctx: CanvasRenderingContext2D,
    video: HTMLVideoElement,
    x: number,
    y: number,
    w: number,
    h: number
  ): void {
    const vw = video.videoWidth
    const vh = video.videoHeight
    if (!vw || !vh) return
    const scale = Math.max(w / vw, h / vh)
    const sw = Math.round(w / scale)
    const sh = Math.round(h / scale)
    const sx = Math.round((vw - sw) / 2)
    const sy = Math.round((vh - sh) / 2)
    ctx.drawImage(video, sx, sy, sw, sh, x, y, w, h)
  }

  private drawRoundedVideo(
    ctx: CanvasRenderingContext2D,
    video: HTMLVideoElement,
    x: number,
    y: number,
    w: number,
    h: number,
    radius: number,
    borderColor: string
  ): void {
    ctx.save()
    this.roundRect(ctx, x, y, w, h, radius)
    ctx.clip()
    this.drawVideoCover(ctx, video, x, y, w, h)
    ctx.restore()

    ctx.save()
    this.roundRect(ctx, x, y, w, h, radius)
    ctx.lineWidth = 3
    ctx.strokeStyle = borderColor
    ctx.stroke()
    ctx.restore()
  }

  private roundRect(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    w: number,
    h: number,
    r: number
  ): void {
    ctx.beginPath()
    ctx.moveTo(x + r, y)
    ctx.lineTo(x + w - r, y)
    ctx.quadraticCurveTo(x + w, y, x + w, y + r)
    ctx.lineTo(x + w, y + h - r)
    ctx.quadraticCurveTo(x + w, y + h, x + w - r, y + h)
    ctx.lineTo(x + r, y + h)
    ctx.quadraticCurveTo(x, y + h, x, y + h - r)
    ctx.lineTo(x, y + r)
    ctx.quadraticCurveTo(x, y, x + r, y)
    ctx.closePath()
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    if (this.renderTimer !== null) {
      window.clearInterval(this.renderTimer)
      this.renderTimer = null
    }
    this.stopWebcam()
    this.screenVideo.srcObject = null
    this.horizontalStream.getTracks().forEach((t) => t.stop())
    this.verticalStream.getTracks().forEach((t) => t.stop())
  }
}
