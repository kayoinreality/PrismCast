//! Controlador de Bitrate Dinâmico (ABR — Adaptive Bitrate) baseado em AIMD.
//!
//! Monitora continuamente sinais reais de congestionamento de transporte e encoder:
//! - Ocupação de fila de saída/entrada (`queue_occupancy > 0.65`)
//! - Latência real de escrita no socket RTMP/pipe (`write_latency_ms > 180 ms`, suavizada por EMA)
//! - Delta de frames descartados (`dropped_frames_delta`)
//! - Déficit entre bitrate observado e alvo (`observed < current * 0.72`)
//!
//! Política AIMD com histerese anti-flapping:
//! - **Redução multiplicativa rápida** sob congestionamento sustentado ou severo, respeitando
//!   cooldown mínimo (3 s) e piso `min_bitrate_kbps`.
//! - **Recuperação aditiva gradual** após `stability_window_secs` contínuos com congestionamento
//!   baixo (`< 0.20`), subindo em passos de `step_up_kbps` até `configured_bitrate_kbps` sem overshoot.

use crate::protocol::{AbrDestinationTelemetry, DynamicBitrateSettings};
use std::time::{Duration, Instant};

const CONGESTION_TRIGGER_THRESHOLD: f32 = 0.48;
const SEVERE_CONGESTION_THRESHOLD: f32 = 0.72;
const LOW_CONGESTION_THRESHOLD: f32 = 0.20;
const STEP_DOWN_COOLDOWN: Duration = Duration::from_secs(3);
const MIN_CHANGE_KBPS: u32 = 100;
const MIN_ALLOWED_BITRATE_KBPS: u32 = 500;
const MAX_ALLOWED_BITRATE_KBPS: u32 = 25_000;

/// Amostra instantânea de telemetria de rede/encoder alimentada no controlador ABR.
#[derive(Debug, Clone, Copy, Default)]
pub struct CongestionSample {
    /// Fração de ocupação das filas de pipeline (`0.0..=1.0`).
    pub queue_occupancy: f32,
    /// Latência medida de escrita no socket RTMP ou pipe do encoder, em milissegundos.
    pub write_latency_ms: f32,
    /// Quantidade de novos frames descartados desde a avaliação anterior.
    pub dropped_frames_delta: u64,
    /// Bitrate efetivamente transmitido/codificado no intervalo, em kbps (`0` se indisponível).
    pub observed_bitrate_kbps: u32,
}

/// Controlador AIMD de Bitrate Dinâmico por grupo/destino de transmissão.
#[derive(Debug, Clone)]
pub struct DynamicBitrateController {
    settings: DynamicBitrateSettings,
    configured_bitrate_kbps: u32,
    current_bitrate_kbps: u32,
    min_bitrate_kbps: u32,
    observed_bitrate_kbps: u32,
    write_latency_ema_ms: f32,
    congestion_level: f32,
    adjustments: u32,
    last_change_at: Instant,
    stable_since: Instant,
    congested_streak: u32,
    has_samples: bool,
    has_changed: bool,
    state: String, // "nominal" | "congested" | "recovering" | "floor"
}

impl DynamicBitrateController {
    /// Cria um novo controlador ABR ancorado no instante atual.
    pub fn new(configured_bitrate_kbps: u32, settings: DynamicBitrateSettings) -> Self {
        Self::with_now(configured_bitrate_kbps, settings, Instant::now())
    }

