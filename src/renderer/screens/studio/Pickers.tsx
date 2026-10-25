import { useEffect, useState } from 'react'
import type { DesktopSource } from '@shared/ipc'
import type { KitWidget, SourceKind } from '@shared/protocol'
import { Modal } from '@/components/Modal'
import { SourceGrid } from '@/components/SourceGrid'
import { Button, Field, IconButton, Segmented, TextInput } from '@/components/ui'
import { IconFolder, IconGlobe, IconMonitor, IconRefresh } from '@/components/Icons'
import { useStudio } from '@/studio/StudioContext'

/** Escolher tela ou janela para uma fonte de captura. */
export function SourcePickerModal({
  open,
  kind,
  onClose,
  onPick
}: {
  open: boolean
  kind: SourceKind
  onClose: () => void
  onPick: (source: DesktopSource) => void
}): React.JSX.Element {
  const [sources, setSources] = useState<DesktopSource[]>([])
  const [loading, setLoading] = useState(false)
  const [tab, setTab] = useState<SourceKind>(kind)
  const [selected, setSelected] = useState<DesktopSource | null>(null)

  const load = (): void => {
    setLoading(true)
    void window.api.sources
      .list()
      .then(setSources)
      .finally(() => setLoading(false))
  }

  useEffect(() => {
    if (!open) return
    setTab(kind)
    setSelected(null)
    load()
  }, [open, kind])

  return (
    <Modal
      open={open}
      onClose={onClose}
      width={760}
      icon={<IconMonitor size={19} />}
      title={tab === 'screen' ? 'Escolha a tela' : 'Escolha a janela'}
      description="O jogo ou programa precisa estar aberto para aparecer na lista."
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancelar
          </Button>
          <Button variant="primary" disabled={!selected} onClick={() => selected && onPick(selected)}>
            Usar esta {tab === 'screen' ? 'tela' : 'janela'}
          </Button>
        </>
      }
    >
      <div className="picker-bar">
        <Segmented
          size="sm"
          value={tab}
          onChange={(v) => {
            setTab(v)
            setSelected(null)
          }}
          options={[
            { value: 'screen', label: 'Telas' },
            { value: 'window', label: 'Janelas' }
          ]}
        />
        <IconButton label="Atualizar lista" onClick={load}>
          <IconRefresh size={16} />
        </IconButton>
      </div>
      <div className="picker-grid">
        {loading ? (
          <div className="empty-state">
            <span className="spinner" />
          </div>
        ) : (
          <SourceGrid
            sources={sources.filter((s) => s.kind === tab)}
            selectedId={selected?.id ?? null}
            onSelect={setSelected}
          />
        )}
      </div>
    </Modal>
  )
}

export interface WidgetChoice {
  name: string
  url: string
  width: number
  height: number
  kitWidget?: KitWidget
}

/** Adicionar um widget: do kit, de uma URL (StreamElements etc.) ou de um arquivo HTML. */
export function WidgetPickerModal({
  open,
  onClose,
  onPick
}: {
  open: boolean
  onClose: () => void
  onPick: (choice: WidgetChoice) => void
}): React.JSX.Element {
  const { kit } = useStudio()
  const [tab, setTab] = useState<'kit' | 'url'>(kit ? 'kit' : 'url')
  const [url, setUrl] = useState('')
  const [name, setName] = useState('')
  const [size, setSize] = useState('1920x1080')
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (open) {
      setTab(kit ? 'kit' : 'url')
      setUrl('')
      setName('')
      setError(null)
    }
  }, [open, kit])

  const submitUrl = (): void => {
    const u = url.trim()
    if (!/^https?:\/\//i.test(u) && !/^file:\/\//i.test(u)) {
      setError('Cole um endereço que comece com https://')
      return
    }
    const [w, h] = size.split('x').map(Number)
    let label = name.trim()
    if (!label) label = /streamelements\.com/i.test(u) ? 'StreamElements' : new URL(u).hostname || 'Widget'
    onPick({ name: label, url: u, width: w, height: h })
  }

  const pickFile = async (): Promise<void> => {
    const path = await window.api.files.pickHtml()
    if (!path) return
    const fileUrl = `file:///${path.replace(/\\/g, '/').replace(/^\/+/, '')}`
    setUrl(encodeURI(fileUrl))
    if (!name) setName(path.split(/[\\/]/).pop()?.replace(/\.html?$/i, '') ?? 'Widget')
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      width={640}
      icon={<IconGlobe size={19} />}
      title="Adicionar widget"
      description="Widgets entram no vídeo da live como no OBS (fonte de navegador)."
    >
      <Segmented
        size="sm"
        value={tab}
        onChange={setTab}
        options={[
          { value: 'kit', label: kit ? `Kit ${kit.name}` : 'Kit' },
          { value: 'url', label: 'URL ou arquivo' }
        ]}
      />
      {tab === 'kit' ? (
        kit ? (
          <div className="widget-pick-list">
            {kit.widgets.map((w) => (
              <button
                key={w.file}
                className="widget-pick"
                onClick={() => onPick({ name: w.name, url: w.url, width: w.width, height: w.height, kitWidget: w })}
              >
                <span className="widget-pick-name">{w.name}</span>
                <span className="widget-pick-desc">{w.description}</span>
              </button>
            ))}
          </div>
        ) : (
          <div className="hint">Nenhum kit configurado. Escolha a pasta do kit na página Widgets.</div>
        )
      ) : (
        <div className="col" style={{ gap: 'var(--sp-3)', marginTop: 'var(--sp-3)' }}>
          <Field label="Endereço do widget">
            <div className="row" style={{ gap: 'var(--sp-2)' }}>
              <TextInput
                value={url}
                onChange={(e) => {
                  setUrl(e.target.value)
                  setError(null)
                }}
                placeholder="https://streamelements.com/overlay/…"
                autoFocus
              />
              <Button icon={<IconFolder size={15} />} onClick={() => void pickFile()}>
                Arquivo
              </Button>
            </div>
          </Field>
          <div className="form-grid">
            <Field label="Nome">
              <TextInput value={name} onChange={(e) => setName(e.target.value)} placeholder="Alertas" />
            </Field>
            <Field label="Tamanho da página">
              <select className="select" value={size} onChange={(e) => setSize(e.target.value)}>
                <option value="1920x1080">1920 × 1080 (tela cheia)</option>
                <option value="1280x720">1280 × 720</option>
                <option value="800x600">800 × 600</option>
                <option value="520x900">520 × 900 (chat)</option>
                <option value="480x270">480 × 270 (moldura de câmera)</option>
              </select>
            </Field>
          </div>
          {error && <span className="error-text">{error}</span>}
          <div className="hint">
            StreamElements: abra o overlay no site deles, clique no ícone de link e cole aqui. Os alertas aparecem
            quando chegam follows, subs e raids.
          </div>
          <div className="row" style={{ justifyContent: 'flex-end' }}>
            <Button variant="primary" onClick={submitUrl} disabled={!url.trim()}>
              Adicionar
            </Button>
          </div>
        </div>
      )}
    </Modal>
  )
}
