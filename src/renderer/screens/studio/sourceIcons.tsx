import type { SourceType } from '@shared/protocol'
import {
  IconCamera,
  IconGlobe,
  IconImage,
  IconMic,
  IconMonitor,
  IconPalette,
  IconText,
  IconWindow
} from '@/components/Icons'

export function SourceIcon({ type, size = 15 }: { type: SourceType; size?: number }): React.JSX.Element {
  switch (type) {
    case 'screen_capture':
      return <IconMonitor size={size} />
    case 'window_capture':
      return <IconWindow size={size} />
    case 'camera_input':
      return <IconCamera size={size} />
    case 'browser_source':
      return <IconGlobe size={size} />
    case 'image_source':
      return <IconImage size={size} />
    case 'color_source':
      return <IconPalette size={size} />
    case 'text_source':
      return <IconText size={size} />
    case 'audio_input':
      return <IconMic size={size} />
  }
}
