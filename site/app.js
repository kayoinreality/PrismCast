(() => {
  'use strict';

  const REPO = 'kayoinreality/PrismCast';
  const RELEASES_PAGE = `https://github.com/${REPO}/releases/latest`;
  const RELEASES_API = `https://api.github.com/repos/${REPO}/releases/latest`;
  const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  // ------------------------------------------------------- Interactive Studio State
  const state = {
    scene: 'gameplay',
    dualOutput: true,
    cameraOn: true,
    transitionMode: 'fade', // 'fade' | 'cut'
    transitionAlpha: 1,
    countdownSec: 294,
    channels: {
      desktop: { db: -6, muted: false, level: 0.45, peak: 0.52 },
      mic: { db: -2, muted: false, level: 0.58, peak: 0.68 },
      app: { db: -10, muted: false, level: 0.32, peak: 0.4 }
    }
  };

  const sceneLabels = {
    gameplay: 'Gameplay + Cam',
    chatting: 'Só conversando',
    starting: 'Começando em breve',
    brb: 'Volto já (Escudo)'
  };

  // ------------------------------------------------------- Canvas Drawing Helpers
  const wideCanvas = document.getElementById('canvas-wide');
  const vertCanvas = document.getElementById('canvas-vertical');
  const vertUnit = document.getElementById('monitor-vertical-unit');

  function setupCanvas(canvas, w, h) {
    if (!canvas) return null;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    const ctx = canvas.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    return { ctx, w, h };
  }

  const wide = setupCanvas(wideCanvas, 640, 360);
  const vert = setupCanvas(vertCanvas, 216, 384);

  function formatTimecode(t) {
    const total = Math.floor(t);
    const hrs = String(Math.floor(total / 3600) % 24).padStart(2, '0');
    const mins = String(Math.floor((total % 3600) / 60)).padStart(2, '0');
    const secs = String(total % 60).padStart(2, '0');
    const frames = String(Math.floor((t % 1) * 60)).padStart(2, '0');
    return `${hrs}:${mins}:${secs}:${frames}`;
  }

  function formatCountdown(sec) {
    const s = Math.max(0, Math.floor(sec));
    const m = String(Math.floor(s / 60)).padStart(2, '0');
    const r = String(s % 60).padStart(2, '0');
    return `${m}:${r}`;
  }

  function drawWebcamPip(ctx, x, y, w, h, compact) {
    ctx.save();
    ctx.fillStyle = '#1b1a18';
    ctx.fillRect(x, y, w, h);

    // Warm tungsten key light gradient inside camera frame
    const g = ctx.createLinearGradient(x, y, x + w, y + h);
    g.addColorStop(0, '#2c2721');
    g.addColorStop(1, '#161513');
    ctx.fillStyle = g;
    ctx.fillRect(x + 1, y + 1, w - 2, h - 2);

    // Subtle silhouette (host at studio desk)
    ctx.fillStyle = '#3c3730';
    ctx.beginPath();
    ctx.arc(x + w * 0.5, y + h * 0.42, Math.min(w, h) * 0.19, 0, Math.PI * 2);
    ctx.fill();
    ctx.beginPath();
    ctx.ellipse(x + w * 0.5, y + h * 0.96, w * 0.34, h * 0.36, 0, Math.PI, 0);
    ctx.fill();

    // Broadcast frame rule + corner ticks
    ctx.strokeStyle = 'rgba(241, 237, 228, 0.55)';
    ctx.lineWidth = 1;
    ctx.strokeRect(x + 0.5, y + 0.5, w - 1, h - 1);

    ctx.fillStyle = '#d63626';
    ctx.fillRect(x + 6, y + 6, 5, 5);

    if (!compact) {
      ctx.fillStyle = 'rgba(241, 237, 228, 0.78)';
      ctx.font = '500 9px "IBM Plex Mono", monospace';
      ctx.fillText('CAM 01 · 1080p', x + 15, y + 12);
    }
    ctx.restore();
  }

  function drawSceneFrame(target, t, isVertical) {
    if (!target) return;
    const { ctx, w, h } = target;
    ctx.save();

    // Base studio canvas
    ctx.fillStyle = '#11110f';
    ctx.fillRect(0, 0, w, h);

    if (state.scene === 'gameplay') {
      // Procedural motorsport/tactical dark radar viewport (no purple neon!)
      const horizonY = isVertical ? h * 0.44 : h * 0.5;
      const sky = ctx.createLinearGradient(0, 0, 0, horizonY);
      sky.addColorStop(0, '#131618');
      sky.addColorStop(1, '#23201c');
      ctx.fillStyle = sky;
      ctx.fillRect(0, 0, w, horizonY);

      // Asphalt surface
      const road = ctx.createLinearGradient(0, horizonY, 0, h);
      road.addColorStop(0, '#191815');
      road.addColorStop(1, '#0e0e0c');
      ctx.fillStyle = road;
      ctx.fillRect(0, horizonY, w, h - horizonY);

      // Perspective lane lines (warm sodium streetlamp feel)
      const cx = w * 0.5;
      ctx.strokeStyle = 'rgba(235, 196, 128, 0.22)';
      ctx.lineWidth = 1;
      for (let i = -3; i <= 3; i++) {
        ctx.beginPath();
        ctx.moveTo(cx + i * (isVertical ? 14 : 26), horizonY);
        ctx.lineTo(cx + i * (isVertical ? 96 : 210), h);
        ctx.stroke();
      }

      // Moving road markers
      const speed = (t * 0.75) % 1;
      for (let i = 0; i < 6; i++) {
        const p = (i / 6 + speed / 6) % 1;
        const y = horizonY + p * p * (h - horizonY);
        const half = (isVertical ? 18 : 34) + p * p * (isVertical ? 88 : 190);
        ctx.strokeStyle = `rgba(241, 237, 228, ${0.06 + p * 0.18})`;
        ctx.beginPath();
        ctx.moveTo(cx - half, y);
        ctx.lineTo(cx + half, y);
        ctx.stroke();
      }

      // Warm horizon glow + sodium streetlamps + subtle rain reflection on asphalt
      const horizonGlow = ctx.createRadialGradient(cx, horizonY, 6, cx, horizonY, isVertical ? 90 : 190);
      horizonGlow.addColorStop(0, 'rgba(227, 159, 68, 0.22)');
      horizonGlow.addColorStop(0.55, 'rgba(214, 54, 38, 0.08)');
      horizonGlow.addColorStop(1, 'rgba(17, 17, 15, 0)');
      ctx.fillStyle = horizonGlow;
      ctx.fillRect(0, horizonY - 56, w, 128);

      // Skyline silhouette along horizon
      ctx.fillStyle = '#151513';
      const blocks = isVertical
        ? [[0.06, 22, 18], [0.22, 34, 28], [0.46, 24, 20], [0.68, 38, 32], [0.85, 20, 14]]
        : [[0.04, 48, 22], [0.14, 62, 36], [0.28, 42, 20], [0.41, 74, 42], [0.58, 52, 26], [0.72, 66, 34], [0.87, 50, 22]];
      blocks.forEach(([px, bw, bh]) => {
        ctx.fillRect(w * px, horizonY - bh, bw, bh);
      });

      // Warm horizon lamp dots & subtle wet reflections
      for (let i = 0; i < 5; i++) {
        const lx = w * (0.14 + i * 0.18);
        ctx.fillStyle = 'rgba(235, 174, 78, 0.65)';
        ctx.fillRect(lx, horizonY - 4, 4, 2);
        ctx.fillStyle = 'rgba(235, 174, 78, 0.14)';
        ctx.fillRect(lx - 1, horizonY + 3, 6, 18);
      }

      // Minimal HUD telemetry inside game capture
      ctx.fillStyle = 'rgba(241, 237, 228, 0.75)';
      ctx.font = `600 ${isVertical ? 9 : 11}px "IBM Plex Mono", monospace`;
      ctx.fillText('248 KM/H · LAP 04', 14, 22);

      // Camera PiP (respects Camera ON/OFF toggle)
      if (state.cameraOn) {
        if (isVertical) {
          drawWebcamPip(ctx, 14, h - 122, w - 28, 86, true);
        } else {
          drawWebcamPip(ctx, w - 168, h - 114, 152, 98, false);
        }
      }

      // Lower-third broadcast overlay tag
      const tagY = isVertical ? h - 26 : h - 28;
      ctx.fillStyle = 'rgba(17, 17, 15, 0.86)';
      ctx.fillRect(14, tagY - 12, isVertical ? w - 28 : 228, 22);
      ctx.fillStyle = '#d63626';
      ctx.fillRect(14, tagY - 12, 3, 22);
      ctx.fillStyle = '#f1ede4';
      ctx.font = '500 10px "IBM Plex Mono", monospace';
      ctx.fillText(
        state.cameraOn ? 'OVERLAY · GAMEPLAY + CAM' : 'OVERLAY · SEM MOLDURA',
        24,
        tagY + 2
      );
    } else if (state.scene === 'chatting') {
      // Studio talking layout
      ctx.fillStyle = '#171613';
      ctx.fillRect(0, 0, w, h);

      if (state.cameraOn) {
        if (isVertical) {
          drawWebcamPip(ctx, 14, 22, w - 28, h * 0.56, false);
          ctx.fillStyle = '#f1ede4';
          ctx.font = '600 11px "IBM Plex Mono", monospace';
          ctx.fillText('SÓ CONVERSANDO', 14, h * 0.56 + 48);
          ctx.fillStyle = '#9a9488';
          ctx.font = '400 10px "IBM Plex Sans", sans-serif';
          ctx.fillText('Bate-papo ao vivo + Multichat', 14, h * 0.56 + 66);
        } else {
          drawWebcamPip(ctx, 22, 24, w * 0.62, h - 48, false);
          const rx = w * 0.62 + 40;
          ctx.fillStyle = '#d63626';
          ctx.fillRect(rx, 28, 26, 3);
          ctx.fillStyle = '#f1ede4';
          ctx.font = '700 18px "Bricolage Grotesque", sans-serif';
          ctx.fillText('Só conversando', rx, 56);
          ctx.fillStyle = '#9a9488';
          ctx.font = '400 11px "IBM Plex Mono", monospace';
          ctx.fillText('TWITCH · YT · KICK', rx, 78);
          ctx.strokeStyle = 'rgba(241, 237, 228, 0.14)';
          ctx.beginPath();
          ctx.moveTo(rx, 94);
          ctx.lineTo(w - 22, 94);
          ctx.stroke();
          ctx.fillStyle = '#c8c3b8';
          ctx.font = '400 11px "IBM Plex Sans", sans-serif';
          ctx.fillText('Tópico da live:', rx, 118);
          ctx.fillStyle = '#f1ede4';
          ctx.fillText('Testando motor Rust', rx, 136);
          ctx.fillText('e mixer 48 kHz', rx, 152);
        }
      } else {
        // Real PrismCast no-camera mode: clean typographic host placard
        ctx.strokeStyle = 'rgba(241, 237, 228, 0.16)';
        ctx.strokeRect(18, 18, w - 36, h - 36);
        ctx.fillStyle = '#d63626';
        ctx.fillRect(w * 0.5 - 16, h * 0.42 - 22, 32, 3);
        ctx.fillStyle = '#f1ede4';
        ctx.textAlign = 'center';
        ctx.font = `700 ${isVertical ? 18 : 26}px "Bricolage Grotesque", sans-serif`;
        ctx.fillText('Só conversando', w * 0.5, h * 0.46 + 4);
        ctx.fillStyle = '#9a9488';
        ctx.font = '400 11px "IBM Plex Mono", monospace';
        ctx.fillText('MODO SEM CÂMERA (CTRL+SHIFT+C)', w * 0.5, h * 0.46 + 26);
        ctx.textAlign = 'left';
      }
    } else if (state.scene === 'starting') {
      ctx.fillStyle = '#141412';
      ctx.fillRect(0, 0, w, h);

      // Subtle architectural frame
      ctx.strokeStyle = 'rgba(241, 237, 228, 0.14)';
      ctx.strokeRect(16, 16, w - 32, h - 32);

      ctx.textAlign = 'center';
      ctx.fillStyle = '#9a9488';
      ctx.font = '500 10px "IBM Plex Mono", monospace';
      ctx.fillText('COMEÇANDO EM BREVE', w * 0.5, h * 0.38);

      ctx.fillStyle = '#f1ede4';
      ctx.font = `700 ${isVertical ? 40 : 62}px "IBM Plex Mono", monospace`;
      ctx.fillText(formatCountdown(state.countdownSec), w * 0.5, h * 0.55);

      ctx.fillStyle = '#d63626';
      ctx.fillRect(w * 0.5 - 24, h * 0.63, 48, 2);

      ctx.fillStyle = '#a19b8e';
      ctx.font = '400 11px "IBM Plex Sans", sans-serif';
      ctx.fillText('Kit OBS · Timer sincronizado ao entrar no ar', w * 0.5, h * 0.72);
      ctx.textAlign = 'left';
    } else if (state.scene === 'brb') {
      // Privacy shield / BRB screen
      ctx.fillStyle = '#121210';
      ctx.fillRect(0, 0, w, h);

      // Clean broadcast color bar reference strip at top
      const bars = ['#e6e2d8', '#d9a441', '#4aa89b', '#33b06b', '#d63626', '#33312b'];
      const bw = w / bars.length;
      bars.forEach((c, idx) => {
        ctx.fillStyle = c;
        ctx.globalAlpha = 0.75;
        ctx.fillRect(idx * bw, 0, bw + 1, 5);
      });
      ctx.globalAlpha = 1;

      ctx.textAlign = 'center';
      ctx.fillStyle = '#f1ede4';
      ctx.font = `700 ${isVertical ? 22 : 34}px "Bricolage Grotesque", sans-serif`;
      ctx.fillText('Volto já', w * 0.5, h * 0.48);

      ctx.fillStyle = '#9a9488';
      ctx.font = '400 10.5px "IBM Plex Mono", monospace';
      ctx.fillText('TELA DE PAUSA ATIVA · CTRL+SHIFT+P', w * 0.5, h * 0.48 + 26);
      ctx.textAlign = 'left';
    }

    // Subtle broadcast safe-area corner brackets + live timecode
    ctx.strokeStyle = 'rgba(241, 237, 228, 0.25)';
    ctx.lineWidth = 1;
    const m = 8;
    const len = 8;
    // Top-left & top-right
    ctx.beginPath();
    ctx.moveTo(m, m + len); ctx.lineTo(m, m); ctx.lineTo(m + len, m);
    ctx.moveTo(w - m - len, m); ctx.lineTo(w - m, m); ctx.lineTo(w - m, m + len);
    ctx.moveTo(m, h - m - len); ctx.lineTo(m, h - m); ctx.lineTo(m + len, h - m);
    ctx.moveTo(w - m - len, h - m); ctx.lineTo(w - m, h - m); ctx.lineTo(w - m, h - m - len);
    ctx.stroke();

    if (!isVertical) {
      ctx.fillStyle = 'rgba(241, 237, 228, 0.65)';
      ctx.font = '500 10px "IBM Plex Mono", monospace';
      ctx.textAlign = 'right';
      ctx.fillText(`TC ${formatTimecode(5412 + t)}`, w - 14, 22);
      ctx.textAlign = 'left';
    }

    // Smooth fade transition flash when switching scenes in Fade mode
    if (state.transitionAlpha < 1) {
      ctx.fillStyle = `rgba(12, 12, 10, ${1 - state.transitionAlpha})`;
      ctx.fillRect(0, 0, w, h);
    }

    ctx.restore();
  }

  // ------------------------------------------------------- Stereo VU Meters
  const meterEls = {
    desktop: document.getElementById('meter-desktop'),
    mic: document.getElementById('meter-mic'),
    app: document.getElementById('meter-app')
  };

  function drawMeter(canvas, level, peak, muted) {
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    const w = Math.max(120, Math.round(rect.width));
    const h = 12;
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    const ctx = canvas.getContext('2d');
    ctx.clearRect(0, 0, w, h);

    const segments = 34;
    const gap = 2;
    const segW = (w - gap * (segments - 1)) / segments;
    const activeCount = muted ? 0 : Math.round(level * segments);
    const peakIdx = muted ? -1 : Math.min(segments - 1, Math.round(peak * segments));

    for (let i = 0; i < segments; i++) {
      const ratio = i / segments;
      let color = '#33b06b';
      if (ratio > 0.84) color = '#e03e2d';
      else if (ratio > 0.66) color = '#d98a22';

      const x = i * (segW + gap);
      if (i < activeCount || i === peakIdx) {
        ctx.fillStyle = color;
        ctx.globalAlpha = i === peakIdx && i >= activeCount ? 0.95 : 0.9;
      } else {
        ctx.fillStyle = '#252420';
        ctx.globalAlpha = 1;
      }
      ctx.fillRect(x, 1, segW, h - 2);
    }
    ctx.globalAlpha = 1;
  }

  function dbToGain(db) {
    if (db <= -59) return 0;
    return Math.min(1.25, Math.pow(10, db / 20));
  }

  function formatDb(db) {
    if (db <= -59) return '-∞ dB';
    return `${db > 0 ? '+' : ''}${db.toFixed(1)} dB`;
  }

  // ------------------------------------------------------- Bind Console UI
  const activeSceneReadout = document.getElementById('active-scene-readout');

  document.querySelectorAll('[data-scene]').forEach((btn) => {
    btn.addEventListener('click', () => {
      const next = btn.getAttribute('data-scene');
      if (!next || next === state.scene) return;
      state.scene = next;
      if (state.transitionMode === 'fade' && !reducedMotion) {
        state.transitionAlpha = 0.15;
      }
      document.querySelectorAll('[data-scene]').forEach((b) => {
        b.setAttribute('aria-pressed', String(b === btn));
      });
      if (activeSceneReadout) {
        activeSceneReadout.textContent = sceneLabels[next] || next;
      }
      if (reducedMotion) renderOnce(performance.now() / 1000);
    });
  });

  const btnDual = document.getElementById('toggle-dual');
  if (btnDual) {
    btnDual.addEventListener('click', () => {
      state.dualOutput = !state.dualOutput;
      btnDual.setAttribute('aria-pressed', String(state.dualOutput));
      if (vertUnit) vertUnit.classList.toggle('is-hidden', !state.dualOutput);
      if (reducedMotion) renderOnce(performance.now() / 1000);
    });
  }

  const btnCam = document.getElementById('toggle-camera');
  if (btnCam) {
    btnCam.addEventListener('click', () => {
      state.cameraOn = !state.cameraOn;
      btnCam.setAttribute('aria-pressed', String(state.cameraOn));
      btnCam.textContent = state.cameraOn ? 'Câmera: Ligada' : 'Câmera: Oculta';
      if (reducedMotion) renderOnce(performance.now() / 1000);
    });
  }

  const btnTrans = document.getElementById('toggle-transition');
  if (btnTrans) {
    btnTrans.addEventListener('click', () => {
      state.transitionMode = state.transitionMode === 'fade' ? 'cut' : 'fade';
      btnTrans.textContent =
        state.transitionMode === 'fade' ? 'Transição: Esmaecer' : 'Transição: Corte';
    });
  }

  ['desktop', 'mic', 'app'].forEach((id) => {
    const fader = document.getElementById(`fader-${id}`);
    const readout = document.getElementById(`db-${id}`);
    const muteBtn = document.getElementById(`mute-${id}`);
    const ch = state.channels[id];

    if (fader) {
      fader.addEventListener('input', () => {
        ch.db = parseFloat(fader.value);
        if (readout) readout.textContent = ch.muted ? 'MUDO' : formatDb(ch.db);
      });
    }
    if (muteBtn) {
      muteBtn.addEventListener('click', () => {
        ch.muted = !ch.muted;
        muteBtn.setAttribute('aria-pressed', String(ch.muted));
        muteBtn.textContent = ch.muted ? 'MUDO' : 'ATIVO';
        if (readout) readout.textContent = ch.muted ? 'MUDO' : formatDb(ch.db);
      });
    }
  });

  // ------------------------------------------------------- Animation Loop
  let lastTs = performance.now();

  function renderOnce(t) {
    drawSceneFrame(wide, t, false);
    if (state.dualOutput) drawSceneFrame(vert, t, true);
    ['desktop', 'mic', 'app'].forEach((id) => {
      const ch = state.channels[id];
      const gain = dbToGain(ch.db);
      drawMeter(meterEls[id], ch.muted ? 0 : Math.min(0.92, ch.level * gain), ch.peak * gain, ch.muted);
    });
  }

  function tick(now) {
    const dt = Math.min(0.1, (now - lastTs) / 1000);
    lastTs = now;
    const t = now / 1000;

    if (state.transitionAlpha < 1) {
      state.transitionAlpha = Math.min(1, state.transitionAlpha + dt * 4.5);
    }

    if (state.scene === 'starting') {
      state.countdownSec -= dt;
      if (state.countdownSec <= 0) state.countdownSec = 300;
    }

    // Realistic audio dynamics + subtle sidechain ducking when mic speaks
    const micVoice =
      Math.max(0, Math.sin(t * 3.1) * 0.5 + Math.sin(t * 7.4) * 0.28 + Math.sin(t * 13.2) * 0.16);
    const micActive = !state.channels.mic.muted && dbToGain(state.channels.mic.db) > 0.1 && micVoice > 0.25;
    const duckFactor = micActive ? 0.68 : 1.0;

    const rawLevels = {
      desktop: (0.34 + Math.abs(Math.sin(t * 2.2) * 0.22 + Math.cos(t * 5.7) * 0.14)) * duckFactor,
      mic: 0.12 + micVoice * 0.72,
      app: 0.24 + Math.abs(Math.sin(t * 1.6 + 1.2) * 0.24 + Math.sin(t * 4.3) * 0.12)
    };

    ['desktop', 'mic', 'app'].forEach((id) => {
      const ch = state.channels[id];
      const gain = dbToGain(ch.db);
      const target = ch.muted ? 0 : Math.min(0.96, rawLevels[id] * gain);
      ch.level += (target - ch.level) * Math.min(1, dt * 14);
      if (ch.level > ch.peak) ch.peak = ch.level;
      else ch.peak = Math.max(ch.level, ch.peak - dt * 0.35);
      drawMeter(meterEls[id], ch.level, ch.peak, ch.muted);
    });

    drawSceneFrame(wide, t, false);
    if (state.dualOutput) drawSceneFrame(vert, t, true);

    requestAnimationFrame(tick);
  }

  if (reducedMotion) {
    renderOnce(0);
  } else {
    requestAnimationFrame(tick);
  }

  // ------------------------------------------------------- OS Tabs & GitHub Releases
  function detectOs() {
    const plat = (
      (navigator.userAgentData && navigator.userAgentData.platform) ||
      navigator.platform ||
      navigator.userAgent ||
      ''
    ).toLowerCase();
    if (plat.includes('linux')) return 'linux';
    return 'windows';
  }

  const osTabs = document.querySelectorAll('[data-os-tab]');
  const osPanels = document.querySelectorAll('[data-os-panel]');
  const heroPrimaryCta = document.getElementById('hero-download-btn');
  const heroFootnote = document.getElementById('hero-download-note');
  const ffmpegCmd = document.getElementById('dl-ffmpeg-cmd');

  function selectOs(os) {
    osTabs.forEach((tab) => {
      const active = tab.getAttribute('data-os-tab') === os;
      tab.setAttribute('aria-selected', String(active));
    });
    osPanels.forEach((panel) => {
      const active = panel.getAttribute('data-os-panel') === os;
      panel.hidden = !active;
    });
    if (heroPrimaryCta) {
      heroPrimaryCta.textContent = os === 'linux' ? 'Baixar para Linux' : 'Baixar para Windows';
    }
    if (heroFootnote) {
      heroFootnote.textContent =
        os === 'linux'
          ? 'AppImage, .deb, .rpm e .tar.gz · x86_64 · Licença MIT'
          : 'Windows 10 e 11 (64 bits) · Portátil .exe, NSIS e MSI · Licença MIT';
    }
    if (ffmpegCmd) {
      ffmpegCmd.textContent = os === 'linux' ? 'sudo apt install ffmpeg' : 'winget install ffmpeg';
    }
  }

  osTabs.forEach((tab) => {
    tab.addEventListener('click', () => {
      selectOs(tab.getAttribute('data-os-tab'));
    });
  });

  selectOs(detectOs());

  function formatBytes(bytes) {
    if (!bytes || typeof bytes !== 'number') return '';
    const mb = bytes / (1024 * 1024);
    return `${mb.toFixed(1)} MB`;
  }

  async function hydrateRelease() {
    try {
      const ctrl = new AbortController();
      const timer = setTimeout(() => ctrl.abort(), 4500);
      const res = await fetch(RELEASES_API, {
        headers: { Accept: 'application/vnd.github+json' },
        signal: ctrl.signal
      });
      clearTimeout(timer);
      if (!res.ok) return;
      const rel = await res.json();
      if (!rel || !rel.tag_name) return;

      document.querySelectorAll('[data-release-version]').forEach((el) => {
        el.textContent = rel.tag_name;
      });

      const assets = Array.isArray(rel.assets) ? rel.assets : [];
      const findAsset = (pred) => assets.find((a) => a && typeof a.name === 'string' && pred(a.name.toLowerCase()));

      const matches = {
        'win-portable': findAsset((n) => n.endsWith('.exe') && !n.includes('setup')),
        'win-nsis': findAsset((n) => n.endsWith('-setup.exe') || (n.endsWith('.exe') && n.includes('setup'))),
        'win-msi': findAsset((n) => n.endsWith('.msi')),
        'linux-appimage': findAsset((n) => n.endsWith('.appimage')),
        'linux-deb': findAsset((n) => n.endsWith('.deb')),
        'linux-rpm': findAsset((n) => n.endsWith('.rpm')),
        'linux-tar': findAsset((n) => n.endsWith('.tar.gz'))
      };

      Object.entries(matches).forEach(([key, asset]) => {
        const link = document.querySelector(`[data-asset-link="${key}"]`);
        const meta = document.querySelector(`[data-asset-meta="${key}"]`);
        if (link && asset && asset.browser_download_url) {
          link.href = asset.browser_download_url;
        }
        if (meta && asset) {
          const sizeStr = formatBytes(asset.size);
          meta.textContent = sizeStr ? `${asset.name} · ${sizeStr}` : asset.name;
        }
      });
    } catch {
      // Offline or GitHub API rate-limited: fallback links to releases/latest stay active.
      document.querySelectorAll('[data-asset-link]').forEach((link) => {
        if (!link.getAttribute('href')) link.href = RELEASES_PAGE;
      });
    }
  }

  hydrateRelease();
})();
