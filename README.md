# PrismCast Studio (`v2.2.0`)

Estúdio de transmissão para Windows: monta cenas com tela, janela, câmera e widgets HTML, e transmite ao mesmo
tempo para Twitch, YouTube, Kick, TikTok e RTMP próprio. Também compartilha o que está no ar com outros PCs da rede
local, com latência de LAN.

---

## Como usar

1. **Destinos** → conecte a Twitch (botão *Entrar com a Twitch* ou colando a chave) e as outras plataformas.
2. **Widgets** → aponte para a pasta do seu kit (ex.: `E:\kayoinreality-kit`) e clique em *Criar cenas do kit*.
3. **Estúdio** → escolha a cena, ajuste as fontes no palco e clique em **Entrar ao vivo**.

## Estúdio

- **Palco editável**: clique numa fonte para selecionar, arraste para mover, puxe os cantos para redimensionar
  (Shift solta a proporção), setas para ajuste fino, Delete remove. Encaixa nas bordas e no centro.
- **Fontes**: tela, janela (cada fonte tem a própria captura), câmera, widget/navegador, imagem, texto e cor.
- **Saída 9:16**: um segundo canvas para TikTok/Shorts, com posição própria de cada fonte.
- **Modo estúdio**: edite a prévia enquanto outra cena está no ar e mande com *Transição* (corte ou esmaecer).
- **Mixer**: áudio do PC (loopback) e microfone, com redução de ruído, cancelamento de eco e compressor.
- **Tela de pausa**: cobre tudo na hora (Ctrl+Shift+P).
- **Gravação e replay**: grava em MP4 em `Vídeos\PrismCast`; *Replay* salva os últimos segundos (Ctrl+Shift+S).

## Widgets (browser sources)

Cada widget roda numa janela Chromium invisível e transparente; os quadros entram no canvas do compositor, então
aparecem **na live e na gravação**, não só na pré-visualização. Funciona com:

- **Kit de overlays** (pasta com `obs/*.html`): Começando, Volto já, Encerrando, Só conversando, Overlay de gameplay,
  Moldura de câmera e Chat da Twitch. Os parâmetros de cada tela (minutos, jogo do dia, canto da câmera…) aparecem
  como campos no painel de propriedades.
- **StreamElements** e qualquer URL `https://`, ou um arquivo `.html` local.

## Destinos e login

- As chaves de transmissão ficam criptografadas com o DPAPI do Windows (`safeStorage`) num arquivo separado e nunca
  voltam para a interface depois de salvas. O formato é validado (Twitch `live_…`, YouTube `xxxx-xxxx-…`, Kick `sk_…`).
- **Entrar com a Twitch** usa OAuth (fluxo *device code*): o app mostra um código, você confirma em
  `twitch.tv/activate` e ele busca nome, avatar e a chave pela API. Precisa de um Client ID:
  1. Registre um app em <https://dev.twitch.tv/console/apps/create>.
  2. URL de redirecionamento `https://localhost` (o fluxo device code não usa, mas a Twitch exige uma URL HTTPS),
     categoria *Broadcaster Suite*, tipo de cliente **Público**.
  3. Cole o Client ID em Destinos → Twitch → *Configurar login*.
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

## Desenvolvimento

```bash
npm install
npm run dev          # app em modo desenvolvimento
npm run typecheck
npm run build        # typecheck + cargo build --release + electron-vite build
npm run dist         # executável portátil em dist/
```

### Sobre o `src-rust/`

O motor em Rust (`prismcast.exe`) é iniciado junto com o app quando existe em `target/`, mas **a interface ainda
não conversa com ele**: descoberta na LAN, sinalização, multistream e gravação rodam no processo main do Electron
(`src/main`). O RPC do Rust (porta 41236) fica disponível para uma migração futura.

### Preferências

`%APPDATA%\prismcast-studio\prismcast-settings.json` (cenas, destinos, ajustes) e `prismcast-secrets.json` (chaves,
criptografadas). A versão 2.2 migra o arquivo da 2.1 sozinha: as cenas de demonstração e as chaves que a 2.1
inventava ao clicar em "Conectar" são descartadas; chaves reais são mantidas.