    /// Cria um controlador ABR com relógio inicial explícito (útil para testes determinísticos).
    pub fn with_now(
        configured_bitrate_kbps: u32,
        mut settings: DynamicBitrateSettings,
        now: Instant,
    ) -> Self {
        settings.normalize();
        let configured =
            configured_bitrate_kbps.clamp(MIN_ALLOWED_BITRATE_KBPS, MAX_ALLOWED_BITRATE_KBPS);
        let min_kbps = settings.min_bitrate_kbps.min(configured);

        Self {
            settings,
            configured_bitrate_kbps: configured,
            current_bitrate_kbps: configured,
            min_bitrate_kbps: min_kbps,
            observed_bitrate_kbps: 0,
            write_latency_ema_ms: 0.0,
            congestion_level: 0.0,
            adjustments: 0,
            last_change_at: now,
            stable_since: now,
            congested_streak: 0,
            has_samples: false,
            has_changed: false,
            state: "nominal".to_string(),
        }
    }

    /// Atualiza configurações ABR em tempo real preservando o histórico de telemetria.
    pub fn update_settings(&mut self, mut settings: DynamicBitrateSettings) {
        settings.normalize();
        self.min_bitrate_kbps = settings.min_bitrate_kbps.min(self.configured_bitrate_kbps);
        self.settings = settings;
        if !self.settings.enabled {
            self.state = "nominal".to_string();
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.settings.enabled
    }

    pub fn configured_bitrate_kbps(&self) -> u32 {
        self.configured_bitrate_kbps
    }

    pub fn current_bitrate_kbps(&self) -> u32 {
        self.current_bitrate_kbps
    }

    pub fn min_bitrate_kbps(&self) -> u32 {
        self.min_bitrate_kbps
    }

    pub fn observed_bitrate_kbps(&self) -> u32 {
        self.observed_bitrate_kbps
    }

    pub fn write_latency_ema_ms(&self) -> f32 {
        self.write_latency_ema_ms
    }

    pub fn congestion_level(&self) -> f32 {
        self.congestion_level
    }

    pub fn adjustments(&self) -> u32 {
        self.adjustments
    }

    pub fn state(&self) -> &str {
        &self.state
    }

    /// Avalia uma nova amostra de congestionamento no instante `now`.
    ///
    /// Retorna `Some(new_bitrate_kbps)` somente quando uma alteração significativa
    /// (`>= 100 kbps`) é necessária segundo a política AIMD.
    pub fn evaluate(&mut self, sample: CongestionSample, now: Instant) -> Option<u32> {
        let queue = if sample.queue_occupancy.is_finite() {
            sample.queue_occupancy.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let write_lat = if sample.write_latency_ms.is_finite() {
            sample.write_latency_ms.max(0.0)
        } else {
            0.0
        };

        if sample.observed_bitrate_kbps > 0 {
            self.observed_bitrate_kbps = sample.observed_bitrate_kbps;
        }

        // Suavização assimétrica da latência de escrita: ataque rápido para stalls, decaimento suave.
        if !self.has_samples {
            self.write_latency_ema_ms = write_lat;
        } else {
            let lat_alpha = if write_lat >= self.write_latency_ema_ms {
                0.35
            } else {
                0.20
            };
            self.write_latency_ema_ms =
                lat_alpha * write_lat + (1.0 - lat_alpha) * self.write_latency_ema_ms;
        }

        // 1. Saturação de fila (> 0.65 indica backpressure real).
        let queue_score = if queue > 0.65 {
            0.50 + ((queue - 0.65) / 0.35).clamp(0.0, 1.0) * 0.50
        } else {
            ((queue - 0.35).max(0.0) / 0.30) * 0.15
        };

        // 2. Stalls de escrita no socket/pipe (> 180 ms na EMA).
        let latency_score = if self.write_latency_ema_ms > 180.0 {
            0.50 + ((self.write_latency_ema_ms - 180.0) / 320.0).clamp(0.0, 1.0) * 0.50
        } else {
            ((self.write_latency_ema_ms - 60.0).max(0.0) / 120.0) * 0.15
        };

        // 3. Delta de frames descartados.
        let drop_score = if sample.dropped_frames_delta == 0 {
            0.0
        } else {
            (0.35 + (sample.dropped_frames_delta.min(20) as f32 / 20.0) * 0.65).min(1.0)
        };

        // 4. Déficit de throughput observado (< 72% do bitrate alvo atual).
        let deficit_score = if sample.observed_bitrate_kbps > 0 && self.current_bitrate_kbps > 0 {
            let ratio = sample.observed_bitrate_kbps as f32 / self.current_bitrate_kbps as f32;
            if ratio < 0.72 {
                0.50 + ((0.72 - ratio) / 0.42).clamp(0.0, 1.0) * 0.50
            } else if ratio < 0.85 {
                ((0.85 - ratio) / 0.13) * 0.12
            } else {
                0.0
            }
        } else {
            0.0
        };

        let max_signal = queue_score
            .max(latency_score)
            .max(drop_score)
            .max(deficit_score);
        let weighted_avg = queue_score * 0.35
            + latency_score * 0.35
            + drop_score * 0.15
            + deficit_score * 0.15;
        let instant_congestion = (max_signal * 0.75 + weighted_avg * 0.25).clamp(0.0, 1.0);

        if !self.has_samples {
            self.congestion_level = instant_congestion;
            self.has_samples = true;
        } else {
            let cong_alpha = if instant_congestion >= self.congestion_level {
                0.55
            } else {
                0.25
            };
            self.congestion_level = (cong_alpha * instant_congestion
                + (1.0 - cong_alpha) * self.congestion_level)
                .clamp(0.0, 1.0);
        }

        if !self.settings.enabled {
            self.state = "nominal".to_string();
            return None;
        }

        if self.congestion_level >= LOW_CONGESTION_THRESHOLD {
            self.stable_since = now;
        }

        let sample_congested = sample.queue_occupancy > 0.45
            || sample.write_latency_ms > 180.0
            || sample.dropped_frames_delta > 0
            || deficit_score >= 0.40;

        if sample_congested && self.congestion_level >= CONGESTION_TRIGGER_THRESHOLD {
            self.congested_streak = self.congested_streak.saturating_add(1);
        } else {
            self.congested_streak = 0;
        }

        // --- Fase 1: Redução Multiplicativa Rápida (MD) ---
        // Só reduz bitrate quando a amostra atual confirma congestionamento ativo
        // (evita queda fantasma enquanto a EWMA esfria após recuperação da rede).
        let congestion_triggered = sample_congested
            && (self.congestion_level >= SEVERE_CONGESTION_THRESHOLD || self.congested_streak >= 2);

        if congestion_triggered {
            if self.current_bitrate_kbps <= self.min_bitrate_kbps {
                self.state = "floor".to_string();
                return None;
            }

            let cooldown_ok = !self.has_changed
                || now.saturating_duration_since(self.last_change_at) >= STEP_DOWN_COOLDOWN;

            if cooldown_ok {
                let raw =
                    (self.current_bitrate_kbps as f32 * self.settings.step_down_ratio).round()
                        as u32;
                let mut candidate = round_to_nearest_50(raw).max(self.min_bitrate_kbps);
                if candidate > self.min_bitrate_kbps
                    && candidate.saturating_sub(self.min_bitrate_kbps) < MIN_CHANGE_KBPS
                {
                    candidate = self.min_bitrate_kbps;
                }

                if self.current_bitrate_kbps.saturating_sub(candidate) >= MIN_CHANGE_KBPS {
                    self.current_bitrate_kbps = candidate;
                    self.adjustments = self.adjustments.saturating_add(1);
                    self.has_changed = true;
                    self.last_change_at = now;
                    self.stable_since = now;
                    self.congested_streak = 0;
                    self.state = if candidate <= self.min_bitrate_kbps {
                        "floor".to_string()
                    } else {
                        "congested".to_string()
                    };
                    return Some(candidate);
                }
            }

            self.state = if self.current_bitrate_kbps <= self.min_bitrate_kbps {
                "floor".to_string()
            } else {
                "congested".to_string()
            };
            return None;
        }

        // --- Fase 2: Recuperação Aditiva Gradual (AI) ---
        if self.current_bitrate_kbps < self.configured_bitrate_kbps {
            if self.congestion_level < LOW_CONGESTION_THRESHOLD {
                let window = Duration::from_secs(self.settings.stability_window_secs as u64);
                let stable_elapsed = now.saturating_duration_since(self.stable_since);
                let since_change = now.saturating_duration_since(self.last_change_at);

                if stable_elapsed >= window && since_change >= window {
                    let mut candidate = self
                        .current_bitrate_kbps
                        .saturating_add(self.settings.step_up_kbps)
                        .min(self.configured_bitrate_kbps);
                    if candidate < self.configured_bitrate_kbps
                        && self.configured_bitrate_kbps.saturating_sub(candidate) < MIN_CHANGE_KBPS
                    {
                        candidate = self.configured_bitrate_kbps;
                    }

                    if candidate.saturating_sub(self.current_bitrate_kbps) >= MIN_CHANGE_KBPS {
                        self.current_bitrate_kbps = candidate;
                        self.adjustments = self.adjustments.saturating_add(1);
                        self.has_changed = true;
                        self.last_change_at = now;
                        self.stable_since = now;
                        self.state = if candidate >= self.configured_bitrate_kbps {
                            "nominal".to_string()
                        } else {
                            "recovering".to_string()
                        };
                        return Some(candidate);
                    }
                }

                self.state = if self.current_bitrate_kbps <= self.min_bitrate_kbps {
                    "floor".to_string()
                } else {
                    "recovering".to_string()
                };
                return None;
            }

            self.state = if self.current_bitrate_kbps <= self.min_bitrate_kbps {
                "floor".to_string()
            } else {
                "congested".to_string()
            };
            return None;
        }

        // --- Fase 3: Bitrate nominal ---
        self.state = if self.congestion_level >= CONGESTION_TRIGGER_THRESHOLD {
            "congested".to_string()
        } else {
            "nominal".to_string()
        };
        None
    }

    /// Gera um snapshot de telemetria ABR compatível com `protocol::AbrDestinationTelemetry`.
    pub fn telemetry(&self, id: &str) -> AbrDestinationTelemetry {
        AbrDestinationTelemetry {
            id: id.to_string(),
            configured_bitrate_kbps: self.configured_bitrate_kbps,
            current_bitrate_kbps: self.current_bitrate_kbps,
            observed_bitrate_kbps: self.observed_bitrate_kbps,
            congestion_level: (self.congestion_level * 1000.0).round() / 1000.0,
            write_latency_ms: (self.write_latency_ema_ms * 10.0).round() / 10.0,
            adjustments: self.adjustments,
            state: self.state.clone(),
        }
    }
}

fn round_to_nearest_50(value: u32) -> u32 {
    ((value + 25) / 50) * 50
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_settings() -> DynamicBitrateSettings {
        DynamicBitrateSettings {
            enabled: true,
            min_bitrate_kbps: 1500,
            step_down_ratio: 0.75,
            step_up_kbps: 250,
            stability_window_secs: 10,
            use_persistent_rtmp: true,
        }
    }

    fn nominal_sample(bitrate_kbps: u32) -> CongestionSample {
        CongestionSample {
            queue_occupancy: 0.12,
            write_latency_ms: 18.0,
            dropped_frames_delta: 0,
            observed_bitrate_kbps: bitrate_kbps,
        }
    }

    fn severe_congestion_sample(current_kbps: u32) -> CongestionSample {
        CongestionSample {
            queue_occupancy: 0.92,
            write_latency_ms: 340.0,
            dropped_frames_delta: 12,
            observed_bitrate_kbps: (current_kbps as f32 * 0.45) as u32,
        }
    }

    #[test]
    fn test_nominal_traffic_keeps_bitrate_unchanged() {
        let t0 = Instant::now();
        let mut ctrl = DynamicBitrateController::with_now(6000, default_settings(), t0);

        for sec in 1..=30 {
            let now = t0 + Duration::from_secs(sec);
            let change = ctrl.evaluate(nominal_sample(6000), now);
            assert!(
                change.is_none(),
                "nominal traffic should never adjust bitrate at sec={sec}"
            );
        }

        assert_eq!(ctrl.current_bitrate_kbps(), 6000);
        assert_eq!(ctrl.adjustments(), 0);
        assert_eq!(ctrl.state(), "nominal");
        assert!(ctrl.congestion_level() < 0.05);

        let tel = ctrl.telemetry("dest-twitch");
        assert_eq!(tel.id, "dest-twitch");
        assert_eq!(tel.configured_bitrate_kbps, 6000);
        assert_eq!(tel.current_bitrate_kbps, 6000);
        assert_eq!(tel.observed_bitrate_kbps, 6000);
        assert_eq!(tel.state, "nominal");
    }

    #[test]
    fn test_congestion_spike_steps_down_multiplicatively_and_hits_floor() {
        let t0 = Instant::now();
        let mut ctrl = DynamicBitrateController::with_now(6000, default_settings(), t0);

        // Aquecimento nominal
        ctrl.evaluate(nominal_sample(6000), t0 + Duration::from_secs(1));

        let mut steps = Vec::new();
        for sec in 2..=40 {
            let now = t0 + Duration::from_secs(sec);
            let cur = ctrl.current_bitrate_kbps();
            if let Some(next) = ctrl.evaluate(severe_congestion_sample(cur), now) {
                steps.push(next);
            }
            assert!(
                ctrl.current_bitrate_kbps() >= 1500,
                "bitrate must never drop below min_bitrate_kbps"
            );
        }

        // Sequência esperada para 6000 * 0.75^k arredondado para múltiplos de 50:
        // 6000 -> 4500 -> 3400 -> 2550 -> 1900 -> 1500 (piso)
        assert_eq!(steps, vec![4500, 3400, 2550, 1900, 1500]);
        assert_eq!(ctrl.current_bitrate_kbps(), 1500);
        assert_eq!(ctrl.state(), "floor");
        assert_eq!(ctrl.adjustments(), 5);

        // Congestionamento contínuo no piso não deve reduzir mais nem emitir novas trocas
        let extra = ctrl.evaluate(
            severe_congestion_sample(1500),
            t0 + Duration::from_secs(50),
        );
        assert!(extra.is_none());
        assert_eq!(ctrl.current_bitrate_kbps(), 1500);
        assert_eq!(ctrl.state(), "floor");
    }

    #[test]
    fn test_sustained_clean_network_recovers_additively_without_overshoot() {
        let t0 = Instant::now();
        let mut settings = default_settings();
        settings.stability_window_secs = 5;
        settings.step_up_kbps = 500;
        let mut ctrl = DynamicBitrateController::with_now(6000, settings, t0);

        // Força dois degraus de redução (6000 -> 4500 -> 3400)
        let mut sec = 1u64;
        while ctrl.current_bitrate_kbps() > 3500 && sec < 25 {
            let cur = ctrl.current_bitrate_kbps();
            ctrl.evaluate(severe_congestion_sample(cur), t0 + Duration::from_secs(sec));
            sec += 1;
        }
        assert_eq!(ctrl.current_bitrate_kbps(), 3400);

        // Rede volta ao normal de forma sustentada: recuperação aditiva em passos de 500 kbps até 6000
        let mut recovery_steps = Vec::new();
        for _ in 0..80 {
            sec += 1;
            let cur = ctrl.current_bitrate_kbps();
            if let Some(next) = ctrl.evaluate(nominal_sample(cur), t0 + Duration::from_secs(sec)) {
                recovery_steps.push(next);
            }
            assert!(
                ctrl.current_bitrate_kbps() <= 6000,
                "recovery must never overshoot configured_bitrate_kbps"
            );
        }

        // 3400 -> 3900 -> 4400 -> 4900 -> 5400 -> 5900 -> 6000 (snap para configured pois 6000-5900=100)
        assert_eq!(recovery_steps, vec![3900, 4400, 4900, 5400, 5900, 6000]);
        assert_eq!(ctrl.current_bitrate_kbps(), 6000);
        assert_eq!(ctrl.state(), "nominal");

        // Permanecer limpo não ultrapassa 6000
        for _ in 0..20 {
            sec += 1;
            assert!(ctrl
                .evaluate(nominal_sample(6000), t0 + Duration::from_secs(sec))
                .is_none());
        }
        assert_eq!(ctrl.current_bitrate_kbps(), 6000);
    }

    #[test]
    fn test_hysteresis_prevents_flapping_on_transient_jitter() {
        let t0 = Instant::now();
        let mut ctrl = DynamicBitrateController::with_now(6000, default_settings(), t0);

        let mut sec = 0u64;
        // 5 ciclos de tráfego nominal intercalados por um único blip transitório de jitter
        for _ in 0..5 {
            for _ in 0..5 {
                sec += 1;
                assert!(ctrl
                    .evaluate(nominal_sample(6000), t0 + Duration::from_secs(sec))
                    .is_none());
            }
            sec += 1;
            let jitter_blip = CongestionSample {
                queue_occupancy: 0.72,
                write_latency_ms: 210.0,
                dropped_frames_delta: 1,
                observed_bitrate_kbps: 5200,
            };
            let res = ctrl.evaluate(jitter_blip, t0 + Duration::from_secs(sec));
            assert!(
                res.is_none(),
                "single transient jitter blip must not trigger bitrate flapping"
            );
        }

        assert_eq!(ctrl.current_bitrate_kbps(), 6000);
        assert_eq!(ctrl.adjustments(), 0);
    }

    #[test]
    fn test_step_down_cooldown_prevents_instant_collapse() {
        let t0 = Instant::now();
        let mut ctrl = DynamicBitrateController::with_now(6000, default_settings(), t0);

        // Primeiro pico severo reduz 6000 -> 4500 imediatamente
        let first = ctrl.evaluate(severe_congestion_sample(6000), t0 + Duration::from_secs(1));
        assert_eq!(first, Some(4500));

        // Amostras nos próximos 2 segundos ficam bloqueadas pelo cooldown de 3 s
        assert!(ctrl
            .evaluate(severe_congestion_sample(4500), t0 + Duration::from_secs(2))
            .is_none());
        assert!(ctrl
            .evaluate(severe_congestion_sample(4500), t0 + Duration::from_secs(3))
            .is_none());
        assert_eq!(ctrl.current_bitrate_kbps(), 4500);

        // Após 3 segundos completos de cooldown, permite o próximo degrau (4500 -> 3400)
        let second = ctrl.evaluate(severe_congestion_sample(4500), t0 + Duration::from_secs(4));
        assert_eq!(second, Some(3400));
    }

    #[test]
    fn test_disabled_controller_keeps_observability_without_adjustments() {
        let t0 = Instant::now();
        let mut settings = default_settings();
        settings.enabled = false;
        let mut ctrl = DynamicBitrateController::with_now(6000, settings, t0);

        for sec in 1..=10 {
            assert!(ctrl
                .evaluate(severe_congestion_sample(6000), t0 + Duration::from_secs(sec))
                .is_none());
        }
        assert_eq!(ctrl.current_bitrate_kbps(), 6000);
        assert_eq!(ctrl.adjustments(), 0);
        assert_eq!(ctrl.state(), "nominal");
        assert!(ctrl.congestion_level() > 0.5);
        assert!(ctrl.write_latency_ema_ms() > 180.0);
    }
}
