import { useDiscovery } from '@/hooks/useDiscovery'
import { Badge } from '@/components/ui'
import {
  IconBroadcast,
  IconEye,
  IconGlobe,
  IconLayers,
  IconMic,
  IconSmartphone,
  IconUsers,
  IconWifi
} from '@/components/Icons'
import './Home.css'

export function Home({
  onShare,
  onWatch
}: {
  onShare: () => void
  onWatch: () => void
}): React.JSX.Element {
  const { hosts } = useDiscovery(true)

  return (
    <div className="screen home fade-in">
      <div className="home-inner">
        <header className="home-head">
          <div className="home-pills">
            <Badge tone="live">Core Nativo em Rust</Badge>
            <Badge tone="accent" icon={<IconSmartphone size={12} />}>
              Dual Output 16:9 + 9:16
            </Badge>
            <Badge tone="neutral" icon={<IconGlobe size={12} />}>
              Twitch · YouTube · Kick · TikTok
            </Badge>
          </div>
          <h1 className="home-title">PrismCast Studio</h1>
          <p className="home-sub">
            Estúdio completo de transmissão P2P na LAN com latência zero, Multistreaming RTMP,
            Tela Vertical 9:16, Mixer de Áudio Duplo, Replay Buffer e Widgets StreamElements.
          </p>
        </header>

        <div className="home-cards">
          <button className="home-card home-card-share" onClick={onShare}>
            <span className="home-card-icon">
              <IconBroadcast size={26} />
            </span>
            <span className="home-card-title">Abrir Estúdio & Transmitir</span>
            <span className="home-card-desc">
              Capture tela ou janela com Webcam PiP, mixe áudio do sistema + microfone, adicione
              overlays do StreamElements e transmita na LAN e para Twitch, YouTube, Kick e TikTok.
            </span>
            <span className="home-card-foot">
              <Badge tone="accent" icon={<IconMic size={12} />}>
                Mixer Sistema + Mic · Replay 30s
              </Badge>
              <Badge tone="neutral" icon={<IconLayers size={12} />}>
                Cenas & Widgets
              </Badge>
            </span>
          </button>

          <button className="home-card home-card-watch" onClick={onWatch}>
            <span className="home-card-icon">
              <IconEye size={26} />
            </span>
            <span className="home-card-title">Assistir na Rede Local</span>
            <span className="home-card-desc">
              Conecte-se instantaneamente a uma transmissão na mesma rede com áudio Opus estéreo,
              Chat ao vivo, Reações flutuantes e Retorno de Voz (Party Voice).
            </span>
            <span className="home-card-foot">
              {hosts.length > 0 ? (
                <Badge tone="live" icon={<IconUsers />}>
                  {hosts.length === 1
                    ? '1 estúdio ao vivo agora'
                    : `${hosts.length} estúdios ao vivo agora`}
                </Badge>
              ) : (
                <Badge tone="neutral" icon={<IconWifi />}>
                  Procurando estúdios na LAN
                </Badge>
              )}
            </span>
          </button>
        </div>

        <p className="home-note">
          Atalhos Globais: <code>Ctrl+Shift+M</code> Mutar Mic · <code>Ctrl+Shift+D</code> Mutar
          Sistema · <code>Ctrl+Shift+P</code> Modo Volto Já · <code>Ctrl+Shift+S</code> Salvar
          Replay 30s · <code>Ctrl+Shift+V</code> Preview 9:16
        </p>
      </div>
    </div>
  )
}
