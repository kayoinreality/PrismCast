# PrismCast Studio v2.5.1 (Windows & Linux)

A **v2.5.1** corrige o crash na inicialização no Windows/Linux, remove a abertura forçada do seletor de compartilhamento de tela ao abrir o aplicativo e adiciona o **serviço de atualização automática integrado ao GitHub Releases**.

---

## Correções

- **Crash na inicialização (`Cannot read properties of undefined (reading 'title')`)**: corrigida a serialização de `AppSettings.live_info` no backend Rust (`#[serde(rename = "live", alias = "liveInfo")]`), alinhando o JSON enviado ao frontend e preservando compatibilidade com arquivos de configuração já salvos, além de proteção defensiva em `ControlBar` e `GoLive`.
- **Abertura limpa do estúdio sem popup forçado de compartilhamento**: `CaptureManager` e `StudioEngine` deixam de disparar `navigator.mediaDevices.getDisplayMedia()` automaticamente no boot para fontes de tela/janela e áudio do PC. O usuário entra no estúdio normalmente e inicia a captura sob demanda com um clique direto no palco, no painel de Propriedades, ao adicionar uma fonte ou no canal de áudio do PC no mixer.

---

## Novidades

- **Serviço de atualização automática via GitHub Releases (`updater.rs`)**:
  - Verificação automática não-bloqueante de novas builds publicadas no GitHub Releases (e botão manual em **Ajustes → Atualizações automáticas**).
  - Detecção automática do tipo de distribuição (**Windows Instalador NSIS/MSI**, **Windows Portable `.exe`**, **Linux AppImage** e pacotes **`.deb`/`.rpm`/`.tar.gz`**).
  - Download em streaming com barra de progresso em tempo real, cancelamento e validação de integridade **SHA-256** contra `SHA256SUMS.txt` gerado no CI.
  - Banner discreto no topo quando houver nova versão disponível, com proteção contra atualização acidental durante transmissões ou gravações ativas.

---

## Pacotes disponíveis nesta release
- **Windows (x64)**: executável portátil (`.exe`), instalador **NSIS** (`.exe`) e instalador **MSI** (`.msi`).
- **Linux (x64)**: **AppImage**, pacote **Debian/Ubuntu (`.deb`)**, pacote **Fedora/RPM (`.rpm`)** e arquivo **`.tar.gz`** com `prismcast` + `prismcast-audio`.
- **Integridade**: manifesto `SHA256SUMS.txt` assinado por hash SHA-256 para validação automática pelo app.
