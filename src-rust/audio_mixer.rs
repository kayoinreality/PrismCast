use crate::protocol::{
    AudioApp, AudioChannel, AudioChannelKind, AudioChannelStatus, AudioLevelSample,
    AudioMixerSettings, AudioMonitorMode, MAX_DB, MIN_DB,
};
use std::collections::HashMap;
use std::f32::consts::PI;

pub const SAMPLE_RATE: f32 = 48_000.0;
pub const SAMPLE_RATE_HZ: usize = 48_000;
pub const CHANNELS: usize = 2;
pub const QUANTUM_FRAMES: usize = 480; // 10 ms @ 48 kHz
pub const QUANTUM_SAMPLES: usize = QUANTUM_FRAMES * CHANNELS;
pub const FRAME_BYTES: usize = CHANNELS * std::mem::size_of::<f32>(); // 8 bytes por frame estéreo
const RING_CAP_FRAMES: usize = SAMPLE_RATE_HZ * 2; // 2 s
const TARGET_RING_FRAMES: usize = (SAMPLE_RATE_HZ * 40) / 1000; // 40 ms
const MAX_RING_FRAMES: usize = (SAMPLE_RATE_HZ * 250) / 1000; // 250 ms
const MICRO_FADE_FRAMES: usize = (SAMPLE_RATE_HZ * 3) / 1000; // 144 frames = 3 ms
const MAX_DELAY_FRAMES: usize = SAMPLE_RATE_HZ * 2; // 2000 ms
const MUTED_SPEECH_DB: f32 = -35.0;
const MUTED_SPEECH_HOLD_QUANTUMS: u32 = 120; // 120 * 10 ms = 1.2 s

#[derive(Debug, Clone)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub kind: String, // "input" | "output"
    pub is_default: bool,
}

#[inline]
pub fn db_to_linear(db: f32) -> f32 {
    if db <= MIN_DB {
        0.0
    } else {
        10.0_f32.powf(db / 20.0)
    }
}

#[inline]
pub fn linear_to_db(v: f32) -> f32 {
    if v <= 0.000_001 {
        MIN_DB
    } else {
        (20.0 * v.log10()).clamp(MIN_DB, MAX_DB)
    }
}

// ------------------------------------------------------------------ Ring Buffer Estéreo Alinhado

/// Ring buffer de áudio f32 estéreo alinhado a frames (8 bytes = L+R).
/// Nunca permite leitura/escrita parcial de frame (sem inversão L/R) nem wraparound silencioso.
pub struct StereoFrameRing {
    buf: Vec<f32>,
    cap_frames: usize,
    read_frame: usize,
    write_frame: usize,
    buffered_frames: usize,
    primed: bool,
    fade_in_remaining: usize,
    byte_tail: [u8; FRAME_BYTES],
    byte_tail_len: usize,
}

impl StereoFrameRing {
    pub fn new(cap_frames: usize) -> Self {
        let cap = cap_frames.max(QUANTUM_FRAMES * 2);
        Self {
            buf: vec![0.0; cap * CHANNELS],
            cap_frames: cap,
            read_frame: 0,
            write_frame: 0,
            buffered_frames: 0,
            primed: false,
            fade_in_remaining: 0,
            byte_tail: [0u8; FRAME_BYTES],
            byte_tail_len: 0,
        }
    }

    pub fn buffered_frames(&self) -> usize {
        self.buffered_frames
    }

    /// Empurra bytes crus PCM f32le estéreo preservando sobras menores que 8 bytes (sem corromper alinhamento).
    pub fn push_pcm_bytes(&mut self, bytes: &[u8]) {
        let mut cursor = 0usize;
        if self.byte_tail_len > 0 {
            let need = FRAME_BYTES - self.byte_tail_len;
            let take = need.min(bytes.len());
            self.byte_tail[self.byte_tail_len..self.byte_tail_len + take]
                .copy_from_slice(&bytes[..take]);
            self.byte_tail_len += take;
            cursor += take;
            if self.byte_tail_len == FRAME_BYTES {
                let l = f32::from_le_bytes(self.byte_tail[0..4].try_into().unwrap());
                let r = f32::from_le_bytes(self.byte_tail[4..8].try_into().unwrap());
                self.push_interleaved(&[l, r]);
                self.byte_tail_len = 0;
            }
        }

        let rem = &bytes[cursor..];
        let full_frames = rem.len() / FRAME_BYTES;
        for f in 0..full_frames {
            let off = f * FRAME_BYTES;
            let l = f32::from_le_bytes(rem[off..off + 4].try_into().unwrap());
            let r = f32::from_le_bytes(rem[off + 4..off + 8].try_into().unwrap());
            self.push_frame(l, r);
        }
        let leftover = rem.len() % FRAME_BYTES;
        if leftover > 0 {
            let tail_start = rem.len() - leftover;
            self.byte_tail[..leftover].copy_from_slice(&rem[tail_start..]);
            self.byte_tail_len = leftover;
        }
    }

    /// Empurra amostras intercaladas `[L, R, L, R, ...]` (ignora amostra ímpar solta).
    pub fn push_interleaved(&mut self, samples: &[f32]) {
        let frames = samples.len() / CHANNELS;
        for f in 0..frames {
            self.push_frame(samples[f * 2], samples[f * 2 + 1]);
        }
    }

