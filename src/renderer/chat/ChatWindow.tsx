import { IconChat, IconClose, IconMinimize } from '@/components/Icons'
import { useSettings } from '@/hooks/useSettings'
import { ChatPanel } from './ChatPanel'
import '@/components/Titlebar.css'

/** Janela separada do chat: arraste para o segundo monitor ou deixe por cima do jogo. */
export function ChatWindow(): React.JSX.Element {
  const { settings, update } = useSettings()
  return (
    <div className="chat-window">
      <div className="chat-window-bar">
        <IconChat size={14} />
        <span>Chat · PrismCast</span>
        <div className="spacer" />
        <button className="win-btn" onClick={() => window.api.app.minimize()} aria-label="Minimizar" title="Minimizar">
          <IconMinimize size={14} />
        </button>
        <button className="win-btn win-close" onClick={() => window.api.app.close()} aria-label="Fechar" title="Fechar">
          <IconClose size={14} />
        </button>
      </div>
      {settings ? (
        <ChatPanel settings={settings} update={update} variant="window" />
      ) : (
        <div className="chat-empty">
          <span className="spinner" />
        </div>
      )}
    </div>
  )
}
