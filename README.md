# PrismCast Studio (`v2.1.0`)

**PrismCast Studio** é um estúdio comercial de transmissão e compartilhamento de tela de ultra-baixa latência na rede local (LAN P2P) e Multistreaming RTMP simultâneo, alimentado por um **Core Nativo em Rust (`prismcast`)**, interface moderna em **layout modular estilo OBS Studio / Streamlabs Desktop** e aceleração de vídeo por hardware com GPU zero-copy.

---

## Principais Funcionalidades

1. **Layout e Experiência Estilo OBS Studio**:
   - **Preparação e Edição Antes de Transmitir**: edite cenas, adicione fontes, organize camadas e configure seu áudio e overlays antes de iniciar a transmissão de fato.
   - **Modo Estúdio (Prévia | Programa)**: visualize e edite a cena no palco de prévia e envie para o ar ao vivo com transições de **Corte (Cut)** ou **Esmaecer (Fade)**.
   - **Docks Modulares Inferiores**: Cenas, Fontes da Cena Ativa, Mixer de Áudio com VU meters ao vivo, Transições e Controles da Live.

2. **Gerenciador de Cenas e Fontes**:
   - Cenas prontas para uso: `🎮 Gameplay & Cam`, `💬 Just Chatting`, `⏳ Começando em Breve`, `🛑 Pausa / Volto Já` e `🏁 Encerramento`.
   - Adicione e gerencie fontes com visibilidade (olho), bloqueio de posição (cadeado), reordenação de camadas (Z-Index) e propriedades:
     - 🖥️ Captura de Tela (Monitores completos)
     - 🪟 Captura de Janela (Jogos e aplicativos)
     - 📷 Câmera / Webcam (Facecam integrada)
     - 🎙️ Entrada de Áudio (Microfone USB / XLR específico)
     - 🔊 Saída de Áudio (Sons do PC / Desktop)
     - 🌐 Browser Source (StreamElements, Alertas, Chat)
     - 🎨 Fonte de Cor (Fundos e tarjas)
     - 📝 Texto GDI+ / Personalizado

3. **Sistema de Login & Conexão de Plataformas (Multicasting Automático)**:
   - Login com **PrismCast Cloud ID** (nome, email, avatar, plano Studio).
   - Conexão em 1 clique com as principais plataformas:
     - 🟣 **Twitch** (`rtmp://live.twitch.tv/app`)
     - 🔴 **YouTube Live** (`rtmp://a.rtmp.youtube.com/live2`)
     - 🟢 **Kick** (`rtmps://fa723fc1b171.global-contribute.live-video.net/app`)
     - ⬛ **TikTok Live** (`rtmp://push.tiktokv.com/live`)
   - Ao conectar sua conta ou chave de stream, os destinos de multistreaming são ativados e configurados automaticamente sem necessidade de copiar URLs ou chaves manualmente.

4. **Dual Output Simultâneo (`16:9 Horizontal` + `9:16 Vertical` Lado a Lado)**:
   - Preview visual duplo renderizado em tempo real: **16:9 (1920x1080)** e **9:16 (1080x1920)** lado a lado no estúdio.
   - 4 presets inteligentes de enquadramento vertical: **Split Cam + Jogo**, **Fundo Desfocado**, **Corte Central** e **PiP Vertical**.

5. **Aceleração por Hardware GPU & Decodificação Zero-Copy**:
   - Flags Chromium habilitadas para rasterização por GPU (`--enable-gpu-rasterization`, `--enable-zero-copy`, `--enable-hardware-overlays`).
   - Detecção automática de encoders de hardware FFmpeg (`h264_nvenc`, `h264_amf`, `h264_qsv`, `libx264`).

6. **Core Nativo em Rust (`src-rust/`)**:
   - Descoberta UDP na LAN com cálculo inteligente de broadcast e filtro anti-adaptadores virtuais (`net.rs`, `discovery.rs`).
   - Servidor e Cliente de Sinalização WebSocket assíncronos em Tokio com proteção anti-força bruta de PIN, Chat ao vivo, Reações e Party Voice (`signaling.rs`).
   - Gravador local contínuo + **Circular Replay Buffer** em memória (`VecDeque`) para salvar os últimos 30 segundos instantaneamente (`recorder.rs`).
   - Gerenciador de Firewall do Windows (`netsh advfirewall`) e persistência atômica (`firewall.rs`, `store.rs`).