    #[inline]
    fn push_frame(&mut self, l: f32, r: f32) {
        let l = if l.is_finite() { l } else { 0.0 };
        let r = if r.is_finite() { r } else { 0.0 };

        if self.buffered_frames >= self.cap_frames {
            // Buffer cheio: avança read_frame em 1 frame inteiro (mantém par L/R intacto).
            self.read_frame = (self.read_frame + 1) % self.cap_frames;
            self.buffered_frames -= 1;
        }

        let idx = self.write_frame * CHANNELS;
        self.buf[idx] = l;
        self.buf[idx + 1] = r;
        self.write_frame = (self.write_frame + 1) % self.cap_frames;
        self.buffered_frames += 1;

        if self.buffered_frames > MAX_RING_FRAMES {
            // Ressincroniza para TARGET_RING_FRAMES sem descartar alinhamento de frame.
            let drop_frames = self.buffered_frames - TARGET_RING_FRAMES;
            self.read_frame = (self.read_frame + drop_frames) % self.cap_frames;
            self.buffered_frames = TARGET_RING_FRAMES;
            self.fade_in_remaining = MICRO_FADE_FRAMES;
        }
    }

    /// Lê um quantum completo (`out.len() / 2` frames) com micro-fade de 3ms após underrun/resync.
    pub fn read_quantum(&mut self, out: &mut [f32]) -> bool {
        let frames = out.len() / CHANNELS;
        if !self.primed && self.buffered_frames >= TARGET_RING_FRAMES {
            self.primed = true;
            self.fade_in_remaining = MICRO_FADE_FRAMES;
        }
        if !self.primed || self.buffered_frames < frames {
            self.primed = false;
            out.fill(0.0);
            return false;
        }

        for f in 0..frames {
            let idx = self.read_frame * CHANNELS;
            let mut l = self.buf[idx];
            let mut r = self.buf[idx + 1];
            if self.fade_in_remaining > 0 {
                let done = MICRO_FADE_FRAMES - self.fade_in_remaining + 1;
                let g = (done as f32 / MICRO_FADE_FRAMES as f32).min(1.0);
                l *= g;
                r *= g;
                self.fade_in_remaining -= 1;
            }
            out[f * 2] = l;
            out[f * 2 + 1] = r;
            self.read_frame = (self.read_frame + 1) % self.cap_frames;
        }
        self.buffered_frames -= frames;
        true
    }
}

// ------------------------------------------------------------------ Blocos DSP Zero-Allocation

pub struct StereoDelayLine {
    buf: Vec<f32>,
    write_frame: usize,
    delay_frames: usize,
}

impl StereoDelayLine {
    pub fn new() -> Self {
        Self {
            buf: vec![0.0; (MAX_DELAY_FRAMES + 1) * CHANNELS],
            write_frame: 0,
            delay_frames: 0,
        }
    }

    pub fn set_delay_ms(&mut self, ms: i32) {
        let clamped = ms.clamp(0, 2000) as usize;
        self.delay_frames = (clamped * SAMPLE_RATE_HZ / 1000).min(MAX_DELAY_FRAMES);
    }

    #[inline]
    pub fn process_frame(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.delay_frames == 0 {
            return (l, r);
        }
        let cap = MAX_DELAY_FRAMES + 1;
        let w = self.write_frame * CHANNELS;
        self.buf[w] = l;
        self.buf[w + 1] = r;
        let read_frame = (self.write_frame + cap - self.delay_frames) % cap;
        let rd = read_frame * CHANNELS;
        let out = (self.buf[rd], self.buf[rd + 1]);
        self.write_frame = (self.write_frame + 1) % cap;
        out
    }
}

#[derive(Clone, Copy)]
pub struct BiquadFilter {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1_l: f32,
    z2_l: f32,
    z1_r: f32,
    z2_r: f32,
    bypass: bool,
}

impl BiquadFilter {
    pub fn bypass() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1_l: 0.0,
            z2_l: 0.0,
            z1_r: 0.0,
            z2_r: 0.0,
            bypass: true,
        }
    }

    pub fn set_highpass(&mut self, freq_hz: f32) {
        if freq_hz <= 0.0 {
            self.bypass = true;
            return;
        }
        self.bypass = false;
        let f = freq_hz.clamp(20.0, 500.0);
        let w0 = 2.0 * PI * f / SAMPLE_RATE;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        self.b0 = ((1.0 + cos_w0) * 0.5) / a0;
        self.b1 = (-(1.0 + cos_w0)) / a0;
        self.b2 = ((1.0 + cos_w0) * 0.5) / a0;
        self.a1 = (-2.0 * cos_w0) / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    pub fn set_low_shelf(&mut self, freq_hz: f32, gain_db: f32) {
        if gain_db.abs() < 0.05 {
            self.bypass = true;
            return;
        }
        self.bypass = false;
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq_hz / SAMPLE_RATE;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 * 0.5 * ((a + 1.0 / a) * (1.0 / 0.707 - 1.0) + 2.0).max(0.001).sqrt();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        self.b0 = (a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha)) / a0;
        self.b1 = (2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0;
        self.b2 = (a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha)) / a0;
        self.a1 = (-2.0 * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0;
        self.a2 = ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha) / a0;
    }

    pub fn set_peaking(&mut self, freq_hz: f32, q: f32, gain_db: f32) {
        if gain_db.abs() < 0.05 {
            self.bypass = true;
            return;
        }
        self.bypass = false;
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq_hz / SAMPLE_RATE;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha / a;
        self.b0 = (1.0 + alpha * a) / a0;
        self.b1 = (-2.0 * cos_w0) / a0;
        self.b2 = (1.0 - alpha * a) / a0;
        self.a1 = (-2.0 * cos_w0) / a0;
        self.a2 = (1.0 - alpha / a) / a0;
    }

    pub fn set_high_shelf(&mut self, freq_hz: f32, gain_db: f32) {
        if gain_db.abs() < 0.05 {
            self.bypass = true;
            return;
        }
        self.bypass = false;
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq_hz / SAMPLE_RATE;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 * 0.5 * ((a + 1.0 / a) * (1.0 / 0.707 - 1.0) + 2.0).max(0.001).sqrt();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        self.b0 = (a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha)) / a0;
        self.b1 = (-2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0;
        self.b2 = (a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha)) / a0;
        self.a1 = (2.0 * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0;
        self.a2 = ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha) / a0;
    }

    #[inline]
    pub fn process_frame(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.bypass {
            return (l, r);
        }
        let yl = self.b0 * l + self.z1_l;
        self.z1_l = self.b1 * l - self.a1 * yl + self.z2_l;
        self.z2_l = self.b2 * l - self.a2 * yl;

        let yr = self.b0 * r + self.z1_r;
        self.z1_r = self.b1 * r - self.a1 * yr + self.z2_r;
        self.z2_r = self.b2 * r - self.a2 * yr;
        (yl, yr)
    }
}

