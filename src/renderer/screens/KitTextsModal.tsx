import { useEffect, useState } from 'react'
import type { KitConfig } from '@shared/protocol'
import { Modal } from '@/components/Modal'
import { Button, Field, IconButton, TextInput } from '@/components/ui'
import { IconEdit, IconPlus, IconTrash } from '@/components/Icons'
import { useStudio } from '@/studio/StudioContext'

const REDES = ['twitch', 'youtube', 'tiktok', 'instagram', 'x', 'discord', 'kick']

/**
 * Textos do kit (src/config.js): nome do canal, jogo do dia, agenda, recados
 * e redes. Valem para todas as telas de uma vez e entram na live na hora.
 */
export function KitTextsModal({ open, onClose }: { open: boolean; onClose: () => void }): React.JSX.Element {
  const { kit, engine, toast } = useStudio()
  const [config, setConfig] = useState<KitConfig | null>(null)
  const [missing, setMissing] = useState(false)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (!open || !kit) return
    setConfig(null)
    setMissing(false)
    void window.api.kit.readConfig(kit.path).then((c) => {
      setConfig(c)
      setMissing(c === null)
    })
  }, [open, kit])

  const set = (p: Partial<KitConfig>): void => setConfig((c) => (c ? { ...c, ...p } : c))

  const save = async (): Promise<void> => {
    if (!kit || !config) return
    setSaving(true)
    const res = await window.api.kit.writeConfig(kit.path, config)
    setSaving(false)
    if (!res.ok) {
      toast(`Não deu para salvar: ${res.error}`, 'bad')
      return
    }
    engine.reloadKitWidgets()
    toast('Textos do kit salvos — os widgets já atualizaram', 'good')
    onClose()
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      width={640}
      icon={<IconEdit size={19} />}
      title="Textos do kit"
      description="Valem para todas as telas do kit de uma vez (começando, volto já, gameplay, encerrando…)."
      footer={
        <>
          <span className="hint" style={{ marginRight: 'auto' }}>
            {kit ? `Grava em ${kit.name}\\src\\config.js` : ''}
          </span>
          <Button variant="ghost" onClick={onClose}>
            Cancelar
          </Button>
          <Button variant="primary" onClick={() => void save()} loading={saving} disabled={!config}>
            Salvar e atualizar
          </Button>
        </>
      }
    >
      {missing ? (
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
                value={config.contagem}
                onChange={(e) => set({ contagem: Number(e.target.value) || 5 })}
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
              <Button size="sm" variant="ghost" icon={<IconPlus size={13} />} onClick={() => set({ recados: [...config.recados, ''] })}>
                Recado
              </Button>
            </div>
            {config.recados.map((r, i) => (
              <div key={i} className="kit-list-row">
                <TextInput
                  value={r}
                  placeholder="bebe uma água aí também"
                  onChange={(e) => set({ recados: config.recados.map((x, j) => (j === i ? e.target.value : x)) })}
                />
                <IconButton label="Apagar recado" onClick={() => set({ recados: config.recados.filter((_, j) => j !== i) })}>
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
                onClick={() => set({ redes: [...config.redes, { rede: 'instagram', handle: '' }] })}
              >
                Rede
              </Button>
            </div>
            {config.redes.map((r, i) => (
              <div key={i} className="kit-list-row">
                <select
                  className="select kit-rede"
                  value={r.rede}
                  onChange={(e) => set({ redes: config.redes.map((x, j) => (j === i ? { ...x, rede: e.target.value } : x)) })}
                >
                  {[...new Set([...REDES, r.rede])].map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </select>
                <TextInput
                  value={r.handle}
                  placeholder="@seuuser"
                  onChange={(e) => set({ redes: config.redes.map((x, j) => (j === i ? { ...x, handle: e.target.value } : x)) })}
                />
                <IconButton label="Apagar rede" onClick={() => set({ redes: config.redes.filter((_, j) => j !== i) })}>
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
