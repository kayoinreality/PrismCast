import { useCallback, useEffect, useRef, useState } from 'react'
import { Titlebar } from '@/components/Titlebar'
import { Modal } from '@/components/Modal'
import { Rail, type Page } from '@/components/Rail'
import { Button } from '@/components/ui'
import { IconAlert, IconPrismCastMark } from '@/components/Icons'
import { StudioScreen } from '@/screens/studio/StudioScreen'
import { DestinationsScreen } from '@/screens/DestinationsScreen'
import { WidgetsScreen } from '@/screens/WidgetsScreen'
import { LanScreen } from '@/screens/LanScreen'
import { SettingsScreen } from '@/screens/SettingsScreen'
import { ViewerBrowse } from '@/screens/ViewerBrowse'
import { ViewerPlayer } from '@/screens/ViewerPlayer'
import { useSettings } from '@/hooks/useSettings'
import { useHostBroadcast } from '@/hooks/useHostBroadcast'
import { useViewerStream, type ConnectTarget } from '@/hooks/useViewerStream'
import { StudioProvider, useStudio } from '@/studio/StudioContext'
import {
  useLiveSlice,
  useRecorderSlice,
  useLanSlice,
  useUpdateStatusSlice,
  studioStore
} from '@/studio/StudioStore'
import { LiveStatus } from '@/components/LiveStatus'
import { ensureBridge } from '@/bridge'
import './App.css'

export function App(): React.JSX.Element {
  useEffect(() => {
    ensureBridge()
    studioStore.init()
  }, [])

  const { settings, update, flush } = useSettings()

  if (!settings) {
    return (
      <div className="app">
        <Titlebar />
        <div className="screen boot">
          <span className="spinner" />
        </div>
      </div>
    )
  }

  return (
    <StudioProvider settings={settings} update={update} flushSettings={flush}>
      <Shell />
    </StudioProvider>
  )
}

/** Isola o LiveStatus da Titlebar para que updates 1Hz não re-renderizem o Shell inteiro. */
function TitlebarStatus({
  onClick,
  lanActive
}: {
  onClick: () => void
  lanActive: boolean
}): React.JSX.Element {
  const { settings, engine } = useStudio()
  const multistream = useLiveSlice()
  const recorder = useRecorderSlice()
  const { viewers } = useLanSlice()
  const engineState = engine.state
  const mergedState = {
    ...engineState,
    multistream: multistream.destinations.length > 0 || !engineState.live ? multistream : engineState.multistream,
    recorder: recorder.recording || !engineState.recorder.recording ? recorder : engineState.recorder
  }
  return (
    <LiveStatus
      state={mergedState}
      destinations={settings.destinations}
      lanViewers={lanActive ? viewers.length : null}
      onClick={onClick}
    />
  )
}

/** Banner discreto no topo quando houver nova build disponível no GitHub Releases. */
function UpdateBanner({
  busy,
  onOpenSettings
}: {
  busy: boolean
  onOpenSettings: () => void
}): React.JSX.Element | null {
  const { settings, update, toast } = useStudio()
  const upd = useUpdateStatusSlice()
  const [dismissed, setDismissed] = useState(false)
  const [acting, setActing] = useState(false)

  const latest = upd.latestVersion
  const skipped = settings.updater?.skippedVersion ?? null
  if (
    dismissed ||
    !latest ||
    (upd.state !== 'available' && upd.state !== 'downloading' && upd.state !== 'ready') ||
    (upd.state === 'available' && skipped === latest)
  ) {
    return null
  }

  const startDownload = async (): Promise<void> => {
    const api = ensureBridge().updater
    if (!api) return
    setActing(true)
    try {
      const res = await api.download()
      studioStore.setUpdateStatus(res)
      if (res.state === 'error' && res.error) toast(res.error, 'bad')
      else if (res.state === 'ready') toast(`v${latest} baixada e pronta para instalar`, 'good')
    } finally {
      setActing(false)
    }
  }

  const installNow = async (): Promise<void> => {
    if (busy) {
      toast('Encerre a live ou gravação antes de instalar a atualização.', 'bad')
      return
    }
    const api = ensureBridge().updater
    if (!api) return
    setActing(true)
    try {
      const res = await api.install()
      if (!res.ok) toast(res.error || 'Falha ao iniciar instalação', 'bad')
      else if (res.message) toast(res.message, 'good')
    } finally {
      setActing(false)
    }
  }

  return (
    <div className="update-banner" role="status">
      <div className="update-banner-info">
        <strong>PrismCast Studio v{latest} disponível</strong>
        {upd.state === 'downloading' && (
          <span className="mono">
            · baixando {Math.round(upd.progressPercent)}%
          </span>
        )}
        {upd.state === 'ready' && (
          <span>
            · pronto para instalar{upd.verifiedSha256 ? ' (SHA-256 verificado)' : ''}
          </span>
        )}
        {upd.state === 'available' && upd.asset && (
          <span className="hint">{upd.asset.name}</span>
        )}
      </div>
      <div className="update-banner-actions">
        <Button size="sm" variant="ghost" onClick={onOpenSettings}>
          Detalhes
        </Button>
        {upd.state === 'available' && upd.asset && (
          <Button size="sm" variant="primary" loading={acting} onClick={() => void startDownload()}>
            Baixar atualização
          </Button>
        )}
        {upd.state === 'available' && !upd.asset && upd.releaseUrl && (
          <Button
            size="sm"
            variant="primary"
            onClick={() => ensureBridge().app.openExternal(upd.releaseUrl!)}
          >
            Abrir no GitHub
          </Button>
        )}
        {upd.state === 'downloading' && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => void ensureBridge().updater?.cancel()}
          >
            Cancelar
          </Button>
        )}
        {upd.state === 'ready' && (
          <Button
            size="sm"
            variant="primary"
            loading={acting}
            disabled={busy}
            title={busy ? 'Encerre a live/gravação para atualizar' : 'Instalar e reiniciar'}
            onClick={() => void installNow()}
          >
            Instalar e reiniciar
          </Button>
        )}
        {upd.state === 'available' && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setDismissed(true)
              update({
                updater: {
                  autoCheck: settings.updater?.autoCheck ?? true,
                  autoDownload: settings.updater?.autoDownload ?? false,
                  includePrereleases: settings.updater?.includePrereleases ?? false,
                  skippedVersion: latest
                }
              })
            }}
          >
            Depois
          </Button>
        )}
      </div>
    </div>
  )
}