2. **Cenas Ao Vivo, Troca de Fonte Sem Queda, PiP e Modo Privacidade ("Volto Já")**:
   - Troque a tela ou janela compartilhada no meio da transmissão sem derrubar nenhum espectador WebRTC nem interromper o Multistream ou a gravação (`RTCRtpSender.replaceTrack` + compositor contínuo).
   - **Picture-in-Picture (Webcam)**: sobreponha sua webcam com recorte arredondado e borda neon em qualquer canto do palco `16:9` e no layout vertical `9:16`.
   - **Escudo de Privacidade ("Volto Já")**: pause visualmente a transmissão com 1 clique ou atalho global (`Ctrl+Shift+P`) para digitar senhas ou alternar janelas com segurança.

3. **Dual Output Simultâneo (`16:9 Horizontal` + `9:16 Vertical`)**:
   - Renderiza em tempo real um canvas horizontal (`1920x1080`) e um canvas vertical (`1080x1920`) ideal para **TikTok Live, YouTube Shorts e Instagram Reels**.
   - 4 presets inteligentes de enquadramento vertical: **Split Cam + Jogo**, **Fundo Desfocado**, **Corte Central** e **PiP Vertical**.

4. **Multistreaming Simultâneo (YouTube, Twitch, Kick, TikTok e Custom RTMP)**:
   - Transmita simultaneamente a cena horizontal (`16:9`) para **YouTube Live, Twitch e Kick**, e a cena vertical (`9:16`) para **TikTok Live**, além de servidores RTMP personalizados.

5. **Mixer de Áudio Duplo Ao Vivo (Sistema WASAPI + Microfone)**:
   - Mixagem em tempo real de 48 kHz combinando o loopback de áudio do sistema e o microfone do host.
   - Controle de ganho individual de `0%` a `200%`, mute independente sem queda de track, **Filtro Anti-Ruído (High-Pass 85Hz + WebRTC Noise Suppression)** e **Compressor Dinâmico Master**.

6. **Compatibilidade com Widgets Nativos & StreamElements**:
   - Suporte nativo para URLs de overlay do **StreamElements** e Browser Sources personalizados, além de widgets integrados de **Barra de Meta**, **Lower Third**, **Contador de Espectadores** e **AlertBox Ao Vivo**.

7. **Gravação Local & Replay Buffer de 30 Segundos**:
   - Grave toda a sessão localmente em `.webm` (`Videos/PrismCast`) ou salve instantaneamente os últimos 30 segundos com 1 clique ou `Ctrl+Shift+S`.

8. **Chat Ao Vivo, Reações Flutuantes e Party Voice na LAN**:
   - Chat em tempo real integrado para host e espectadores, reações com emojis flutuantes no palco e **Party Voice** (retorno de voz opcional dos espectadores na LAN).

---

## Atalhos Globais do Windows

| Atalho | Ação |
|---|---|
| `Ctrl+Shift+M` | Mutar / Desmutar Microfone do Host |
| `Ctrl+Shift+D` | Mutar / Desmutar Áudio do Sistema |
| `Ctrl+Shift+P` | Ativar / Desativar Escudo de Privacidade ("Volto Já") |
| `Ctrl+Shift+S` | Salvar Clipe de Replay Instantâneo (últimos 30s) |
| `Ctrl+Shift+V` | Alternar Saída / Preview Vertical `9:16` |

---

## Desenvolvimento e Build

```bash
# Instalar dependências Node/Electron
npm install

# Rodar os testes unitários do motor nativo em Rust
npm run rust:test

# Compilar o motor nativo em Rust (modo release otimizado)
npm run rust:build

# Verificar tipos TypeScript e compilar todo o projeto (Rust + Electron-Vite)
npm run build

# Iniciar em modo desenvolvimento
npm run dev

# Gerar executável portátil para Windows (dist/PrismCast-Studio-2.0.0.exe)
npm run dist
```
