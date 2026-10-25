import { useEffect, useState } from 'react'
import type { KitWidget } from '@shared/protocol'
import { Button } from '@/components/ui'
import { IconExternal, IconFolder, IconGlobe, IconPlus, IconRefresh } from '@/components/Icons'
import { useStudio } from '@/studio/StudioContext'
import { addItem, buildKitScenes, editingScene, kitItem, makeItem } from '@/studio/scenes'
import { WidgetPickerModal, type WidgetChoice } from './studio/Pickers'
import './WidgetsScreen.css'

const ROLE_LABEL: Record<KitWidget['role'], string> = {
  scene: 'Tela cheia',
  overlay: 'Overlay transparente',
  frame: 'Moldura',
  chat: 'Chat'
}

function useThumb(kitPath: string, file: string): string | null {
  const [src, setSrc] = useState<string | null>(null)
  useEffect(() => {
    const png = `${kitPath}\\export\\obs-previews\\${file.replace(/\.html?$/i, '.png')}`
    void window.api.files.readImage(png).then(setSrc)
  }, [kitPath, file])
  return src
}

function WidgetCard({ kitPath, widget, onAdd }: { kitPath: string; widget: KitWidget; onAdd: () => void }): React.JSX.Element {
  const thumb = useThumb(kitPath, widget.file)
  return (
    <div className="wcard">
      <div className={`wcard-thumb ${widget.role === 'scene' ? '' : 'is-transparent'}`}>
        {thumb ? <img src={thumb} alt="" draggable={false} /> : <IconGlobe size={26} />}
      </div>
      <div className="wcard-body">
        <div className="wcard-title">
          {widget.name}
          <span className="wcard-role">{ROLE_LABEL[widget.role]}</span>
        </div>
        <p className="wcard-desc">{widget.description}</p>
      </div>
      <Button size="sm" icon={<IconPlus size={14} />} onClick={onAdd}>
        Adicionar à cena
      </Button>
    </div>
  )
}

export function WidgetsScreen({ onDone }: { onDone: () => void }): React.JSX.Element {
  const { settings, update, kit, reloadKit, toast } = useStudio()
  const [picker, setPicker] = useState(false)
  const scene = editingScene(settings.scenes)

  const addToScene = (name: string, build: () => ReturnType<typeof kitItem>): void => {
    if (!scene) return
    update({ scenes: addItem(settings.scenes, scene.id, build()) })
    toast(`“${name}” adicionado em ${scene.name}`, 'good')
  }

  const createKitScenes = (): void => {
    if (!kit) return
    const existing = new Set(settings.scenes.scenes.map((s) => s.name.toLowerCase()))
    const fresh = buildKitScenes(kit.widgets).filter((s) => !existing.has(s.name.toLowerCase()))
    if (fresh.length === 0) {
      toast('As cenas do kit já existem. Renomeie ou apague para criar de novo.')
      return
    }
    update({ scenes: { ...settings.scenes, scenes: [...settings.scenes.scenes, ...fresh] } })
    toast(`${fresh.length} cenas criadas: ${fresh.map((s) => s.name).join(', ')}`, 'good')
    onDone()
  }

  const pickFolder = async (): Promise<void> => {
    const path = await window.api.kit.pickFolder()
    if (!path) return
    const loaded = await window.api.kit.load(path)
    if (!loaded) {
      toast('Essa pasta não tem uma pasta “obs” com páginas HTML.', 'bad')
      return
    }
    update({ kitPath: path })
  }

  const addCustom = (c: WidgetChoice): void => {
    setPicker(false)
    addToScene(c.name, () =>
      c.kitWidget
        ? kitItem(c.kitWidget)
        : makeItem(
            'browser_source',
            c.name,
            { url: c.url, width: c.width, height: c.height, fps: 30 },
            c.width >= 1920 ? {} : { x: 0.05, y: 0.05, width: c.width / 1920, height: c.height / 1080 }
          )
    )
  }

  return (
    <div className="page">
      <div className="page-inner wide">
        <header className="page-header">
          <h1>Widgets</h1>
          <p>
            Overlays HTML entram no vídeo da live como no OBS. Os do seu kit já vêm configurados com os parâmetros de
            cada tela; você ajusta textos e contagem no painel de propriedades da fonte.
          </p>
        </header>

        {kit ? (
          <section className="section">
            <div className="kit-head">
              <div>
                <div className="section-title">Kit</div>
                <div className="kit-name">{kit.name}</div>
                <div className="hint mono">{kit.path}</div>
              </div>
              <div className="row" style={{ gap: 'var(--sp-2)' }}>
                <Button size="sm" variant="ghost" icon={<IconRefresh size={14} />} onClick={reloadKit}>
                  Recarregar
                </Button>
                <Button size="sm" variant="ghost" icon={<IconFolder size={14} />} onClick={() => void pickFolder()}>
                  Trocar pasta
                </Button>
                <Button variant="primary" onClick={createKitScenes}>
                  Criar cenas do kit
                </Button>
              </div>
            </div>
            <p className="hint">
              “Criar cenas do kit” monta Começando, Gameplay, Só conversando, Volto já e Encerrando, com a câmera já
              encaixada nas molduras. Os widgets abaixo vão para a cena que está aberta no editor
              {scene ? ` (${scene.name})` : ''}.
            </p>
            <div className="wgrid">
              {kit.widgets.map((w) => (
                <WidgetCard key={w.file} kitPath={kit.path} widget={w} onAdd={() => addToScene(w.name, () => kitItem(w))} />
              ))}
            </div>
          </section>
        ) : (
          <section className="panel kit-empty">
            <div>
              <div className="kit-name">Nenhum kit de widgets</div>
              <p className="hint">
                Escolha a pasta do seu kit (a que tem a pasta <code>obs</code> com os arquivos .html).
              </p>
            </div>
            <Button variant="primary" icon={<IconFolder size={15} />} onClick={() => void pickFolder()}>
              Escolher pasta
            </Button>
          </section>
        )}

        <section className="section">
          <div className="section-title">Alertas do StreamElements</div>
          <div className="panel">
            <p className="hint">
              Os alertas de follow, sub e raid {kit?.hasStreamElements ? 'do seu kit ' : ''}rodam dentro do
              StreamElements, porque é lá que chegam os eventos da Twitch. Instale o widget no site deles
              {kit?.hasStreamElements ? ' (passo 5 do LEIA-ME do kit)' : ''}, copie a URL do overlay e adicione aqui
              como widget de tela cheia.
            </p>
            <div className="row" style={{ gap: 'var(--sp-2)' }}>
              <Button icon={<IconPlus size={15} />} onClick={() => setPicker(true)}>
                Colar URL do overlay
              </Button>
              <Button
                variant="ghost"
                icon={<IconExternal size={14} />}
                onClick={() => window.api.app.openExternal('https://streamelements.com/dashboard/overlays')}
              >
                Abrir StreamElements
              </Button>
            </div>
          </div>
        </section>
      </div>
      <WidgetPickerModal open={picker} onClose={() => setPicker(false)} onPick={addCustom} />
    </div>
  )
}
