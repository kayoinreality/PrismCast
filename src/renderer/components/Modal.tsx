import { useEffect, useId, useRef, type ReactNode } from 'react'
import { IconButton } from './ui'
import { IconClose } from './Icons'
import './Modal.css'

/**
 * Pilha global de modais abertos: garante que `Escape` feche apenas o modal
 * no topo da pilha (ex.: `AppPickerModal` aberto sobre `AudioPropertiesModal`).
 */
const modalStack: number[] = []
let nextModalId = 1

const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])'
].join(', ')

function getFocusable(container: HTMLElement | null): HTMLElement[] {
  if (!container) return []
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (el) => el.offsetParent !== null || el === document.activeElement
  )
}

export function Modal({
  open,
  title,
  description,
  icon,
  onClose,
  children,
  footer,
  width = 460
}: {
  open: boolean
  title: string
  description?: ReactNode
  icon?: ReactNode
  onClose?: () => void
  children?: ReactNode
  footer?: ReactNode
  width?: number
}): React.JSX.Element | null {
  const titleId = useId()
  const descId = useId()
  const dialogRef = useRef<HTMLDivElement>(null)
  const onCloseRef = useRef(onClose)
  onCloseRef.current = onClose

  useEffect(() => {
    if (!open) return

    const id = nextModalId++
    modalStack.push(id)
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null

    // Se nada dentro do modal recebeu foco via autoFocus, foca o próprio diálogo.
    const focusTimer = window.requestAnimationFrame(() => {
      const dialog = dialogRef.current
      if (!dialog) return
      if (!dialog.contains(document.activeElement)) {
        const focusables = getFocusable(dialog)
        ;(focusables[0] ?? dialog).focus()
      }
    })

    const onKey = (e: KeyboardEvent): void => {
      if (modalStack[modalStack.length - 1] !== id) return

      if (e.key === 'Escape') {
        if (onCloseRef.current) {
          e.preventDefault()
          e.stopPropagation()
          onCloseRef.current()
        }
        return
      }

      if (e.key === 'Tab') {
        const dialog = dialogRef.current
        const focusables = getFocusable(dialog)
        if (focusables.length === 0) {
          e.preventDefault()
          dialog?.focus()
          return
        }
        const first = focusables[0]
        const last = focusables[focusables.length - 1]
        const active = document.activeElement as HTMLElement | null
        if (e.shiftKey) {
          if (!active || active === first || !dialog?.contains(active)) {
            e.preventDefault()
            last.focus()
          }
        } else if (!active || active === last || !dialog?.contains(active)) {
          e.preventDefault()
          first.focus()
        }
      }
    }

    window.addEventListener('keydown', onKey, true)
    return () => {
      window.cancelAnimationFrame(focusTimer)
      window.removeEventListener('keydown', onKey, true)
      const idx = modalStack.lastIndexOf(id)
      if (idx !== -1) modalStack.splice(idx, 1)
      if (previousFocus && previousFocus.isConnected && modalStack.length === 0) {
        previousFocus.focus()
      }
    }
  }, [open])

  if (!open) return null

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div
        ref={dialogRef}
        className="modal"
        style={{ maxWidth: width }}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={description ? descId : undefined}
        tabIndex={-1}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          {icon && <div className="modal-icon">{icon}</div>}
          <div className="modal-heading">
            <h2 id={titleId} className="modal-title">
              {title}
            </h2>
            {description && (
              <div id={descId} className="modal-desc">
                {description}
              </div>
            )}
          </div>
          {onClose && (
            <IconButton label="Fechar" onClick={onClose}>
              <IconClose size={17} />
            </IconButton>
          )}
        </div>

        {children && <div className="modal-body">{children}</div>}
        {footer && <div className="modal-foot">{footer}</div>}
      </div>
    </div>
  )
}