pub struct NoiseGate {
    enabled: bool,
    threshold_lin: f32,
    attack_coeff: f32,
    release_coeff: f32,
    env_fall_coeff: f32,
    hold_frames: u32,
    env: f32,
    gain: f32,
    hold_left: u32,
}

impl NoiseGate {
    pub fn new() -> Self {
        Self {
            enabled: false,
            threshold_lin: 0.0,
            attack_coeff: 1.0 - (-1.0 / (0.004 * SAMPLE_RATE)).exp(),
            release_coeff: 1.0 - (-1.0 / (0.15 * SAMPLE_RATE)).exp(),
            env_fall_coeff: 1.0 - (-1.0 / (0.02 * SAMPLE_RATE)).exp(),
            hold_frames: (0.08 * SAMPLE_RATE) as u32,
            env: 0.0,
            gain: 1.0,
            hold_left: 0,
        }
    }

    pub fn configure(&mut self, enabled: bool, threshold_db: f32, release_ms: u32) {
        if !enabled || threshold_db <= -99.5 {
            self.enabled = false;
            self.gain = 1.0;
            return;
        }
        self.enabled = true;
        self.threshold_lin = 10.0_f32.powf(threshold_db / 20.0);
        let rel_sec = (release_ms.clamp(10, 2000) as f32) / 1000.0;
        self.release_coeff = 1.0 - (-1.0 / (rel_sec * SAMPLE_RATE)).exp();
    }

    #[inline]
    pub fn process_frame(&mut self, l: f32, r: f32) -> (f32, f32) {
        if !self.enabled {
            return (l, r);
        }
        let peak = l.abs().max(r.abs());
        if peak > self.env {
            self.env = peak;
        } else {
            self.env += (peak - self.env) * self.env_fall_coeff;
        }
        if self.env >= self.threshold_lin {
            self.hold_left = self.hold_frames;
            self.gain += (1.0 - self.gain) * self.attack_coeff;
        } else if self.hold_left > 0 {
            self.hold_left -= 1;
        } else {
            self.gain += (0.0 - self.gain) * self.release_coeff;
        }
        (l * self.gain, r * self.gain)
    }
}

pub struct Compressor {
    enabled: bool,
    threshold_db: f32,
    ratio: f32,
    attack_coeff: f32,
    release_coeff: f32,
    makeup_lin: f32,
    env_db: f32,
}

impl Compressor {
    pub fn new() -> Self {
        Self {
            enabled: false,
            threshold_db: -18.0,
            ratio: 4.0,
            attack_coeff: 1.0 - (-1.0 / (0.006 * SAMPLE_RATE)).exp(),
            release_coeff: 1.0 - (-1.0 / (0.15 * SAMPLE_RATE)).exp(),
            makeup_lin: 1.0,
            env_db: MIN_DB,
        }
    }

    pub fn configure(
        &mut self,
        enabled: bool,
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
        makeup_db: f32,
    ) {
        self.enabled = enabled;
        self.threshold_db = threshold_db.clamp(-60.0, 0.0);
        self.ratio = ratio.clamp(1.0, 20.0);
        let atk = (attack_ms.clamp(0.5, 250.0) / 1000.0) * SAMPLE_RATE;
        let rel = (release_ms.clamp(10.0, 2000.0) / 1000.0) * SAMPLE_RATE;
        self.attack_coeff = 1.0 - (-1.0 / atk).exp();
        self.release_coeff = 1.0 - (-1.0 / rel).exp();
        self.makeup_lin = if enabled { db_to_linear(makeup_db) } else { 1.0 };
    }

    #[inline]
    pub fn process_frame(&mut self, l: f32, r: f32) -> (f32, f32) {
        if !self.enabled {
            return (l, r);
        }
        let peak = l.abs().max(r.abs());
        let level_db = linear_to_db(peak);
        let coeff = if level_db > self.env_db {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.env_db += (level_db - self.env_db) * coeff;

        let over = (self.env_db - self.threshold_db).max(0.0);
        let gain_reduction_db = over * (1.0 - 1.0 / self.ratio);
        let gain = db_to_linear(-gain_reduction_db) * self.makeup_lin;
        (l * gain, r * gain)
    }
}

pub struct Limiter {
    enabled: bool,
    ceiling_lin: f32,
    gain: f32,
    release_coeff: f32,
}

impl Limiter {
    pub fn new() -> Self {
        Self {
            enabled: false,
            ceiling_lin: db_to_linear(-1.0),
            gain: 1.0,
            release_coeff: 1.0 - (-1.0 / (0.06 * SAMPLE_RATE)).exp(),
        }
    }

