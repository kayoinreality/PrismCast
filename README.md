<p align="left">
  <img src="build/icon.svg" alt="PrismCast Studio" width="64" height="64" />
</p>

# PrismCast Studio (`v2.5.0`)

Estúdio de transmissão nativo em **Rust + Tauri v2** para **Windows e Linux**: monta cenas com tela, janela, câmera e widgets HTML em saída dupla (**16:9 + 9:16**), transmite simultaneamente para **Twitch, YouTube, Kick, TikTok e RTMP/RTMPS próprio** com controle dinâmico de bitrate, compartilha a tela em **LAN P2P de baixíssima latência** e entrega recursos de produção profissional — GPU zero-copy, isolamento de áudio por aplicativo, plugins VST3/CLAP, câmera virtual nativa, transições personalizadas e servidor `obs-websocket` v5 para Stream Deck.

---

## Como usar

1. **Destinos** → conecte a Twitch (*Entrar com a Twitch* via OAuth Device Code ou colando a chave) e habilite YouTube, Kick, TikTok ou RTMP customizado.
2. **Widgets** → aponte para a pasta do seu kit de overlays (ex.: `E:\kayoinreality-kit`) e clique em *Criar cenas do kit*.
3. **Estúdio** → monte suas cenas no palco, configure o mixer e clique em **Entrar ao vivo**. A tela *Preparar a live* confirma título, categoria, destinos ativos, câmera e cena de abertura.

---

## Recursos do Estúdio

### Composição, Palco e GPU Zero-Copy
- **Pipeline de captura Zero-Copy na GPU**: captura GPU → GPU via **DXGI Desktop Duplication** (`D3D11_RESOURCE_MISC_SHARED`) no Windows e **PipeWire / `xdg-desktop-portal` / DMA-BUF** no Linux, com fallback seguro para software.
- **Compositor `wgpu 24`**: composição acelerada por shaders WGSL com telemetria em tempo real de FPS, render time p95 e backend ativo.
- **Palco editável**: clique para selecionar, arraste para mover, redimensione pelos cantos (`Shift` libera proporção), ajuste fino nas setas, `Delete` remove, snap magnético nas bordas/centro e histórico de **Desfazer/Refazer** (`Ctrl+Z` / `Ctrl+Shift+Z`).
- **Fontes**: captura de tela, captura de janela (independente por fonte), câmera, widget/navegador HTML, imagem, texto e cor sólida.
- **Saída dupla 16:9 + 9:16**: segundo canvas vertical para TikTok/Shorts com posicionamento e ordem Z independentes por cena.
- **Modo estúdio**: edite a prévia enquanto outra cena está no ar e aplique a troca com *Transição*.
- **Transições personalizadas**:
  - **Corte, Esmaecer, Fade to Black e Slide**;
  - **Stinger** (vídeo com canal alpha, ponto de corte em ms ou frame e gatilho automático de áudio/SFX);
  - **Luma Wipe** (presets procedurais ou imagem matte customizada, suavidade e inversão);
  - **Custom Shader** (presets visuais ou shader WGSL customizado com validação estática);
  - Curvas de easing (`linear`, `ease-in-out`, `cubic-bezier`) e **override individual por cena**.
- **Sem câmera & Tela de pausa**: desative a câmera com um clique (`Ctrl+Shift+C`) adaptando automaticamente os overlays do kit, ou acione a tela de pausa instantânea (`Ctrl+Shift+P`).

### Áudio Profissional, Isolamento por Aplicativo & Plugins VST3/CLAP
- **Runtime de áudio nativo em 10 ms**: motor DSP contínuo em 48 kHz f32 estéreo (`AudioRuntime` + `AudioMixerEngine`) rodando em Rust.
- **Isolamento de áudio por aplicativo**: separe jogo, música e Discord em canais independentes via helper nativo `prismcast-audio` (WASAPI process loopback no Windows 10 2004+ e PipeWire/PulseAudio no Linux), com reconexão automática caso o programa seja aberto depois.
- **Cadeia DSP completa por canal**: medidor VU com pico, fader em dB, mudo, filtro passa-alta, EQ, noise gate (com linha de limiar no medidor), compressor, ganho, limitador, monitoramento no fone e atraso de sincronia.
- **Host nativo de plugins VST3 / CLAP**: escaneie plugins instalados nas pastas padrão do sistema, monte cadeias por canal, ajuste parâmetros em tempo real, abra a interface do plugin e conte com proteção contra crashes (`catch_unwind`) e auto-bypass em saída `NaN`/`Inf`, além de mix wet/dry e estado salvo em Base64.
- **Captura de janela com áudio**: nas propriedades de uma janela, *Capturar o áudio desta janela* cria e vincula automaticamente o canal do processo.

