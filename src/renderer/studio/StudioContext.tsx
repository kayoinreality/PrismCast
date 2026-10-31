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
import type { Scene, WidgetKit } from '@shared/protocol'
import { StudioEngine, type EngineState } from './StudioEngine'

interface StudioCtx {
  engine: StudioEngine
  settings: AppSettings
  update: (patch: Partial<AppSettings>) => void
  kit: WidgetKit | null
  reloadKit: () => void
  toast: (message: string, tone?: ToastTone, action?: ToastAction) => void
  /** Ctrl+Z das cenas e fontes. Devolve false se não havia o que desfazer. */
  undo: () => boolean
  redo: () => boolean
}

export interface ToastAction {
  label: string
  run: () => void
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
  const [toasts, setToasts] = useState<Array<{ id: number; message: string; tone: ToastTone; action?: ToastAction }>>([])
  const nextToast = useRef(0)

  useEffect(() => engine.apply(settings), [engine, settings])

  // Histórico das cenas: mudanças seguidas (arrastar uma fonte) viram um passo só.
  // Desfazer não troca a cena que está no ar, só o conteúdo das cenas.
  const history = useRef<{ past: Scene[][]; future: Scene[][]; last: Scene[]; at: number; restoring: boolean }>({
    past: [],
    future: [],
    last: settings.scenes.scenes,
    at: 0,
    restoring: false
  })
  useEffect(() => {
    const h = history.current
    const now = settings.scenes.scenes
    if (now === h.last) return
    if (h.restoring) {
      h.restoring = false
    } else {
      if (Date.now() - h.at > 700) {
        h.past = [...h.past.slice(-49), h.last]
        h.future = []
      }
      h.at = Date.now()
    }
    h.last = now
  }, [settings.scenes.scenes])

  const restore = useCallback(
    (scenes: Scene[]) => {
      const st = settings.scenes
      const ids = new Set(scenes.map((s) => s.id))
      history.current.restoring = true
      update({
        scenes: {
          ...st,
          scenes,
          activeSceneId: ids.has(st.activeSceneId) ? st.activeSceneId : (scenes[0]?.id ?? ''),
          previewSceneId: ids.has(st.previewSceneId) ? st.previewSceneId : (scenes[0]?.id ?? '')
        }
      })
    },
    [settings.scenes, update]
  )

  const undo = useCallback((): boolean => {
    const h = history.current
    const prev = h.past.pop()
    if (!prev) return false
    h.future.push(h.last)
    h.at = 0
    restore(prev)
    return true
  }, [restore])

  const redo = useCallback((): boolean => {
    const h = history.current
    const next = h.future.pop()
    if (!next) return false
    h.past.push(h.last)
    h.at = 0
    restore(next)
    return true
  }, [restore])

  const reloadKit = useCallback(() => {
    void window.api.kit.load(settings.kitPath).then((k) => {
      setKit(k)
      engine.setKit(k)
    })
  }, [settings.kitPath, engine])
  useEffect(reloadKit, [reloadKit])

  const toast = useCallback((message: string, tone: ToastTone = 'info', action?: ToastAction) => {
    const id = ++nextToast.current
    setToasts((prev) => [...prev.slice(-2), { id, message, tone, action }])
    // Com botão (ex.: Desfazer) fica mais tempo: dá para ler e reagir.
    window.setTimeout(() => setToasts((prev) => prev.filter((t) => t.id !== id)), action ? 8000 : 4500)
  }, [])

  return (
    <Ctx.Provider value={{ engine, settings, update, kit, reloadKit, toast, undo, redo }}>
      {children}
      <div className="toasts" aria-live="polite">
        {toasts.map((t) => (
          <div key={t.id} className={`toast toast-${t.tone}`}>
            <span>{t.message}</span>
            {t.action && (
              <button
                className="toast-action"
                onClick={() => {
                  t.action!.run()
                  setToasts((prev) => prev.filter((x) => x.id !== t.id))
                }}
              >
                {t.action.label}
              </button>
            )}
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