    pub fn configure(&mut self, enabled: bool, ceiling_db: f32) {
        self.enabled = enabled;
        self.ceiling_lin = db_to_linear(ceiling_db.clamp(-12.0, 0.0)).max(0.01);
        if !enabled {
            self.gain = 1.0;
        }
    }

    #[inline]
    pub fn process_frame(&mut self, l: f32, r: f32) -> (f32, f32) {
        if !self.enabled {
            return (l, r);
        }
        let peak = l.abs().max(r.abs());
        let target_gain = if peak > self.ceiling_lin {
            self.ceiling_lin / peak
        } else {
            1.0
        };
        if target_gain < self.gain {
            self.gain = target_gain;
        } else {
            self.gain += (1.0 - self.gain) * self.release_coeff;
        }
        (
            (l * self.gain).clamp(-self.ceiling_lin, self.ceiling_lin),
            (r * self.gain).clamp(-self.ceiling_lin, self.ceiling_lin),
        )
    }
}

// ------------------------------------------------------------------ Channel Strip & Mixer Engine

struct ChannelStrip {
    config: AudioChannel,
    ring: StereoFrameRing,
    delay: StereoDelayLine,
    highpass: BiquadFilter,
    eq_low: BiquadFilter,
    eq_mid: BiquadFilter,
    eq_high: BiquadFilter,
    gate: NoiseGate,
    compressor: Compressor,
    limiter: Limiter,
    pre_gain_lin: f32,
    fader_lin: f32,
    duck_gain: f32,
    duck_target_lin: f32,
    duck_attack_coeff: f32,
    duck_release_coeff: f32,
    in_scene: bool,
    quantum_buf: Vec<f32>,
    last_input_db: f32,
    last_peak_db: f32,
    muted_speech_quantums: u32,
    muted_speech_active: bool,
    status: AudioChannelStatus,
}

impl ChannelStrip {
    fn new(ch: AudioChannel) -> Self {
        let mut strip = Self {
            status: AudioChannelStatus {
                id: ch.id.clone(),
                state: "off".to_string(),
                message: None,
            },
            config: ch.clone(),
            ring: StereoFrameRing::new(RING_CAP_FRAMES),
            delay: StereoDelayLine::new(),
            highpass: BiquadFilter::bypass(),
            eq_low: BiquadFilter::bypass(),
            eq_mid: BiquadFilter::bypass(),
            eq_high: BiquadFilter::bypass(),
            gate: NoiseGate::new(),
            compressor: Compressor::new(),
            limiter: Limiter::new(),
            pre_gain_lin: 1.0,
            fader_lin: 1.0,
            duck_gain: 1.0,
            duck_target_lin: 1.0,
            duck_attack_coeff: 1.0 - (-1.0 / (0.015 * SAMPLE_RATE)).exp(),
            duck_release_coeff: 1.0 - (-1.0 / (0.35 * SAMPLE_RATE)).exp(),
            in_scene: true,
            quantum_buf: vec![0.0; QUANTUM_SAMPLES],
            last_input_db: MIN_DB,
            last_peak_db: MIN_DB,
            muted_speech_quantums: 0,
            muted_speech_active: false,
        };
        strip.update_config(ch);
        strip
    }

    fn update_config(&mut self, mut ch: AudioChannel) {
        ch.normalize();
        self.delay.set_delay_ms(ch.sync_offset_ms);
        self.highpass.set_highpass(ch.filters.highpass_hz);
        if ch.filters.eq_enabled {
            self.eq_low.set_low_shelf(100.0, ch.filters.eq_low_db);
            self.eq_mid.set_peaking(1000.0, 1.0, ch.filters.eq_mid_db);
            self.eq_high.set_high_shelf(8000.0, ch.filters.eq_high_db);
        } else {
            self.eq_low = BiquadFilter::bypass();
            self.eq_mid = BiquadFilter::bypass();
            self.eq_high = BiquadFilter::bypass();
        }
        self.gate.configure(
            ch.filters.gate.enabled,
            ch.filters.gate.threshold_db,
            ch.filters.gate.release_ms,
        );
        self.compressor.configure(
            ch.filters.compressor.enabled,
            ch.filters.compressor.threshold_db,
            ch.filters.compressor.ratio,
            ch.filters.compressor.attack_ms,
            ch.filters.compressor.release_ms,
            ch.filters.compressor.makeup_db,
        );
        self.limiter
            .configure(ch.filters.limiter.enabled, ch.filters.limiter.ceiling_db);
        self.pre_gain_lin = db_to_linear(ch.filters.gain_db);
        self.fader_lin = db_to_linear(ch.volume_db);
        self.duck_target_lin = db_to_linear(ch.filters.ducking_amount_db);
        let rel_sec = (ch.filters.ducking_release_ms.clamp(50, 5000) as f32) / 1000.0;
        self.duck_release_coeff = 1.0 - (-1.0 / (rel_sec * SAMPLE_RATE)).exp();
        self.config = ch;
    }

