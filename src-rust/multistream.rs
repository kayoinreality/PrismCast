use crate::protocol::{
    CanvasOrientation, MultistreamStatus, MultistreamTarget, MultistreamTargetStatus,
};
use std::collections::HashMap;
use std::io::Write;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Motor de Multistreaming + Dual Output (Horizontal 16:9 e Vertical 9:16) em Rust.
///
/// Inspirado diretamente no `StreamingService` + `DualOutputService` + `RestreamService`
/// extraidos via engenharia reversa do Streamlabs Desktop:
/// - Mantem dois contextos independentes de codificacao/saida (`horizontal` e `vertical`).
/// - Detecta encoders de hardware via FFmpeg (`h264_nvenc`, `h264_amf`, `h264_qsv`, `libx264`).
/// - Recebe chunks WebM/Matroska de cada canvas e faz fan-out simultaneo para
///   Twitch, YouTube Live, Kick, TikTok Live (9:16) e servidores RTMP personalizados.
pub struct MultistreamEngine {
    encoder: String,
    active: bool,
    started_at: Option<Instant>,
    horizontal_proc: Option<FfmpegPipeline>,
    vertical_proc: Option<FfmpegPipeline>,
    targets: Vec<MultistreamTarget>,
    bytes_sent: HashMap<String, u64>,
    errors: HashMap<String, String>,
}

struct FfmpegPipeline {
    child: Child,
    stdin: Option<ChildStdin>,
    target_ids: Vec<String>,
}

impl MultistreamEngine {
    pub fn new() -> Self {
        let encoder = detect_best_h264_encoder();
        Self {
            encoder,
            active: false,
            started_at: None,
            horizontal_proc: None,
            vertical_proc: None,
            targets: Vec::new(),
            bytes_sent: HashMap::new(),
            errors: HashMap::new(),
        }
    }

    pub fn detected_encoder(&self) -> &str {
        &self.encoder
    }

    pub fn start(&mut self, targets: Vec<MultistreamTarget>, fps: u32) -> Result<MultistreamStatus, String> {
        self.stop();

        let enabled: Vec<MultistreamTarget> = targets
            .into_iter()
            .filter(|t| t.enabled && !t.rtmpUrl.trim().is_empty() && !t.streamKey.trim().is_empty())
            .collect();

        if enabled.is_empty() {
            return Err("Ative pelo menos um destino (Twitch, YouTube, Kick ou TikTok) e informe a Stream Key.".to_string());
        }

        let horiz_targets: Vec<MultistreamTarget> = enabled
            .iter()
            .filter(|t| t.orientation == CanvasOrientation::Horizontal)
            .cloned()
            .collect();

        let vert_targets: Vec<MultistreamTarget> = enabled
            .iter()
            .filter(|t| t.orientation == CanvasOrientation::Vertical)
            .cloned()
            .collect();

        self.bytes_sent.clear();
        self.errors.clear();

        if !horiz_targets.is_empty() {
            match spawn_ffmpeg_fanout(&self.encoder, &horiz_targets, fps, false) {
                Ok(pipe) => self.horizontal_proc = Some(pipe),
                Err(err) => {
                    for t in &horiz_targets {
                        self.errors.insert(t.id.clone(), err.clone());
                    }
                }
            }
        }

        if !vert_targets.is_empty() {
            match spawn_ffmpeg_fanout(&self.encoder, &vert_targets, fps, true) {
                Ok(pipe) => self.vertical_proc = Some(pipe),
                Err(err) => {
                    for t in &vert_targets {
                        self.errors.insert(t.id.clone(), err.clone());
                    }
                }
            }
        }

        self.targets = enabled;
        self.active = self.horizontal_proc.is_some() || self.vertical_proc.is_some();
        self.started_at = if self.active { Some(Instant::now()) } else { None };

        Ok(self.status())
    }

    /// Alimenta o pipeline Horizontal (16:9) ou Vertical (9:16) com chunks de midia em tempo real.
    pub fn push_chunk(&mut self, orientation: CanvasOrientation, chunk: &[u8]) {
        if !self.active || chunk.is_empty() {
            return;
        }
        let pipe_opt = match orientation {
            CanvasOrientation::Horizontal => self.horizontal_proc.as_mut(),
            CanvasOrientation::Vertical => self.vertical_proc.as_mut(),
        };
        if let Some(pipe) = pipe_opt {
            if let Some(stdin) = pipe.stdin.as_mut() {
                if stdin.write_all(chunk).is_ok() {
                    for id in &pipe.target_ids {
                        *self.bytes_sent.entry(id.clone()).or_insert(0) += chunk.len() as u64;
                    }
                } else {
                    for id in &pipe.target_ids {
                        self.errors.insert(
                            id.clone(),
                            "Conexão RTMP encerrada pelo servidor remoto.".to_string(),
                        );
                    }
                }
            }
        }
    }

