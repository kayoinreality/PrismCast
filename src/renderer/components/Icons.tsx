import type { SVGProps } from 'react'

type P = SVGProps<SVGSVGElement> & { size?: number }

function Svg({ size = 20, children, ...rest }: P): React.JSX.Element {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...rest}
    >
      {children}
    </svg>
  )
}

export const IconMonitor = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="2" y="3.5" width="20" height="14" rx="2" />
    <path d="M8 21h8M12 17.5V21" />
  </Svg>
)

export const IconWindow = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="3" y="4" width="18" height="16" rx="2" />
    <path d="M3 9h18M6.5 6.5h.01M9.5 6.5h.01" />
  </Svg>
)

export const IconBroadcast = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="2.5" />
    <path d="M7.8 7.8a6 6 0 0 0 0 8.4M16.2 16.2a6 6 0 0 0 0-8.4" />
    <path d="M4.9 4.9a10 10 0 0 0 0 14.2M19.1 19.1a10 10 0 0 0 0-14.2" />
  </Svg>
)

export const IconPlay = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M6 4.5v15l13-7.5-13-7.5Z" fill="currentColor" strokeWidth={1.5} />
  </Svg>
)

export const IconStop = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" strokeWidth={1.5} />
  </Svg>
)

export const IconEye = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12Z" />
    <circle cx="12" cy="12" r="3" />
  </Svg>
)

export const IconEyeOff = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M3 3l18 18" />
    <path d="M10.6 10.6a2 2 0 0 0 2.8 2.8" />
    <path d="M9.9 5.1A10.5 10.5 0 0 1 12 5c6 0 9.5 7 9.5 7a17 17 0 0 1-3.2 4.3" />
    <path d="M6.2 6.2C3.9 8 2.5 12 2.5 12S6 18.5 12 18.5a10 10 0 0 0 5.8-1.8" />
  </Svg>
)

export const IconArrowLeft = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M19 12H5M11 6l-6 6 6 6" />
  </Svg>
)

export const IconClose = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M18 6 6 18M6 6l12 12" />
  </Svg>
)

export const IconMinimize = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M5 12h14" />
  </Svg>
)

export const IconMaximize = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="5" y="5" width="14" height="14" rx="1.5" />
  </Svg>
)

export const IconRestore = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="8" y="4" width="12" height="12" rx="1.5" />
    <path d="M16 8v10a2 2 0 0 1-2 2H4V10a2 2 0 0 1 2-2h10Z" fill="var(--bg-elevated)" />
  </Svg>
)

export const IconUsers = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M16 19v-1.5a3.5 3.5 0 0 0-3.5-3.5h-5A3.5 3.5 0 0 0 4 17.5V19" />
    <circle cx="10" cy="8" r="3.2" />
    <path d="M20 19v-1.5a3.5 3.5 0 0 0-2.6-3.4M15.5 5.2a3.2 3.2 0 0 1 0 5.6" />
  </Svg>
)

export const IconLock = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="4.5" y="10.5" width="15" height="9.5" rx="2" />
    <path d="M8 10.5V7.5a4 4 0 0 1 8 0v3" />
  </Svg>
)

export const IconCopy = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="9" y="9" width="11" height="11" rx="2" />
    <path d="M5 15H4a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v1" />
  </Svg>
)

export const IconCheck = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M4.5 12.5 9.5 17.5 19.5 7" />
  </Svg>
)

export const IconRefresh = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M20 11a8 8 0 0 0-13.6-4.6L3 9.5" />
    <path d="M3 4.5v5h5" />
    <path d="M4 13a8 8 0 0 0 13.6 4.6L21 14.5" />
    <path d="M21 19.5v-5h-5" />
  </Svg>
)

export const IconVolume = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M11 5 6.5 8.8H3v6.4h3.5L11 19V5Z" />
    <path d="M15.5 9.2a4 4 0 0 1 0 5.6M18.3 6.4a8 8 0 0 1 0 11.2" />
  </Svg>
)

export const IconVolumeOff = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M11 5 6.5 8.8H3v6.4h3.5L11 19V5Z" />
    <path d="m16 9.5 5 5M21 9.5l-5 5" />
  </Svg>
)