    fn process_quantum(
        &mut self,
        ducking_triggers: &HashMap<String, bool>,
        default_mic_active: bool,
        program_out: &mut [f32],
        monitor_out: &mut [f32],
    ) {
        let has_audio = self.ring.read_quantum(&mut self.quantum_buf);
        if !has_audio {
            self.last_input_db = MIN_DB;
            self.last_peak_db = MIN_DB;
            if self.muted_speech_quantums > 0 {
                self.muted_speech_quantums = 0;
                self.muted_speech_active = false;
            }
            return;
        }

        let duck_active = self.config.filters.ducking_enabled
            && match self.config.filters.ducking_source_id.as_deref() {
                Some(src) if !src.is_empty() => *ducking_triggers.get(src).unwrap_or(&false),
                _ => default_mic_active,
            };
        let target_duck = if duck_active {
            self.duck_target_lin
        } else {
            1.0
        };

        let muted = self.config.muted || !self.in_scene;
        let to_program = if self.config.monitor == AudioMonitorMode::MonitorOnly || muted {
            0.0
        } else {
            1.0
        };
        let to_monitor = if self.config.monitor == AudioMonitorMode::Off || muted {
            0.0
        } else {
            1.0
        };

        let mut raw_peak = 0.0_f32;
        let mut out_peak = 0.0_f32;

        for f in 0..QUANTUM_FRAMES {
            let l0 = self.quantum_buf[f * 2];
            let r0 = self.quantum_buf[f * 2 + 1];

            let (dl, dr) = self.delay.process_frame(l0, r0);
            let (hl, hr) = self.highpass.process_frame(dl, dr);
            raw_peak = raw_peak.max(hl.abs().max(hr.abs()));

            let (el1, er1) = self.eq_low.process_frame(hl, hr);
            let (el2, er2) = self.eq_mid.process_frame(el1, er1);
            let (el3, er3) = self.eq_high.process_frame(el2, er2);
            let (gl, gr) = self.gate.process_frame(el3, er3);
            let (cl, cr) = self.compressor.process_frame(gl, gr);

            let gl_pre = cl * self.pre_gain_lin;
            let gr_pre = cr * self.pre_gain_lin;
            let (ll, lr) = self.limiter.process_frame(gl_pre, gr_pre);

            let coeff = if target_duck < self.duck_gain {
                self.duck_attack_coeff
            } else {
                self.duck_release_coeff
            };
            self.duck_gain += (target_duck - self.duck_gain) * coeff;

            let post_l = ll * self.fader_lin * self.duck_gain;
            let post_r = lr * self.fader_lin * self.duck_gain;

            if !muted {
                out_peak = out_peak.max(post_l.abs().max(post_r.abs()));
            }

            program_out[f * 2] += post_l * to_program;
            program_out[f * 2 + 1] += post_r * to_program;
            monitor_out[f * 2] += post_l * to_monitor;
            monitor_out[f * 2 + 1] += post_r * to_monitor;
        }

        self.last_input_db = linear_to_db(raw_peak);
        self.last_peak_db = if muted { MIN_DB } else { linear_to_db(out_peak) };

        if self.config.kind == AudioChannelKind::Mic
            && self.config.muted
            && self.in_scene
            && self.last_input_db > MUTED_SPEECH_DB
        {
            self.muted_speech_quantums = self.muted_speech_quantums.saturating_add(1);
            if self.muted_speech_quantums >= MUTED_SPEECH_HOLD_QUANTUMS {
                self.muted_speech_active = true;
            }
        } else {
            self.muted_speech_quantums = 0;
            self.muted_speech_active = false;
        }
    }
}

/// Motor completo de áudio nativo em Rust (substitui `AudioMixerEngine.ts`, `audio-worklets.ts` e `audio-apps.ts`).
pub struct AudioMixerEngine {
    settings: AudioMixerSettings,
    strips: HashMap<String, ChannelStrip>,
    order: Vec<String>,
    live_mics: Vec<String>,
    used_mics: Vec<String>,
    duck_triggers: HashMap<String, bool>,
}

impl AudioMixerEngine {
    pub fn new(initial: AudioMixerSettings) -> Self {
        let mut engine = Self {
            settings: initial.clone(),
            strips: HashMap::new(),
            order: Vec::new(),
            live_mics: Vec::new(),
            used_mics: Vec::new(),
            duck_triggers: HashMap::new(),
        };
        engine.apply_settings(initial);
        engine
    }

    pub fn apply_settings(&mut self, mut next: AudioMixerSettings) {
        next.normalize();
        let wanted_ids: Vec<String> = next.channels.iter().map(|c| c.id.clone()).collect();
        self.strips.retain(|id, _| wanted_ids.iter().any(|w| w == id));
        self.order = wanted_ids;

        for ch in &next.channels {
            if let Some(strip) = self.strips.get_mut(&ch.id) {
                strip.update_config(ch.clone());
            } else {
                self.strips.insert(ch.id.clone(), ChannelStrip::new(ch.clone()));
            }
        }
        self.settings = next;
        self.refresh_scene_mics();
    }

    pub fn set_scene_mics(&mut self, live: Vec<String>, used: Vec<String>) {
        self.live_mics = live;
        self.used_mics = used;
        self.refresh_scene_mics();
    }

    fn refresh_scene_mics(&mut self) {
        let has_scene_mics = !self.used_mics.is_empty();
        for strip in self.strips.values_mut() {
            strip.in_scene = if strip.config.kind == AudioChannelKind::Mic && has_scene_mics {
                self.live_mics.iter().any(|id| id == &strip.config.id)
            } else if strip.config.kind == AudioChannelKind::PartyVoice {
                self.settings.party_voice_enabled
            } else {
                true
            };
        }
    }

    pub fn set_channel_status(&mut self, id: &str, state: &str, message: Option<String>) {
        if let Some(strip) = self.strips.get_mut(id) {
            strip.status = AudioChannelStatus {
                id: id.to_string(),
                state: state.to_string(),
                message,
            };
        }
    }

