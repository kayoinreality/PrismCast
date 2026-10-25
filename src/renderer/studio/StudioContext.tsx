import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode
} from 'react'
import type { AppSettings } from '@shared/ipc'
import type { WidgetKit } from '@shared/protocol'
import { StudioEngine, type EngineState } from './StudioEngine'

interface StudioCtx {
  engine: StudioEngine
  settings: AppSettings
  update: (patch: Partial<AppSettings>) => void
  kit: WidgetKit | null
  reloadKit: () => void
  toast: (message: string, tone?: ToastTone) => void
}

export type ToastTone = 'info' | 'good' | 'bad'

const Ctx = createContext<StudioCtx | null>(null)

/**
 * Um motor por janela, para a vida toda dela. Não pode morar só no estado do
 * React: o StrictMode desmonta e remonta em dev, e isso derrubaria capturas e a live.
 */
let singleton: StudioEngine | null = null

export function StudioProvider({
  settings,
  update,
  children
}: {
  settings: AppSettings
  update: (patch: Partial<AppSettings>) => void
  children: ReactNode
}): React.JSX.Element {
  const [engine] = useState(() => (singleton ??= new StudioEngine(settings)))
  const [kit, setKit] = useState<WidgetKit | null>(null)
  const [toasts, setToasts] = useState<Array<{ id: number; message: string; tone: ToastTone }>>([])
  const nextToast = useRef(0)

  useEffect(() => engine.apply(settings), [engine, settings])

  const reloadKit = useCallback(() => {
    void window.api.kit.load(settings.kitPath).then(setKit)
  }, [settings.kitPath])
  useEffect(reloadKit, [reloadKit])

  const toast = useCallback((message: string, tone: ToastTone = 'info') => {
    const id = ++nextToast.current
    setToasts((prev) => [...prev.slice(-2), { id, message, tone }])
    window.setTimeout(() => setToasts((prev) => prev.filter((t) => t.id !== id)), 4500)
  }, [])

  return (
    <Ctx.Provider value={{ engine, settings, update, kit, reloadKit, toast }}>
      {children}
      <div className="toasts" aria-live="polite">
        {toasts.map((t) => (
          <div key={t.id} className={`toast toast-${t.tone}`}>
            {t.message}
          </div>
        ))}
      </div>
    </Ctx.Provider>
  )
}

export function useStudio(): StudioCtx {
  const ctx = useContext(Ctx)
  if (!ctx) throw new Error('useStudio fora do StudioProvider')
  return ctx
}

export function useEngineState(): EngineState {
  const { engine } = useStudio()
  return useSyncExternalStore(engine.subscribe, engine.getState)
}
