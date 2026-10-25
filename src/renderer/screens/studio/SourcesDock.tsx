import { useEffect, useRef, useState } from 'react'
import type { AudioChannel, Scene, SceneItem, SourceType } from '@shared/protocol'
import { makeChannel } from '@shared/audio'
import { IconButton } from '@/components/ui'
import { IconCameraOff, IconDown, IconEye, IconEyeOff, IconLock, IconPlus, IconTrash, IconUp } from '@/components/Icons'
import { useStudio } from '@/studio/StudioContext'
import { addItem, audioItem, kitItem, makeItem, moveItem, removeItem, sceneMics, SOURCE_LABEL, updateItem } from '@/studio/scenes'
import { SourceIcon } from './sourceIcons'
import { SourcePickerModal, WidgetPickerModal, type WidgetChoice } from './Pickers'
import { hiddenWithoutCamera } from '@/studio/widgets'

const ADD_MENU: Array<{ type: SourceType; hint: string }> = [
  { type: 'screen_capture', hint: 'Monitor inteiro' },
  { type: 'window_capture', hint: 'Só o jogo ou um programa' },
  { type: 'camera_input', hint: 'Webcam / facecam' },
  { type: 'browser_source', hint: 'Kit, StreamElements, chat, URL' },
  { type: 'image_source', hint: 'PNG, JPG, GIF' },
  { type: 'text_source', hint: 'Título ou aviso' },
  { type: 'color_source', hint: 'Fundo de cor sólida' }
]

