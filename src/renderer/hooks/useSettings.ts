import { useCallback, useEffect, useRef, useState } from 'react'
import type { AppSettings } from '@shared/ipc'
import { ensureBridge } from '@/bridge'

export interface SettingsResult {
  settings: AppSettings | null
  update(patch: Partial<AppSettings>): void
  flush(): void
}

/** Espera este tempo parado antes de escrever no disco. */
const FLUSH_DELAY_MS = 250

/**
 * Preferências persistidas via backend autoritativo Rust.
 *
 * A escrita é otimista e coalescida com debounce curto para arraste contínuo no palco/slider,
 * com `flush()` explícito antes de operações críticas (ex.: entrar ao vivo) e `pagehide`.
 */
export function useSettings(): SettingsResult {
  const [settings, setSettings] = useState<AppSettings | null>(null)
  const pending = useRef<Partial<AppSettings>>({})
  const timer = useRef<number>(0)

  useEffect(() => {
    let alive = true
    const api = ensureBridge()
    void api.settings
      .get()
      .then((s) => {
        if (alive) setSettings(s)
      })
      .catch((err) => {
        console.warn('[settings] falha ao carregar preferências:', err)
      })
    const off = api.settings.onChanged((fromMain) => {
      setSettings({ ...fromMain, ...pending.current })
    })
    return () => {
      alive = false
      off()
    }
  }, [])

  const flush = useCallback((): void => {
    window.clearTimeout(timer.current)
    const patch = pending.current
    pending.current = {}
    if (Object.keys(patch).length > 0) {
      void ensureBridge()
        .settings.set(patch)
        .catch((err) => {
          console.warn('[settings] falha ao persistir preferências:', err)
        })
    }
  }, [])

  useEffect(() => {
    const onHide = (): void => flush()
    window.addEventListener('pagehide', onHide)
    return () => {
      window.removeEventListener('pagehide', onHide)
      flush()
    }
  }, [flush])

  const update = useCallback(
    (patch: Partial<AppSettings>): void => {
      setSettings((prev) => (prev ? { ...prev, ...patch } : prev))
      pending.current = { ...pending.current, ...patch }
      window.clearTimeout(timer.current)
      timer.current = window.setTimeout(flush, FLUSH_DELAY_MS)
    },
    [flush]
  )

  return { settings, update, flush }
}