/**
 * Casca do app: fica montada o tempo todo para que trocar de página não
 * derrube a live, o compartilhamento na rede local nem a conexão de espectador.
 */
function Shell(): React.JSX.Element {
  const { engine, settings, update, toast } = useStudio()
  const liveSlice = useLiveSlice()
  const recSlice = useRecorderSlice()
  const [page, setPage] = useState<Page>('studio')
  const [watching, setWatching] = useState(false)
  const [confirmQuit, setConfirmQuit] = useState(false)
  const [closingAll, setClosingAll] = useState(false)
  const broadcast = useHostBroadcast()
  const viewer = useViewerStream()

  const settingsRef = useRef(settings)
  settingsRef.current = settings

  const isBusyLiveOrRec = engine.state.live || liveSlice.active || engine.state.recorder.recording || recSlice.recording

  const requestClose = useCallback(() => {
    if (isBusyLiveOrRec) {
      setConfirmQuit(true)
    } else {
      ensureBridge().app.close()
    }
  }, [isBusyLiveOrRec])

  const quitAfterStopping = async (): Promise<void> => {
    setClosingAll(true)
    try {
      if (engine.state.live || liveSlice.active) {
        await engine.stopLive().catch(() => {})
      }
      if (engine.state.recorder.recording || recSlice.recording) {
        await ensureBridge().recorder.stop().catch(() => {})
      }
      if (broadcast.info) {
        await broadcast.stop().catch(() => {})
      }
    } finally {
      setClosingAll(false)
      setConfirmQuit(false)
      ensureBridge().app.close(true)
    }
  }

  // Listener de evento nativo `app:confirm-quit` vindo do Tauri/Rust quando fechar janela pela barra do SO.
  useEffect(() => {
    const onConfirmQuitEvent = (): void => {
      if (isBusyLiveOrRec) setConfirmQuit(true)
      else ensureBridge().app.close(true)
    }
    window.addEventListener('app:confirm-quit', onConfirmQuitEvent)
    let offBackend: (() => void) | undefined
    const tauri = window.__TAURI_INTERNALS__
    if (tauri) {
      let active = true
      const handlerId = tauri.transformCallback(() => {
        if (active) onConfirmQuitEvent()
      })
      void tauri.invoke('plugin:event|listen', {
        event: 'app:confirm-quit',
        target: { kind: 'Any' },
        handler: handlerId
      })
      offBackend = () => {
        active = false
      }
    }
    return () => {
      window.removeEventListener('app:confirm-quit', onConfirmQuitEvent)
      offBackend?.()
    }
  }, [isBusyLiveOrRec])

  // Atalhos globais (funcionam com o jogo em foco) usando ref para não recriar o listener.
  useEffect(() => {
    return ensureBridge().app.onHotkey((action) => {
      const cur = settingsRef.current
      const mixer = cur.audioMixer
      const studio = cur.studio
      const toggleKind = (kind: 'mic' | 'desktop', on: string, off: string): void => {
        const chans = mixer.channels.filter((c) => c.kind === kind)
        if (chans.length === 0) return
        const mute = chans.some((c) => !c.muted)
        update({
          audioMixer: { ...mixer, channels: mixer.channels.map((c) => (c.kind === kind ? { ...c, muted: mute } : c)) }
        })
        toast(mute ? off : on)
      }
      switch (action) {
        case 'toggle-mic-mute':
          toggleKind('mic', 'Microfone ligado', 'Microfone mudo')
          break
        case 'toggle-system-mute':
          toggleKind('desktop', 'Áudio do PC ligado', 'Áudio do PC mudo')
          break
        case 'toggle-camera':
          update({ studio: { ...studio, cameraEnabled: !studio.cameraEnabled } })
          toast(studio.cameraEnabled ? 'Câmera desligada — molduras escondidas' : 'Câmera ligada')
          break
        case 'toggle-privacy-shield':
          update({ studio: { ...studio, privacyShieldActive: !studio.privacyShieldActive } })
          break
        case 'toggle-vertical-preview':
          update({ studio: { ...studio, dualOutputEnabled: !studio.dualOutputEnabled } })
          break
        case 'save-replay-clip':
          void engine.saveReplay().then((r) => toast(r.message, r.ok ? 'good' : 'bad'))
          break
      }
    })
  }, [engine, update, toast])

  const connectToHost = async (target: ConnectTarget, pin: string | null): Promise<string | null> => {
    const result = await viewer.connect(target, pin, settingsRef.current.viewerName)
    if (!result.ok) return result.message
    setWatching(true)
    return null
  }

  if (watching) {
    return (
      <div className="app">
        <ViewerPlayer
          viewer={viewer}
          onLeave={() => {
            viewer.disconnect()
            setWatching(false)
          }}
        />
        <Modal
          open={viewer.closed !== null}
          width={400}
          icon={<IconAlert size={19} />}
          title={viewer.closed?.reason === 'kicked' ? 'Você foi removido' : 'Transmissão encerrada'}
          description={viewer.closed?.message}
          footer={
            <Button
              variant="primary"
              onClick={() => {
                viewer.dismissClosed()
                setWatching(false)
              }}
            >
              Voltar ao estúdio
            </Button>
          }
        />
      </div>
    )
  }

  return (
    <div className="app">
      <Titlebar
        onRequestClose={requestClose}
        center={<TitlebarStatus lanActive={broadcast.info !== null} onClick={() => setPage('studio')} />}
      />
      <UpdateBanner busy={isBusyLiveOrRec} onOpenSettings={() => setPage('settings')} />
      <div className="shell">
        <Rail
          page={page}
          onChange={setPage}
          live={engine.state.live || liveSlice.active}
          lan={broadcast.info !== null}
        />
        <main className="shell-page">
          {page === 'studio' && <StudioScreen onOpenDestinations={() => setPage('destinations')} />}
          {page === 'widgets' && <WidgetsScreen onDone={() => setPage('studio')} />}
          {page === 'destinations' && <DestinationsScreen />}
          {page === 'lan' && (
            <LanScreen broadcast={broadcast} onWatch={() => setPage('lan-watch')} />
          )}
          {page === 'lan-watch' && (
            <ViewerBrowse
              settings={settings}
              onSettingsChange={update}
              onBack={() => setPage('lan')}
              onConnect={connectToHost}
            />
          )}
          {page === 'settings' && <SettingsScreen />}
        </main>
      </div>

      <Modal
        open={confirmQuit}
        onClose={closingAll ? undefined : () => setConfirmQuit(false)}
        width={420}
        icon={<IconAlert size={19} />}
        title="Você ainda está ao vivo ou gravando"
        description="Fechar o PrismCast agora vai encerrar sua transmissão e finalizar o arquivo de gravação."
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirmQuit(false)} disabled={closingAll}>
              Continuar no estúdio
            </Button>
            <Button variant="danger" loading={closingAll} onClick={() => void quitAfterStopping()}>
              Encerrar tudo e fechar
            </Button>
          </>
        }
      />
    </div>
  )
}

export function AppFallback({ error }: { error: Error }): React.JSX.Element {
  return (
    <div className="app">
      <Titlebar />
      <div className="screen boot">
        <div className="empty-state">
          <IconPrismCastMark size={36} />
          <span className="empty-title">O app encontrou um erro</span>
          <span className="mono selectable">{error.message}</span>
        </div>
      </div>
    </div>
  )
}
