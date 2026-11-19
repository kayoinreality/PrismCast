import { Component, StrictMode, type ErrorInfo, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { App, AppFallback } from './App'
import { ChatWindow } from './chat/ChatWindow'
import './styles/global.css'

class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null }

  static getDerivedStateFromError(error: Error): { error: Error } {
    return { error }
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error('[ErrorBoundary] Erro capturado na interface:', error, info.componentStack)
  }

  render(): ReactNode {
    if (this.state.error) {
      return <AppFallback error={this.state.error} />
    }
    return this.props.children
  }
}

const container = document.getElementById('root')
if (!container) throw new Error('Elemento #root não encontrado.')

// A mesma interface abre a janela separada do chat (#chat).
const isChatWindow = window.location.hash === '#chat'
if (isChatWindow) document.title = 'Chat · PrismCast'

createRoot(container).render(
  <StrictMode>
    <ErrorBoundary>{isChatWindow ? <ChatWindow /> : <App />}</ErrorBoundary>
  </StrictMode>
)