### Transmissão, Dynamic Bitrate (ABR), Gravação & Câmera Virtual
- **Multistream simultâneo**: codificação H.264 por hardware (NVENC → AMF → QSV, sondados em tempo real; fallback para `libx264`), GOP de 2 s e AAC 48 kHz. Cada destino roda isolado — se uma plataforma oscilar, as outras continuam no ar.
- **Controle Dinâmico de Bitrate (ABR) + RTMP/RTMPS persistente**: controlador AIMD monitora fila, latência de escrita e frames descartados, ajustando o bitrate automaticamente através de uma sessão RTMP/RTMPS persistente (`rml_rtmp` + `tokio-rustls`) com rebasing monotônico de timestamps (**zero desconexão** durante a troca de bitrate).
- **Câmera Virtual nativa**: envie o canvas do estúdio (16:9 ou 9:16) para Discord, Zoom, Meet ou OBS via `v4l2loopback` no Linux e Shared Memory / Media Foundation no Windows, com conversão otimizada `BGRA → YUYV` e `BGRA → NV12`.
- **Gravação MP4 e Replay Buffer**: gravação local com remux automático para MP4 (sem deadlock) e salvamento instantâneo dos últimos segundos (`Ctrl+Shift+S`).
- **LAN P2P de baixa latência**: descoberta automática na rede local e compartilhamento direto com proteção opcional por PIN.

### Controladores Externos (Stream Deck & `obs-websocket` v5)
- **Servidor `obs-websocket` v5 integrado** (porta padrão `4455`): compatível com **Elgato Stream Deck**, Touch Portal, Bitfocus Companion e clientes OBS v5.
- **Autenticação SHA-256** conforme especificação oficial v5, suporte a opcodes `0–9`, batching, eventos em tempo real e controle bidirecional de cenas, modo estúdio, visibilidade de fontes, transições, mute/volume de áudio, live, gravação, replay buffer, câmera virtual e hotkeys.
- **Ponte RPC WebSocket local** (porta `41236`): autenticada com token de 256 bits (`ConstantTimeEq`) e proteção contra CSWSH (rejeição estrita de `Origin` por padrão, com suporte opcional a allowlist explícita sem wildcard `*`).

### Multichat, Twitch & Widgets HTML
- **Multichat unificado**: Twitch, YouTube e Kick em tempo real na lateral do Estúdio ou em **janela popout** dedicada (com suporte a emotes 7TV/Twitch e envio de mensagens na Twitch autenticada).
- **Título e categoria ao vivo**: edição direta na barra do estúdio com sincronização via API Helix da Twitch.
- **Widgets & Kits de overlay**: leitura e edição segura de `src/config.js` sem `eval`/VM JavaScript, suporte a StreamElements, URLs `https://` e arquivos `.html` locais, com reinício opcional de contagens regressivas ao trocar de cena.

---

## Segurança e armazenamento de segredos

- Chaves de transmissão, tokens de refresh da Twitch e senha do `obs-websocket` v5 ficam no **cofre nativo do sistema operacional** via `keyring` (Windows DPAPI / Credential Manager e Linux Secret Service, com fallback local criptografado) e nunca retornam em texto puro para a interface.
- Formatos de stream key são validados antes de salvar (Twitch `live_…`, YouTube `xxxx-xxxx-…`, Kick `sk_…`).
- **Entrar com a Twitch** usa OAuth *Device Code* (`twitch.tv/activate`), sem necessidade de expor client secret; é possível configurar um Client ID próprio em **Destinos → Twitch → Avançado**.
- Preferências são salvas no diretório de configuração do aplicativo (`prismcast-settings.json`), com migração automática a partir de versões anteriores.

---

## Requisitos

- **FFmpeg no PATH** para multistream e remux MP4 (`winget install ffmpeg` no Windows; `sudo apt install ffmpeg` no Linux).
- **Linux — Câmera Virtual**: módulo `v4l2loopback` carregado (`sudo modprobe v4l2loopback exclusive_caps=1 card_label="PrismCast Virtual Camera"`).
- **Linux — Isolamento de áudio**: PipeWire (`pipewire-pulse`) ou PulseAudio com `pactl`, `parec` e `pacat`.

