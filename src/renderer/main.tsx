import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
import { ChatWindow } from './chat/ChatWindow'
import './styles/global.css'

const container = document.getElementById('root')
if (!container) throw new Error('Elemento #root não encontrado.')

// A mesma interface abre a janela separada do chat (#chat).
const isChatWindow = window.location.hash === '#chat'
if (isChatWindow) document.title = 'Chat · PrismCast'

createRoot(container).render(<StrictMode>{isChatWindow ? <ChatWindow /> : <App />}</StrictMode>)