    pub fn statuses(&self) -> Vec<AudioChannelStatus> {
        self.order
            .iter()
            .filter_map(|id| self.strips.get(id).map(|s| s.status.clone()))
            .collect()
    }

    pub fn push_external_pcm(&mut self, channel_id: &str, interleaved_f32: &[f32]) {
        if let Some(strip) = self.strips.get_mut(channel_id) {
            strip.ring.push_interleaved(interleaved_f32);
        }
    }

    pub fn push_external_pcm_bytes(&mut self, channel_id: &str, pcm_bytes: &[u8]) {
        if let Some(strip) = self.strips.get_mut(channel_id) {
            strip.ring.push_pcm_bytes(pcm_bytes);
        }
    }

    /// Processa 1 quantum (480 frames = 10ms @ 48kHz estéreo) somando no Program Bus e Monitor Bus.
    pub fn process_quantum(&mut self, program_out: &mut [f32], monitor_out: &mut [f32]) {
        debug_assert!(program_out.len() >= QUANTUM_SAMPLES);
        debug_assert!(monitor_out.len() >= QUANTUM_SAMPLES);
        program_out[..QUANTUM_SAMPLES].fill(0.0);
        monitor_out[..QUANTUM_SAMPLES].fill(0.0);

        // Passagem 1: processa microfones e PartyVoice primeiro para alimentar sidechain de Ducking.
        self.duck_triggers.clear();
        let mut any_mic_active = false;
        for id in &self.order {
            let Some(strip) = self.strips.get_mut(id) else {
                continue;
            };
            if matches!(
                strip.config.kind,
                AudioChannelKind::Mic | AudioChannelKind::PartyVoice
            ) {
                strip.process_quantum(
                    &HashMap::new(),
                    false,
                    &mut program_out[..QUANTUM_SAMPLES],
                    &mut monitor_out[..QUANTUM_SAMPLES],
                );
                let speaking = strip.config.kind == AudioChannelKind::Mic
                    && !strip.config.muted
                    && strip.in_scene
                    && strip.last_peak_db > -42.0;
                if speaking {
                    any_mic_active = true;
                }
                self.duck_triggers.insert(id.clone(), speaking);
            }
        }

        // Passagem 2: processa desktop e apps com sidechain ducking aplicado.
        for id in &self.order {
            let Some(strip) = self.strips.get_mut(id) else {
                continue;
            };
            if matches!(
                strip.config.kind,
                AudioChannelKind::Desktop | AudioChannelKind::App
            ) {
                strip.process_quantum(
                    &self.duck_triggers,
                    any_mic_active,
                    &mut program_out[..QUANTUM_SAMPLES],
                    &mut monitor_out[..QUANTUM_SAMPLES],
                );
            }
        }

        for s in &mut program_out[..QUANTUM_SAMPLES] {
            *s = s.clamp(-1.0, 1.0);
        }
        for s in &mut monitor_out[..QUANTUM_SAMPLES] {
            *s = s.clamp(-1.0, 1.0);
        }
    }

    pub fn read_levels(&self) -> Vec<AudioLevelSample> {
        self.order
            .iter()
            .filter_map(|id| {
                self.strips.get(id).map(|s| AudioLevelSample {
                    id: id.clone(),
                    peak_db: s.last_peak_db,
                    input_db: s.last_input_db,
                    muted_speech: s.muted_speech_active,
                })
            })
            .collect()
    }

    pub fn is_mic_muted_speaking(&self) -> bool {
        self.strips.values().any(|s| s.muted_speech_active)
    }

    /// Lista aplicativos abertos/com áudio ativo via API nativa do SO.
    pub fn list_audio_apps() -> Vec<AudioApp> {
        platform::list_audio_apps()
    }

    /// Lista dispositivos de entrada/saída de áudio para seleção de microfone e monitoramento.
    pub fn list_devices() -> Vec<AudioDeviceInfo> {
        platform::list_devices()
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::collections::{BTreeMap, HashSet};
    use wasapi::{initialize_mta, DeviceEnumerator, Direction, SessionState};
    use windows::core::{BOOL, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowLongW, GetWindowTextW, GetWindowThreadProcessId,
        IsWindowVisible, GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW,
    };

    struct Win {
        hwnd: isize,
        pid: u32,
        title: String,
    }

    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = &mut *(lparam.0 as *mut Vec<Win>);
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        if GetWindow(hwnd, GW_OWNER)
            .map(|h| !h.is_invalid())
            .unwrap_or(false)
        {
            return BOOL(1);
        }
        if (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32) & WS_EX_TOOLWINDOW.0 != 0 {
            return BOOL(1);
        }
        let mut cloaked: u32 = 0;
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        );
        if cloaked != 0 {
            return BOOL(1);
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n <= 0 {
            return BOOL(1);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        out.push(Win {
            hwnd: hwnd.0 as isize,
            pid,
            title: String::from_utf16_lossy(&buf[..n as usize]),
        });
        BOOL(1)
    }

    fn exe_name_for_pid(pid: u32) -> Option<String> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut size = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(
                h,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut size,
            )
            .is_ok();
            let _ = CloseHandle(h);
            if !ok {
                return None;
            }
            let path = String::from_utf16_lossy(&buf[..size as usize]);
            Some(path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string())
        }
    }

