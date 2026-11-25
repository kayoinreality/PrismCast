<p align="left">
  <img src="build/icon.svg" alt="PrismCast Studio" width="64" height="64" />
</p>

# PrismCast Studio (`v3.0.0`)

Estúdio de transmissão **100% nativo em Rust (`egui` + `wgpu 24`)** para **Windows e Linux** — **sem Tauri, sem WebView2, sem Electron e sem popups de navegador**. Monta cenas com captura nativa direta de telas, janelas e imagens em saída dupla (**16:9 + 9:16**), transmite simultaneamente para **Twitch, YouTube, Kick, TikTok e RTMP/RTMPS próprio** com controle dinâmico de bitrate (ABR), compartilha a tela em **LAN P2P de baixíssima latência** e entrega recursos de produção profissional — GPU zero-copy, isolamento de áudio por aplicativo, plugins VST3/CLAP, câmera virtual nativa, transições personalizadas e servidor `obs-websocket` v5 para Stream Deck.

---

## O que mudou na `v3.0.0` (100% Rust Nativo)

- **Adeus Tauri / WebView2 / `getDisplayMedia()`**: toda a interface desktop agora é renderizada diretamente na GPU via `eframe` + `egui` + `wgpu 24`, num binário 100% Rust.
- **Captura 100% nativa sem popup, sem barra de compartilhamento e sem roubo de foco**:
  - **Windows**: monitores via **DXGI Desktop Duplication** (com fallback GDI automático) e janelas individuais diretamente por `HWND` via `PrintWindow(PW_RENDERFULLCONTENT)` — captura janelas aceleradas por hardware mesmo atrás do estúdio, sem jamais abrir popup de navegador, sem a barra `"http://tauri.localhost está compartilhando..."` e sem tirar o foco da edição.
  - **Miniaturas reais ao vivo**: o seletor de telas e janelas exibe prévias reais em alta nitidez de cada monitor e janela (filtrando janelas invisíveis e UWP *cloaked* via `DWMWA_CLOAKED`).
- **Design Broadcast Carbon + Glassmorphism Espectral**: a interface desktop adota a mesma identidade visual da página oficial do GitHub Pages — chassi carbono fosco (`#141412`), painéis translúcidos com brilho especular, feixes espectrais suaves do prisma (`#FF3E52`, `#FFAE2B`, `#18DCB8`, `#3892FF`), tipografia marfim quente (`#F1EDE4`) e vermilion Tally (`#D63626`).
- **Palco limpo sem botões invasivos**: fontes de tela e janela iniciam automaticamente de forma nativa e nunca sobrepõem cards/botões flutuantes em cima de outras fontes ativas no palco.

---

## Como usar

1. **Destinos** → conecte a Twitch (*Entrar com a Twitch* via OAuth Device Code ou colando a chave) e habilite YouTube, Kick, TikTok ou RTMP customizado.
2. **Widgets** → aponte para a pasta do seu kit de overlays (ex.: `E:\kayoinreality-kit`) com parsing estático seguro em Rust.
3. **Estúdio** → adicione telas/janelas pelo seletor nativo com miniaturas reais, ajuste o palco e o mixer e clique em **Entrar ao vivo**.

---

## Recursos do Estúdio

