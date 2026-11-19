import { useEffect, useRef, useState } from 'react'
import type { FirewallStatus } from '@shared/ipc'
import { Modal } from './Modal'
import { Button } from './ui'
import { IconAlert, IconCheck, IconCopy, IconShield } from './Icons'
import './FirewallModal.css'

/**
 * Quem transmite roda um servidor, e o Windows bloqueia isso por padrao. Quem
 * so assiste faz conexao de saida e nao precisa de nada — vale dizer isso na
 * tela, senao as duas pontas vao mexer no firewall sem necessidade.
 */
export function FirewallModal({
  open,
  status,
  onDone,
  onSkip
}: {
  open: boolean
  status: FirewallStatus | null
  onDone: () => void
  onSkip: () => void
}): React.JSX.Element | null {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const [manual, setManual] = useState(false)
  const copyTimer = useRef<number>(0)

  useEffect(() => () => window.clearTimeout(copyTimer.current), [])

  if (!status) return null

  const authorize = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      const result = await window.api.firewall.authorize()
      if (result.ok) onDone()
      else setError(result.error ?? 'Não foi possível criar a regra.')
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Não foi possível criar a regra.')
    } finally {
      setBusy(false)
    }
  }

  const copy = async (): Promise<void> => {
    try {
      await navigator.clipboard.writeText(status.command)
      setCopied(true)
      window.clearTimeout(copyTimer.current)
      copyTimer.current = window.setTimeout(() => setCopied(false), 1800)
    } catch {
      /* fallback silencioso se área de transferência falhar */
    }
  }

  return (
    <Modal
      open={open}
      width={520}
      icon={<IconShield size={20} />}
      title="Liberar o app no firewall do Windows"
      description="Sem isso o Windows bloqueia as conexões de entrada e ninguém consegue encontrar nem entrar na sua transmissão."
      onClose={busy ? undefined : onSkip}
      footer={
        <>
          <Button variant="ghost" onClick={onSkip} disabled={busy}>
            Tentar assim mesmo
          </Button>
          <Button variant="primary" onClick={() => void authorize()} loading={busy} icon={<IconShield size={17} />}>
            Liberar agora
          </Button>
        </>
      }
    >
      <div className="fw-body">
        <p className="fw-line">
          O Windows vai pedir sua confirmação. Só quem transmite precisa disso —
          quem apenas assiste não precisa mexer em nada.
        </p>

        {status.profile === 'public' && (
          <div className="fw-warn">
            <IconAlert size={17} />
            <span>
              Uma das suas redes ativas está como <strong>Pública</strong> — normalmente é
              um adaptador de VPN. A regra cobre esse perfil, então vai funcionar; só
              lembre que quem estiver nessa rede vê sua transmissão na lista. O
              <strong> código de acesso</strong> é o que a protege.
            </span>
          </div>
        )}

        {error && (
          <div className="fw-error">
            <IconAlert size={17} />
            <span>{error}</span>
          </div>
        )}

        <button className="fw-toggle" onClick={() => setManual((m) => !m)}>
          {manual ? 'Esconder' : 'Prefiro fazer manualmente'}
        </button>

        {manual && (
          <div className="fw-manual">
            <p className="fw-line dim">
              Abra o Prompt de Comando <strong>como administrador</strong> e cole:
            </p>
            <div className="fw-command">
              <code className="mono selectable">{status.command}</code>
              <Button
                size="sm"
                variant="subtle"
                onClick={() => void copy()}
                icon={copied ? <IconCheck size={15} /> : <IconCopy size={15} />}
              >
                {copied ? 'Copiado' : 'Copiar'}
              </Button>
            </div>
          </div>
        )}
      </div>
    </Modal>
  )
}
