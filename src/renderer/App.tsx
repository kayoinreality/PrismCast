import { useCallback, useEffect, useRef, useState } from 'react'
import { Titlebar } from '@/components/Titlebar'
import { Modal } from '@/components/Modal'
import { Rail, type Page } from '@/components/Rail'
import { Button } from '@/components/ui'
import { IconAlert, IconBroadcast } from '@/components/Icons'
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
import { useLiveSlice, useRecorderSlice, useLanSlice, studioStore } from '@/studio/StudioStore'
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
          <IconBroadcast size={30} />
          <span className="empty-title">O app encontrou um erro</span>
          <span className="mono selectable">{error.message}</span>
        </div>
      </div>
    </div>
  )
}