---

## Atalhos globais

| Atalho | Ação |
|---|---|
| `Ctrl+Shift+M` | Mutar / ligar microfone |
| `Ctrl+Shift+D` | Mutar / ligar áudio do PC |
| `Ctrl+Shift+P` | Tela de pausa |
| `Ctrl+Shift+S` | Salvar replay |
| `Ctrl+Shift+V` | Ligar / desligar saída 9:16 |
| `Ctrl+Shift+C` | Ligar / desligar câmera |

Todos os atalhos podem ser personalizados em **Ajustes → Atalhos globais** (com detecção automática de conflitos).

---

## Arquitetura do repositório

```text
src-rust/
├── main.rs              # Entrada do binário nativo (Tauri v2 desktop + modo headless RPC)
├── tauri_app.rs         # Comandos #[tauri::command], janelas main/chat e eventos
├── rpc.rs               # AppEngine autoritativo, ponte WebSocket binária e ObsControlHost
├── protocol.rs          # Contratos 1:1 Rust <-> TypeScript + normalização de settings
├── store.rs             # Persistência deep-merge, migrações e cofre de segredos
├── gpu_capture.rs       # Captura GPU zero-copy (DXGI Desktop Duplication / PipeWire DMA-BUF)
├── gpu_compositor.rs    # Backend wgpu 24 + shaders WGSL + validação estática de shaders
├── compositor.rs        # Compositor Dual Output + Stinger, Luma Wipe, Custom Shader e easing
├── abr.rs               # Controlador AIMD de Dynamic Bitrate com histerese
├── rtmp_publisher.rs    # Demuxer FLV, TimestampRebaser e sessão RTMP/RTMPS persistente
├── multistream.rs       # Gerenciamento FFmpeg multi-destino, sonda de encoder HW e ABR
├── audio_runtime.rs     # Loop DSP de 10 ms, captura nativa WASAPI/PipeWire e supervisão per-app
├── audio_mixer.rs       # Mixer multicanal, biquad EQ, gate, compressor, limiter e PluginChain
├── vst_host.rs          # Scanner e host dinâmico C-ABI para plugins VST3 e CLAP
├── audio_helper.rs      # Binário auxiliar prismcast-audio (captura isolada por processo)
├── virtual_cam.rs       # Câmera virtual (v4l2loopback / Windows ShMem + MF probe, YUYV/NV12)
├── obs_ws.rs            # Servidor obs-websocket v5 completo (SHA-256, opcodes 0-9, eventos)
├── recorder.rs          # Gravação local, replay buffer em RAM e remux assíncrono para MP4
├── multichat.rs         # Hub unificado de chat (Twitch IRC, YouTube, Kick + 7TV)
├── auth_twitch.rs       # Twitch OAuth Device Flow + Helix API
├── discovery.rs / signaling.rs / rtc_host.rs  # Descoberta LAN, sinalização e WebRTC P2P
└── hotkeys.rs / kit.rs / firewall.rs / net.rs # Atalhos globais, kits HTML e utilitários OS

src/
├── shared/              # Contratos TypeScript espelhando src-rust/protocol.rs e IPC
└── renderer/            # Interface React 19 + TypeScript (Estúdio, Mixer, Cenas, Ajustes, Chat)

build/                   # Ícones multi-DPI (SVG, PNG, ICO) e identidade visual dos instaladores NSIS/MSI
site/                    # Landing page interativa publicada no GitHub Pages
```

---

## Desenvolvimento e build

### Pré-requisitos
- **Node.js 22+** e **npm**
- **Rust stable** (`rustup`)
- **Linux**: `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev patchelf`

### Comandos

```bash
npm install

# Frontend (Vite + TypeScript)
npm run dev                  # servidor Vite de desenvolvimento
npm run typecheck            # checagem estrita de tipos (tsc --noEmit)
npm run build                # typecheck + build de produção (dist/)

# App Desktop (Tauri v2 + motor Rust)
npm run dev:desktop          # abre o PrismCast Studio em modo dev
npm run dist                 # gera pacotes para o sistema atual
npm run dist:win             # Windows: instaladores NSIS e MSI + executável portátil
npm run dist:linux           # Linux: AppImage, .deb e .rpm

# Verificação e testes Rust
npm run rust:test            # cargo test --all-targets (headless)
npm run rust:test:desktop    # cargo test --all-targets --features tauri-app
npm run rust:check:desktop   # cargo check --all-targets --features tauri-app
```