export function SourcesDock({
  scene,
  selectedId,
  onSelect
}: {
  scene: Scene | null
  selectedId: string | null
  onSelect: (id: string | null) => void
}): React.JSX.Element {
  const { settings, update, toast, kit, undo } = useStudio()
  const st = settings.scenes
  const [menu, setMenu] = useState(false)
  const [renaming, setRenaming] = useState<string | null>(null)
  const cameraOn = settings.studio.cameraEnabled
  const [picker, setPicker] = useState<'screen' | 'window' | null>(null)
  const [widgetPicker, setWidgetPicker] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!menu) return
    const close = (e: MouseEvent): void => {
      if (!menuRef.current?.contains(e.target as Node)) setMenu(false)
    }
    window.addEventListener('mousedown', close)
    return () => window.removeEventListener('mousedown', close)
  }, [menu])

  if (!scene) return <div className="dock" />

  const add = (item: SceneItem, atBottom = false): void => {
    let next = addItem(st, scene.id, item)
    if (atBottom) next = moveItem(next, scene.id, item.id, -scene.items.length)
    update({ scenes: next })
    onSelect(item.id)
  }

  const mics = settings.audioMixer.channels.filter((c) => c.kind === 'mic')
  const inScene = sceneMics(scene, false)
  const micsToAdd = mics.filter((c) => !inScene.has(c.id))

  /** Liga um microfone do mixer nesta cena (sem canal ainda, cria um no mixer). */
  const addMic = (ch: AudioChannel | null): void => {
    setMenu(false)
    const channel = ch ?? makeChannel('mic', { name: 'Microfone' })
    const scenes = addItem(st, scene.id, audioItem(channel))
    update(ch ? { scenes } : { scenes, audioMixer: { ...settings.audioMixer, channels: [...settings.audioMixer.channels, channel] } })
    toast(`${channel.name} ligado na cena “${scene.name}”`, 'good')
  }

  const choose = async (type: SourceType): Promise<void> => {
    setMenu(false)
    switch (type) {
      case 'screen_capture':
        setPicker('screen')
        break
      case 'window_capture':
        setPicker('window')
        break
      case 'browser_source':
        setWidgetPicker(true)
        break
      case 'camera_input':
        add(
          makeItem('camera_input', 'Câmera', { deviceId: 'default' }, { x: 0.7, y: 0.66, width: 0.28, height: 0.28 }, {
            x: 0,
            y: 0,
            width: 1,
            height: 0.32,
            zIndex: 0
          })
        )
        break
      case 'image_source': {
        const path = await window.api.files.pickImage()
        if (!path) return
        add(makeItem('image_source', path.split(/[\\/]/).pop() ?? 'Imagem', { path }, { x: 0.3, y: 0.3, width: 0.4, height: 0.4 }))
        break
      }
      case 'text_source':
        add(
          makeItem('text_source', 'Texto', { text: 'Seu texto aqui', fontSize: 72, color: '#ffffff' }, {
            x: 0.1,
            y: 0.42,
            width: 0.8,
            height: 0.16
          })
        )
        break
      case 'color_source':
        add(makeItem('color_source', 'Fundo', { color: '#10131c' }), true)
        break
    }
  }

  const addWidget = (c: WidgetChoice): void => {
    setWidgetPicker(false)
    if (c.kitWidget) {
      add(kitItem(c.kitWidget))
    } else {
      const full = c.width >= 1920
      add(
        makeItem(
          'browser_source',
          c.name,
          { url: c.url, width: c.width, height: c.height, fps: 30 },
          full ? {} : { x: 0.05, y: 0.05, width: c.width / 1920, height: c.height / 1080 }
        )
      )
    }
    toast(`Widget “${c.name}” adicionado à cena`, 'good')
  }

  const patch = (it: SceneItem, p: Partial<SceneItem>): void =>
    update({ scenes: updateItem(st, scene.id, it.id, p) })

  // Lista de cima para baixo = da frente para o fundo (como no OBS).
  const ordered = [...scene.items].reverse()
  const selected = scene.items.find((it) => it.id === selectedId)

  return (
    <div className="dock">
      <div className="dock-head">
        <span>Fontes</span>
        <span className="dock-sub">{scene.name}</span>
      </div>
      <div className="dock-list">
        {ordered.length === 0 && (
          <button className="dock-empty" onClick={() => setMenu(true)}>
            <IconPlus size={15} /> Adicionar a primeira fonte
          </button>
        )}
        {ordered.map((it) => (
          <div
            key={it.id}
            className={`dock-row ${it.id === selectedId ? 'is-selected' : ''} ${it.visible ? '' : 'is-hidden'}`}
            onClick={() => onSelect(it.id)}
            onDoubleClick={() => setRenaming(it.id)}
            title={`${SOURCE_LABEL[it.type]} · duplo clique renomeia`}
          >
            <span className="dock-icon">
              <SourceIcon type={it.type} />
            </span>
            {renaming === it.id ? (
              <input
                className="dock-rename"
                defaultValue={it.name}
                autoFocus
                onFocus={(e) => e.target.select()}
                onClick={(e) => e.stopPropagation()}
                onBlur={(e) => {
                  if (e.target.value.trim()) patch(it, { name: e.target.value.trim() })
                  setRenaming(null)
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
                  if (e.key === 'Escape') setRenaming(null)
                }}
              />
            ) : (
              <span className="dock-name">{it.name}</span>
            )}
            {it.type === 'audio_input' && !mics.some((c) => c.id === it.settings.audioChannelId) && (
              <span className="dock-flag" title="Este microfone saiu do mixer">
                sem canal
              </span>
            )}
            {!cameraOn && hiddenWithoutCamera(it, kit) && (
              <span className="dock-flag" title="Câmera desligada: não aparece na live">
                <IconCameraOff size={13} />
              </span>
            )}
            {it.type !== 'audio_input' && (
              <button
                className={`dock-toggle is-lock ${it.locked ? 'is-on' : ''}`}
                title={it.locked ? 'Destravar' : 'Travar posição'}
                onClick={(e) => {
                  e.stopPropagation()
                  patch(it, { locked: !it.locked })
                }}
              >
                <IconLock size={13} />
              </button>
            )}
            <button
              className="dock-toggle is-on"
              title={it.type === 'audio_input' ? (it.visible ? 'Desligar nesta cena' : 'Ligar nesta cena') : it.visible ? 'Esconder' : 'Mostrar'}
              onClick={(e) => {
                e.stopPropagation()
                patch(it, { visible: !it.visible })
              }}
            >
              {it.visible ? <IconEye size={14} /> : <IconEyeOff size={14} />}
            </button>
          </div>
        ))}
      </div>
      <div className="dock-tools" ref={menuRef}>
        <IconButton label="Adicionar fonte" active={menu} onClick={() => setMenu((m) => !m)}>
          <IconPlus size={16} />
        </IconButton>
        <IconButton
          label="Trazer para frente"
          disabled={!selected}
          onClick={() => selected && update({ scenes: moveItem(st, scene.id, selected.id, 1) })}
        >
          <IconUp size={16} />
        </IconButton>
        <IconButton
          label="Mandar para trás"
          disabled={!selected}
          onClick={() => selected && update({ scenes: moveItem(st, scene.id, selected.id, -1) })}
        >
          <IconDown size={16} />
        </IconButton>
        <div className="spacer" />
        <IconButton
          label="Remover fonte"
          variant="danger"
          disabled={!selected}
          onClick={() => {
            if (!selected) return
            update({ scenes: removeItem(st, scene.id, selected.id) })
            onSelect(null)
            toast(`“${selected.name}” removida`, 'info', { label: 'Desfazer', run: () => undo() })
          }}
        >
          <IconTrash size={15} />
        </IconButton>

        {menu && (
          <div className="add-menu" role="menu">
            {ADD_MENU.map((m) => (
              <button key={m.type} className="add-menu-item" role="menuitem" onClick={() => void choose(m.type)}>
                <SourceIcon type={m.type} size={16} />
                <span className="add-menu-text">
                  <span>{SOURCE_LABEL[m.type]}</span>
                  <small>{m.hint}</small>
                </span>
              </button>
            ))}
            {(mics.length === 0 ? [null] : micsToAdd).map((ch) => (
              <button key={ch?.id ?? 'new-mic'} className="add-menu-item" role="menuitem" onClick={() => addMic(ch)}>
                <SourceIcon type="audio_input" size={16} />
                <span className="add-menu-text">
                  <span>{ch && mics.length > 1 ? ch.name : 'Microfone'}</span>
                  <small>Só vai para a live nesta cena</small>
                </span>
              </button>
            ))}
          </div>
        )}
      </div>

      <SourcePickerModal
        open={picker !== null}
        kind={picker ?? 'screen'}
        onClose={() => setPicker(null)}
        onPick={(src) => {
          setPicker(null)
          add(
            makeItem(src.kind === 'screen' ? 'screen_capture' : 'window_capture', src.name, {
              captureId: src.id,
              captureName: src.name
            }),
            true
          )
        }}
      />
      <WidgetPickerModal open={widgetPicker} onClose={() => setWidgetPicker(false)} onPick={addWidget} />
    </div>
  )
}
