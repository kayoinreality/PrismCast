(() => {
  'use strict';

  const REPO = 'kayoinreality/PrismCast';
  const RELEASES_PAGE = `https://github.com/${REPO}/releases/latest`;
  const RELEASES_API = `https://api.github.com/repos/${REPO}/releases/latest`;
  const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  // Trigger hero masked line entrance as soon as DOM is ready
  requestAnimationFrame(() => {
    document.body.classList.add('is-loaded');
  });

  // ------------------------------------------------------- Inertial Smooth Scroll
  const smoothScroll = (() => {
    const isCoarsePointer = window.matchMedia('(pointer: coarse)').matches;
    const enabled = !reducedMotion && !isCoarsePointer;
    let currentY = window.scrollY;
    let targetY = window.scrollY;
    let active = false;
    let lastTime = performance.now();

    function maxScroll() {
      return Math.max(0, document.documentElement.scrollHeight - window.innerHeight);
    }

    function hasScrollableAncestor(el, deltaY) {
      let node = el;
      while (node && node !== document.body && node !== document.documentElement) {
        const style = window.getComputedStyle(node);
        const overflowY = style.overflowY;
        if ((overflowY === 'auto' || overflowY === 'scroll') && node.scrollHeight > node.clientHeight) {
          if ((deltaY < 0 && node.scrollTop > 0) || (deltaY > 0 && node.scrollTop + node.clientHeight < node.scrollHeight)) {
            return true;
          }
        }
        node = node.parentElement;
      }
      return false;
    }

    function step(now) {
      const dt = Math.min(0.05, Math.max(0.001, (now - lastTime) / 1000));
      lastTime = now;

      const diff = targetY - currentY;
      if (Math.abs(diff) < 0.35) {
        currentY = targetY;
        window.scrollTo(0, Math.round(currentY));
        active = false;
        return;
      }

      // Frame-rate independent exponential smoothing (silky on 60Hz/120Hz/144Hz)
      const factor = 1 - Math.exp(-9.6 * dt);
      currentY += diff * factor;
      window.scrollTo(0, currentY);
      requestAnimationFrame(step);
    }

    function start() {
      if (!active) {
        active = true;
        lastTime = performance.now();
        requestAnimationFrame(step);
      }
    }

    function cancel() {
      if (active) {
        active = false;
        currentY = window.scrollY;
        targetY = currentY;
      }
    }

    if (enabled) {
      window.addEventListener(
        'wheel',
        (ev) => {
          if (ev.ctrlKey || ev.metaKey || ev.altKey) return;
          if (hasScrollableAncestor(ev.target, ev.deltaY)) return;

          ev.preventDefault();
          let delta = ev.deltaY;
          if (ev.deltaMode === 1) delta *= 36;
          else if (ev.deltaMode === 2) delta *= window.innerHeight * 0.85;

          // Keep native sync if user dragged scrollbar or used keyboard
          if (!active) {
            currentY = window.scrollY;
            targetY = currentY;
          }
          targetY = Math.max(0, Math.min(maxScroll(), targetY + delta));
          start();
        },
        { passive: false }
      );

      // Immediately yield to keyboard scroll keys, scrollbar drags, or touch
      window.addEventListener(
        'keydown',
        (ev) => {
          if (['ArrowDown', 'ArrowUp', 'PageDown', 'PageUp', 'Home', 'End', ' '].includes(ev.key)) {
            cancel();
          }
        },
        { passive: true }
      );
      window.addEventListener('pointerdown', (ev) => {
        if (ev.clientX >= document.documentElement.clientWidth) cancel();
      }, { passive: true });
      window.addEventListener('touchstart', cancel, { passive: true });

      // Resync when native scrollbar drag or keyboard navigation moves viewport
      window.addEventListener(
        'scroll',
        () => {
          if (!active) {
            currentY = window.scrollY;
            targetY = currentY;
          }
        },
        { passive: true }
      );
    }

    function scrollToY(y) {
      const clamped = Math.max(0, Math.min(maxScroll(), y));
      if (!enabled) {
        window.scrollTo({ top: clamped, behavior: reducedMotion ? 'auto' : 'smooth' });
        return;
      }
      currentY = window.scrollY;
      targetY = clamped;
      start();
    }

    // Intercept anchor links for smooth offset gliding
    document.querySelectorAll('a[href^="#"]').forEach((link) => {
      link.addEventListener('click', (ev) => {
        const id = link.getAttribute('href');
        if (!id || id === '#') return;
        const target = document.querySelector(id);
        if (!target) return;
        ev.preventDefault();
        const headerOffset = 72;
        const y = target.getBoundingClientRect().top + window.scrollY - (id === '#topo' ? 0 : headerOffset);
        scrollToY(y);
      });
    });

    return { scrollToY };
  })();

  // ------------------------------------------------------- Scroll Choreography
  const headerEl = document.getElementById('site-header');
  const progressBar = document.getElementById('scroll-progress');
  const consoleFrame = document.getElementById('console-frame');
  const navEl = document.getElementById('site-nav');
  const navIndicator = document.getElementById('nav-indicator');
  const navLinks = navEl ? Array.from(navEl.querySelectorAll('a[href^="#"]')) : [];
  const navSections = navLinks
    .map((a) => ({ link: a, el: document.querySelector(a.getAttribute('href')) }))
    .filter((item) => item.el);

  function updateNavIndicator(activeLink) {
    if (!navIndicator || !navEl) return;
    if (!activeLink) {
      navIndicator.style.opacity = '0';
      return;
    }
    const navRect = navEl.getBoundingClientRect();
    const linkRect = activeLink.getBoundingClientRect();
    navIndicator.style.opacity = '1';
    navIndicator.style.width = `${Math.round(linkRect.width)}px`;
    navIndicator.style.transform = `translateX(${Math.round(linkRect.left - navRect.left)}px)`;
  }

  let scrollScheduled = false;
  function onScrollFrame() {
    scrollScheduled = false;
    const y = window.scrollY;
    const max = Math.max(1, document.documentElement.scrollHeight - window.innerHeight);
    const ratio = Math.max(0, Math.min(1, y / max));

    if (progressBar) {
      progressBar.style.transform = `scaleX(${ratio.toFixed(4)})`;
    }
    if (headerEl) {
      headerEl.classList.toggle('is-scrolled', y > 14);
    }

    // Subtle 3D hardware console pitch & scale settle on scroll into view
    if (consoleFrame && !reducedMotion) {
      const rect = consoleFrame.getBoundingClientRect();
      const vh = window.innerHeight;
      const enter = Math.max(0, Math.min(1, (vh - rect.top) / (vh * 0.6)));
      const eased = 1 - Math.pow(1 - enter, 2);
      const scale = 0.968 + eased * 0.032;
      const shift = (1 - eased) * 18;
      const tilt = (1 - eased) * 8.5;
      consoleFrame.style.setProperty('--console-scale', scale.toFixed(4));
      consoleFrame.style.setProperty('--console-y', `${shift.toFixed(1)}px`);
      consoleFrame.style.setProperty('--console-tilt', `${tilt.toFixed(2)}deg`);
    }

    // Active section nav tracking
    let current = null;
    const probe = y + window.innerHeight * 0.34;
    for (const item of navSections) {
      const top = item.el.getBoundingClientRect().top + y;
      if (probe >= top) current = item.link;
    }
    navLinks.forEach((a) => a.classList.toggle('is-current', a === current));
    updateNavIndicator(current);
  }

  window.addEventListener(
    'scroll',
    () => {
      if (!scrollScheduled) {
        scrollScheduled = true;
        requestAnimationFrame(onScrollFrame);
      }
    },
    { passive: true }
  );
  window.addEventListener('resize', () => requestAnimationFrame(onScrollFrame), { passive: true });
  requestAnimationFrame(onScrollFrame);

  // ------------------------------------------------------- Scroll Reveal Observer
  const revealEls = document.querySelectorAll('[data-reveal]');
  if (reducedMotion || !('IntersectionObserver' in window)) {
    revealEls.forEach((el) => el.classList.add('is-visible'));
  } else {
    const revealObs = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (entry.isIntersecting) {
            entry.target.classList.add('is-visible');
            revealObs.unobserve(entry.target);
          }
        });
      },
      { threshold: 0.12, rootMargin: '0px 0px -40px 0px' }
    );
    revealEls.forEach((el) => revealObs.observe(el));
  }

  // ------------------------------------------------------- Rolling Engineering Counters
  const counterEls = document.querySelectorAll('[data-counter]');
  if (!reducedMotion && 'IntersectionObserver' in window) {
    const counterObs = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (!entry.isIntersecting) return;
          const el = entry.target;
          counterObs.unobserve(el);
          const target = parseInt(el.getAttribute('data-counter') || '0', 10);
          if (target <= 0) {
            el.textContent = '0';
            return;
          }
          const duration = 900;
          const startTs = performance.now();
          function animCount(now) {
            const p = Math.min(1, (now - startTs) / duration);
            const eased = 1 - Math.pow(1 - p, 3);
            el.textContent = String(Math.round(target * eased));
            if (p < 1) requestAnimationFrame(animCount);
          }
          requestAnimationFrame(animCount);
        });
      },
      { threshold: 0.45 }
    );
    counterEls.forEach((el) => counterObs.observe(el));
  }

  // ------------------------------------------------------- Interactive Studio State
  const state = {
    scene: 'gameplay',
    dualOutput: true,
    cameraOn: true,
    transitionMode: 'fade',
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

    const g = ctx.createLinearGradient(x, y, x + w, y + h);
    g.addColorStop(0, '#2c2721');
    g.addColorStop(1, '#161513');
    ctx.fillStyle = g;
    ctx.fillRect(x + 1, y + 1, w - 2, h - 2);

    ctx.fillStyle = '#3c3730';
    ctx.beginPath();
    ctx.arc(x + w * 0.5, y + h * 0.42, Math.min(w, h) * 0.19, 0, Math.PI * 2);
    ctx.fill();
    ctx.beginPath();
    ctx.ellipse(x + w * 0.5, y + h * 0.96, w * 0.34, h * 0.36, 0, Math.PI, 0);
    ctx.fill();

    ctx.strokeStyle = 'rgba(241, 237, 228, 0.55)';
    ctx.lineWidth = 1;
    ctx.strokeRect(x + 0.5, y + 0.5, w - 1, h - 1);

    ctx.fillStyle = '#d63626';
    ctx.fillRect(x + 6, y + 6, 5, 5);

    if (!compact) {
      ctx.fillStyle = 'rgba(241, 237, 228, 0.78)';
      ctx.font = '500 9px "JetBrains Mono", monospace';
      ctx.fillText('CAM 01 · 1080p', x + 15, y + 12);
    }
    ctx.restore();
  }

  function drawSceneFrame(target, t, isVertical) {
    if (!target) return;
    const { ctx, w, h } = target;
    ctx.save();

    ctx.fillStyle = '#11110f';
    ctx.fillRect(0, 0, w, h);

    if (state.scene === 'gameplay') {
      const horizonY = isVertical ? h * 0.44 : h * 0.5;
      const sky = ctx.createLinearGradient(0, 0, 0, horizonY);
      sky.addColorStop(0, '#131618');
      sky.addColorStop(1, '#23201c');
      ctx.fillStyle = sky;
      ctx.fillRect(0, 0, w, horizonY);

      const road = ctx.createLinearGradient(0, horizonY, 0, h);
      road.addColorStop(0, '#191815');
      road.addColorStop(1, '#0e0e0c');
      ctx.fillStyle = road;
      ctx.fillRect(0, horizonY, w, h - horizonY);

      const cx = w * 0.5;
      ctx.strokeStyle = 'rgba(235, 196, 128, 0.22)';
      ctx.lineWidth = 1;
      for (let i = -3; i <= 3; i++) {
        ctx.beginPath();
        ctx.moveTo(cx + i * (isVertical ? 14 : 26), horizonY);
        ctx.lineTo(cx + i * (isVertical ? 96 : 210), h);
        ctx.stroke();
      }

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

      const horizonGlow = ctx.createRadialGradient(cx, horizonY, 6, cx, horizonY, isVertical ? 90 : 190);
      horizonGlow.addColorStop(0, 'rgba(227, 159, 68, 0.22)');
      horizonGlow.addColorStop(0.55, 'rgba(214, 54, 38, 0.08)');
      horizonGlow.addColorStop(1, 'rgba(17, 17, 15, 0)');
      ctx.fillStyle = horizonGlow;
      ctx.fillRect(0, horizonY - 56, w, 128);

      ctx.fillStyle = '#151513';
      const blocks = isVertical
        ? [[0.06, 22, 18], [0.22, 34, 28], [0.46, 24, 20], [0.68, 38, 32], [0.85, 20, 14]]
        : [[0.04, 48, 22], [0.14, 62, 36], [0.28, 42, 20], [0.41, 74, 42], [0.58, 52, 26], [0.72, 66, 34], [0.87, 50, 22]];
      blocks.forEach(([px, bw, bh]) => {
        ctx.fillRect(w * px, horizonY - bh, bw, bh);
      });

      for (let i = 0; i < 5; i++) {
        const lx = w * (0.14 + i * 0.18);
        ctx.fillStyle = 'rgba(235, 174, 78, 0.65)';
        ctx.fillRect(lx, horizonY - 4, 4, 2);
        ctx.fillStyle = 'rgba(235, 174, 78, 0.14)';
        ctx.fillRect(lx - 1, horizonY + 3, 6, 18);
      }

      ctx.fillStyle = 'rgba(241, 237, 228, 0.75)';
      ctx.font = `600 ${isVertical ? 9 : 11}px "JetBrains Mono", monospace`;
      ctx.fillText('248 KM/H · LAP 04', 14, 22);

      if (state.cameraOn) {
        if (isVertical) {
          drawWebcamPip(ctx, 14, h - 122, w - 28, 86, true);
        } else {
          drawWebcamPip(ctx, w - 168, h - 114, 152, 98, false);
        }
      }

      const tagY = isVertical ? h - 26 : h - 28;
      ctx.fillStyle = 'rgba(17, 17, 15, 0.86)';
      ctx.fillRect(14, tagY - 12, isVertical ? w - 28 : 228, 22);
      ctx.fillStyle = '#d63626';
      ctx.fillRect(14, tagY - 12, 3, 22);
      ctx.fillStyle = '#f1ede4';
      ctx.font = '500 10px "JetBrains Mono", monospace';
      ctx.fillText(state.cameraOn ? 'OVERLAY · GAMEPLAY + CAM' : 'OVERLAY · SEM MOLDURA', 24, tagY + 2);
    } else if (state.scene === 'chatting') {
      ctx.fillStyle = '#171613';
      ctx.fillRect(0, 0, w, h);

      if (state.cameraOn) {
        if (isVertical) {
          drawWebcamPip(ctx, 14, 22, w - 28, h * 0.56, false);
          ctx.fillStyle = '#f1ede4';
          ctx.font = '600 11px "JetBrains Mono", monospace';
          ctx.fillText('SÓ CONVERSANDO', 14, h * 0.56 + 48);
          ctx.fillStyle = '#9a9488';
          ctx.font = '400 10px "Archivo", sans-serif';
          ctx.fillText('Bate-papo ao vivo + Multichat', 14, h * 0.56 + 66);
        } else {
          drawWebcamPip(ctx, 22, 24, w * 0.62, h - 48, false);
          const rx = w * 0.62 + 40;
          ctx.fillStyle = '#d63626';
          ctx.fillRect(rx, 28, 26, 3);
          ctx.fillStyle = '#f1ede4';
          ctx.font = '700 18px "Archivo", sans-serif';
          ctx.fillText('Só conversando', rx, 56);
          ctx.fillStyle = '#9a9488';
          ctx.font = '400 11px "JetBrains Mono", monospace';
          ctx.fillText('TWITCH · YT · KICK', rx, 78);
          ctx.strokeStyle = 'rgba(241, 237, 228, 0.14)';
          ctx.beginPath();
          ctx.moveTo(rx, 94);
          ctx.lineTo(w - 22, 94);
          ctx.stroke();
          ctx.fillStyle = '#c8c3b8';
          ctx.font = '400 11px "Archivo", sans-serif';
          ctx.fillText('Tópico da live:', rx, 118);
          ctx.fillStyle = '#f1ede4';
          ctx.fillText('Testando motor Rust', rx, 136);
          ctx.fillText('e mixer 48 kHz', rx, 152);
        }
      } else {
        ctx.strokeStyle = 'rgba(241, 237, 228, 0.16)';
        ctx.strokeRect(18, 18, w - 36, h - 36);
        ctx.fillStyle = '#d63626';
        ctx.fillRect(w * 0.5 - 16, h * 0.42 - 22, 32, 3);
        ctx.fillStyle = '#f1ede4';
        ctx.textAlign = 'center';
        ctx.font = `700 ${isVertical ? 18 : 26}px "Archivo", sans-serif`;
        ctx.fillText('Só conversando', w * 0.5, h * 0.46 + 4);
        ctx.fillStyle = '#9a9488';
        ctx.font = '400 11px "JetBrains Mono", monospace';
        ctx.fillText('MODO SEM CÂMERA (CTRL+SHIFT+C)', w * 0.5, h * 0.46 + 26);
        ctx.textAlign = 'left';
      }
    } else if (state.scene === 'starting') {
      ctx.fillStyle = '#141412';
      ctx.fillRect(0, 0, w, h);

      ctx.strokeStyle = 'rgba(241, 237, 228, 0.14)';
      ctx.strokeRect(16, 16, w - 32, h - 32);

      ctx.textAlign = 'center';
      ctx.fillStyle = '#9a9488';
      ctx.font = '500 10px "JetBrains Mono", monospace';
      ctx.fillText('COMEÇANDO EM BREVE', w * 0.5, h * 0.38);

      ctx.fillStyle = '#f1ede4';
      ctx.font = `700 ${isVertical ? 40 : 62}px "JetBrains Mono", monospace`;
      ctx.fillText(formatCountdown(state.countdownSec), w * 0.5, h * 0.55);

      ctx.fillStyle = '#d63626';
      ctx.fillRect(w * 0.5 - 24, h * 0.63, 48, 2);

      ctx.fillStyle = '#a19b8e';
      ctx.font = '400 11px "Archivo", sans-serif';
      ctx.fillText('Kit OBS · Timer sincronizado ao entrar no ar', w * 0.5, h * 0.72);
      ctx.textAlign = 'left';
    } else if (state.scene === 'brb') {
      ctx.fillStyle = '#121210';
      ctx.fillRect(0, 0, w, h);

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
      ctx.font = `700 ${isVertical ? 22 : 34}px "Archivo", sans-serif`;
      ctx.fillText('Volto já', w * 0.5, h * 0.48);

      ctx.fillStyle = '#9a9488';
      ctx.font = '400 10.5px "JetBrains Mono", monospace';
      ctx.fillText('TELA DE PAUSA ATIVA · CTRL+SHIFT+P', w * 0.5, h * 0.48 + 26);
      ctx.textAlign = 'left';
    }

    ctx.strokeStyle = 'rgba(241, 237, 228, 0.25)';
    ctx.lineWidth = 1;
    const m = 8;
    const len = 8;
    ctx.beginPath();
    ctx.moveTo(m, m + len); ctx.lineTo(m, m); ctx.lineTo(m + len, m);
    ctx.moveTo(w - m - len, m); ctx.lineTo(w - m, m); ctx.lineTo(w - m, m + len);
    ctx.moveTo(m, h - m - len); ctx.lineTo(m, h - m); ctx.lineTo(m + len, h - m);
    ctx.moveTo(w - m - len, h - m); ctx.lineTo(w - m, h - m); ctx.lineTo(w - m, h - m - len);
    ctx.stroke();

    if (!isVertical) {
      ctx.fillStyle = 'rgba(241, 237, 228, 0.65)';
      ctx.font = '500 10px "JetBrains Mono", monospace';
      ctx.textAlign = 'right';
      ctx.fillText(`TC ${formatTimecode(5412 + t)}`, w - 14, 22);
      ctx.textAlign = 'left';
    }

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

  function setScene(next) {
    if (!next || !sceneLabels[next]) return;
    if (next !== state.scene && state.transitionMode === 'fade' && !reducedMotion) {
      state.transitionAlpha = 0.15;
    }
    state.scene = next;
    document.querySelectorAll('[data-scene]').forEach((b) => {
      b.setAttribute('aria-pressed', String(b.getAttribute('data-scene') === next));
    });
    if (activeSceneReadout) {
      activeSceneReadout.textContent = sceneLabels[next];
    }
    if (reducedMotion) renderOnce(performance.now() / 1000);
  }

  document.querySelectorAll('[data-scene]').forEach((btn) => {
    btn.addEventListener('click', () => setScene(btn.getAttribute('data-scene')));
  });

  const btnDual = document.getElementById('toggle-dual');
  function toggleDual() {
    state.dualOutput = !state.dualOutput;
    if (btnDual) btnDual.setAttribute('aria-pressed', String(state.dualOutput));
    if (vertUnit) vertUnit.classList.toggle('is-hidden', !state.dualOutput);
    if (reducedMotion) renderOnce(performance.now() / 1000);
    return state.dualOutput;
  }
  if (btnDual) btnDual.addEventListener('click', toggleDual);

  const btnCam = document.getElementById('toggle-camera');
  function toggleCamera() {
    state.cameraOn = !state.cameraOn;
    if (btnCam) {
      btnCam.setAttribute('aria-pressed', String(state.cameraOn));
      btnCam.textContent = state.cameraOn ? 'Câmera: Ligada' : 'Câmera: Oculta';
    }
    if (reducedMotion) renderOnce(performance.now() / 1000);
    return state.cameraOn;
  }
  if (btnCam) btnCam.addEventListener('click', toggleCamera);

  const btnTrans = document.getElementById('toggle-transition');
  if (btnTrans) {
    btnTrans.addEventListener('click', () => {
      state.transitionMode = state.transitionMode === 'fade' ? 'cut' : 'fade';
      btnTrans.textContent = state.transitionMode === 'fade' ? 'Transição: Esmaecer' : 'Transição: Corte';
    });
  }

  function setChannelMuted(id, muted) {
    const ch = state.channels[id];
    const muteBtn = document.getElementById(`mute-${id}`);
    const readout = document.getElementById(`db-${id}`);
    if (!ch) return;
    ch.muted = muted;
    if (muteBtn) {
      muteBtn.setAttribute('aria-pressed', String(ch.muted));
      muteBtn.textContent = ch.muted ? 'MUDO' : 'ATIVO';
    }
    if (readout) readout.textContent = ch.muted ? 'MUDO' : formatDb(ch.db);
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
      muteBtn.addEventListener('click', () => setChannelMuted(id, !ch.muted));
    }
  });

  // Gentle live MultiChat rotation inside the Studio preview
  const chatFeed = document.getElementById('chat-feed');
  const chatPool = [
    { src: 'TW', cls: '', user: 'rafa_gg', text: 'liso demais em 60fps no Linux!' },
    { src: 'YT', cls: 'is-yt', user: 'thiagom', text: 'testando aqui com NVENC, zero drop' },
    { src: 'KK', cls: 'is-kick', user: 'nina_play', text: 'o modo sem câmera escondendo a moldura salvou' },
    { src: 'TW', cls: '', user: 'pedro_s', text: 'lan p2p no segundo pc sem atraso nenhum' },
    { src: 'YT', cls: 'is-yt', user: 'larissa.dev', text: 'chegando pelo shorts 9:16, sem corte na câmera!' },
    { src: 'KK', cls: 'is-kick', user: 'diegok', text: 'salva o replay dessa curva' }
  ];
  let chatIdx = 0;

  if (chatFeed && !reducedMotion) {
    setInterval(() => {
      const next = chatPool[chatIdx % chatPool.length];
      chatIdx += 1;
      const el = document.createElement('div');
      el.className = 'chat-msg';
      el.innerHTML = `<span class="chat-src ${next.cls}">${next.src}</span><span class="chat-user">${next.user}</span>${next.text}`;
      chatFeed.appendChild(el);
      while (chatFeed.children.length > 4) {
        chatFeed.removeChild(chatFeed.firstElementChild);
      }
    }, 4800);
  }

  // ------------------------------------------------------- Interactive 4-Step Workflow
  const wfRoot = document.getElementById('workflow-root');
  const wfSteps = Array.from(document.querySelectorAll('[data-wf-step]'));
  const wfPanels = Array.from(document.querySelectorAll('[data-wf-panel]'));
  const wfStageLabel = document.getElementById('wf-stage-label');
  const wfStageCounter = document.getElementById('wf-stage-counter');
  const eqCurvePath = document.getElementById('eq-curve-path');
  const eqAreaPath = document.getElementById('eq-area-path');
  const eqPeakReadout = document.getElementById('eq-peak-readout');
  const wfRateTw = document.getElementById('wf-rate-tw');
  const wfRateYt = document.getElementById('wf-rate-yt');
  const wfRateTt = document.getElementById('wf-rate-tt');
  const wfRateLan = document.getElementById('wf-rate-lan');

  const wfMeta = [
    'ETAPA 01 · AUTENTICAÇÃO & COFRE',
    'ETAPA 02 · COMPOSITOR 16:9 + 9:16',
    'ETAPA 03 · MIXER DSP 48 KHZ',
    'ETAPA 04 · MULTISTREAM & LAN P2P'
  ];

  let wfActive = 0;
  let wfElapsed = 0;
  let wfPaused = false;
  const WF_DURATION = 5.8;

  function setWorkflowStep(idx, resetTimer = true) {
    wfActive = idx % wfSteps.length;
    if (resetTimer) wfElapsed = 0;
    wfSteps.forEach((btn, i) => {
      const active = i === wfActive;
      btn.classList.toggle('is-active', active);
      btn.setAttribute('aria-selected', String(active));
      const fill = btn.querySelector('.wf-step-fill');
      if (fill && !active) fill.style.transform = 'scaleY(0)';
      if (fill && active && reducedMotion) fill.style.transform = 'scaleY(1)';
    });
    wfPanels.forEach((panel, i) => {
      panel.classList.toggle('is-active', i === wfActive);
    });
    if (wfStageLabel) wfStageLabel.textContent = wfMeta[wfActive] || '';
    if (wfStageCounter) wfStageCounter.textContent = `${wfActive + 1} / ${wfSteps.length}`;
  }

  wfSteps.forEach((btn, idx) => {
    btn.addEventListener('click', () => {
      setWorkflowStep(idx, true);
    });
  });

  if (wfRoot) {
    wfRoot.addEventListener('mouseenter', () => { wfPaused = true; });
    wfRoot.addEventListener('mouseleave', () => { wfPaused = false; });
    wfRoot.addEventListener('focusin', () => { wfPaused = true; });
    wfRoot.addEventListener('focusout', () => { wfPaused = false; });
  }
  setWorkflowStep(0, true);

  // ------------------------------------------------------- Scroll-Linked Capabilities Deck
  const capItems = Array.from(document.querySelectorAll('[data-cap-idx]'));
  const capMonIndex = document.getElementById('cap-mon-index');
  const capMonTitle = document.getElementById('cap-mon-title');
  const capMonLines = document.getElementById('cap-mon-lines');
  const capMonBody = document.getElementById('cap-mon-body');
  const capMonSegs = Array.from(document.querySelectorAll('#cap-mon-segs span'));

  const capTelemetry = [
    {
      idx: '01 / 06',
      title: 'compositor.rs · Dual Output',
      lines: [
        ['Canvas horizontal', '1920×1080 @ 60fps'],
        ['Canvas vertical', '1080×1920 independente'],
        ['Transições', 'Cut · Fade · Black · Slide']
      ]
    },
    {
      idx: '02 / 06',
      title: 'multistream.rs · Encoders HW',
      lines: [
        ['Fallback automático', 'NVENC → AMF → QSV → x264'],
        ['Isolamento', '1 processo FFmpeg / destino'],
        ['Protocolos', 'RTMP + RTMPS (Kick)']
      ]
    },
    {
      idx: '03 / 06',
      title: 'audio_mixer.rs · DSP 48 kHz',
      lines: [
        ['Formato interno', 'PCM f32 estéreo'],
        ['Cadeia por canal', 'HPF · Gate · EQ3 · Comp · Lim'],
        ['Captura por app', 'WASAPI / PipeWire / Pulse']
      ]
    },
    {
      idx: '04 / 06',
      title: 'multichat.rs · IRC + Centrifugo',
      lines: [
        ['Plataformas', 'Twitch · YouTube · Kick'],
        ['Emotes nativos', '7TV · BTTV · FFZ'],
        ['Interface', '100% Rust Nativo (egui + wgpu)']
      ]
    },
    {
      idx: '05 / 06',
      title: 'kit.rs · Overlays & Widgets',
      lines: [
        ['Leitura config.js', 'Parser estático sem eval()'],
        ['Modo sem câmera', 'Oculta moldura automaticamente'],
        ['Browser Sources', 'StreamElements + HTML local']
      ]
    },
    {
      idx: '06 / 06',
      title: 'rtc_host.rs + recorder.rs',
      lines: [
        ['Descoberta LAN', 'UDP broadcast + PIN seguro'],
        ['Replay Buffer', 'Cluster EBML em RAM'],
        ['Exportação', 'Remux MP4 sem re-encode']
      ]
    }
  ];

  let activeCap = -1;
  let capSwapTimer = null;
  function setActiveCap(idx) {
    if (idx === activeCap || !capTelemetry[idx]) return;
    activeCap = idx;
    capItems.forEach((el, i) => el.classList.toggle('is-active', i === idx));
    capMonSegs.forEach((seg, i) => seg.classList.toggle('is-on', i <= idx));
    const data = capTelemetry[idx];
    if (capMonIndex) capMonIndex.textContent = data.idx;
    clearTimeout(capSwapTimer);
    if (capMonBody && !reducedMotion) {
      capMonBody.style.opacity = '0.35';
      capMonBody.style.transform = 'translateY(4px)';
      capSwapTimer = setTimeout(() => {
        if (capMonTitle) capMonTitle.textContent = data.title;
        if (capMonLines) {
          capMonLines.innerHTML = data.lines
            .map(([k, v]) => `<div class="cap-monitor-line"><span>${k}</span><strong>${v}</strong></div>`)
            .join('');
        }
        capMonBody.style.opacity = '1';
        capMonBody.style.transform = 'translateY(0)';
      }, 95);
    } else {
      if (capMonTitle) capMonTitle.textContent = data.title;
      if (capMonLines) {
        capMonLines.innerHTML = data.lines
          .map(([k, v]) => `<div class="cap-monitor-line"><span>${k}</span><strong>${v}</strong></div>`)
          .join('');
      }
    }
  }

  capItems.forEach((el, idx) => {
    el.addEventListener('click', () => setActiveCap(idx));
    el.addEventListener('mouseenter', () => setActiveCap(idx));
  });

  if ('IntersectionObserver' in window && capItems.length) {
    const capObs = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (entry.isIntersecting) {
            const idx = parseInt(entry.target.getAttribute('data-cap-idx') || '0', 10);
            setActiveCap(idx);
          }
        });
      },
      { rootMargin: '-34% 0px -46% 0px', threshold: 0 }
    );
    capItems.forEach((el) => capObs.observe(el));
  }

  // ------------------------------------------------------- Interactive Shortcut Tester
  const shortcutFeedback = document.getElementById('shortcut-feedback');
  let feedbackTimer = null;

  function triggerShortcutDemo(action, rowEl) {
    if (rowEl) {
      rowEl.classList.add('is-pressed');
      setTimeout(() => rowEl.classList.remove('is-pressed'), 220);
    }

    let msg = '';
    if (action === 'mic') {
      const next = !state.channels.mic.muted;
      setChannelMuted('mic', next);
      msg = next ? 'Microfone mudo no mixer' : 'Microfone reativado no mixer';
    } else if (action === 'desktop') {
      const next = !state.channels.desktop.muted;
      setChannelMuted('desktop', next);
      msg = next ? 'Áudio do PC mudo no mixer' : 'Áudio do PC reativado no mixer';
    } else if (action === 'brb') {
      const nextScene = state.scene === 'brb' ? 'gameplay' : 'brb';
      setScene(nextScene);
      msg = nextScene === 'brb' ? 'Tela de Pausa (Volto já) ativada no palco' : 'Voltando para cena Gameplay + Cam';
    } else if (action === 'replay') {
      msg = 'Replay de 30s salvo em Vídeos/PrismCast/replay.mp4';
    } else if (action === 'dual') {
      const on = toggleDual();
      msg = on ? 'Saída vertical 9:16 ativada no console' : 'Saída vertical 9:16 ocultada no console';
    } else if (action === 'camera') {
      const on = toggleCamera();
      msg = on ? 'Câmera e molduras ligadas' : 'Modo sem câmera ativado — molduras ocultas';
    }

    if (shortcutFeedback) {
      shortcutFeedback.innerHTML = `<span>${msg}</span><span>OK</span>`;
      shortcutFeedback.classList.add('is-triggered');
      clearTimeout(feedbackTimer);
      feedbackTimer = setTimeout(() => shortcutFeedback.classList.remove('is-triggered'), 1800);
    }
  }

  document.querySelectorAll('[data-hotkey-test]').forEach((row) => {
    const action = row.getAttribute('data-hotkey-test');
    row.addEventListener('click', () => triggerShortcutDemo(action, row));
    row.addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter' || ev.key === ' ') {
        ev.preventDefault();
        triggerShortcutDemo(action, row);
      }
    });
  });

  // ------------------------------------------------------- Animation Loop (Console + Workflow + EQ)
  const telBitrate = document.getElementById('telemetry-bitrate');
  const telFps = document.getElementById('telemetry-fps');
  let lastTs = performance.now();
  let lastTelUpdate = 0;

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

    // Subtle live telemetry readout
    if (now - lastTelUpdate > 900) {
      lastTelUpdate = now;
      if (telBitrate) {
        const kbps = 5975 + Math.round(Math.sin(t * 1.7) * 24 + Math.cos(t * 3.1) * 14);
        telBitrate.textContent = `${kbps} kbps`;
      }
      if (telFps) {
        const fps = (59.9 + Math.abs(Math.sin(t * 0.9)) * 0.1).toFixed(1);
        telFps.textContent = `${fps} FPS`;
      }
      if (wfActive === 2 && eqPeakReadout) {
        const dbfs = (-14.8 + Math.sin(t * 2.1) * 1.8).toFixed(1);
        eqPeakReadout.textContent = `${dbfs} dBFS`;
      }
      if (wfActive === 3) {
        if (wfRateTw) wfRateTw.textContent = `${5982 + Math.round(Math.sin(t * 1.9) * 22)} kbps`;
        if (wfRateYt) wfRateYt.textContent = `${8976 + Math.round(Math.cos(t * 1.4) * 32)} kbps`;
        if (wfRateTt) wfRateTt.textContent = `${4490 + Math.round(Math.sin(t * 2.3) * 18)} kbps`;
        if (wfRateLan) wfRateLan.textContent = `${(1.7 + Math.abs(Math.sin(t * 1.2)) * 0.3).toFixed(1)} ms RTT`;
      }
    }

    // Audio meters + realistic sidechain ducking
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

    // Workflow auto-advance progress rail
    if (!wfPaused && wfSteps.length) {
      wfElapsed += dt;
      if (wfElapsed >= WF_DURATION) {
        setWorkflowStep(wfActive + 1, true);
      } else {
        const activeBtn = wfSteps[wfActive];
        const fill = activeBtn && activeBtn.querySelector('.wf-step-fill');
        if (fill) {
          fill.style.transform = `scaleY(${Math.min(1, wfElapsed / WF_DURATION).toFixed(4)})`;
        }
      }
    }

    // Subtle live EQ curve & area fill breathing when step 3 is active
    if (eqCurvePath && wfActive === 2) {
      const l = 52 + Math.sin(t * 2.4) * 5;
      const m = 66 + Math.cos(t * 1.9) * 4;
      const h = 48 + Math.sin(t * 3.1 + 1) * 5;
      const curveD = `M 0 112 C 58 106, 85 ${l.toFixed(1)}, 145 ${m.toFixed(1)} C 205 ${(m + 10).toFixed(1)}, 250 ${h.toFixed(1)}, 315 ${(h + 8).toFixed(1)} C 365 ${(h + 12).toFixed(1)}, 392 ${h.toFixed(1)}, 420 ${(h + 5).toFixed(1)}`;
      eqCurvePath.setAttribute('d', curveD);
      if (eqAreaPath) {
        eqAreaPath.setAttribute('d', `${curveD} L 420 150 L 0 150 Z`);
      }
    }

    requestAnimationFrame(tick);
  }

  if (reducedMotion) {
    renderOnce(0);
  } else {
    requestAnimationFrame(tick);
  }

  // ------------------------------------------------------- OS Tabs, Copy & GitHub Releases
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

  const osTabsWrap = document.getElementById('os-tabs');
  const osTabs = document.querySelectorAll('[data-os-tab]');
  const osPanels = document.querySelectorAll('[data-os-panel]');
  const heroPrimaryLabel = document.getElementById('hero-download-label');
  const heroFootnote = document.getElementById('hero-download-note');
  const ffmpegCmd = document.getElementById('dl-ffmpeg-cmd');
  const copyCmdBtn = document.getElementById('copy-ffmpeg-btn');

  function selectOs(os) {
    if (osTabsWrap) osTabsWrap.setAttribute('data-active', os);
    osTabs.forEach((tab) => {
      const active = tab.getAttribute('data-os-tab') === os;
      tab.setAttribute('aria-selected', String(active));
    });
    osPanels.forEach((panel) => {
      const active = panel.getAttribute('data-os-panel') === os;
      panel.hidden = !active;
    });
    if (heroPrimaryLabel) {
      heroPrimaryLabel.textContent = os === 'linux' ? 'Baixar para Linux' : 'Baixar para Windows';
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

  if (copyCmdBtn && ffmpegCmd) {
    copyCmdBtn.addEventListener('click', async () => {
      const text = ffmpegCmd.textContent || '';
      try {
        await navigator.clipboard.writeText(text);
      } catch {
        // Fallback selection copy
        const range = document.createRange();
        range.selectNodeContents(ffmpegCmd);
        const sel = window.getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
      }
      copyCmdBtn.textContent = 'Copiado ✓';
      copyCmdBtn.classList.add('is-copied');
      setTimeout(() => {
        copyCmdBtn.textContent = 'Copiar';
        copyCmdBtn.classList.remove('is-copied');
      }, 1800);
    });
  }

  // ------------------------------------------------------- FAQ Smooth Accordion
  document.querySelectorAll('.faq-item').forEach((item) => {
    const trigger = item.querySelector('.faq-trigger');
    if (!trigger) return;
    trigger.addEventListener('click', () => {
      const isOpen = item.classList.contains('is-open');
      // Close siblings for tidy vertical cadence
      document.querySelectorAll('.faq-item.is-open').forEach((openItem) => {
        if (openItem !== item) {
          openItem.classList.remove('is-open');
          const t = openItem.querySelector('.faq-trigger');
          if (t) t.setAttribute('aria-expanded', 'false');
        }
      });
      item.classList.toggle('is-open', !isOpen);
      trigger.setAttribute('aria-expanded', String(!isOpen));
    });
  });

  // ------------------------------------------------------- Live GitHub Releases Hydration
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
      document.querySelectorAll('[data-asset-link]').forEach((link) => {
        if (!link.getAttribute('href')) link.href = RELEASES_PAGE;
      });
    }
  }

  // ------------------------------------------------------- 3D Hero Prism Parallax
  (() => {
    const stage = document.getElementById('hero-visual');
    const prismTilt = document.getElementById('prism-tilt');
    const coarse = window.matchMedia('(pointer: coarse)').matches;
    if (!stage || reducedMotion || coarse) return;

    const layers = Array.from(stage.querySelectorAll('[data-depth]')).map((el) => ({
      el,
      depth: parseFloat(el.getAttribute('data-depth') || '1')
    }));

    let targetX = 0;
    let targetY = 0;
    let curX = 0;
    let curY = 0;
    let rafId = 0;

    function animate() {
      curX += (targetX - curX) * 0.1;
      curY += (targetY - curY) * 0.1;

      if (prismTilt) {
        const rx = -14 - curY * 14;
        const ry = -24 + curX * 20;
        prismTilt.style.setProperty('--prism-rx', `${rx.toFixed(2)}deg`);
        prismTilt.style.setProperty('--prism-ry', `${ry.toFixed(2)}deg`);
      }

      for (const { el, depth } of layers) {
        const px = curX * depth * 11;
        const py = curY * depth * 9;
        el.style.setProperty('--px', `${px.toFixed(2)}px`);
        el.style.setProperty('--py', `${py.toFixed(2)}px`);
      }

      if (Math.abs(targetX - curX) > 0.002 || Math.abs(targetY - curY) > 0.002) {
        rafId = requestAnimationFrame(animate);
      } else {
        rafId = 0;
      }
    }

    function kick() {
      if (!rafId) rafId = requestAnimationFrame(animate);
    }

    stage.addEventListener(
      'pointermove',
      (ev) => {
        const rect = stage.getBoundingClientRect();
        if (!rect.width || !rect.height) return;
        targetX = Math.max(-1, Math.min(1, ((ev.clientX - rect.left) / rect.width - 0.5) * 2));
        targetY = Math.max(-1, Math.min(1, ((ev.clientY - rect.top) / rect.height - 0.5) * 2));
        kick();
      },
      { passive: true }
    );

    stage.addEventListener('pointerleave', () => {
      targetX = 0;
      targetY = 0;
      kick();
    });
  })();

  // ------------------------------------------------------- 3D Specular Card Tilt
  (() => {
    const coarse = window.matchMedia('(pointer: coarse)').matches;
    if (reducedMotion || coarse) return;

    const tiltEls = document.querySelectorAll('[data-tilt]');
    tiltEls.forEach((el) => {
      const maxDeg = el.classList.contains('tilt-soft') ? 3.8 : 7.5;

      el.addEventListener('pointerenter', () => {
        el.classList.add('is-tilting');
      });

      el.addEventListener(
        'pointermove',
        (ev) => {
          const rect = el.getBoundingClientRect();
          if (!rect.width || !rect.height) return;
          const nx = (ev.clientX - rect.left) / rect.width;
          const ny = (ev.clientY - rect.top) / rect.height;
          const rx = (0.5 - ny) * maxDeg * 2;
          const ry = (nx - 0.5) * maxDeg * 2;

          el.style.setProperty('--rx', `${rx.toFixed(2)}deg`);
          el.style.setProperty('--ry', `${ry.toFixed(2)}deg`);
          el.style.setProperty('--gx', `${(nx * 100).toFixed(1)}%`);
          el.style.setProperty('--gy', `${(ny * 100).toFixed(1)}%`);
        },
        { passive: true }
      );

      el.addEventListener('pointerleave', () => {
        el.classList.remove('is-tilting');
        el.style.setProperty('--rx', '0deg');
        el.style.setProperty('--ry', '0deg');
      });
    });
  })();

  hydrateRelease();
  void smoothScroll;
})();