    fn processes() -> BTreeMap<u32, String> {
        let mut out = BTreeMap::new();
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return out;
            };
            let mut e = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut ok = Process32FirstW(snap, &mut e).is_ok();
            while ok {
                let len = e
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(e.szExeFile.len());
                out.insert(
                    e.th32ProcessID,
                    String::from_utf16_lossy(&e.szExeFile[..len]),
                );
                ok = Process32NextW(snap, &mut e).is_ok();
            }
            let _ = CloseHandle(snap);
        }
        out
    }

    fn active_audio_pids() -> HashSet<u32> {
        let mut pids = HashSet::new();
        let Ok(enumerator) = DeviceEnumerator::new() else {
            return pids;
        };
        let Ok(devices) = enumerator.get_device_collection(&Direction::Render) else {
            return pids;
        };
        for device in &devices {
            let Ok(dev) = device else { continue };
            let Ok(manager) = dev.get_iaudiosessionmanager() else {
                continue;
            };
            let Ok(sessions) = manager.get_audiosessionenumerator() else {
                continue;
            };
            for i in 0..sessions.get_count().unwrap_or(0) {
                let Ok(control) = sessions.get_session(i) else {
                    continue;
                };
                if control.get_state().ok() != Some(SessionState::Active) {
                    continue;
                }
                if let Ok(pid) = control.get_process_id() {
                    if pid != 0 {
                        pids.insert(pid);
                    }
                }
            }
        }
        pids
    }

    pub fn list_audio_apps() -> Vec<AudioApp> {
        let _ = initialize_mta();
        let mut wins: Vec<Win> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut wins as *mut Vec<Win> as isize));
        }
        let me = std::process::id();
        let audio = active_audio_pids();
        let procs = processes();
        let audio_exes: HashSet<String> = audio
            .iter()
            .filter_map(|pid| procs.get(pid).map(|e| e.to_lowercase()))
            .collect();

        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for w in wins {
            if w.pid == me {
                continue;
            }
            let Some(exe) = exe_name_for_pid(w.pid) else {
                continue;
            };
            let lower = exe.to_lowercase();
            if seen.contains(&lower)
                || lower == "explorer.exe"
                || lower == "textinputhost.exe"
                || lower == "applicationframehost.exe"
                || lower == "prismcast.exe"
                || lower == "prismcast-audio.exe"
            {
                continue;
            }
            seen.insert(lower.clone());
            out.push(AudioApp {
                hwnd: w.hwnd,
                pid: w.pid,
                exe,
                title: w.title,
                audio: audio_exes.contains(&lower),
            });
        }
        for pid in audio {
            if pid == me {
                continue;
            }
            let Some(exe) = procs.get(&pid) else {
                continue;
            };
            let lower = exe.to_lowercase();
            if seen.contains(&lower) || lower == "audiodg.exe" || lower == "prismcast.exe" {
                continue;
            }
            seen.insert(lower);
            out.push(AudioApp {
                hwnd: 0,
                pid,
                exe: exe.clone(),
                title: exe.trim_end_matches(".exe").to_string(),
                audio: true,
            });
        }
        out
    }

    pub fn list_devices() -> Vec<AudioDeviceInfo> {
        let _ = initialize_mta();
        let Ok(enumerator) = DeviceEnumerator::new() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (dir, kind) in [(Direction::Capture, "input"), (Direction::Render, "output")] {
            let def_id = enumerator
                .get_default_device(&dir)
                .ok()
                .and_then(|d| d.get_id().ok());
            if let Ok(col) = enumerator.get_device_collection(&dir) {
                for dev in &col {
                    let Ok(d) = dev else { continue };
                    let Ok(id) = d.get_id() else { continue };
                    let name = d.get_friendlyname().unwrap_or_else(|_| id.clone());
                    out.push(AudioDeviceInfo {
                        is_default: def_id.as_deref() == Some(id.as_str()),
                        id,
                        name,
                        kind: kind.to_string(),
                    });
                }
            }
        }
        out
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub fn list_audio_apps() -> Vec<AudioApp> {
        Vec::new()
    }

    pub fn list_devices() -> Vec<AudioDeviceInfo> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AudioFilters;

    fn make_channel(id: &str, kind: AudioChannelKind) -> AudioChannel {
        AudioChannel {
            id: id.to_string(),
            kind,
            name: id.to_string(),
            filters: AudioFilters::default_for(kind),
            ..Default::default()
        }
    }

    #[test]
    fn test_ring_stereo_alignment_and_micro_fade() {
        let mut ring = StereoFrameRing::new(4096);
        let mut raw_bytes = Vec::new();
        // Gera exatamente TARGET_RING_FRAMES + QUANTUM_FRAMES frames com L = 0.5, R = -0.5
        let total = TARGET_RING_FRAMES + QUANTUM_FRAMES;
        for _ in 0..total {
            raw_bytes.extend_from_slice(&0.5_f32.to_le_bytes());
            raw_bytes.extend_from_slice(&(-0.5_f32).to_le_bytes());
        }
        // Alimenta em pedaços ímpares (5, 7, 13 bytes) para estressar alinhamento de 8 bytes!
        let mut pos = 0usize;
        for step in [5, 7, 13, 3, 19].into_iter().cycle() {
            if pos >= raw_bytes.len() {
                break;
            }
            let end = (pos + step).min(raw_bytes.len());
            ring.push_pcm_bytes(&raw_bytes[pos..end]);
            pos = end;
        }

        let mut out = vec![0.0_f32; QUANTUM_SAMPLES];
        assert!(ring.read_quantum(&mut out));
        // Primeiro sample deve ter micro-fade (próximo de 0, subindo) e sinal L>0, R<0 (sem inversão!)
        assert!(out[0] > 0.0 && out[0] < 0.02);
        assert!(out[1] < 0.0 && out[1] > -0.02);
        // Após 144 frames de fade, atinge valor exato 0.5 / -0.5
        let f_after = MICRO_FADE_FRAMES + 10;
        assert!((out[f_after * 2] - 0.5).abs() < 1e-5);
        assert!((out[f_after * 2 + 1] + 0.5).abs() < 1e-5);
    }

    #[test]
    fn test_real_bypass_hpf_and_noise_gate() {
        let mut hpf = BiquadFilter::bypass();
        hpf.set_highpass(0.0);
        assert_eq!(hpf.process_frame(0.123, -0.456), (0.123, -0.456));

        let mut gate = NoiseGate::new();
        gate.configure(false, -10.0, 150);
        assert_eq!(gate.process_frame(0.001, -0.002), (0.001, -0.002));

        gate.configure(true, -10.0, 50);
        // Sinal bem abaixo de -10dB (-60dB) é atenuado quando ligado
        let (gl, _) = gate.process_frame(0.001, 0.001);
        assert!(gl < 0.001);
    }

    #[test]
    fn test_3band_eq_boosts_low_and_attenuates_high() {
        let mut low = BiquadFilter::bypass();
        low.set_low_shelf(100.0, 12.0);
        let mut high = BiquadFilter::bypass();
        high.set_high_shelf(8000.0, -12.0);

        let mut energy_low = 0.0_f32;
        let mut energy_high = 0.0_f32;
        // Senoide 80 Hz passa por low-shelf (+12dB) vs high-shelf
        for i in 0..480 {
            let s = (2.0 * PI * 80.0 * (i as f32) / SAMPLE_RATE).sin() * 0.1;
            let (l1, _) = low.process_frame(s, s);
            let (l2, _) = high.process_frame(s, s);
            energy_low += l1 * l1;
            energy_high += l2 * l2;
        }
        assert!(energy_low > energy_high * 2.0);
    }

    #[test]
    fn test_sidechain_ducking_attenuates_desktop_when_mic_speaks() {
        let mut desk = make_channel("desktop", AudioChannelKind::Desktop);
        desk.filters.ducking_enabled = true;
        desk.filters.ducking_source_id = Some("mic".to_string());
        desk.filters.ducking_amount_db = -18.0;

        let mut mic = make_channel("mic", AudioChannelKind::Mic);
        mic.filters.highpass_hz = 0.0;
        mic.filters.compressor.enabled = false;
        mic.filters.limiter.enabled = false;

        let mut engine = AudioMixerEngine::new(AudioMixerSettings {
            channels: vec![desk, mic],
            party_voice_enabled: true,
            monitor_device_id: None,
        });

        let loud_mic = vec![0.4_f32; (TARGET_RING_FRAMES + QUANTUM_FRAMES * 4) * 2];
        let loud_desk = vec![0.25_f32; (TARGET_RING_FRAMES + QUANTUM_FRAMES * 4) * 2];
        engine.push_external_pcm("mic", &loud_mic);
        engine.push_external_pcm("desktop", &loud_desk);

        let mut prog = vec![0.0_f32; QUANTUM_SAMPLES];
        let mut mon = vec![0.0_f32; QUANTUM_SAMPLES];
        // Quantum 1 ativa detecção do mic; quantum 2 e 3 aplicam ducking no desktop.
        for _ in 0..4 {
            engine.process_quantum(&mut prog, &mut mon);
        }
        let levels = engine.read_levels();
        let desk_lvl = levels.iter().find(|l| l.id == "desktop").unwrap();
        // Desktop original é 0.25 (-12 dBFS); após ducking de -18 dB cai bem abaixo de -22 dBFS
        assert!(
            desk_lvl.peak_db < -20.0,
            "expected ducked desktop < -20 dB, got {}",
            desk_lvl.peak_db
        );
    }

    #[test]
    fn test_muted_mic_speaking_detection() {
        let mut mic = make_channel("mic", AudioChannelKind::Mic);
        mic.muted = true;
        mic.filters.highpass_hz = 0.0;

        let mut engine = AudioMixerEngine::new(AudioMixerSettings {
            channels: vec![mic],
            party_voice_enabled: true,
            monitor_device_id: None,
        });

        // Alimenta em blocos de 10 quantums por passo para simular stream contínuo sem estourar MAX_RING_FRAMES (250 ms)
        let total_quantums = MUTED_SPEECH_HOLD_QUANTUMS as usize + 10;
        let mut prog = vec![0.0_f32; QUANTUM_SAMPLES];
        let mut mon = vec![0.0_f32; QUANTUM_SAMPLES];
        let mut sample_idx = 0usize;

        for q in 0..total_quantums {
            let mut chunk = Vec::with_capacity(QUANTUM_SAMPLES);
            for _ in 0..QUANTUM_FRAMES {
                let v = (2.0 * PI * 200.0 * (sample_idx as f32) / SAMPLE_RATE).sin() * 0.3;
                chunk.push(v);
                chunk.push(v);
                sample_idx += 1;
            }
            engine.push_external_pcm("mic", &chunk);
            if q == 0 {
                // Garante priming inicial do ring buffer (40 ms)
                let mut prime = vec![0.0_f32; TARGET_RING_FRAMES * 2];
                for s in prime.chunks_exact_mut(2) {
                    let v = (2.0 * PI * 200.0 * (sample_idx as f32) / SAMPLE_RATE).sin() * 0.3;
                    s[0] = v;
                    s[1] = v;
                    sample_idx += 1;
                }
                engine.push_external_pcm("mic", &prime);
            }
            engine.process_quantum(&mut prog, &mut mon);
            assert!(prog.iter().all(|&s| s == 0.0));
        }
        assert!(engine.is_mic_muted_speaking());
    }
}