### Composição, Palco GPU & Captura Nativa
- **Captura Nativa Direta (Zero WebView)**: captura GPU → GPU via **DXGI Desktop Duplication** (`D3D11_RESOURCE_MISC_SHARED`) e `PrintWindow(PW_RENDERFULLCONTENT)` no Windows, e **PipeWire / X11** no Linux, com auto-recuperação automática se um jogo/janela for reaberto.
- **Compositor `wgpu 24`**: composição acelerada por shaders WGSL com telemetria em tempo real de FPS, render time p95 e backend ativo.
- **Palco editável nativo**: clique para selecionar, arraste para mover com snap magnético turquesa nas bordas/centro, redimensione pelos 4 cantos (`Shift` solta proporção, `Alt` recorta), ajuste fino nas setas, `Delete` remove, botão direito abre menu de contexto e **Desfazer** (`Ctrl+Z`).
- **Fontes**: captura de tela, captura de janela (independente por fonte), imagem (PNG/JPG/WebP/GIF/BMP), cor sólida, texto, câmera e widget HTML.
- **Saída dupla 16:9 + 9:16**: segundo canvas vertical para TikTok/Shorts com posicionamento e ordem Z independentes por cena.
- **Modo estúdio**: edite a prévia enquanto outra cena está no ar e aplique a troca com *Transição*.
- **Transições personalizadas**: Corte, Esmaecer, Fade to Black, Slide, Stinger (alpha + áudio), Luma Wipe e Custom Shader WGSL com curvas de easing e override por cena.

### Áudio Profissional, Isolamento por Aplicativo & Plugins VST3/CLAP
- **Runtime de áudio nativo em 10 ms**: motor DSP contínuo em 48 kHz f32 estéreo (`AudioRuntime` + `AudioMixerEngine`) rodando em Rust.
- **Isolamento de áudio por aplicativo**: separe jogo, música e Discord em canais independentes via helper nativo `prismcast-audio` (WASAPI process loopback no Windows 10 2004+ e PipeWire/PulseAudio no Linux).
- **Cadeia DSP completa por canal**: medidor VU segmentado broadcast com pico, fader em dB, mudo, filtro passa-alta, EQ 3 bandas, noise gate, compressor, ganho, limitador, monitoramento e atraso de sincronia.
- **Host nativo de plugins VST3 / CLAP**: scanner de plugins do sistema com proteção `catch_unwind` e auto-bypass contra `NaN`/`Inf`.

### Transmissão, Dynamic Bitrate (ABR), Gravação & Câmera Virtual
- **Multistream simultâneo nativo**: codificação H.264 por hardware (NVENC → AMF → QSV, fallback `libx264`) recebendo frames BGRA diretamente do compositor Rust (**sem `MediaRecorder`**).
- **Controle Dinâmico de Bitrate (ABR) + RTMP/RTMPS persistente**: ajuste contínuo de bitrate sem desconexão (`rml_rtmp` + `tokio-rustls`).
- **Câmera Virtual nativa**: envie o canvas 16:9 ou 9:16 para Discord, Zoom, Meet ou OBS via `v4l2loopback` (Linux) e Shared Memory / Media Foundation (Windows).
- **Gravação MP4 e Replay Buffer**: gravação local com remux assíncrono para MP4 e salvamento instantâneo dos últimos segundos.
- **LAN P2P de baixa latência**: descoberta UDP automática na rede local e compartilhamento com proteção por PIN.

### Controladores Externos (`obs-websocket` v5) & Multichat
- **Servidor `obs-websocket` v5 integrado** (porta `4455`): compatível com **Elgato Stream Deck**, Touch Portal e Bitfocus Companion, com autenticação SHA-256.
- **Ponte RPC WebSocket local** (porta `41236`): autenticada com token de 256 bits (`ConstantTimeEq`) e proteção contra CSWSH.
- **Multichat unificado**: Twitch, YouTube e Kick em tempo real direto na lateral do Estúdio, com envio de mensagens autenticado na Twitch.
- **Atualizador automático**: verifica novas versões no GitHub Releases com validação SHA-256 contra `SHA256SUMS.txt`.

---

## Segurança e armazenamento de segredos

- Chaves de transmissão, tokens de refresh da Twitch e senha do `obs-websocket` v5 ficam no **cofre nativo do sistema operacional** via `keyring` (Windows DPAPI / Credential Manager e Linux Secret Service, com fallback local criptografado).
- **Entrar com a Twitch** usa OAuth *Device Code* (`twitch.tv/activate`), sem expor client secret.
- Preferências são salvas em `prismcast-settings.json` com escrita atômica e migração automática de versões anteriores.

