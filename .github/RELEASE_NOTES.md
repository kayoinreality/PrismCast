# PrismCast Studio v3.2.0 — Nova interface Prism (Windows & Linux)

A **v3.2.0** leva a interface nativa para a linguagem **Prism** do [`egui-prismglass` 0.3](https://crates.io/crates/egui-prismglass) e redesenha a experiência do estúdio.

### Visual
- **Tipografia Inter** embutida (3 pesos + algarismos tabulares para FPS, dB e cronômetros), rampa neutra calibrada, uma cor de destaque e bordas hairline.
- **Material Layered**: conteúdo sólido e legível, vidro fosco só no que flutua (trilho lateral, menus, toasts).
- **Prism Carbon** reconstruído na nova linguagem (neutros carbono quentes + Aurora espectral) e novos temas **Prism Dark/Light** e **Prism Glass Dark/Light**. Os 5 temas clássicos continuam disponíveis.
- **9 cores de destaque** (padrão: turquesa *Sinal* da marca) e galeria de temas com miniaturas renderizadas ao vivo.
- **46 ícones vetoriais nativos** no lugar de emojis.

### Experiência
- Barra superior com contexto (aba › cena), **Tally com cronômetro** da live/gravação e ações agrupadas.
- **Barra de status** com telemetria: FPS, render p95, quadros perdidos, bitrate, encoder, GPU e zero-copy.
- Docks de Cenas/Fontes/Mixer redesenhados: **renomear cena com duplo clique**, menu "Adicionar fonte", estados vazios explicativos.
- Inspetor com abas segmentadas (Propriedades · Chat), Multichat com chips por plataforma.
- Atalho claro/escuro no trilho lateral; telas de Destinos, Widgets, Rede local e Ajustes centralizadas numa coluna legível.

### Compatibilidade
- Preferências antigas são migradas automaticamente (novos campos `accent` e `showStatusBar` com valores padrão).
