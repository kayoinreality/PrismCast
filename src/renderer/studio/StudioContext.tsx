import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode
} from 'react'
import type { AppSettings } from '@shared/ipc'
import type { Scene, WidgetKit } from '@shared/protocol'
import { ensureBridge } from '@/bridge'
import { StudioEngine, type EngineState } from './StudioEngine'

interface StudioCtx {
  engine: StudioEngine
  settings: AppSettings
  update: (patch: Partial<AppSettings>) => void
  flushSettings: () => void
  kit: WidgetKit | null
  reloadKit: () => void
  toast: (message: string, tone?: ToastTone, action?: ToastAction) => void
  /** Registra um snapshot manual antes de uma exclusão destrutiva. */
  pushUndoSnapshot: (scenes?: Scene[]) => void
  /** Ctrl+Z das cenas e fontes. Devolve false se não havia o que desfazer. */
  undo: () => boolean
  redo: () => boolean
}

export interface ToastAction {
  label: string
  run: () => void
}

export type ToastTone = 'info' | 'good' | 'bad'

interface ToastEntry {
  id: number
  message: string
  tone: ToastTone
  action?: ToastAction
}

const Ctx = createContext<StudioCtx | null>(null)

let singleton: StudioEngine | null = null

function getOrCreateEngine(settings: AppSettings): StudioEngine {
  if (!singleton) {
    singleton = new StudioEngine(settings)
    window.addEventListener(
      'beforeunload',
      () => {
        singleton?.dispose()
        singleton = null
      },
      { once: true }
    )
  }
  return singleton
}

/** Viewport isolado de toasts para que notificações não re-renderizem o estúdio inteiro. */
export function ToastViewport({
  toasts,
  onDismiss
}: {
  toasts: ToastEntry[]
  onDismiss: (id: number) => void
}): React.JSX.Element {
  return (
    <div className="toasts" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className={`toast toast-${t.tone}`}>
          <span>{t.message}</span>
          {t.action && (
            <button
              className="toast-action"
              onClick={() => {
                t.action!.run()
                onDismiss(t.id)
              }}
            >
              {t.action.label}
            </button>
          )}
        </div>
      ))}
    </div>
  )
}

export function StudioProvider({
  settings,
  update,
  flushSettings,
  children
}: {
  settings: AppSettings
  update: (patch: Partial<AppSettings>) => void
  flushSettings?: () => void
  children: ReactNode
}): React.JSX.Element {
  const [engine] = useState(() => getOrCreateEngine(settings))
  const [kit, setKit] = useState<WidgetKit | null>(null)
  const [toasts, setToasts] = useState<ToastEntry[]>([])
  const nextToast = useRef(0)
  const toastTimers = useRef<Map<number, number>>(new Map())
  const settingsRef = useRef(settings)
  settingsRef.current = settings

  useEffect(() => engine.apply(settings), [engine, settings])

  useEffect(() => {
    const timers = toastTimers.current
    return () => {
      for (const id of timers.values()) window.clearTimeout(id)
      timers.clear()
    }
  }, [])

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
      }
      h.future = []
      h.at = Date.now()
    }
    h.last = now
  }, [settings.scenes.scenes])

  const pushUndoSnapshot = useCallback((scenes?: Scene[]) => {
    const h = history.current
    const snap = scenes ?? settingsRef.current.scenes.scenes
    h.past = [...h.past.slice(-49), snap]
    h.future = []
    h.at = 0
  }, [])

  const restore = useCallback(
    (scenes: Scene[]) => {
      const st = settingsRef.current.scenes
      const ids = new Set(scenes.map((s) => s.id))
      if (scenes !== history.current.last) {
        history.current.restoring = true
      }
      update({
        scenes: {
          ...st,
          scenes,
          activeSceneId: ids.has(st.activeSceneId) ? st.activeSceneId : (scenes[0]?.id ?? ''),
          previewSceneId: ids.has(st.previewSceneId) ? st.previewSceneId : (scenes[0]?.id ?? '')
        }
      })
    },
    [update]
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
    let active = true
    void ensureBridge()
      .kit.load(settingsRef.current.kitPath)
      .then((k) => {
        if (!active) return
        setKit(k)
        engine.setKit(k)
      })
      .catch(() => {})
    return () => {
      active = false
    }
  }, [engine])

  useEffect(() => {
    let active = true
    void ensureBridge()
      .kit.load(settings.kitPath)
      .then((k) => {
        if (!active) return
        setKit(k)
        engine.setKit(k)
      })
      .catch(() => {})
    return () => {
      active = false
    }
  }, [settings.kitPath, engine])

  const dismissToast = useCallback((id: number) => {
    const t = toastTimers.current.get(id)
    if (t !== undefined) {
      window.clearTimeout(t)
      toastTimers.current.delete(id)
    }
    setToasts((prev) => prev.filter((x) => x.id !== id))
  }, [])

  const toast = useCallback(
    (message: string, tone: ToastTone = 'info', action?: ToastAction) => {
      const id = ++nextToast.current
      setToasts((prev) => [...prev.slice(-2), { id, message, tone, action }])
      const timerId = window.setTimeout(() => {
        toastTimers.current.delete(id)
        setToasts((prev) => prev.filter((t) => t.id !== id))
      }, action ? 8000 : 4500)
      toastTimers.current.set(id, timerId)
    },
    []
  )

  const flushSafe = useCallback(() => flushSettings?.(), [flushSettings])

  const value = useMemo<StudioCtx>(
    () => ({
      engine,
      settings,
      update,
      flushSettings: flushSafe,
      kit,
      reloadKit,
      toast,
      pushUndoSnapshot,
      undo,
      redo
    }),
    [engine, settings, update, flushSafe, kit, reloadKit, toast, pushUndoSnapshot, undo, redo]
  )

  return (
    <Ctx.Provider value={value}>
      {children}
      <ToastViewport toasts={toasts} onDismiss={dismissToast} />
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
