import { useEffect, useState } from 'react'
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
import { StudioProvider, useEngineState, useStudio } from '@/studio/StudioContext'
import { LiveStatus } from '@/components/LiveStatus'
import './App.css'

export function App(): React.JSX.Element {
  const { settings, update } = useSettings()

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
    <StudioProvider settings={settings} update={update}>
      <Shell />
    </StudioProvider>
  )
}

/**
 * Casca do app: fica montada o tempo todo para que trocar de página não
 * derrube a live, o compartilhamento na rede local nem a conexão de espectador.
 */
function Shell(): React.JSX.Element {
  const { engine, settings, update, toast } = useStudio()
  const state = useEngineState()
  const [page, setPage] = useState<Page>('studio')
  const [watching, setWatching] = useState(false)
  const broadcast = useHostBroadcast()
  const viewer = useViewerStream()

  // Atalhos globais (funcionam com o jogo em foco).
  useEffect(() => {
    return window.api.app.onHotkey((action) => {
      const mixer = settings.audioMixer
      const studio = settings.studio
      /** Muta/desmuta todos os canais de um tipo (vários microfones contam juntos). */
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
  }, [engine, settings.audioMixer, settings.studio, update, toast])

  const connectToHost = async (target: ConnectTarget, pin: string | null): Promise<string | null> => {
    const result = await viewer.connect(target, pin, settings.viewerName)
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
        center={
          <LiveStatus
            state={state}
            destinations={settings.destinations}
            lanViewers={broadcast.info ? broadcast.viewers.length : null}
            onClick={() => setPage('studio')}
          />
        }
      />
      <div className="shell">
        <Rail page={page} onChange={setPage} live={state.live} lan={broadcast.info !== null} />
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
