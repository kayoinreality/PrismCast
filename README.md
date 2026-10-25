# PrismCast Studio (`v2.3.0`)

Estúdio de transmissão para Windows: monta cenas com tela, janela, câmera e widgets HTML, e transmite ao mesmo
tempo para Twitch, YouTube, Kick, TikTok e RTMP próprio. Também compartilha o que está no ar com outros PCs da rede
local, com latência de LAN.

---

## Como usar

1. **Destinos** → conecte a Twitch (botão *Entrar com a Twitch* ou colando a chave) e as outras plataformas.
2. **Widgets** → aponte para a pasta do seu kit (ex.: `E:\kayoinreality-kit`) e clique em *Criar cenas do kit*.
3. **Estúdio** → escolha a cena, ajuste as fontes no palco e clique em **Entrar ao vivo**. A tela *Preparar a live*
   confirma título, categoria, para quais plataformas vai hoje, com ou sem câmera e a cena de abertura.

## Estúdio

- **Palco editável**: clique numa fonte para selecionar, arraste para mover, puxe os cantos para redimensionar
  (Shift solta a proporção), setas para ajuste fino, Delete remove. Encaixa nas bordas e no centro.
- **Fontes**: tela, janela (cada fonte tem a própria captura), câmera, widget/navegador, imagem, texto e cor.
- **Saída 9:16**: um segundo canvas para TikTok/Shorts, com posição própria de cada fonte.
- **Modo estúdio**: edite a prévia enquanto outra cena está no ar e mande com *Transição* (corte ou esmaecer).
- **Mixer no estilo do OBS**: áudio do PC, quantos microfones quiser e **áudio de um programa só** (o jogo, o
  Spotify, o Discord…). Cada fonte tem medidor com pico, fader em dB, mudo e **Propriedades** (engrenagem ou clique
  direito): dispositivo, supressão de ruído, cancelamento de eco, ganho automático, passa-alta, noise gate (com a linha
  do limiar no medidor), compressor, ganho, limitador, monitoramento no fone e atraso de sincronia.
- **Captura de janela com áudio**: nas propriedades de uma janela, *Capturar o áudio desta janela* cria o canal dela.
- **Sem câmera**: botão *Câmera* na barra (Ctrl+Shift+C). Desligada, as fontes de câmera somem e os overlays do kit
  escondem a moldura (gameplay sem moldura, "Só conversando" vira um cartaz com o seu nome).
- **Chat da Twitch** em tempo real na lateral do Estúdio, ou numa **janela separada** para o segundo monitor (pode
  ficar por cima do jogo). Com o login da Twitch dá para escrever por ali.
- **Título e categoria da live** na própria barra de baixo; vão para a Twitch ao entrar ao vivo (e dá para mudar no
  meio da live). Os destinos da barra ligam/desligam com um clique para a live de hoje.
- **Desfazer** (Ctrl+Z / Ctrl+Shift+Z) para fontes e cenas, teclas **1–9** trocam de cena, duplo clique renomeia.
- **Tela de pausa**: cobre tudo na hora (Ctrl+Shift+P).
- **Gravação e replay**: grava em MP4 em `Vídeos\PrismCast`; *Replay* salva os últimos segundos (Ctrl+Shift+S).

## Widgets (browser sources)

Cada widget roda numa janela Chromium invisível e transparente; os quadros entram no canvas do compositor, então
aparecem **na live e na gravação**, não só na pré-visualização. Funciona com:

- **Kit de overlays** (pasta com `obs/*.html`): Começando, Volto já, Encerrando, Só conversando, Overlay de gameplay,
  Moldura de câmera e Chat da Twitch. As opções de cada tela (contagem, canto da câmera, tempo para o chat sumir…)
  aparecem como controles no painel de propriedades; os textos comuns a todas (jogo do dia, recados, redes, próxima
  live) mudam em **Widgets → Textos do kit**, que grava o `src/config.js` do kit e atualiza tudo na hora.
- Contagem regressiva e cronômetro **recomeçam quando a cena entra no ar** (opção por widget).
- **StreamElements** e qualquer URL `https://`, ou um arquivo `.html` local.

## Destinos e login

- As chaves de transmissão ficam criptografadas com o DPAPI do Windows (`safeStorage`) num arquivo separado e nunca
  voltam para a interface depois de salvas. O formato é validado (Twitch `live_…`, YouTube `xxxx-xxxx-…`, Kick `sk_…`).
- **Entrar com a Twitch** usa OAuth (fluxo *device code*): o app mostra um código, você confirma em
  `twitch.tv/activate` e ele busca nome, avatar e a chave pela API. O Client ID do app PrismCast (tipo **Público**,
  sem secret) já vem embutido em `src/shared/ipc.ts`, então ninguém precisa registrar nada. Para usar um app
  próprio (dev.twitch.tv, tipo Público, redirect `https://localhost`), cole o ID em Destinos → Twitch → *Avançado*.
- Kick e TikTok mudam o servidor por conta: copie servidor e chave do painel de cada um.

## Como a live sai

O canvas (1920×1080, e 1080×1920 se houver destino vertical) é gravado com `MediaRecorder` e enviado ao FFmpeg,
um processo por destino: H.264 por hardware (NVENC → AMF → QSV, testados de verdade; senão x264), GOP de 2 s, AAC
48 kHz, FLV/RTMP. Um destino que cai não derruba os outros, e o erro aparece traduzido no próprio destino.

**Requer FFmpeg no PATH** (`winget install ffmpeg`). Para RTMPS (Kick), use uma build "full".

## Atalhos globais

| Atalho | Ação |
|---|---|
| `Ctrl+Shift+M` | Mutar / ligar microfone |
| `Ctrl+Shift+D` | Mutar / ligar áudio do PC |
| `Ctrl+Shift+P` | Tela de pausa |
| `Ctrl+Shift+S` | Salvar replay |
| `Ctrl+Shift+V` | Ligar / desligar saída 9:16 |
| `Ctrl+Shift+C` | Ligar / desligar câmera |

Os atalhos podem ser trocados em **Ajustes → Atalhos globais**.

## Desenvolvimento

```bash
npm install
npm run dev          # app em modo desenvolvimento
npm run typecheck
npm run build        # typecheck + cargo build --release + electron-vite build
npm run dist         # executável portátil em dist/
```

### Áudio de aplicativo (`src-rust/audio_helper.rs`)

`prismcast-audio.exe` usa o loopback por processo do WASAPI (Windows 10 2004+). O main abre um por canal
(`capture --exe jogo.exe`) e repassa o PCM (f32, 48 kHz, estéreo) para um AudioWorklet do mixer. Programa fechado não
é erro: o canal fica esperando ele abrir. `npm run build` compila junto; no `.exe` portátil ele vai em
`app.asar.unpacked`.

### Sobre o `src-rust/`

O motor em Rust (`prismcast.exe`) é iniciado junto com o app quando existe em `target/`, mas **a interface ainda
não conversa com ele**: descoberta na LAN, sinalização, multistream e gravação rodam no processo main do Electron
(`src/main`). O RPC do Rust (porta 41236) fica disponível para uma migração futura.

### Preferências

`%APPDATA%\prismcast-studio\prismcast-settings.json` (cenas, destinos, ajustes) e `prismcast-secrets.json` (chaves,
criptografadas). A versão 2.2 migra o arquivo da 2.1 sozinha: as cenas de demonstração e as chaves que a 2.1
inventava ao clicar em "Conectar" são descartadas; chaves reais são mantidas.
