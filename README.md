# PrismCast Studio (`v2.0.0`)

**PrismCast Studio** é um estúdio comercial de transmissão e compartilhamento de tela de ultra-baixa latência na rede local (LAN P2P) e Multistreaming RTMP simultâneo, alimentado por um **Core Nativo em Rust (`prismcast`)** e uma arquitetura de estúdio inspirada na engenharia reversa do **Streamlabs Desktop (`1.21.9`)**.

---

## Principais Funcionalidades

1. **Core Nativo em Rust (`src-rust/`)**:
   - Descoberta UDP na LAN com cálculo inteligente de broadcast e filtro anti-adaptadores virtuais (`net.rs`, `discovery.rs`).
   - Servidor e Cliente de Sinalização WebSocket assíncronos em Tokio com proteção anti-força bruta de PIN, Chat ao vivo, Reações e Party Voice (`signaling.rs`).
   - Motor de Multistreaming RTMP e Dual Output com detecção automática de encoders de hardware (`h264_nvenc`, `h264_amf`, `h264_qsv`, `libx264`) (`multistream.rs`).
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