---

## Requisitos de runtime

- **FFmpeg no PATH** para multistream e remux MP4 (`winget install ffmpeg` no Windows; `sudo apt install ffmpeg` no Linux).
- **Linux — Câmera Virtual**: módulo `v4l2loopback` carregado (`sudo modprobe v4l2loopback exclusive_caps=1 card_label="PrismCast Virtual Camera"`).
- **Linux — Isolamento de áudio**: PipeWire (`pipewire-pulse`) ou PulseAudio com `pactl`, `parec` e `pacat`.

---

## Arquitetura do repositório (100% Rust)

```text
src-rust/
├── main.rs              # Entrada do binário nativo (UI Rust desktop + modo --headless RPC)
├── build.rs             # Incorporação nativa de ícone/metadados Windows via winresource
├── native_ui/
│   ├── mod.rs           # App nativo eframe + egui + wgpu 24, atalhos, undo e eventos
│   ├── theme.rs         # Paleta Broadcast Carbon & Glassmorphism Espectral (GH Pages)
│   ├── stage.rs         # Palco GPU interativo (16:9, 9:16, Modo Estúdio, snap e recorte)
│   ├── picker.rs        # Seletor nativo de telas e janelas com miniaturas reais ao vivo
│   └── panels.rs        # Barra broadcast, docks (Cenas/Fontes/Mixer), Destinos, LAN e Ajustes
├── native_capture.rs    # Pump nativo de captura (DXGI / PrintWindow / Imagens) + compositor
├── capture.rs           # Enumeração Win32/Linux, filtro DWMWA_CLOAKED e thumbnails reais
├── gpu_capture.rs       # Captura GPU zero-copy (DXGI Desktop Duplication / PipeWire DMA-BUF)
├── gpu_compositor.rs    # Backend wgpu 24 + shaders WGSL + validação estática de shaders
├── compositor.rs        # Compositor Dual Output + Stinger, Luma Wipe, Custom Shader e easing
├── rpc.rs               # AppEngine autoritativo, ponte WebSocket binária e ObsControlHost
├── protocol.rs          # Contratos de rede, cenas, áudio, multichat e Schema v3
├── store.rs             # Persistência atômica, deep-merge, migrações e cofre de segredos
├── abr.rs / rtmp_publisher.rs / multistream.rs  # ABR, RTMP/RTMPS persistente e FFmpeg
├── audio_runtime.rs / audio_mixer.rs / vst_host.rs / audio_helper.rs  # Áudio 48kHz & VST3/CLAP
├── virtual_cam.rs / obs_ws.rs / recorder.rs / updater.rs  # VCam, Stream Deck, Rec e Updater
├── multichat.rs / auth_twitch.rs / kit.rs                 # Chat, Twitch OAuth e Kits HTML
└── discovery.rs / signaling.rs / rtc_host.rs / hotkeys.rs / firewall.rs / net.rs

build/                   # Ícones multi-DPI (SVG, PNG, ICO) e instalador NSIS nativo
site/                    # Landing page interativa publicada no GitHub Pages
```

---

## Desenvolvimento e build

**Não requer Node.js, npm, Electron nem Tauri.** Apenas Rust stable!

### Pré-requisitos
- **Rust stable** (`rustup`)
- **Linux**: `pkg-config libssl-dev libxkbcommon-dev libwayland-dev libx11-dev libxcb-shape0-dev libxcb-xfixes0-dev libdbus-1-dev`

### Comandos

```bash
# Abrir o PrismCast Studio 100% nativo em modo dev
cargo run

# Rodar apenas o servidor RPC sem janela gráfica (headless)
cargo run -- --headless

# Rodar todos os testes unitários (com UI nativa e headless)
cargo test --verbose
cargo test --no-default-features --verbose

# Checar compilação de todos os alvos
cargo check --all-targets

# Build de produção otimizado (prismcast + prismcast-audio)
cargo build --release --bins
```
