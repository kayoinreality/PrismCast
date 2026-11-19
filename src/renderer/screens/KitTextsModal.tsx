import { useEffect, useState } from 'react'
import type { KitConfig } from '@shared/protocol'
import { Modal } from '@/components/Modal'
import { Button, Field, IconButton, TextInput } from '@/components/ui'
import { IconEdit, IconPlus, IconTrash } from '@/components/Icons'
import { useStudio } from '@/studio/StudioContext'

const REDES = ['twitch', 'youtube', 'tiktok', 'instagram', 'x', 'discord', 'kick']

interface RowItem<T> {
  uid: string
  value: T
}

let nextRowUid = 1
const wrapRows = <T,>(items: T[]): Array<RowItem<T>> =>
  items.map((value) => ({ uid: `row-${nextRowUid++}`, value }))

/**
 * Textos do kit (src/config.js): nome do canal, jogo do dia, agenda, recados
 * e redes. Valem para todas as telas de uma vez e entram na live na hora.
 */
export function KitTextsModal({ open, onClose }: { open: boolean; onClose: () => void }): React.JSX.Element {
  const { kit, engine, toast } = useStudio()
  const [config, setConfig] = useState<KitConfig | null>(null)
  const [contagemDraft, setContagemDraft] = useState('5')
  const [recadoRows, setRecadoRows] = useState<Array<RowItem<string>>>([])
  const [redeRows, setRedeRows] = useState<Array<RowItem<{ rede: string; handle: string }>>>([])
  const [missing, setMissing] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (!open || !kit) return
    let cancelled = false
    setConfig(null)
    setMissing(false)
    setLoadError(null)
    void window.api.kit
      .readConfig(kit.path)
      .then((c) => {
        if (cancelled) return
        setConfig(c)
        setMissing(c === null)
        if (c) {
          setContagemDraft(String(c.contagem))
          setRecadoRows(wrapRows(c.recados))
          setRedeRows(wrapRows(c.redes))
        }
      })
      .catch((err) => {
        if (cancelled) return
        setLoadError(err instanceof Error ? err.message : 'Não foi possível ler src/config.js.')
      })
    return () => {
      cancelled = true
    }
  }, [open, kit])

  const set = (p: Partial<KitConfig>): void => setConfig((c) => (c ? { ...c, ...p } : c))

  const save = async (): Promise<void> => {
    if (!kit || !config) return
    setSaving(true)
    try {
      const payload: KitConfig = {
        ...config,
        contagem: Math.max(1, Math.min(120, Number(contagemDraft) || config.contagem || 5)),
        recados: recadoRows.map((r) => r.value),
        redes: redeRows.map((r) => r.value)
      }
      const res = await window.api.kit.writeConfig(kit.path, payload)
      if (!res.ok) {
        toast(`Não deu para salvar: ${res.error}`, 'bad')
        return
      }
      engine.reloadKitWidgets()
      toast('Textos do kit salvos — os widgets já atualizaram', 'good')
      onClose()
    } catch (err) {
      toast(`Não deu para salvar: ${err instanceof Error ? err.message : 'erro inesperado'}`, 'bad')
    } finally {
      setSaving(false)
    }
  }

  return (
    <Modal
      open={open}
      onClose={saving ? undefined : onClose}
      width={640}
      icon={<IconEdit size={19} />}
      title="Textos do kit"
      description="Valem para todas as telas do kit de uma vez (começando, volto já, gameplay, encerrando…)."
      footer={
        <>
          <span className="hint" style={{ marginRight: 'auto' }}>
            {kit ? `Grava em ${kit.name}\\src\\config.js` : ''}
          </span>
          <Button variant="ghost" onClick={onClose} disabled={saving}>
            Cancelar
          </Button>
          <Button variant="primary" onClick={() => void save()} loading={saving} disabled={!config}>
            Salvar e atualizar
          </Button>
        </>
      }
    >
      {loadError ? (
        <div className="error-text">{loadError}</div>
      ) : missing ? (
        <div className="hint">Este kit não tem o arquivo src/config.js, então os textos ficam dentro de cada página.</div>
      ) : !config ? (
        <div className="empty-state">
          <span className="spinner" />
        </div>
      ) : (
        <div className="kit-texts">
          <div className="form-grid">
            <Field label="Canal" hint="nome na Twitch">
              <TextInput value={config.canal} onChange={(e) => set({ canal: e.target.value })} />
            </Field>
            <Field label="Seu nome" hint="etiqueta da câmera">
              <TextInput value={config.nome} onChange={(e) => set({ nome: e.target.value })} />
            </Field>
            <Field label="Jogo / assunto do dia">
              <TextInput value={config.jogo} onChange={(e) => set({ jogo: e.target.value })} />
            </Field>
            <Field label="Contagem do “Começando”" hint="minutos">
              <TextInput
                type="number"
                min={1}
                max={120}
                value={contagemDraft}
                onChange={(e) => {
                  const raw = e.target.value
                  setContagemDraft(raw)
                  const n = Number(raw)
                  if (raw !== '' && Number.isFinite(n)) set({ contagem: n })
                }}
                onBlur={() => {
                  const clamped = Math.max(1, Math.min(120, Number(contagemDraft) || 5))
                  setContagemDraft(String(clamped))
                  set({ contagem: clamped })
                }}
              />
            </Field>
          </div>
          <Field label="Próxima live" hint="tela de encerramento">
            <TextInput value={config.agenda} onChange={(e) => set({ agenda: e.target.value })} />
          </Field>
          <Field label="Pix">
            <TextInput value={config.pix} onChange={(e) => set({ pix: e.target.value })} />
          </Field>

          <div className="kit-list">
            <div className="kit-list-head">
              <span className="field-label">Recados que giram na tela</span>
              <Button
                size="sm"
                variant="ghost"
                icon={<IconPlus size={13} />}
                onClick={() => setRecadoRows((prev) => [...prev, { uid: `row-${nextRowUid++}`, value: '' }])}
              >
                Recado
              </Button>
            </div>
            {recadoRows.map((row) => (
              <div key={row.uid} className="kit-list-row">
                <TextInput
                  value={row.value}
                  placeholder="bebe uma água aí também"
                  onChange={(e) =>
                    setRecadoRows((prev) =>
                      prev.map((x) => (x.uid === row.uid ? { ...x, value: e.target.value } : x))
                    )
                  }
                />
                <IconButton
                  label="Apagar recado"
                  onClick={() => setRecadoRows((prev) => prev.filter((x) => x.uid !== row.uid))}
                >
                  <IconTrash size={14} />
                </IconButton>
              </div>
            ))}
          </div>

          <div className="kit-list">
            <div className="kit-list-head">
              <span className="field-label">Redes (rodapé das telas)</span>
              <Button
                size="sm"
                variant="ghost"
                icon={<IconPlus size={13} />}
                onClick={() =>
                  setRedeRows((prev) => [
                    ...prev,
                    { uid: `row-${nextRowUid++}`, value: { rede: 'instagram', handle: '' } }
                  ])
                }
              >
                Rede
              </Button>
            </div>
            {redeRows.map((row) => (
              <div key={row.uid} className="kit-list-row">
                <select
                  className="select kit-rede"
                  value={row.value.rede}
                  onChange={(e) =>
                    setRedeRows((prev) =>
                      prev.map((x) =>
                        x.uid === row.uid ? { ...x, value: { ...x.value, rede: e.target.value } } : x
                      )
                    )
                  }
                >
                  {[...new Set([...REDES, row.value.rede])].map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </select>
                <TextInput
                  value={row.value.handle}
                  placeholder="@seuuser"
                  onChange={(e) =>
                    setRedeRows((prev) =>
                      prev.map((x) =>
                        x.uid === row.uid ? { ...x, value: { ...x.value, handle: e.target.value } } : x
                      )
                    )
                  }
                />
                <IconButton
                  label="Apagar rede"
                  onClick={() => setRedeRows((prev) => prev.filter((x) => x.uid !== row.uid))}
                >
                  <IconTrash size={14} />
                </IconButton>
              </div>
            ))}
          </div>
        </div>
      )}
    </Modal>
  )
}
