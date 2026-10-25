import { useCallback, useEffect, useState } from 'react'
import type { DesktopSource } from '@shared/ipc'
import { MAX_VIEWERS } from '@shared/protocol'
import { Titlebar } from '@/components/Titlebar'
import { Modal } from '@/components/Modal'
import { Badge, Button } from '@/components/ui'
import { IconAlert, IconBroadcast, IconUsers } from '@/components/Icons'
import { Home } from '@/screens/Home'
import { HostSetup } from '@/screens/HostSetup'
import { HostLive } from '@/screens/HostLive'
import { ViewerBrowse } from '@/screens/ViewerBrowse'
import { ViewerPlayer } from '@/screens/ViewerPlayer'
import { useSettings } from '@/hooks/useSettings'
import { useHostBroadcast } from '@/hooks/useHostBroadcast'
import { useViewerStream, type ConnectTarget } from '@/hooks/useViewerStream'
import './App.css'

type Route = 'home' | 'host-setup' | 'host-live' | 'viewer-browse' | 'viewer-player'

export function App(): React.JSX.Element {
  const [route, setRoute] = useState<Route>('host-live')
  const [hostStream, setHostStream] = useState<MediaStream | null>(null)
  const [hostSource, setHostSource] = useState<DesktopSource | null>(null)

  const { settings, update } = useSettings()
  const broadcast = useHostBroadcast()
  const viewer = useViewerStream()

  const stopBroadcast = useCallback(async (): Promise<void> => {
    await broadcast.stop()
    setRoute('host-live')
  }, [broadcast])

  useEffect(() => {
    const track = hostStream?.getVideoTracks()[0]
    if (!track || route !== 'host-live') return
    const onEnded = (): void => {
      // Se a janela capturada fechar, o compositor entra em modo standby
      console.warn('[App] A fonte capturada foi encerrada; aguardando troca de fonte.')
    }
    track.addEventListener('ended', onEnded)
    return () => track.removeEventListener('ended', onEnded)
  }, [hostStream, route])

  const startBroadcast = async (
    stream: MediaStream,
    source: DesktopSource
  ): Promise<string | null> => {
    if (!settings) return 'Preferências ainda carregando.'
    const result = await broadcast.start(stream, settings.quality, {
      sourceId: source.id,
      sourceName: source.name,
      sourceKind: source.kind,
      hostName: settings.hostName,
      requirePin: settings.requirePin,
      maxViewers: MAX_VIEWERS
    })
    if (!result.ok) return result.error
    setHostStream(stream)
    setHostSource(source)
    setRoute('host-live')
    return null
  }

  const handleLiveSourceChanged = useCallback(
    (newStream: MediaStream, newSource: DesktopSource): void => {
      setHostStream((old) => {
        if (old && old !== newStream) {
          old.getTracks().forEach((t) => t.stop())
        }
        return newStream
      })
      setHostSource(newSource)
      broadcast.notifySourceChanged(newSource.id, newSource.name, newSource.kind)
    },
    [broadcast]
  )

  const connectToHost = async (
    target: ConnectTarget,
    pin: string | null
  ): Promise<string | null> => {
    if (!settings) return 'Preferências ainda carregando.'
    const result = await viewer.connect(target, pin, settings.viewerName)
    if (!result.ok) return result.message
    setRoute('viewer-player')
    return null
  }

  const leaveViewer = (): void => {
    viewer.disconnect()
    setRoute('host-live')
  }

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

  const broadcasting = broadcast.info !== null

  return (
    <div className="app">
      {route !== 'viewer-player' && (
        <Titlebar
          center={
            broadcasting ? (
              <button className="titlebar-live" onClick={() => setRoute('host-live')}>
                <span className="live-dot" />
                <span>AO VIVO NO AR</span>
                <Badge tone="live" icon={<IconUsers />}>
                  {broadcast.viewers.length}
                </Badge>
              </button>
            ) : (
              <div style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 11, color: 'var(--text-dim)' }}>
                <span>PRISMCAST STUDIO</span>
                <Badge tone="neutral">PREPARAÇÃO DE CENAS</Badge>
              </div>
            )
          }
        />
      )}

      {route === 'home' && (
        <Home
          onShare={() => setRoute('host-live')}
          onWatch={() => setRoute('viewer-browse')}
        />
      )}

      {route === 'host-setup' && (
        <HostSetup
          settings={settings}
          onSettingsChange={update}
          onBack={() => setRoute('host-live')}
          onStart={startBroadcast}
        />
      )}

      {route === 'host-live' && (
        <HostLive
          broadcast={broadcast}
          stream={hostStream}
          sourceName={hostSource?.name ?? ''}
          settings={settings}
          onSettingsChange={update}
          onSourceSwitch={handleLiveSourceChanged}
          onStartBroadcast={startBroadcast}
          onStop={() => void stopBroadcast()}
          onOpenViewer={() => setRoute('viewer-browse')}
        />
      )}

      {route === 'viewer-browse' && (
        <ViewerBrowse
          settings={settings}
          onSettingsChange={update}
          onBack={() => setRoute('host-live')}
          onConnect={connectToHost}
        />
      )}

      {route === 'viewer-player' && <ViewerPlayer viewer={viewer} onLeave={leaveViewer} />}

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
              setRoute('host-live')
            }}
          >
            Voltar ao Studio
          </Button>
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