export const IconMic = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="9" y="3" width="6" height="11" rx="3" />
    <path d="M5 11a7 7 0 0 0 14 0M12 18v3M8 21h8" />
  </Svg>
)

export const IconMicOff = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="m3 3 18 18" />
    <path d="M9 9v2a3 3 0 0 0 5.1 2.1M15 9V6a3 3 0 0 0-5.7-1.3" />
    <path d="M5 11a7 7 0 0 0 11.9 5M12 18v3M8 21h8" />
  </Svg>
)

export const IconCamera = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="2.5" y="6" width="14" height="12" rx="2" />
    <path d="m16.5 10.5 5-3v9l-5-3v-3Z" />
  </Svg>
)

export const IconSmartphone = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="6.5" y="2.5" width="11" height="19" rx="2.2" />
    <path d="M11 18.5h2" />
  </Svg>
)

export const IconLayers = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="m12 3 9 5-9 5-9-5 9-5Z" />
    <path d="m3 12 9 5 9-5" />
    <path d="m3 16.5 9 5 9-5" />
  </Svg>
)

export const IconGlobe = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="9" />
    <path d="M3.5 12h17M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" />
  </Svg>
)

export const IconDisc = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="9" />
    <circle cx="12" cy="12" r="3" fill="currentColor" />
  </Svg>
)

export const IconChat = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M21 12a8 8 0 0 1-8 8H7l-4 3v-6.5A8 8 0 1 1 21 12Z" />
  </Svg>
)

export const IconFullscreen = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M4 9V5.5A1.5 1.5 0 0 1 5.5 4H9M15 4h3.5A1.5 1.5 0 0 1 20 5.5V9M20 15v3.5a1.5 1.5 0 0 1-1.5 1.5H15M9 20H5.5A1.5 1.5 0 0 1 4 18.5V15" />
  </Svg>
)

export const IconPin = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M9 3.5h6l-.7 5.2 3.2 2.6v2.2H6.5v-2.2l3.2-2.6L9 3.5Z" />
    <path d="M12 13.5V21" />
  </Svg>
)

export const IconSettings = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 14.5a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-1.8-.3 1.6 1.6 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.2a1.6 1.6 0 0 0-1-1.5 1.6 1.6 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.6 1.6 0 0 0 .3-1.8 1.6 1.6 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.2a1.6 1.6 0 0 0 1.5-1 1.6 1.6 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.6 1.6 0 0 0 1.8.3H9a1.6 1.6 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.2a1.6 1.6 0 0 0 1 1.5 1.6 1.6 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0-.3 1.8V9a1.6 1.6 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.2a1.6 1.6 0 0 0-1.5 1Z" />
  </Svg>
)

export const IconShield = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M12 3 5 6v5.5c0 4.3 2.9 8.2 7 9.5 4.1-1.3 7-5.2 7-9.5V6l-7-3Z" />
  </Svg>
)

export const IconAlert = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M12 4.5 2.8 20h18.4L12 4.5Z" />
    <path d="M12 10v4.2M12 17.4h.01" />
  </Svg>
)

export const IconActivity = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M3 12h3.5l2.5-7 4 14 2.5-7H21" />
  </Svg>
)

export const IconChevron = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="m6 9 6 6 6-6" />
  </Svg>
)

export const IconSearch = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <circle cx="11" cy="11" r="6.5" />
    <path d="m20 20-4.4-4.4" />
  </Svg>
)

export const IconExit = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M9 4.5H5.5A1.5 1.5 0 0 0 4 6v12a1.5 1.5 0 0 0 1.5 1.5H9" />
    <path d="M15 16.5 19.5 12 15 7.5M19.5 12H9" />
  </Svg>
)

export const IconWifi = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M2.5 9a14 14 0 0 1 19 0" />
    <path d="M6 12.5a9 9 0 0 1 12 0" />
    <path d="M9.5 16a4 4 0 0 1 5 0" />
    <path d="M12 19.5h.01" />
  </Svg>
)

export const IconPlus = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M12 5v14M5 12h14" />
  </Svg>
)

export const IconTrash = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
  </Svg>
)

export const IconEdit = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M17 3a2.83 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3Z" />
  </Svg>
)

