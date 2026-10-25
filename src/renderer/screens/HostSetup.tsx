import { useCallback, useEffect, useRef, useState } from 'react'
import type { AppSettings, DesktopSource, FirewallStatus } from '@shared/ipc'
import { MAX_VIEWERS, type SourceKind } from '@shared/protocol'
import type { QualitySettings } from '@shared/quality'
import { Badge, Button, IconButton, Segmented, Toggle } from '@/components/ui'
import { QualityPanel } from '@/components/QualityPanel'
import { SourceGrid } from '@/components/SourceGrid'
import { AudioMeter } from '@/components/AudioMeter'
import { FirewallModal } from '@/components/FirewallModal'
import { NetworkPicker } from '@/components/NetworkPicker'
import {
  IconAlert,
  IconArrowLeft,
  IconBroadcast,
  IconLock,
  IconRefresh
} from '@/components/Icons'
import './HostSetup.css'

function videoConstraints(q: QualitySettings): MediaTrackConstraints {
  return {
    frameRate: { ideal: q.fps, max: q.fps },
    ...(q.maxHeight ? { height: { max: q.maxHeight } } : {})
  }
}

function stopStream(stream: MediaStream | null): void {
  stream?.getTracks().forEach((t) => t.stop())
}

export function HostSetup({
  settings,
  onSettingsChange,
  onBack,
  onStart
}: {
  settings: AppSettings
  onSettingsChange: (patch: Partial<AppSettings>) => void
  onBack: () => void
  onStart: (stream: MediaStream, source: DesktopSource) => Promise<string | null>
}): React.JSX.Element {
  const [sources, setSources] = useState<DesktopSource[]>([])
  const [loading, setLoading] = useState(true)
  const [tab, setTab] = useState<SourceKind>('screen')
  const [selected, setSelected] = useState<DesktopSource | null>(null)
  const [preview, setPreview] = useState<MediaStream | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [starting, setStarting] = useState(false)

  const [firewall, setFirewall] = useState<FirewallStatus | null>(null)
  const [showFirewall, setShowFirewall] = useState(false)

  const videoRef = useRef<HTMLVideoElement>(null)
  const handedOffRef = useRef(false)

  const quality = settings.quality

  const loadSources = useCallback(async (): Promise<void> => {
    setLoading(true)
    try {
      setSources(await window.api.sources.list())
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadSources()
    void window.api.firewall.status().then(setFirewall)
  }, [loadSources])

  useEffect(() => {
    return () => {
      if (!handedOffRef.current) stopStream(preview)
    }
  }, [preview])

  useEffect(() => {
    if (videoRef.current) videoRef.current.srcObject = preview
  }, [preview])

  useEffect(() => {
    if (!selected) return
    let cancelled = false

    const acquire = async (): Promise<void> => {
      setError(null)
      try {
        await window.api.sources.select(selected.id, quality.audio)
        const stream = await navigator.mediaDevices.getDisplayMedia({
          video: videoConstraints(quality),
          audio: quality.audio
        })
        if (cancelled) {
          stopStream(stream)
          return
        }
        setPreview((old) => {
          if (old !== stream) stopStream(old)
          return stream
        })

        stream.getVideoTracks()[0]?.addEventListener('ended', () => {
          if (cancelled) return
          setSelected(null)
          setPreview(null)
          setError('A janela escolhida foi fechada. Escolha outra fonte.')
          void loadSources()
        })
      } catch (err) {
        if (cancelled) return
        setError(
          `Não foi possível capturar essa fonte: ${(err as Error).message}. Tente outra janela.`
        )
        setSelected(null)
      }
    }

    void acquire()
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected?.id, quality.audio, loadSources])

  useEffect(() => {
    const track = preview?.getVideoTracks()[0]
    if (!track) return
    void track.applyConstraints(videoConstraints(quality)).catch(() => {})
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [preview, quality.fps, quality.maxHeight])

  const begin = async (): Promise<void> => {
    if (!preview || !selected) return
    setStarting(true)
    setError(null)
    handedOffRef.current = true
    const failure = await onStart(preview, selected)
    if (failure) {
      handedOffRef.current = false
      setError(failure)
      setStarting(false)
    }
  }

  const handleStartClick = (): void => {
    if (firewall && !firewall.ruleExists && !settings.firewallPromptDismissed) {
      setShowFirewall(true)
      return
    }
    void begin()
  }

  const visible = sources.filter((s) => s.kind === tab)

  return (
    <div className="screen fade-in">
      <header className="page-head">
        <IconButton label="Voltar" onClick={onBack}>
          <IconArrowLeft size={19} />
        </IconButton>
        <div>
          <div className="page-title">Configurar Estúdio & Fonte</div>
          <div className="page-sub">
            Escolha a tela ou janela principal. Você poderá trocar de fonte ao vivo durante a
            transmissão.
          </div>
        </div>
        <div className="spacer" />
        <Segmented
          value={tab}
          onChange={setTab}
          options={[
            { value: 'screen' as SourceKind, label: 'Telas' },
            { value: 'window' as SourceKind, label: 'Janelas' }
          ]}
        />
        <IconButton label="Atualizar lista" onClick={() => void loadSources()}>
          <IconRefresh size={18} />
        </IconButton>
      </header>

      <div className="setup-body">
        <section className="setup-sources screen-scroll">
          {loading ? (
            <div className="empty-state">
              <span className="spinner" />
              <span>Procurando telas e janelas...</span>
            </div>
          ) : (
            <SourceGrid
              sources={visible}
              selectedId={selected?.id ?? null}
              onSelect={setSelected}
            />
          )}
        </section>

        <aside className="setup-panel">
          <div className="setup-panel-scroll screen-scroll">
            <div className="preview-frame">
              {preview ? (
                <video ref={videoRef} autoPlay muted playsInline />
              ) : (
                <div className="preview-placeholder">
                  <IconBroadcast size={24} />
                  <span>Escolha uma fonte ao lado</span>
                </div>
              )}
            </div>

            {preview && quality.audio && <AudioMeter stream={preview} />}

            {selected && (
              <div className="setup-selected">
                <Badge tone="accent">{selected.kind === 'screen' ? 'Tela' : 'Janela'}</Badge>
                <span className="setup-selected-name">{selected.name}</span>
              </div>
            )}

            <div className="setup-divider" />

            <QualityPanel
              settings={quality}
              onChange={(next) => onSettingsChange({ quality: next })}
            />

            <div className="setup-divider" />

            <Toggle
              checked={settings.audioMixer.micEnabled}
              onChange={(micEnabled) =>
                onSettingsChange({
                  audioMixer: { ...settings.audioMixer, micEnabled }
                })
              }
              label="Incluir Microfone no Mixer Ao Vivo"
              hint="Mixa sua voz com o áudio do sistema (com supressão de ruído e compressor)."
            />

            <Toggle
              checked={settings.studio.webcamEnabled}
              onChange={(webcamEnabled) =>
                onSettingsChange({
                  studio: { ...settings.studio, webcamEnabled }
                })
              }
              label="Picture-in-Picture (Webcam sobre a tela)"
              hint="Adiciona sua câmera com borda arredondada nos layouts 16:9 e 9:16."
            />

            <Toggle
              checked={settings.studio.dualOutputEnabled}
              onChange={(dualOutputEnabled) =>
                onSettingsChange({
                  studio: { ...settings.studio, dualOutputEnabled }
                })
              }
              label="Dual Output Vertical (9:16 TikTok / Shorts)"
              hint="Renderiza simultaneamente uma cena vertical 1080x1920 otimizada."
            />

            <Toggle
              checked={settings.requirePin}
              onChange={(v) => onSettingsChange({ requirePin: v })}
              label="Pedir um código PIN para entrar na LAN"
              hint="Recomendado em redes compartilhadas."
            />

            {firewall && !firewall.ruleExists && firewall.checked && (
              <button className="setup-fw-hint" onClick={() => setShowFirewall(true)}>
                <IconAlert size={16} />
                <span>O app ainda não está liberado no firewall. Resolver agora.</span>
              </button>
            )}

            {error && (
              <div className="setup-error">
                <IconAlert size={16} />
                <span>{error}</span>
              </div>
            )}
          </div>

          <div className="setup-actions">
            <NetworkPicker
              value={settings.preferredInterface}
              onChange={(preferredInterface) => onSettingsChange({ preferredInterface })}
            />
            <Button
              variant="primary"
              size="lg"
              block
              disabled={!preview || !selected}
              loading={starting}
              onClick={handleStartClick}
              icon={<IconBroadcast size={18} />}
            >
              Entrar no Estúdio Ao Vivo
            </Button>
            <p className="setup-actions-note">
              {settings.requirePin ? (
                <>
                  <IconLock size={13} /> Código PIN de 6 dígitos ativo
                </>
              ) : (
                <>Entrada livre na LAN</>
              )}
              {' · '}
              até {MAX_VIEWERS} espectadores P2P + Multistream RTMP
            </p>
          </div>
        </aside>
      </div>

      <FirewallModal
        open={showFirewall}
        status={firewall}
        onDone={() => {
          setShowFirewall(false)
          void window.api.firewall.status().then(setFirewall)
          void begin()
        }}
        onSkip={() => {
          setShowFirewall(false)
          onSettingsChange({ firewallPromptDismissed: true })
          void begin()
        }}
      />
    </div>
  )
}
