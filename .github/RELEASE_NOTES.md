# PrismCast Studio v2.5.0 (Windows & Linux)

A **v2.5.0** conclui a transição definitiva para a arquitetura **Rust-first + Tauri v2** e entrega a suíte completa de produção profissional do PrismCast Studio para **Windows (x64)** e **Linux (x64)** — com GPU zero-copy, bitrate dinâmico sem derrubar a transmissão, áudio isolado por aplicativo em tempo real, plugins VST3/CLAP, câmera virtual nativa, transições avançadas e compatibilidade nativa com Stream Deck via `obs-websocket` v5.

---

## Principais novidades

### 1. Pipeline de Captura Zero-Copy na GPU & Compositor `wgpu`
- **Captura direta na GPU (`gpu_capture.rs`)**:
  - **Windows**: DXGI Desktop Duplication com texturas compartilhadas (`D3D11_RESOURCE_MISC_SHARED`), copiando GPU → GPU sem readback na CPU.
  - **Linux**: detecção e ingestão via PipeWire / `xdg-desktop-portal` / DMA-BUF com fallback transparente para software quando o ambiente for headless.
- **Compositor acelerado por hardware (`gpu_compositor.rs`, `compositor.rs`)**: backend `wgpu 24` com shaders WGSL dedicados para composição de camadas e transições, validação estática de shaders WGSL customizados e telemetria real de backend/GPU na barra do estúdio.
- **Pipe nativo BGRA + PCM**: integração direta do compositor e do runtime de áudio com o pipeline do encoder em `multistream.rs`.

### 2. Controle Dinâmico de Bitrate (ABR) & RTMP/RTMPS Persistente (Zero-Disconnect)
- **Controlador AIMD (`abr.rs`)**: reduz o bitrate rapidamente de forma multiplicativa quando há congestionamento de rede/fila/latência e recupera gradualmente de forma aditiva após janela de estabilidade, com histerese contra jitter transitório, cooldown e piso mínimo configurável.
- **Sessão RTMP/RTMPS persistente (`rtmp_publisher.rs`)**: demuxer FLV + `TimestampRebaser` monotônico + cliente RTMP/RTMPS nativo (`rml_rtmp` + `tokio-rustls` + `webpki-roots`), mantendo o socket aberto durante hot-swaps do encoder para ajustar bitrate **sem desconectar a live**.
- **Telemetria ABR em tempo real**: indicadores de bitrate alvo e congestionamento na barra do estúdio + configuração completa em **Ajustes**.

### 3. Runtime de Áudio em Tempo Real & Isolamento por Aplicativo
- **Loop DSP contínuo de 10 ms (`audio_runtime.rs`)**: processamento em 48 kHz estéreo f32 integrado ao `AudioMixerEngine` com monitoramento em fone e injeção de áudio one-shot (para efeitos sonoros de Stinger).
- **Captura isolada por aplicativo**: supervisão e reconexão automática do helper `prismcast-audio` (WASAPI por processo no Windows e PipeWire/PulseAudio no Linux via `pactl`/`parec`/`pacat`) para separar jogo, música e Discord em canais independentes.

### 4. Host de Plugins VST3 / CLAP
- **Carregamento nativo (`vst_host.rs`)**: varredura automática nas pastas padrão de Windows/Linux/macOS e carregamento dinâmico C-ABI via `libloading` para plugins `.vst3` e `.clap`.
- **Cadeia resiliente por canal (`PluginChain`)**: isolamento com `catch_unwind`, auto-bypass automático em caso de crash ou saída `NaN`/`Inf`, controle de mix wet/dry, ajuste de parâmetros em tempo real, abertura de interface do plugin e persistência de estado em Base64.
- **UI dedicada**: gerenciador completo da cadeia VST3/CLAP em **Mixer → Propriedades do canal**.

### 5. Câmera Virtual Nativa
- **Saída de câmera virtual (`virtual_cam.rs`)**: conversores otimizados `BGRA → YUYV` e `BGRA → NV12`, suporte a **Linux `v4l2loopback`** (ioctls nativos) e **Windows Shared Memory + sonda dinâmica de Media Foundation (`MFCreateVirtualCamera`)**.
- **Suporte Dual Orientation (16:9 / 9:16)**: transmissão binária direta (`0x04`), botão de liga/desliga na barra de controle e seletor de dispositivo/orientação em **Ajustes**.

### 6. Transições Personalizadas
- **Stinger**: suporte a vídeo com canal alpha, ponto de corte configurável (ms ou frame) e gatilho automático de áudio (SFX).
- **Luma Wipe**: presets procedurais e imagem matte customizada, com controles de suavidade e inversão.
- **Custom Shader**: presets visuais e suporte a código WGSL customizado com validação estática.
- **Easing & Override por cena**: curvas (`linear`, `ease-in-out`, `cubic-bezier`) e sobrescrita individual por cena diretamente no painel de cenas com seletor nativo de mídia.

### 7. Controladores Externos (Stream Deck / Touch Portal via `obs-websocket` v5)
- **Servidor `obs-websocket` v5 nativo (`obs_ws.rs`)**: porta padrão `4455`, autenticação SHA-256 oficial da especificação v5, opcodes `0–9`, batching de requisições, ~30 requests (cenas, modo estúdio, visibilidade de itens, transições, volume/mute de canais, stream, gravação, replay buffer, câmera virtual e hotkeys) e broadcast de eventos em tempo real.
- **Controle bidirecional**: alterações feitas via Stream Deck refletem instantaneamente na interface do PrismCast Studio e vice-versa; senha armazenada no cofre criptografado do sistema operacional.
- **RPC local aprimorado**: contagem de clientes conectados e suporte opcional a allowlist explícita de origens (rejeitando wildcard `*` contra CSWSH).

---

## Melhorias de infraestrutura, identidade visual, build e site (desde a v2.4.0)
- **Nova identidade visual & instaladores personalizados**: novo ícone de prisma óptico 3D com feixe de luz refratado no espectro de 5 canais de broadcast (`build/icon.svg`, PNGs multi-DPI `32px–1024px` e `icon.ico` multi-resolução), aplicado na barra de título do app, no site e nos **instaladores Windows NSIS e MSI** (sidebar com o espectro conectando os recursos da v2.5.0, headers/banners personalizados e localização nativa em português `pt-BR`).
- **Migração completa para Rust + Tauri v2**: eliminação do runtime Electron em favor de um binário nativo mais leve, rápido e com cofre de segredos nativo (Windows DPAPI / Linux Secret Service com fallback criptografado).
- **Correções de estabilidade e testes**: correção do deadlock de `recorder.saveReplay`, correções de áudio/microfone na live, fix de `keyring` no Linux headless e **109 testes automatizados** validados em `ubuntu-latest` e `windows-latest`.
- **Novo site no GitHub Pages (`site/`)**: redesign editorial com estúdio interativo, rolagem suave, animações dinâmicas, testador de atalhos e matriz de download atualizada.

---

## Pacotes disponíveis nesta release
- **Windows (x64)**: executável portátil (`.exe`), instalador **NSIS** (`.exe`) e instalador **MSI** (`.msi`) — com identidade visual própria e suporte nativo a `pt-BR`.
- **Linux (x64)**: **AppImage**, pacote **Debian/Ubuntu (`.deb`)**, pacote **Fedora/RPM (`.rpm`)** e arquivo **`.tar.gz`** com `prismcast` + `prismcast-audio`.