export const IconUp = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="m18 15-6-6-6 6" />
  </Svg>
)

export const IconDown = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="m6 9 6 6 6-6" />
  </Svg>
)

export const IconUser = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
    <circle cx="12" cy="7" r="4" />
  </Svg>
)

export const IconCut = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <circle cx="6" cy="6" r="3" />
    <circle cx="6" cy="18" r="3" />
    <path d="M20 4 8.12 15.88M14.47 14.48 20 20M8.12 8.12 12 12" />
  </Svg>
)

export const IconImage = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="3" y="3" width="18" height="18" rx="2" />
    <circle cx="9" cy="9" r="2" />
    <path d="m21 15-3.1-3.1a2 2 0 0 0-2.8 0L6 21" />
  </Svg>
)

export const IconText = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M4 7V4h16v3M9 20h6M12 4v16" />
  </Svg>
)

export const IconPalette = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="3" y="3" width="18" height="18" rx="2" />
    <path d="M3 15 15 3M9 21 21 9" />
  </Svg>
)

export const IconWidget = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="3" y="3" width="7" height="7" rx="1.5" />
    <rect x="14" y="3" width="7" height="7" rx="1.5" />
    <rect x="3" y="14" width="7" height="7" rx="1.5" />
    <path d="M17.5 14v7M14 17.5h7" />
  </Svg>
)

export const IconKey = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <circle cx="7.5" cy="15.5" r="4.5" />
    <path d="m10.7 12.3 9.8-9.8M17 6l3 3M14.5 8.5l2 2" />
  </Svg>
)

export const IconExternal = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M15 3h6v6M10 14 21 3M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
  </Svg>
)

export const IconRecord = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="9" />
    <circle cx="12" cy="12" r="4" fill="currentColor" stroke="none" />
  </Svg>
)

export const IconReplay = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M3 12a9 9 0 1 0 3-6.7L3 8" />
    <path d="M3 3v5h5M12 8v4l3 2" />
  </Svg>
)

export const IconStudio = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="2" y="4" width="20" height="13" rx="2" />
    <path d="M6 21h12M9 8.5v5l4-2.5z" />
  </Svg>
)

export const IconSend = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M5 12h14M13 6l6 6-6 6" />
  </Svg>
)

export const IconFolder = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
  </Svg>
)

export const IconCameraOff = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M10.5 5.5H14l1.5 2h3A2.5 2.5 0 0 1 21 10v7.5M17.5 19.5h-12A2.5 2.5 0 0 1 3 17v-7a2.5 2.5 0 0 1 2.5-2.5H7" />
    <path d="M9.6 10.6a3.3 3.3 0 0 0 4.7 4.7" />
    <path d="M3 3l18 18" />
  </Svg>
)

export const IconSliders = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M4 6h9M17 6h3M4 12h3M11 12h9M4 18h11M19 18h1" />
    <circle cx="15" cy="6" r="2" />
    <circle cx="9" cy="12" r="2" />
    <circle cx="17" cy="18" r="2" />
  </Svg>
)

export const IconMore = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <circle cx="5.5" cy="12" r="1.2" fill="currentColor" />
    <circle cx="12" cy="12" r="1.2" fill="currentColor" />
    <circle cx="18.5" cy="12" r="1.2" fill="currentColor" />
  </Svg>
)

export const IconApp = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="3" y="4" width="18" height="14" rx="2" />
    <path d="M3 8h18" />
    <path d="M10 12.5v3l2.6-1.5z" fill="currentColor" />
  </Svg>
)

export const IconHeadphones = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M4 15v-3a8 8 0 0 1 16 0v3" />
    <rect x="3" y="14" width="4.5" height="6.5" rx="1.5" />
    <rect x="16.5" y="14" width="4.5" height="6.5" rx="1.5" />
  </Svg>
)

export const IconPopout = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <rect x="3" y="7" width="12" height="13" rx="2" />
    <path d="M9 4h9a2 2 0 0 1 2 2v9" />
    <path d="M13 11l7-7M15 4h5v5" />
  </Svg>
)

export const IconUndo = (p: P): React.JSX.Element => (
  <Svg {...p}>
    <path d="M9 14L4 9l5-5" />
    <path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11" />
  </Svg>
)
