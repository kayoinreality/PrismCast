# PrismCast Studio v3.0.0 (100% Rust Nativo — Windows & Linux)

A **v3.0.0** marca a migração definitiva do PrismCast Studio para uma aplicação **100% nativa em Rust (`egui` + `wgpu 24`)**, eliminando completamente Tauri, WebView2 e `getDisplayMedia()`.

### Principais novidades
- **Interface 100% nativa em Rust + `wgpu 24`**: visual **Broadcast Carbon & Glassmorphism Espectral** alinhado à landing page do GitHub Pages (chassi `#141412`, painéis translúcidos foscos, feixes espectrais do prisma e vermilion tally `#D63626`).
- **Captura nativa sem popups de navegador**:
  - Fim do popup de compartilhamento, fim da barra `"http://tauri.localhost está compartilhando..."` e fim do roubo de foco ao selecionar uma janela.
  - Captura direta via **DXGI Desktop Duplication** + fallback GDI para monitores e **`PrintWindow(PW_RENDERFULLCONTENT)`** por `HWND` para janelas no Windows.
- **Miniaturas reais ao vivo no seletor de telas e janelas**: previews nativas com resolução e filtro automático de janelas invisíveis/cloaked (`DWMWA_CLOAKED`).
- **Correção definitiva do palco**: fontes iniciam nativamente sem botão flutuante `"Captura de janela pronta"` bloqueando outras fontes visíveis na cena.
- **Empacotamento leve e sem Node.js**: Windows Portable `.exe` + Instalador NSIS nativo e pacotes Linux `.deb` + `.tar.gz`.