    pub fn stop(&mut self) -> MultistreamStatus {
        if let Some(mut pipe) = self.horizontal_proc.take() {
            drop(pipe.stdin.take());
            let _ = pipe.child.kill();
            let _ = pipe.child.wait();
        }
        if let Some(mut pipe) = self.vertical_proc.take() {
            drop(pipe.stdin.take());
            let _ = pipe.child.kill();
            let _ = pipe.child.wait();
        }
        self.active = false;
        self.started_at = None;
        self.status()
    }

    pub fn status(&self) -> MultistreamStatus {
        let uptime = self
            .started_at
            .map(|s| s.elapsed().as_secs())
            .unwrap_or(0);

        let target_statuses = self
            .targets
            .iter()
            .map(|t| {
                let err = self.errors.get(&t.id).cloned();
                let state = if err.is_some() {
                    "error".to_string()
                } else if self.active {
                    "live".to_string()
                } else {
                    "idle".to_string()
                };
                MultistreamTargetStatus {
                    id: t.id.clone(),
                    provider: t.provider,
                    name: t.name.clone(),
                    orientation: t.orientation,
                    state,
                    uptimeSec: if self.active && err.is_none() { uptime } else { 0 },
                    bytesSent: *self.bytes_sent.get(&t.id).unwrap_or(&0),
                    lastError: err,
                }
            })
            .collect();

        MultistreamStatus {
            active: self.active,
            encoder: self.encoder.clone(),
            horizontalLive: self.horizontal_proc.is_some(),
            verticalLive: self.vertical_proc.is_some(),
            targets: target_statuses,
        }
    }
}

fn detect_best_h264_encoder() -> String {
    let Ok(output) = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
    else {
        return "libx264".to_string();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    for candidate in ["h264_nvenc", "h264_amf", "h264_qsv", "h264_vaapi"] {
        if text.contains(candidate) {
            return candidate.to_string();
        }
    }
    "libx264".to_string()
}

fn build_rtmp_endpoint(base: &str, key: &str) -> String {
    let clean_base = base.trim().trim_end_matches('/');
    let clean_key = key.trim().trim_start_matches('/');
    format!("{}/{}", clean_base, clean_key)
}

fn spawn_ffmpeg_fanout(
    encoder: &str,
    targets: &[MultistreamTarget],
    fps: u32,
    is_vertical: bool,
) -> Result<FfmpegPipeline, String> {
    let max_kbps = targets
        .iter()
        .map(|t| t.bitrateKbps)
        .max()
        .unwrap_or(if is_vertical { 4500 } else { 6000 })
        .clamp(1000, 20000);

    let gop = (fps * 2).max(60);
    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "warning",
        "-fflags",
        "+genpts+nobuffer",
        "-i",
        "pipe:0",
    ]);

    let mut target_ids = Vec::new();
    for target in targets {
        target_ids.push(target.id.clone());
        let endpoint = build_rtmp_endpoint(&target.rtmpUrl, &target.streamKey);
        let br = target.bitrateKbps.clamp(1000, max_kbps);
        cmd.args([
            "-map",
            "0:v:0",
            "-map",
            "0:a:0?",
            "-c:v",
            encoder,
            "-b:v",
            &format!("{}k", br),
            "-maxrate",
            &format!("{}k", br),
            "-bufsize",
            &format!("{}k", br * 2),
            "-g",
            &gop.to_string(),
            "-pix_fmt",
            "yuv420p",
        ]);
        if encoder == "libx264" {
            cmd.args(["-preset", "veryfast", "-tune", "zerolatency"]);
        }
        cmd.args([
            "-c:a",
            "aac",
            "-b:a",
            "160k",
            "-ar",
            "48000",
            "-f",
            "flv",
            &endpoint,
        ]);
    }

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Falha ao iniciar FFmpeg para Multistream: {}", e))?;
    let stdin = child.stdin.take();

    Ok(FfmpegPipeline {
        child,
        stdin,
        target_ids,
    })
}

pub type SharedMultistreamEngine = Arc<Mutex<MultistreamEngine>>;
