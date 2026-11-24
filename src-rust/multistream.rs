use crate::abr::{CongestionSample, DynamicBitrateController};
use crate::protocol::{
    AbrDestinationTelemetry, CanvasOrientation, Destination, DestinationStatus,
    DynamicBitrateSettings, MultistreamEngineStatus, MultistreamStatus, MultistreamTarget,
    MultistreamTargetStatus,
};
use crate::rtmp_publisher::{
    FlvDemuxer, FlvTag, PersistentRtmpSession, TimestampRebaser, FLV_TAG_SCRIPT_DATA,
    FLV_TAG_VIDEO,
};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use url::Url;

const CHUNK_QUEUE_CAP: usize = 64;
const AUDIO_QUEUE_CAP: usize = 64;
const ENCODER_STDIN_QUEUE_CAP: usize = 16;
const FLV_TAG_QUEUE_CAP: usize = 128;
const STDERR_RING_CAP: usize = 60;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const STOP_GRACE: Duration = Duration::from_millis(1500);
const RECONNECT_BACKOFF_SECS: [u64; 5] = [1, 2, 4, 8, 15];
const DEFAULT_CANVAS_H: (u32, u32) = (1920, 1080);
const DEFAULT_CANVAS_V: (u32, u32) = (1080, 1920);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Valida e monta o endpoint RTMP/RTMPS com segurança estrita:
/// - Aceita apenas esquemas `rtmp://` ou `rtmps://` (bloqueia `file://`, paths locais, etc.)
/// - Bloqueia flags FFmpeg (`-*`), whitespace, quebras de linha e credenciais vazias.
pub fn validate_and_build_rtmp_endpoint(server: &str, key: &str) -> Result<String, String> {
    let base = server.trim();
    let key_clean = key.trim();

    if base.is_empty() {
        return Err("Endereço do servidor RTMP não informado.".to_string());
    }
    if key_clean.is_empty() {
        return Err("Chave de transmissão não informada.".to_string());
    }
    if base.starts_with('-') || key_clean.starts_with('-') {
        return Err("URL ou chave inválida (prefixo '-' não permitido).".to_string());
    }
    if base.chars().any(char::is_whitespace) || key_clean.chars().any(char::is_whitespace) {
        return Err("URL ou chave RTMP não pode conter espaços ou quebras de linha.".to_string());
    }

    let parsed = Url::parse(base).map_err(|e| format!("URL RTMP inválida: {e}"))?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    if scheme != "rtmp" && scheme != "rtmps" {
        return Err(format!(
            "Protocolo não permitido ({scheme}): use apenas rtmp:// ou rtmps://"
        ));
    }
    if parsed.host_str().map(str::is_empty).unwrap_or(true) {
        return Err("URL RTMP sem host válido.".to_string());
    }

    let clean_base = base.trim_end_matches('/');
    let clean_key = key_clean.trim_start_matches('/');
    if clean_key.is_empty() || clean_key.contains(['|', '"', '\'', '\\']) {
        return Err("Chave de transmissão contém caracteres inválidos.".to_string());
    }

    let full = format!("{clean_base}/{clean_key}");
    Url::parse(&full).map_err(|e| format!("Endpoint RTMP inválido: {e}"))?;
    Ok(full)
}

/// Redaciona todas as stream keys conhecidas de mensagens de log/erro antes de expor na UI.
pub fn redact_secrets(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        let s = secret.trim();
        if s.len() >= 3 {
            out = out.replace(s, "••••");
        }
    }
    out
}

/// Resolve o binário do FFmpeg respeitando `ffmpeg_path` configurável (ou `"ffmpeg"` no PATH).
pub fn resolve_ffmpeg_bin(custom_path: Option<&str>) -> String {
    match custom_path.map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => p.to_string(),
        None => "ffmpeg".to_string(),
    }
}

fn configure_no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Resultado do probe real de FFmpeg e encoders H.264 disponíveis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub ffmpeg_found: bool,
    pub encoder: String,
}

/// Executa um comando FFmpeg com timeout estrito, sem vazar processo em caso de timeout.
async fn run_ffmpeg_probe(bin: &str, args: &[&str], timeout: Duration) -> Result<bool, String> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_no_window(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("FFmpeg não encontrado".to_string());
        }
        Err(e) => return Err(e.to_string()),
    };

    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => Ok(status.success()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Ok(false)
        }
    }
}

/// Testa o FFmpeg e cada encoder H.264 codificando 1 frame `nullsrc` real com os flags
/// equivalentes aos da live. Nunca cacheia `ffmpeg_found = false` para permitir instalação em runtime.
pub async fn probe_encoder(ffmpeg_path: Option<&str>) -> ProbeResult {
    let bin = resolve_ffmpeg_bin(ffmpeg_path);
    if !Path::new(&bin).exists() && bin != "ffmpeg" {
        return ProbeResult {
            ffmpeg_found: false,
            encoder: "nenhum".to_string(),
        };
    }

    match run_ffmpeg_probe(&bin, &["-hide_banner", "-version"], PROBE_TIMEOUT).await {
        Ok(true) => {}
        _ => {
            return ProbeResult {
                ffmpeg_found: false,
                encoder: "nenhum".to_string(),
            };
        }
    }

    #[cfg(target_os = "linux")]
    let candidates: &[(&str, &[&str])] = &[
        (
            "h264_nvenc",
            &[
                "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                "color=c=black:s=320x240:r=30", "-frames:v", "1", "-c:v", "h264_nvenc",
                "-preset", "p5", "-tune", "ll", "-rc", "cbr", "-f", "null", "-",
            ],
        ),
        (
            "h264_qsv",
            &[
                "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                "color=c=black:s=320x240:r=30", "-frames:v", "1", "-c:v", "h264_qsv",
                "-preset", "veryfast", "-f", "null", "-",
            ],
        ),
        (
            "h264_vaapi",
            &[
                "-hide_banner", "-loglevel", "error", "-vaapi_device", "/dev/dri/renderD128",
                "-f", "lavfi", "-i", "color=c=black:s=320x240:r=30", "-vf",
                "format=nv12,hwupload", "-frames:v", "1", "-c:v", "h264_vaapi", "-f", "null", "-",
            ],
        ),
    ];

    #[cfg(not(target_os = "linux"))]
    let candidates: &[(&str, &[&str])] = &[
        (
            "h264_nvenc",
            &[
                "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                "color=c=black:s=320x240:r=30", "-frames:v", "1", "-c:v", "h264_nvenc",
                "-preset", "p5", "-tune", "ll", "-rc", "cbr", "-f", "null", "-",
            ],
        ),
        (
            "h264_amf",
            &[
                "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                "color=c=black:s=320x240:r=30", "-frames:v", "1", "-c:v", "h264_amf",
                "-quality", "balanced", "-rc", "cbr", "-f", "null", "-",
            ],
        ),
        (
            "h264_qsv",
            &[
                "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                "color=c=black:s=320x240:r=30", "-frames:v", "1", "-c:v", "h264_qsv",
                "-preset", "veryfast", "-f", "null", "-",
            ],
        ),
    ];

    for (enc, args) in candidates {
        if matches!(run_ffmpeg_probe(&bin, args, PROBE_TIMEOUT).await, Ok(true)) {
            return ProbeResult {
                ffmpeg_found: true,
                encoder: (*enc).to_string(),
            };
        }
    }

    ProbeResult {
        ffmpeg_found: true,
        encoder: "libx264".to_string(),
    }
}

/// Escapa caracteres especiais da sintaxe de muxer `tee` do FFmpeg (`\`, `'`, `:`, `[`, `]`, `|`).
fn escape_tee_target(url: &str) -> String {
    let mut out = String::with_capacity(url.len() + 8);
    for ch in url.chars() {
        if matches!(ch, '\\' | '\'' | ':' | '[' | ']' | '|') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn append_video_and_audio_encoder_args(
    args: &mut Vec<String>,
    encoder: &str,
    fps_clamped: u32,
    kbps: u32,
) {
    let gop = (fps_clamped * 2).to_string();
    let rate = [
        "-b:v".to_string(),
        format!("{kbps}k"),
        "-maxrate".to_string(),
        format!("{kbps}k"),
        "-bufsize".to_string(),
        format!("{}k", kbps * 2),
    ];

    match encoder {
        "h264_nvenc" => {
            args.extend([
                "-c:v".to_string(),
                "h264_nvenc".to_string(),
                "-preset".to_string(),
                "p5".to_string(),
                "-tune".to_string(),
                "ll".to_string(),
                "-rc".to_string(),
                "cbr".to_string(),
            ]);
            args.extend(rate);
            args.extend([
                "-bf".to_string(),
                "2".to_string(),
                "-pix_fmt".to_string(),
                "yuv420p".to_string(),
            ]);
        }
        "h264_amf" => {
            args.extend([
                "-c:v".to_string(),
                "h264_amf".to_string(),
                "-quality".to_string(),
                "balanced".to_string(),
                "-rc".to_string(),
                "cbr".to_string(),
            ]);
            args.extend(rate);
            args.extend(["-pix_fmt".to_string(), "yuv420p".to_string()]);
        }
        "h264_qsv" => {
            args.extend([
                "-c:v".to_string(),
                "h264_qsv".to_string(),
                "-preset".to_string(),
                "veryfast".to_string(),
            ]);
            args.extend(rate);
            args.extend(["-pix_fmt".to_string(), "yuv420p".to_string()]);
        }
        "h264_vaapi" => {
            args.extend([
                "-vf".to_string(),
                "format=nv12,hwupload".to_string(),
                "-c:v".to_string(),
                "h264_vaapi".to_string(),
            ]);
            args.extend(rate);
        }
        _ => {
            args.extend([
                "-c:v".to_string(),
                "libx264".to_string(),
                "-preset".to_string(),
                "veryfast".to_string(),
                "-tune".to_string(),
                "zerolatency".to_string(),
            ]);
            args.extend(rate);
            args.extend([
                "-x264-params".to_string(),
                "nal-hrd=cbr".to_string(),
                "-pix_fmt".to_string(),
                "yuv420p".to_string(),
            ]);
        }
    }

    args.extend([
        "-r".to_string(),
        fps_clamped.to_string(),
        "-g".to_string(),
        gop.clone(),
        "-keyint_min".to_string(),
        gop,
        "-c:a".to_string(),
        "aac".to_string(),
        "-b:a".to_string(),
        "160k".to_string(),
        "-ar".to_string(),
        "48000".to_string(),
        "-ac".to_string(),
        "2".to_string(),
    ]);
}

fn append_rtmp_or_flv_output_args(args: &mut Vec<String>, flv_stdout: bool, endpoints: &[String]) {
    if flv_stdout {
        args.extend([
            "-flush_packets".to_string(),
            "1".to_string(),
            "-flvflags".to_string(),
            "no_duration_filesize".to_string(),
            "-f".to_string(),
            "flv".to_string(),
            "pipe:1".to_string(),
        ]);
    } else if endpoints.len() <= 1 {
        args.extend([
            "-f".to_string(),
            "flv".to_string(),
            endpoints.first().cloned().unwrap_or_default(),
        ]);
    } else {
        // Muxer `tee` exige flags de stream global antes do muxer.
        args.extend(["-flags".to_string(), "+global_header".to_string()]);
        let tee_spec = endpoints
            .iter()
            .map(|ep| format!("[f=flv:onfail=ignore]{}", escape_tee_target(ep)))
            .collect::<Vec<_>>()
            .join("|");
        args.extend(["-f".to_string(), "tee".to_string(), tee_spec]);
    }
}

/// Monta os argumentos FFmpeg para um grupo de destinos com mesma orientação e mesmo bitrate.
/// Quando há 2+ destinos no mesmo grupo, usa o muxer `tee` para evitar codificação H.264/AAC duplicada.
pub fn build_ffmpeg_args(
    encoder: &str,
    endpoints: &[String],
    fps: u32,
    bitrate_kbps: u32,
) -> Vec<String> {
    let kbps = bitrate_kbps.clamp(500, 25_000);
    let fps_clamped = fps.clamp(15, 60);

    let mut args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "warning".to_string(),
        "-progress".to_string(),
        "pipe:2".to_string(),
        "-probesize".to_string(),
        "2M".to_string(),
        "-analyzeduration".to_string(),
        "1000000".to_string(),
        "-fflags".to_string(),
        "+genpts".to_string(),
    ];

    if encoder == "h264_vaapi" {
        args.extend(["-vaapi_device".to_string(), "/dev/dri/renderD128".to_string()]);
    }

    args.extend([
        "-i".to_string(),
        "pipe:0".to_string(),
        "-map".to_string(),
        "0:v:0".to_string(),
        "-map".to_string(),
        "0:a:0?".to_string(),
    ]);

    append_video_and_audio_encoder_args(&mut args, encoder, fps_clamped, kbps);
    append_rtmp_or_flv_output_args(&mut args, false, endpoints);
    args
}

/// Monta argumentos FFmpeg para o modo **Zero-Disconnect ABR**:
/// recebe WebM/Matroska via `pipe:0` e emite tags FLV contínuos em `pipe:1` (`-f flv pipe:1`)
/// para publicação pela sessão RTMP/RTMPS persistente em Rust.
pub fn build_ffmpeg_flv_pipe_args(encoder: &str, fps: u32, bitrate_kbps: u32) -> Vec<String> {
    let kbps = bitrate_kbps.clamp(500, 25_000);
    let fps_clamped = fps.clamp(15, 60);

    let mut args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "warning".to_string(),
        "-progress".to_string(),
        "pipe:2".to_string(),
        "-probesize".to_string(),
        "2M".to_string(),
        "-analyzeduration".to_string(),
        "1000000".to_string(),
        "-fflags".to_string(),
        "+genpts".to_string(),
    ];

    if encoder == "h264_vaapi" {
        args.extend(["-vaapi_device".to_string(), "/dev/dri/renderD128".to_string()]);
    }

    args.extend([
        "-i".to_string(),
        "pipe:0".to_string(),
        "-map".to_string(),
        "0:v:0".to_string(),
        "-map".to_string(),
        "0:a:0?".to_string(),
    ]);

    append_video_and_audio_encoder_args(&mut args, encoder, fps_clamped, kbps);
    append_rtmp_or_flv_output_args(&mut args, true, &[]);
    args
}

/// Monta argumentos FFmpeg para o modo **Nativo (Raw BGRA + PCM f32le 48kHz estéreo)**:
/// alimenta frames crus do compositor GPU/CPU diretamente em `pipe:0` e áudio PCM via socket
/// loopback/pipe (`audio_input`), dispensando completamente `MediaRecorder` do WebView.
#[allow(clippy::too_many_arguments)]
pub fn build_ffmpeg_native_args(
    encoder: &str,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_kbps: u32,
    audio_input: Option<&str>,
    flv_stdout: bool,
    endpoints: &[String],
) -> Vec<String> {
    let w = width.clamp(160, 7680) & !1;
    let h = height.clamp(160, 4320) & !1;
    let fps_clamped = fps.clamp(15, 60);
    let kbps = bitrate_kbps.clamp(500, 25_000);

    let mut args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "warning".to_string(),
        "-progress".to_string(),
        "pipe:2".to_string(),
        "-fflags".to_string(),
        "+genpts".to_string(),
    ];

    if encoder == "h264_vaapi" {
        args.extend(["-vaapi_device".to_string(), "/dev/dri/renderD128".to_string()]);
    }

    // Entrada 0: vídeo BGRA cru vindo do compositor Rust em pipe:0
    args.extend([
        "-thread_queue_size".to_string(),
        "64".to_string(),
        "-f".to_string(),
        "rawvideo".to_string(),
        "-pix_fmt".to_string(),
        "bgra".to_string(),
        "-video_size".to_string(),
        format!("{w}x{h}"),
        "-framerate".to_string(),
        fps_clamped.to_string(),
        "-i".to_string(),
        "pipe:0".to_string(),
    ]);

    // Entrada 1: áudio PCM f32le 48kHz estéreo (ou anullsrc se não houver fonte PCM externa)
    if let Some(src) = audio_input.map(str::trim).filter(|s| !s.is_empty()) {
        args.extend([
            "-thread_queue_size".to_string(),
            "64".to_string(),
            "-f".to_string(),
            "f32le".to_string(),
            "-ar".to_string(),
            "48000".to_string(),
            "-ac".to_string(),
            "2".to_string(),
            "-i".to_string(),
            src.to_string(),
        ]);
    } else {
        args.extend([
            "-f".to_string(),
            "lavfi".to_string(),
            "-i".to_string(),
            "anullsrc=channel_layout=stereo:sample_rate=48000".to_string(),
        ]);
    }

    args.extend([
        "-map".to_string(),
        "0:v:0".to_string(),
        "-map".to_string(),
        "1:a:0".to_string(),
    ]);

    append_video_and_audio_encoder_args(&mut args, encoder, fps_clamped, kbps);
    append_rtmp_or_flv_output_args(&mut args, flv_stdout, endpoints);
    args
}

/// Métricas extraídas das linhas `-progress pipe:2` do FFmpeg.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FfmpegProgress {
    pub fps: f32,
    pub bitrate_kbps: u32,
    pub dropped_frames: u64,
    pub total_size_bytes: u64,
}

/// Faz o parsing incremental de uma linha vinda do `-progress pipe:2` (ou `stderr`).
/// Retorna `true` se a linha pertencia ao bloco `-progress` (não devendo poluir o log de erros).
pub fn parse_progress_line(line: &str, prog: &mut FfmpegProgress) -> bool {
    let trimmed = line.trim();
    let Some((key, val)) = trimmed.split_once('=') else {
        return false;
    };
    let k = key.trim();
    let v = val.trim();

    match k {
        "frame" | "stream_0_0_q" | "out_time_us" | "out_time_ms" | "out_time" | "speed" => true,
        "fps" => {
            if let Ok(parsed) = v.parse::<f32>() {
                prog.fps = parsed.max(0.0);
            }
            true
        }
        "bitrate" => {
            if let Some(num) = v.strip_suffix("kbits/s") {
                if let Ok(kbps) = num.trim().parse::<f32>() {
                    prog.bitrate_kbps = kbps.max(0.0).round() as u32;
                }
            }
            true
        }
        "drop_frames" => {
            if let Ok(d) = v.parse::<u64>() {
                prog.dropped_frames = d;
            }
            true
        }
        "total_size" => {
            if let Ok(sz) = v.parse::<u64>() {
                prog.total_size_bytes = sz;
            }
            true
        }
        "progress" => true,
        _ => false,
    }
}

fn explain_ffmpeg_error(log: &VecDeque<String>, names: &str, rtmp_urls: &[String]) -> String {
    let text: String = log.iter().cloned().collect::<Vec<_>>().join("\n");
    let lower = text.to_ascii_lowercase();
    if lower.contains("input/output error")
        || lower.contains("connection reset")
        || lower.contains("broken pipe")
        || lower.contains("end of file")
        || lower.contains("-10053")
        || lower.contains("-10054")
        || lower.contains("error closing file")
        || lower.contains("av_interleaved_write_frame")
    {
        return format!(
            "{names} fechou a conexão. Quase sempre é chave errada ou expirada: cole a chave de novo."
        );
    }
    if lower.contains("connection refused")
        || lower.contains("connection timed out")
        || lower.contains("no route")
        || lower.contains("failed to resolve")
        || lower.contains("getaddrinfo")
    {
        return format!(
            "Não deu para alcançar o servidor de {names}. Confira a internet e o endereço do servidor."
        );
    }
    if (lower.contains("protocol not found") || lower.contains("rtmps"))
        && rtmp_urls.iter().any(|u| u.starts_with("rtmps://"))
    {
        return "Este FFmpeg não tem suporte a RTMPS. Instale uma versão \"full\" (ex.: gyan.dev)."
            .to_string();
    }
    log.iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| format!("FFmpeg: {}", l.trim()))
        .unwrap_or_else(|| format!("A transmissão para {names} parou."))
}

/// Modo de alimentação de mídia do pipeline `MultistreamEngine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MultistreamInputMode {
    /// Fluxo WebM/Matroska muxado pelo MediaRecorder (compatibilidade padrão).
    #[default]
    WebmChunks,
    /// Frames BGRA crus do compositor Rust + PCM f32le 48kHz estéreo do AudioRuntime nativo.
    NativeRaw,
}

/// Estado interno compartilhado entre a task supervisora do pipeline e `MultistreamEngine`.
#[derive(Debug, Clone)]
struct GroupRuntimeState {
    dest_ids: Vec<String>,
    orientation: CanvasOrientation,
    state: String, // "connecting" | "live" | "reconnecting" | "error" | "idle"
    started_at: Option<Instant>,
    bytes_sent: u64,
    dropped_input_chunks: u64,
    progress: FfmpegProgress,
    abr: AbrDestinationTelemetry,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct ResolvedTarget {
    id: String,
    name: String,
    provider: crate::protocol::StreamProvider,
    orientation: CanvasOrientation,
    bitrate_kbps: u32,
    endpoint: String,
    secret: String,
}

struct PipelineGroup {
    orientation: CanvasOrientation,
    input_mode: MultistreamInputMode,
    canvas_size: (u32, u32),
    tx: mpsc::Sender<Vec<u8>>,
    audio_tx: Option<mpsc::Sender<Vec<u8>>>,
    abr_settings_tx: watch::Sender<DynamicBitrateSettings>,
    stop_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
    runtime: Arc<Mutex<GroupRuntimeState>>,
}

/// Motor de Multistreaming + Dual Output (16:9 + 9:16) em Rust.
///
/// Características:
/// - Agrupa destinos da mesma orientação + bitrate usando muxer `tee` ou sessão RTMP persistente.
/// - Bitrate Dinâmico (ABR AIMD) com hot-swap do encoder local e Zero-Disconnect via `PersistentRtmpSession`.
/// - Suporte a alimentação nativa (Raw BGRA + PCM f32le 48kHz estéreo) além de chunks WebM.
/// - Backpressure real via canais limitados sem travar `Mutex` durante I/O.
/// - Reconexão automática com backoff exponencial por grupo em quedas transitórias.
/// - Leitura de `-progress pipe:2` e `stderr` em ring buffer com redação de `streamKey`.
/// - `impl Drop` garantindo encerramento limpo de processos filhos sem órfãos.
pub struct MultistreamEngine {
    encoder: String,
    ffmpeg_found: bool,
    ffmpeg_path: Option<String>,
    dynamic_bitrate: DynamicBitrateSettings,
    input_mode: MultistreamInputMode,
    horizontal_size: (u32, u32),
    vertical_size: (u32, u32),
    active: bool,
    groups: Vec<PipelineGroup>,
    targets: Vec<ResolvedTarget>,
    failed: HashMap<String, String>,
    init_segments: [Option<Vec<u8>>; 2],
}

impl MultistreamEngine {
    pub fn new() -> Self {
        Self {
            encoder: "libx264".to_string(),
            ffmpeg_found: true,
            ffmpeg_path: None,
            dynamic_bitrate: DynamicBitrateSettings::default(),
            input_mode: MultistreamInputMode::WebmChunks,
            horizontal_size: DEFAULT_CANVAS_H,
            vertical_size: DEFAULT_CANVAS_V,
            active: false,
            groups: Vec::new(),
            targets: Vec::new(),
            failed: HashMap::new(),
            init_segments: [None, None],
        }
    }

    pub fn detected_encoder(&self) -> &str {
        &self.encoder
    }

    pub fn set_ffmpeg_path(&mut self, path: Option<String>) {
        self.ffmpeg_path = path.filter(|s| !s.trim().is_empty());
    }

    /// Configura a política de Bitrate Dinâmico (ABR) e propaga alterações ao vivo aos grupos ativos.
    pub fn set_dynamic_bitrate(&mut self, mut settings: DynamicBitrateSettings) {
        settings.normalize();
        self.dynamic_bitrate = settings.clone();
        for group in &self.groups {
            let _ = group.abr_settings_tx.send(settings.clone());
        }
    }

    pub fn dynamic_bitrate(&self) -> &DynamicBitrateSettings {
        &self.dynamic_bitrate
    }

    /// Ativa ou desativa o modo de pipe nativo (Raw BGRA + PCM f32le 48kHz estéreo).
    pub fn set_native_mode(&mut self, enabled: bool) {
        self.input_mode = if enabled {
            MultistreamInputMode::NativeRaw
        } else {
            MultistreamInputMode::WebmChunks
        };
    }

    pub fn is_native_mode(&self) -> bool {
        self.input_mode == MultistreamInputMode::NativeRaw
    }

    /// Define dimensões de canvas usadas no modo nativo (padrão: 1920x1080 e 1080x1920).
    pub fn set_canvas_dimensions(&mut self, horizontal: (u32, u32), vertical: (u32, u32)) {
        self.horizontal_size = (
            horizontal.0.clamp(160, 7680) & !1,
            horizontal.1.clamp(160, 4320) & !1,
        );
        self.vertical_size = (
            vertical.0.clamp(160, 7680) & !1,
            vertical.1.clamp(160, 4320) & !1,
        );
    }

    /// Retorna telemetria ABR por destino ativo.
    pub fn abr_telemetry(&self) -> Vec<AbrDestinationTelemetry> {
        let mut out = Vec::new();
        for group in &self.groups {
            if let Ok(rt) = group.runtime.lock() {
                for id in &rt.dest_ids {
                    let mut item = rt.abr.clone();
                    item.id = id.clone();
                    out.push(item);
                }
            }
        }
        out
    }

    /// Resumo agregado de ABR para `TelemetryStats`: `(abr_active, current_target_kbps, max_congestion_level)`.
    pub fn abr_summary(&self) -> (bool, u32, f32) {
        if !self.active || self.groups.is_empty() {
            return (false, 0, 0.0);
        }
        let mut target_kbps: u32 = 0;
        let mut max_congestion: f32 = 0.0;
        let mut any_running = false;
        for group in &self.groups {
            if let Ok(rt) = group.runtime.lock() {
                if matches!(rt.state.as_str(), "connecting" | "live" | "reconnecting") {
                    any_running = true;
                    target_kbps = target_kbps.saturating_add(rt.abr.current_bitrate_kbps);
                    max_congestion = max_congestion.max(rt.abr.congestion_level);
                }
            }
        }
        (any_running && self.dynamic_bitrate.enabled, target_kbps, max_congestion)
    }

    /// Executa probe real do FFmpeg e encoders HW. Seguro para chamar a cada `start`.
    pub async fn refresh_probe(&mut self) -> ProbeResult {
        let res = probe_encoder(self.ffmpeg_path.as_deref()).await;
        self.ffmpeg_found = res.ffmpeg_found;
        self.encoder = res.encoder.clone();
        res
    }

    /// Inicia transmissão usando a lista moderna de `Destination` + segredos do cofre.
    pub async fn start_destinations(
        &mut self,
        destinations: &[(Destination, Option<String>)],
        fps: u32,
        ffmpeg_path: Option<String>,
    ) -> Result<MultistreamEngineStatus, String> {
        let mut converted = Vec::new();
        let mut pre_failed = HashMap::new();

        for (dest, key_opt) in destinations {
            if !dest.enabled {
                continue;
            }
            let key = key_opt.as_deref().unwrap_or("").trim();
            if key.is_empty() {
                pre_failed.insert(
                    dest.id.clone(),
                    format!("{} está sem chave de transmissão.", dest.name),
                );
                continue;
            }
            converted.push(MultistreamTarget {
                id: dest.id.clone(),
                provider: dest.platform,
                name: dest.name.clone(),
                enabled: true,
                rtmpUrl: dest.server.clone(),
                streamKey: key.to_string(),
                orientation: dest.orientation,
                bitrateKbps: dest.bitrate_kbps,
            });
        }

        let res = self
            .start_internal(converted, pre_failed, fps, ffmpeg_path)
            .await;
        if let Err(ref msg) = res {
            if !self.ffmpeg_found {
                return Err(msg.clone());
            }
        }
        res
    }

    /// Inicia transmissão no formato `MultistreamTarget` (usado pelo RPC atual).
    pub async fn start_async(
        &mut self,
        targets: Vec<MultistreamTarget>,
        fps: u32,
        ffmpeg_path: Option<String>,
    ) -> Result<MultistreamStatus, String> {
        let _ = self
            .start_internal(targets, HashMap::new(), fps, ffmpeg_path)
            .await?;
        Ok(self.status())
    }

    async fn start_internal(
        &mut self,
        targets: Vec<MultistreamTarget>,
        mut pre_failed: HashMap<String, String>,
        fps: u32,
        ffmpeg_path: Option<String>,
    ) -> Result<MultistreamEngineStatus, String> {
        self.stop_async().await;
        if let Some(p) = ffmpeg_path {
            self.set_ffmpeg_path(Some(p));
        }

        let probe = self.refresh_probe().await;
        if !probe.ffmpeg_found {
            return Err(
                "FFmpeg não encontrado. Instale o FFmpeg ou configure o caminho nas Configurações."
                    .to_string(),
            );
        }

        let enabled: Vec<MultistreamTarget> = targets.into_iter().filter(|t| t.enabled).collect();
        if enabled.is_empty() && pre_failed.is_empty() {
            return Err(
                "Nenhum destino ligado. Conecte a Twitch (ou outra plataforma) em Destinos."
                    .to_string(),
            );
        }

        let mut resolved = Vec::new();
        for t in enabled {
            match validate_and_build_rtmp_endpoint(&t.rtmpUrl, &t.streamKey) {
                Ok(endpoint) => {
                    resolved.push(ResolvedTarget {
                        id: t.id,
                        name: t.name,
                        provider: t.provider,
                        orientation: t.orientation,
                        bitrate_kbps: t.bitrateKbps.clamp(1000, 20_000),
                        endpoint,
                        secret: t.streamKey.trim().to_string(),
                    });
                }
                Err(err) => {
                    pre_failed.insert(t.id, format!("{}: {err}", t.name));
                }
            }
        }

        self.failed = pre_failed;
        self.targets = resolved.clone();
        self.init_segments = [None, None];

        if resolved.is_empty() {
            self.active = false;
            let first_err = self
                .failed
                .values()
                .next()
                .cloned()
                .unwrap_or_else(|| "Nada válido para transmitir.".to_string());
            return Err(first_err);
        }

        // Agrupa por (orientação, bitrate_kbps) para usar `tee` sem re-encode duplicado.
        let mut grouped: BTreeMap<(u8, u32), Vec<ResolvedTarget>> = BTreeMap::new();
        for t in resolved {
            let ori_key = match t.orientation {
                CanvasOrientation::Horizontal => 0u8,
                CanvasOrientation::Vertical => 1u8,
            };
            grouped
                .entry((ori_key, t.bitrate_kbps))
                .or_default()
                .push(t);
        }

        let bin = resolve_ffmpeg_bin(self.ffmpeg_path.as_deref());
        let encoder = self.encoder.clone();

        for ((ori_key, bitrate_kbps), group_targets) in grouped {
            let orientation = if ori_key == 0 {
                CanvasOrientation::Horizontal
            } else {
                CanvasOrientation::Vertical
            };
            let canvas_size = match orientation {
                CanvasOrientation::Horizontal => self.horizontal_size,
                CanvasOrientation::Vertical => self.vertical_size,
            };
            let dest_ids: Vec<String> = group_targets.iter().map(|t| t.id.clone()).collect();
            let initial_abr = AbrDestinationTelemetry {
                id: dest_ids.first().cloned().unwrap_or_default(),
                configured_bitrate_kbps: bitrate_kbps,
                current_bitrate_kbps: bitrate_kbps,
                observed_bitrate_kbps: 0,
                congestion_level: 0.0,
                write_latency_ms: 0.0,
                adjustments: 0,
                state: "nominal".to_string(),
            };

            let runtime = Arc::new(Mutex::new(GroupRuntimeState {
                dest_ids,
                orientation,
                state: "connecting".to_string(),
                started_at: Some(Instant::now()),
                bytes_sent: 0,
                dropped_input_chunks: 0,
                progress: FfmpegProgress::default(),
                abr: initial_abr,
                error: None,
            }));

            let (tx, rx) = mpsc::channel::<Vec<u8>>(CHUNK_QUEUE_CAP);
            let (audio_tx, audio_rx) = if self.input_mode == MultistreamInputMode::NativeRaw {
                let (atx, arx) = mpsc::channel::<Vec<u8>>(AUDIO_QUEUE_CAP);
                (Some(atx), Some(arx))
            } else {
                (None, None)
            };
            let (abr_settings_tx, abr_settings_rx) =
                watch::channel(self.dynamic_bitrate.clone());
            let (stop_tx, stop_rx) = watch::channel(false);

            let task = tokio::spawn(run_pipeline_supervisor(
                bin.clone(),
                encoder.clone(),
                fps,
                bitrate_kbps,
                canvas_size,
                self.input_mode,
                group_targets,
                runtime.clone(),
                rx,
                audio_rx,
                abr_settings_rx,
                stop_rx,
            ));

            self.groups.push(PipelineGroup {
                orientation,
                input_mode: self.input_mode,
                canvas_size,
                tx,
                audio_tx,
                abr_settings_tx,
                stop_tx,
                task,
                runtime,
            });
        }

        self.active = !self.groups.is_empty();
        Ok(self.engine_status())
    }

    /// Enfileira um chunk WebM/Matroska de forma não-bloqueante com backpressure real.
    /// Se a fila do grupo estiver cheia (rede travada), descarta o chunk em vez de bloquear o lock ou causar OOM.
    pub fn push_chunk(&mut self, orientation: CanvasOrientation, chunk: &[u8]) {
        if !self.active || chunk.is_empty() {
            return;
        }
        // Preserva o primeiro segmento (EBML init segment) para permitir reconexão transparente.
        let slot = match orientation {
            CanvasOrientation::Horizontal => 0,
            CanvasOrientation::Vertical => 1,
        };
        if self.init_segments[slot].is_none() {
            self.init_segments[slot] = Some(chunk.to_vec());
        }

        for group in &self.groups {
            if group.orientation != orientation || group.input_mode != MultistreamInputMode::WebmChunks
            {
                continue;
            }
            let payload = chunk.to_vec();
            if let Err(err) = group.tx.try_send(payload) {
                match err {
                    mpsc::error::TrySendError::Full(_) => {
                        if let Ok(mut rt) = group.runtime.lock() {
                            rt.dropped_input_chunks = rt.dropped_input_chunks.saturating_add(1);
                        }
                        log::warn!(
                            "[multistream] fila cheia ({:?}): chunk de {} bytes descartado por backpressure",
                            orientation,
                            chunk.len()
                        );
                    }
                    mpsc::error::TrySendError::Closed(_) => {}
                }
            }
        }
    }

    /// Enfileira um frame BGRA cru do compositor GPU/CPU Rust diretamente para os pipelines nativos.
    pub fn push_native_frame(&mut self, orientation: CanvasOrientation, bgra: &[u8]) {
        if !self.active || bgra.is_empty() {
            return;
        }
        for group in &self.groups {
            if group.orientation != orientation || group.input_mode != MultistreamInputMode::NativeRaw
            {
                continue;
            }
            let expected = (group.canvas_size.0 as usize)
                .saturating_mul(group.canvas_size.1 as usize)
                .saturating_mul(4);
            if expected > 0 && bgra.len() != expected {
                log::warn!(
                    "[multistream] frame nativo BGRA ignorado ({:?}): recebido {} bytes, esperado {} ({}x{})",
                    orientation,
                    bgra.len(),
                    expected,
                    group.canvas_size.0,
                    group.canvas_size.1
                );
                continue;
            }
            if let Err(err) = group.tx.try_send(bgra.to_vec()) {
                if matches!(err, mpsc::error::TrySendError::Full(_)) {
                    if let Ok(mut rt) = group.runtime.lock() {
                        rt.dropped_input_chunks = rt.dropped_input_chunks.saturating_add(1);
                    }
                }
            }
        }
    }

    /// Enfileira amostras PCM f32 (48 kHz estéreo intercalado) do AudioRuntime nativo para todos
    /// os grupos ativos em modo nativo (Horizontal + Vertical sincronizados).
    pub fn push_native_audio_pcm(&mut self, pcm_f32: &[f32]) {
        if !self.active || pcm_f32.is_empty() {
            return;
        }
        let mut bytes = Vec::with_capacity(pcm_f32.len() * 4);
        for &sample in pcm_f32 {
            let clamped = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            bytes.extend_from_slice(&clamped.to_le_bytes());
        }

        for group in &self.groups {
            if group.input_mode != MultistreamInputMode::NativeRaw {
                continue;
            }
            if let Some(ref audio_tx) = group.audio_tx {
                let _ = audio_tx.try_send(bytes.clone());
            }
        }
    }

    pub fn init_segment(&self, orientation: CanvasOrientation) -> Option<&[u8]> {
        let slot = match orientation {
            CanvasOrientation::Horizontal => 0,
            CanvasOrientation::Vertical => 1,
        };
        self.init_segments[slot].as_deref()
    }

    /// Encerra todos os pipelines de forma assíncrona, aguardando fechamento gracioso antes de dar `kill`.
    pub async fn stop_async(&mut self) -> MultistreamStatus {
        let groups = std::mem::take(&mut self.groups);
        for g in &groups {
            let _ = g.stop_tx.send(true);
        }
        for g in groups {
            drop(g.tx);
            drop(g.audio_tx);
            let _ = tokio::time::timeout(STOP_GRACE + Duration::from_millis(500), g.task).await;
        }
        self.active = false;
        self.init_segments = [None, None];
        self.status()
    }

    /// Versão síncrona de `stop` (para compatibilidade com callers síncronos).
    pub fn stop(&mut self) -> MultistreamStatus {
        let groups = std::mem::take(&mut self.groups);
        for g in groups {
            let _ = g.stop_tx.send(true);
            drop(g.tx);
            g.task.abort();
        }
        self.active = false;
        self.init_segments = [None, None];
        self.status()
    }

    /// Retorna o status rico (protocolo v3: `MultistreamEngineStatus`) incluindo `bitrate_kbps` e `ffmpeg_found`.
    pub fn engine_status(&self) -> MultistreamEngineStatus {
        let mut by_dest: HashMap<String, DestinationStatus> = HashMap::new();
        let mut horiz_live = false;
        let mut vert_live = false;
        let mut any_running = false;

        for group in &self.groups {
            if let Ok(rt) = group.runtime.lock() {
                let running = matches!(
                    rt.state.as_str(),
                    "connecting" | "live" | "reconnecting"
                );
                if running {
                    any_running = true;
                    match rt.orientation {
                        CanvasOrientation::Horizontal => horiz_live = true,
                        CanvasOrientation::Vertical => vert_live = true,
                    }
                }
                let uptime = if running {
                    rt.started_at.map(|s| s.elapsed().as_secs()).unwrap_or(0)
                } else {
                    0
                };
                for id in &rt.dest_ids {
                    by_dest.insert(
                        id.clone(),
                        DestinationStatus {
                            id: id.clone(),
                            state: rt.state.clone(),
                            uptime_sec: uptime,
                            bytes_sent: rt.bytes_sent,
                            bitrate_kbps: rt.progress.bitrate_kbps,
                            error: rt.error.clone(),
                        },
                    );
                }
            }
        }

        for (id, err) in &self.failed {
            by_dest.entry(id.clone()).or_insert_with(|| DestinationStatus {
                id: id.clone(),
                state: "error".to_string(),
                uptime_sec: 0,
                bytes_sent: 0,
                bitrate_kbps: 0,
                error: Some(err.clone()),
            });
        }

        MultistreamEngineStatus {
            active: self.active && any_running,
            encoder: self.encoder.clone(),
            ffmpeg_found: self.ffmpeg_found,
            horizontal_live: horiz_live,
            vertical_live: vert_live,
            destinations: by_dest.into_values().collect(),
        }
    }

    /// Telemetria agregada para o compositor/UI (FPS/bitrate/drops).
    pub fn telemetry_snapshot(&self) -> FfmpegProgress {
        let mut total = FfmpegProgress::default();
        for g in &self.groups {
            if let Ok(rt) = g.runtime.lock() {
                total.fps = total.fps.max(rt.progress.fps);
                total.bitrate_kbps = total.bitrate_kbps.saturating_add(rt.progress.bitrate_kbps);
                total.dropped_frames = total
                    .dropped_frames
                    .saturating_add(rt.progress.dropped_frames);
                total.total_size_bytes = total.total_size_bytes.saturating_add(rt.bytes_sent);
            }
        }
        total
    }

    /// Status compatível com `MultistreamStatus` legado.
    pub fn status(&self) -> MultistreamStatus {
        let eng = self.engine_status();
        let by_id: HashMap<&str, &DestinationStatus> = eng
            .destinations
            .iter()
            .map(|d| (d.id.as_str(), d))
            .collect();

        let mut targets = Vec::with_capacity(self.targets.len() + self.failed.len());
        for t in &self.targets {
            let ds = by_id.get(t.id.as_str());
            targets.push(MultistreamTargetStatus {
                id: t.id.clone(),
                provider: t.provider,
                name: t.name.clone(),
                orientation: t.orientation,
                state: ds.map(|d| d.state.clone()).unwrap_or_else(|| "idle".to_string()),
                uptimeSec: ds.map(|d| d.uptime_sec).unwrap_or(0),
                bytesSent: ds.map(|d| d.bytes_sent).unwrap_or(0),
                lastError: ds.and_then(|d| d.error.clone()),
            });
        }

        MultistreamStatus {
            active: eng.active,
            encoder: eng.encoder,
            horizontalLive: eng.horizontal_live,
            verticalLive: eng.vertical_live,
            targets,
        }
    }
}

impl Default for MultistreamEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MultistreamEngine {
    fn drop(&mut self) {
        let groups = std::mem::take(&mut self.groups);
        for g in groups {
            let _ = g.stop_tx.send(true);
            g.task.abort();
        }
    }
}

async fn terminate_child(child: &mut Child) {
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.shutdown().await;
    }
    match tokio::time::timeout(STOP_GRACE, child.wait()).await {
        Ok(_) => {}
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

struct SupervisorConfig {
    bin: String,
    encoder: String,
    fps: u32,
    canvas_size: (u32, u32),
    input_mode: MultistreamInputMode,
    targets: Vec<ResolvedTarget>,
    endpoints: Vec<String>,
    secrets: Vec<String>,
    names: String,
    primary_dest_id: String,
}

enum PersistentCycleOutcome {
    Stop,
    HotSwap(u32),
    TransportError(String),
    EncoderFailed(String),
}

enum PersistentGroupOutcome {
    Stopped,
    FallbackClassic(String),
    Reconnect(String),
}

enum ClassicCycleOutcome {
    Stop,
    HotSwap(u32),
    Reconnect(String),
    Fatal(String),
}

async fn recv_opt_audio(opt: &mut Option<mpsc::Receiver<Vec<u8>>>) -> Option<Vec<u8>> {
    match opt {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Abre um socket TCP loopback efêmero (`127.0.0.1:0`) para alimentar áudio PCM f32le 48kHz
/// diretamente ao FFmpeg de forma 100% portável entre Windows e Linux.
async fn start_native_audio_bridge(
    input_mode: MultistreamInputMode,
) -> (
    Option<String>,
    Option<mpsc::Sender<Vec<u8>>>,
    Option<JoinHandle<()>>,
) {
    if input_mode != MultistreamInputMode::NativeRaw {
        return (None, None, None);
    }

    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(l) => l,
        Err(e) => {
            log::warn!("[multistream] falha ao abrir bridge TCP de áudio nativo: {e}");
            return (None, None, None);
        }
    };
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(_) => return (None, None, None),
    };

    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(AUDIO_QUEUE_CAP);
    let task = tokio::spawn(async move {
        let Ok(Ok((mut stream, _))) =
            tokio::time::timeout(Duration::from_secs(10), listener.accept()).await
        else {
            return;
        };
        let _ = stream.set_nodelay(true);
        while let Some(pcm_bytes) = rx.recv().await {
            if stream.write_all(&pcm_bytes).await.is_err() {
                break;
            }
        }
        let _ = stream.shutdown().await;
    });

    (
        Some(format!("tcp://127.0.0.1:{port}")),
        Some(tx),
        Some(task),
    )
}

fn spawn_stderr_reader(
    stderr: tokio::process::ChildStderr,
    log_ring: Arc<Mutex<VecDeque<String>>>,
    runtime_ref: Arc<Mutex<GroupRuntimeState>>,
    secrets_owned: Vec<String>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut prog = FfmpegProgress::default();
        while let Ok(Some(line)) = lines.next_line().await {
            if parse_progress_line(&line, &mut prog) {
                if let Ok(mut rt) = runtime_ref.lock() {
                    rt.progress = prog.clone();
                }
                continue;
            }
            let secret_slices: Vec<&str> = secrets_owned.iter().map(String::as_str).collect();
            let safe = redact_secrets(&line, &secret_slices);
            if let Ok(mut ring) = log_ring.lock() {
                if ring.len() >= STDERR_RING_CAP {
                    ring.pop_front();
                }
                ring.push_back(safe);
            }
        }
    })
}

fn sync_abr_runtime(
    runtime: &Arc<Mutex<GroupRuntimeState>>,
    abr: &DynamicBitrateController,
    primary_dest_id: &str,
    observed_override_kbps: Option<u32>,
) {
    if let Ok(mut rt) = runtime.lock() {
        rt.abr = abr.telemetry(primary_dest_id);
        if let Some(obs) = observed_override_kbps {
            if obs > 0 && rt.progress.bitrate_kbps == 0 {
                rt.progress.bitrate_kbps = obs;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_persistent_rtmp_group(
    cfg: &SupervisorConfig,
    current_bitrate_kbps: &mut u32,
    abr: &mut DynamicBitrateController,
    runtime: &Arc<Mutex<GroupRuntimeState>>,
    cached_init: &mut Option<Vec<u8>>,
    rx: &mut mpsc::Receiver<Vec<u8>>,
    audio_rx: &mut Option<mpsc::Receiver<Vec<u8>>>,
    abr_rx: &mut watch::Receiver<DynamicBitrateSettings>,
    stop_rx: &mut watch::Receiver<bool>,
    had_ever_published: &mut bool,
) -> PersistentGroupOutcome {
    let secret_refs: Vec<&str> = cfg.secrets.iter().map(String::as_str).collect();
    let mut sessions: Vec<(String, PersistentRtmpSession)> = Vec::new();
    let mut last_connect_err = String::new();

    for target in &cfg.targets {
        if *stop_rx.borrow() {
            for (_, mut s) in sessions {
                s.close().await;
            }
            return PersistentGroupOutcome::Stopped;
        }

        let connect_fut = PersistentRtmpSession::connect(&target.endpoint);
        tokio::pin!(connect_fut);

        let connected = tokio::select! {
            res = &mut connect_fut => res,
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    for (_, mut s) in sessions {
                        s.close().await;
                    }
                    return PersistentGroupOutcome::Stopped;
                }
                connect_fut.await
            }
        };

        match connected {
            Ok(session) => sessions.push((target.id.clone(), session)),
            Err(err) => {
                let safe = redact_secrets(&err, &secret_refs);
                log::warn!(
                    "[rtmp_publisher] falha ao conectar sessão persistente em {}: {safe}",
                    target.name
                );
                last_connect_err = safe;
            }
        }
    }

    if sessions.is_empty() {
        return if !*had_ever_published {
            PersistentGroupOutcome::FallbackClassic(last_connect_err)
        } else {
            PersistentGroupOutcome::Reconnect(last_connect_err)
        };
    }

    *had_ever_published = true;
    if let Ok(mut rt) = runtime.lock() {
        rt.state = "live".to_string();
        rt.error = None;
    }
    sync_abr_runtime(runtime, abr, &cfg.primary_dest_id, None);

    let mut rebaser = TimestampRebaser::new();
    let mut local_encoder_failures: usize = 0;

    loop {
        if *stop_rx.borrow() {
            for (_, mut s) in sessions {
                s.close().await;
            }
            return PersistentGroupOutcome::Stopped;
        }

        let (audio_addr, audio_pipe_tx, audio_bridge_task) =
            start_native_audio_bridge(cfg.input_mode).await;

        let args = match cfg.input_mode {
            MultistreamInputMode::WebmChunks => {
                build_ffmpeg_flv_pipe_args(&cfg.encoder, cfg.fps, *current_bitrate_kbps)
            }
            MultistreamInputMode::NativeRaw => build_ffmpeg_native_args(
                &cfg.encoder,
                cfg.canvas_size.0,
                cfg.canvas_size.1,
                cfg.fps,
                *current_bitrate_kbps,
                audio_addr.as_deref(),
                true,
                &[],
            ),
        };

        let mut cmd = Command::new(&cfg.bin);
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        configure_no_window(&mut cmd);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                for (_, mut s) in sessions {
                    s.close().await;
                }
                return PersistentGroupOutcome::Reconnect(format!("Falha ao abrir FFmpeg: {e}"));
            }
        };

        let child_stdin = child.stdin.take();
        let child_stdout = child.stdout.take();
        let child_stderr = child.stderr.take();
        let log_ring = Arc::new(Mutex::new(VecDeque::<String>::with_capacity(STDERR_RING_CAP)));

        let stderr_task = child_stderr.map(|err_pipe| {
            spawn_stderr_reader(err_pipe, log_ring.clone(), runtime.clone(), cfg.secrets.clone())
        });

        // Writer dedicado para stdin do FFmpeg local (desacoplado da rede RTMP para evitar deadlock).
        let (enc_in_tx, mut enc_in_rx) = mpsc::channel::<Vec<u8>>(ENCODER_STDIN_QUEUE_CAP);
        let init_for_encoder = if cfg.input_mode == MultistreamInputMode::WebmChunks
            && rebaser.segment_count() > 0
        {
            cached_init.clone()
        } else {
            None
        };
        let stdin_task = tokio::spawn(async move {
            let Some(mut w) = child_stdin else {
                return;
            };
            if let Some(init) = init_for_encoder {
                if w.write_all(&init).await.is_err() {
                    return;
                }
            }
            while let Some(chunk) = enc_in_rx.recv().await {
                if w.write_all(&chunk).await.is_err() {
                    break;
                }
            }
            let _ = w.shutdown().await;
        });

        // Demuxer FLV dedicado lendo stdout (`-f flv pipe:1`) do FFmpeg local.
        let (flv_tx, mut flv_rx) = mpsc::channel::<FlvTag>(FLV_TAG_QUEUE_CAP);
        let stdout_task = tokio::spawn(async move {
            let Some(mut out) = child_stdout else {
                return;
            };
            let mut demuxer = FlvDemuxer::new();
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match out.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => match demuxer.push(&buf[..n]) {
                        Ok(tags) => {
                            for tag in tags {
                                if flv_tx.send(tag).await.is_err() {
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("[rtmp_publisher] erro no FlvDemuxer local: {e}");
                            break;
                        }
                    },
                    Err(_) => break,
                }
            }
        });

        let mut awaiting_keyframe = rebaser.segment_count() > 0;
        let mut interval_write_lat_max_ms: f32 = 0.0;
        let mut interval_bytes_sent: u64 = 0;
        let mut interval_local_drops: u64 = 0;
        let mut last_runtime_drops: u64 = runtime
            .lock()
            .map(|rt| rt.progress.dropped_frames.saturating_add(rt.dropped_input_chunks))
            .unwrap_or(0);
        let mut interval_started = Instant::now();

        let mut abr_tick = tokio::time::interval(Duration::from_secs(1));
        abr_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Consome o primeiro tick imediato do `tokio::time::interval`.
        abr_tick.tick().await;

        let outcome = loop {
            tokio::select! {
                _ = stop_rx.changed() => {
                    if *stop_rx.borrow() {
                        break PersistentCycleOutcome::Stop;
                    }
                }
                Ok(()) = abr_rx.changed() => {
                    let next_settings = abr_rx.borrow().clone();
                    abr.update_settings(next_settings);
                    sync_abr_runtime(runtime, abr, &cfg.primary_dest_id, None);
                }
                maybe_chunk = rx.recv() => {
                    match maybe_chunk {
                        Some(chunk) => {
                            if cfg.input_mode == MultistreamInputMode::WebmChunks && cached_init.is_none() {
                                *cached_init = Some(chunk.clone());
                            }
                            if let Err(mpsc::error::TrySendError::Full(_)) = enc_in_tx.try_send(chunk) {
                                interval_local_drops = interval_local_drops.saturating_add(1);
                            }
                        }
                        None => break PersistentCycleOutcome::Stop,
                    }
                }
                maybe_pcm = recv_opt_audio(audio_rx), if audio_rx.is_some() => {
                    match maybe_pcm {
                        Some(pcm) => {
                            if let Some(ref atx) = audio_pipe_tx {
                                let _ = atx.try_send(pcm);
                            }
                        }
                        None => {
                            *audio_rx = None;
                        }
                    }
                }
                maybe_tag = flv_rx.recv() => {
                    match maybe_tag {
                        Some(mut tag) => {
                            if awaiting_keyframe {
                                if tag.is_sequence_header || tag.tag_type == FLV_TAG_SCRIPT_DATA {
                                    // Headers de sequência e metadados sempre passam no hot-swap.
                                } else if tag.tag_type == FLV_TAG_VIDEO && tag.is_video_keyframe {
                                    awaiting_keyframe = false;
                                } else {
                                    continue;
                                }
                            }

                            rebaser.rebase_tag(&mut tag);

                            let mut dead_indices = Vec::new();
                            let mut last_send_err = String::new();
                            for (idx, (_, session)) in sessions.iter_mut().enumerate() {
                                let bytes_before = session.bytes_sent();
                                match session.send_flv_tag(&tag).await {
                                    Ok(write_dur) => {
                                        let lat_ms = write_dur.as_secs_f32() * 1000.0;
                                        if lat_ms > interval_write_lat_max_ms {
                                            interval_write_lat_max_ms = lat_ms;
                                        }
                                        let delta = session.bytes_sent().saturating_sub(bytes_before);
                                        interval_bytes_sent = interval_bytes_sent.saturating_add(delta);
                                        if let Ok(mut rt) = runtime.lock() {
                                            rt.bytes_sent = rt.bytes_sent.saturating_add(delta);
                                        }
                                    }
                                    Err(err) => {
                                        last_send_err = redact_secrets(&err, &secret_refs);
                                        dead_indices.push(idx);
                                    }
                                }
                            }

                            for idx in dead_indices.into_iter().rev() {
                                let (dead_id, mut dead_session) = sessions.remove(idx);
                                log::warn!(
                                    "[rtmp_publisher] sessão persistente encerrada ({dead_id}): {last_send_err}"
                                );
                                dead_session.close().await;
                            }

                            if sessions.is_empty() {
                                break PersistentCycleOutcome::TransportError(last_send_err);
                            }
                        }
                        None => {
                            let friendly = log_ring
                                .lock()
                                .map(|r| explain_ffmpeg_error(&r, &cfg.names, &cfg.endpoints))
                                .unwrap_or_else(|_| "Encoder local encerrou inesperadamente.".to_string());
                            break PersistentCycleOutcome::EncoderFailed(friendly);
                        }
                    }
                }
                _ = abr_tick.tick() => {
                    let dt_secs = interval_started.elapsed().as_secs_f32().max(0.25);
                    let target_count = sessions.len().max(1) as f64;
                    let measured_kbps = ((interval_bytes_sent as f64 / target_count) * 8.0
                        / 1000.0
                        / dt_secs as f64)
                        .round() as u32;

                    let (ffmpeg_kbps, total_runtime_drops) = runtime
                        .lock()
                        .map(|rt| {
                            (
                                rt.progress.bitrate_kbps,
                                rt.progress.dropped_frames.saturating_add(rt.dropped_input_chunks),
                            )
                        })
                        .unwrap_or((0, last_runtime_drops));
                    let observed_kbps = if measured_kbps > 0 { measured_kbps } else { ffmpeg_kbps };
                    let runtime_drop_delta = total_runtime_drops.saturating_sub(last_runtime_drops);
                    last_runtime_drops = total_runtime_drops;

                    let in_occ = (rx.len() as f32 / CHUNK_QUEUE_CAP as f32).clamp(0.0, 1.0);
                    let enc_occ = (ENCODER_STDIN_QUEUE_CAP.saturating_sub(enc_in_tx.capacity()) as f32
                        / ENCODER_STDIN_QUEUE_CAP as f32)
                        .clamp(0.0, 1.0);
                    let flv_occ = (flv_rx.len() as f32 / FLV_TAG_QUEUE_CAP as f32).clamp(0.0, 1.0);

                    let sample = CongestionSample {
                        queue_occupancy: in_occ.max(enc_occ).max(flv_occ),
                        write_latency_ms: interval_write_lat_max_ms,
                        dropped_frames_delta: runtime_drop_delta.saturating_add(interval_local_drops),
                        observed_bitrate_kbps: observed_kbps,
                    };

                    let decision = abr.evaluate(sample, Instant::now());
                    sync_abr_runtime(runtime, abr, &cfg.primary_dest_id, Some(observed_kbps));

                    interval_write_lat_max_ms = 0.0;
                    interval_bytes_sent = 0;
                    interval_local_drops = 0;
                    interval_started = Instant::now();

                    if let Some(new_kbps) = decision {
                        log::info!(
                            "[abr] hot-swap zero-disconnect ({}): {} -> {} kbps",
                            cfg.names,
                            *current_bitrate_kbps,
                            new_kbps
                        );
                        break PersistentCycleOutcome::HotSwap(new_kbps);
                    }
                }
            }
        };

        // Encerra APENAS o encoder FFmpeg local mantendo `sessions` RTMP vivas!
        drop(enc_in_tx);
        drop(audio_pipe_tx);
        terminate_child(&mut child).await;
        let _ = stdin_task.await;
        let _ = stdout_task.await;
        if let Some(t) = stderr_task {
            let _ = t.await;
        }
        if let Some(t) = audio_bridge_task {
            t.abort();
        }

        match outcome {
            PersistentCycleOutcome::Stop => {
                for (_, mut s) in sessions {
                    s.close().await;
                }
                return PersistentGroupOutcome::Stopped;
            }
            PersistentCycleOutcome::HotSwap(new_kbps) => {
                *current_bitrate_kbps = new_kbps;
                rebaser.begin_new_segment();
                local_encoder_failures = 0;
                continue;
            }
            PersistentCycleOutcome::TransportError(err) => {
                for (_, mut s) in sessions {
                    s.close().await;
                }
                return PersistentGroupOutcome::Reconnect(err);
            }
            PersistentCycleOutcome::EncoderFailed(err) => {
                local_encoder_failures += 1;
                if local_encoder_failures <= 3 && !sessions.is_empty() {
                    rebaser.begin_new_segment();
                    continue;
                }
                for (_, mut s) in sessions {
                    s.close().await;
                }
                return PersistentGroupOutcome::Reconnect(err);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_classic_ffmpeg_cycle(
    cfg: &SupervisorConfig,
    current_bitrate_kbps: u32,
    abr: &mut DynamicBitrateController,
    runtime: &Arc<Mutex<GroupRuntimeState>>,
    cached_init: &mut Option<Vec<u8>>,
    replay_init: bool,
    rx: &mut mpsc::Receiver<Vec<u8>>,
    audio_rx: &mut Option<mpsc::Receiver<Vec<u8>>>,
    abr_rx: &mut watch::Receiver<DynamicBitrateSettings>,
    stop_rx: &mut watch::Receiver<bool>,
    attempt: &mut usize,
) -> ClassicCycleOutcome {
    let secret_refs: Vec<&str> = cfg.secrets.iter().map(String::as_str).collect();
    let (audio_addr, audio_pipe_tx, audio_bridge_task) =
        start_native_audio_bridge(cfg.input_mode).await;

    let args = match cfg.input_mode {
        MultistreamInputMode::WebmChunks => {
            build_ffmpeg_args(&cfg.encoder, &cfg.endpoints, cfg.fps, current_bitrate_kbps)
        }
        MultistreamInputMode::NativeRaw => build_ffmpeg_native_args(
            &cfg.encoder,
            cfg.canvas_size.0,
            cfg.canvas_size.1,
            cfg.fps,
            current_bitrate_kbps,
            audio_addr.as_deref(),
            false,
            &cfg.endpoints,
        ),
    };

    let mut cmd = Command::new(&cfg.bin);
    cmd.args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_no_window(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            if let Some(t) = audio_bridge_task {
                t.abort();
            }
            return ClassicCycleOutcome::Fatal(format!("Falha ao abrir FFmpeg: {e}"));
        }
    };

    let mut stdin = child.stdin.take();
    let stderr = child.stderr.take();
    let log_ring = Arc::new(Mutex::new(VecDeque::<String>::with_capacity(STDERR_RING_CAP)));
    let stderr_task = stderr.map(|err_pipe| {
        spawn_stderr_reader(err_pipe, log_ring.clone(), runtime.clone(), cfg.secrets.clone())
    });

    if replay_init && cfg.input_mode == MultistreamInputMode::WebmChunks {
        if let (Some(ref init), Some(ref mut w)) = (&cached_init, &mut stdin) {
            let _ = w.write_all(init).await;
        }
    }

    let live_timer = tokio::time::sleep(Duration::from_secs(4));
    tokio::pin!(live_timer);
    let mut live_promoted = false;

    let mut abr_tick = tokio::time::interval(Duration::from_secs(1));
    abr_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    abr_tick.tick().await;

    let mut interval_write_lat_max_ms: f32 = 0.0;
    let mut last_runtime_drops: u64 = runtime
        .lock()
        .map(|rt| rt.progress.dropped_frames.saturating_add(rt.dropped_input_chunks))
        .unwrap_or(0);

    let outcome = loop {
        tokio::select! {
            _ = &mut live_timer, if !live_promoted => {
                live_promoted = true;
                *attempt = 0;
                if let Ok(mut rt) = runtime.lock() {
                    rt.state = "live".to_string();
                    rt.error = None;
                }
            }
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    break ClassicCycleOutcome::Stop;
                }
            }
            Ok(()) = abr_rx.changed() => {
                abr.update_settings(abr_rx.borrow().clone());
                sync_abr_runtime(runtime, abr, &cfg.primary_dest_id, None);
            }
            maybe_chunk = rx.recv() => {
                match maybe_chunk {
                    Some(chunk) => {
                        if cfg.input_mode == MultistreamInputMode::WebmChunks && cached_init.is_none() {
                            *cached_init = Some(chunk.clone());
                        }
                        if let Some(ref mut w) = stdin {
                            let write_start = Instant::now();
                            if let Err(e) = w.write_all(&chunk).await {
                                break ClassicCycleOutcome::Reconnect(redact_secrets(
                                    &e.to_string(),
                                    &secret_refs,
                                ));
                            }
                            let lat_ms = write_start.elapsed().as_secs_f32() * 1000.0;
                            if lat_ms > interval_write_lat_max_ms {
                                interval_write_lat_max_ms = lat_ms;
                            }
                            if let Ok(mut rt) = runtime.lock() {
                                rt.bytes_sent = rt.bytes_sent.saturating_add(chunk.len() as u64);
                            }
                        }
                    }
                    None => break ClassicCycleOutcome::Stop,
                }
            }
            maybe_pcm = recv_opt_audio(audio_rx), if audio_rx.is_some() => {
                match maybe_pcm {
                    Some(pcm) => {
                        if let Some(ref atx) = audio_pipe_tx {
                            let _ = atx.try_send(pcm);
                        }
                    }
                    None => {
                        *audio_rx = None;
                    }
                }
            }
            _ = abr_tick.tick() => {
                let (observed_kbps, total_drops) = runtime
                    .lock()
                    .map(|rt| {
                        (
                            rt.progress.bitrate_kbps,
                            rt.progress.dropped_frames.saturating_add(rt.dropped_input_chunks),
                        )
                    })
                    .unwrap_or((0, last_runtime_drops));
                let drop_delta = total_drops.saturating_sub(last_runtime_drops);
                last_runtime_drops = total_drops;

                let sample = CongestionSample {
                    queue_occupancy: (rx.len() as f32 / CHUNK_QUEUE_CAP as f32).clamp(0.0, 1.0),
                    write_latency_ms: interval_write_lat_max_ms,
                    dropped_frames_delta: drop_delta,
                    observed_bitrate_kbps: observed_kbps,
                };
                interval_write_lat_max_ms = 0.0;

                let decision = abr.evaluate(sample, Instant::now());
                sync_abr_runtime(runtime, abr, &cfg.primary_dest_id, None);
                if let Some(new_kbps) = decision {
                    log::info!(
                        "[abr] ajuste de bitrate em modo FFmpeg clássico ({}): {} -> {} kbps",
                        cfg.names,
                        current_bitrate_kbps,
                        new_kbps
                    );
                    break ClassicCycleOutcome::HotSwap(new_kbps);
                }
            }
            status = child.wait() => {
                let reason = match status {
                    Ok(st) if st.success() => format!("A transmissão para {} encerrou.", cfg.names),
                    Ok(st) => format!("FFmpeg encerrou (código {:?})", st.code()),
                    Err(e) => e.to_string(),
                };
                break ClassicCycleOutcome::Reconnect(reason);
            }
        }
    };

    drop(stdin);
    drop(audio_pipe_tx);
    terminate_child(&mut child).await;
    if let Some(t) = stderr_task {
        let _ = t.await;
    }
    if let Some(t) = audio_bridge_task {
        t.abort();
    }

    match outcome {
        ClassicCycleOutcome::Reconnect(_) => {
            let friendly = log_ring
                .lock()
                .map(|r| explain_ffmpeg_error(&r, &cfg.names, &cfg.endpoints))
                .unwrap_or_else(|_| format!("A transmissão para {} caiu.", cfg.names));
            ClassicCycleOutcome::Reconnect(friendly)
        }
        other => other,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_pipeline_supervisor(
    bin: String,
    encoder: String,
    fps: u32,
    configured_bitrate_kbps: u32,
    canvas_size: (u32, u32),
    input_mode: MultistreamInputMode,
    targets: Vec<ResolvedTarget>,
    runtime: Arc<Mutex<GroupRuntimeState>>,
    mut rx: mpsc::Receiver<Vec<u8>>,
    mut audio_rx: Option<mpsc::Receiver<Vec<u8>>>,
    mut abr_rx: watch::Receiver<DynamicBitrateSettings>,
    mut stop_rx: watch::Receiver<bool>,
) {
    let endpoints: Vec<String> = targets.iter().map(|t| t.endpoint.clone()).collect();
    let secrets: Vec<String> = targets.iter().map(|t| t.secret.clone()).collect();
    let names = targets
        .iter()
        .map(|t| t.name.as_str())
        .collect::<Vec<_>>()
        .join(" + ");
    let primary_dest_id = targets.first().map(|t| t.id.clone()).unwrap_or_default();

    let cfg = SupervisorConfig {
        bin,
        encoder,
        fps,
        canvas_size,
        input_mode,
        targets,
        endpoints,
        secrets,
        names,
        primary_dest_id,
    };

    let mut abr =
        DynamicBitrateController::new(configured_bitrate_kbps, abr_rx.borrow().clone());
    sync_abr_runtime(&runtime, &abr, &cfg.primary_dest_id, None);

    let mut current_bitrate_kbps = configured_bitrate_kbps;
    let mut attempt: usize = 0;
    let mut cached_init: Option<Vec<u8>> = None;
    let mut replay_init_next = false;
    let mut persistent_rtmp_disabled = false;
    let mut had_ever_published_persistent = false;

    loop {
        if *stop_rx.borrow() {
            break;
        }

        if attempt > 0 {
            if let Ok(mut rt) = runtime.lock() {
                rt.state = "reconnecting".to_string();
            }
            let delay = Duration::from_secs(
                RECONNECT_BACKOFF_SECS[(attempt - 1).min(RECONNECT_BACKOFF_SECS.len() - 1)],
            );
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = stop_rx.changed() => {
                    if *stop_rx.borrow() { break; }
                }
            }
        }

        let settings_now = abr_rx.borrow().clone();
        abr.update_settings(settings_now.clone());

        if settings_now.enabled
            && settings_now.use_persistent_rtmp
            && !persistent_rtmp_disabled
        {
            match run_persistent_rtmp_group(
                &cfg,
                &mut current_bitrate_kbps,
                &mut abr,
                &runtime,
                &mut cached_init,
                &mut rx,
                &mut audio_rx,
                &mut abr_rx,
                &mut stop_rx,
                &mut had_ever_published_persistent,
            )
            .await
            {
                PersistentGroupOutcome::Stopped => break,
                PersistentGroupOutcome::FallbackClassic(reason) => {
                    log::warn!(
                        "[multistream] RTMP persistente indisponível para {} ({reason}); usando FFmpeg direto com ABR.",
                        cfg.names
                    );
                    persistent_rtmp_disabled = true;
                    replay_init_next = cached_init.is_some();
                    continue;
                }
                PersistentGroupOutcome::Reconnect(reason) => {
                    attempt += 1;
                    if let Ok(mut rt) = runtime.lock() {
                        rt.error = Some(reason);
                    }
                    if attempt > RECONNECT_BACKOFF_SECS.len() {
                        if let Ok(mut rt) = runtime.lock() {
                            rt.state = "error".to_string();
                        }
                        break;
                    }
                    continue;
                }
            }
        }

        let cycle = run_classic_ffmpeg_cycle(
            &cfg,
            current_bitrate_kbps,
            &mut abr,
            &runtime,
            &mut cached_init,
            attempt > 0 || replay_init_next,
            &mut rx,
            &mut audio_rx,
            &mut abr_rx,
            &mut stop_rx,
            &mut attempt,
        )
        .await;
        replay_init_next = false;

        match cycle {
            ClassicCycleOutcome::Stop => break,
            ClassicCycleOutcome::HotSwap(new_kbps) => {
                current_bitrate_kbps = new_kbps;
                replay_init_next = true;
                attempt = 0;
            }
            ClassicCycleOutcome::Fatal(err) => {
                if let Ok(mut rt) = runtime.lock() {
                    rt.state = "error".to_string();
                    rt.error = Some(err);
                }
                break;
            }
            ClassicCycleOutcome::Reconnect(friendly) => {
                attempt += 1;
                if let Ok(mut rt) = runtime.lock() {
                    rt.error = Some(friendly);
                }
                if attempt > RECONNECT_BACKOFF_SECS.len() {
                    if let Ok(mut rt) = runtime.lock() {
                        rt.state = "error".to_string();
                    }
                    break;
                }
            }
        }
    }

    if let Ok(mut rt) = runtime.lock() {
        if rt.state != "error" {
            rt.state = "idle".to_string();
        }
    }
}

pub type SharedMultistreamEngine = Arc<AsyncMutex<MultistreamEngine>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_rtmp_endpoint_security() {
        assert_eq!(
            validate_and_build_rtmp_endpoint("rtmp://live.twitch.tv/app/", "live_123_abc").unwrap(),
            "rtmp://live.twitch.tv/app/live_123_abc"
        );
        assert_eq!(
            validate_and_build_rtmp_endpoint("rtmps://a.rtmp.youtube.com/live2", "/key-xyz")
                .unwrap(),
            "rtmps://a.rtmp.youtube.com/live2/key-xyz"
        );

        assert!(validate_and_build_rtmp_endpoint("file:///C:/secret.txt", "abc").is_err());
        assert!(validate_and_build_rtmp_endpoint("http://example.com/live", "abc").is_err());
        assert!(validate_and_build_rtmp_endpoint("-y", "abc").is_err());
        assert!(validate_and_build_rtmp_endpoint("rtmp://host/app", "-flag").is_err());
        assert!(validate_and_build_rtmp_endpoint("rtmp://host/app", "key|evil").is_err());
        assert!(validate_and_build_rtmp_endpoint("rtmp://host/app", "key with space").is_err());
    }

    #[test]
    fn test_build_ffmpeg_args_tee_and_single() {
        let single = build_ffmpeg_args(
            "h264_nvenc",
            &["rtmp://live.twitch.tv/app/secret".to_string()],
            60,
            6000,
        );
        assert!(single.contains(&"-f".to_string()));
        assert!(single.contains(&"flv".to_string()));
        assert!(!single.contains(&"tee".to_string()));
        assert!(single.contains(&"pipe:2".to_string()));

        let multi = build_ffmpeg_args(
            "libx264",
            &[
                "rtmp://live.twitch.tv/app/k1".to_string(),
                "rtmps://a.rtmp.youtube.com/live2/k2".to_string(),
            ],
            60,
            6000,
        );
        let tee_pos = multi.iter().position(|a| a == "tee").expect("tee muxer");
        let tee_spec = &multi[tee_pos + 1];
        assert!(tee_spec.contains("[f=flv:onfail=ignore]rtmp\\://live.twitch.tv/app/k1"));
        assert!(tee_spec.contains("|[f=flv:onfail=ignore]rtmps\\://a.rtmp.youtube.com/live2/k2"));
        assert!(multi.contains(&"+global_header".to_string()));
    }

    #[test]
    fn test_build_ffmpeg_flv_pipe_args() {
        let args = build_ffmpeg_flv_pipe_args("h264_nvenc", 60, 4500);
        assert!(args.contains(&"pipe:0".to_string()));
        assert!(args.contains(&"pipe:1".to_string()));
        assert!(args.contains(&"pipe:2".to_string()));
        assert!(args.contains(&"no_duration_filesize".to_string()));
        assert!(args.contains(&"-flush_packets".to_string()));
        assert!(args.contains(&"4500k".to_string()));
        assert!(args.contains(&"9000k".to_string()));
        assert!(!args.contains(&"tee".to_string()));
    }

    #[test]
    fn test_build_ffmpeg_native_args() {
        let flv_args = build_ffmpeg_native_args(
            "libx264",
            1920,
            1080,
            60,
            6000,
            Some("tcp://127.0.0.1:45555"),
            true,
            &[],
        );
        assert!(flv_args.contains(&"rawvideo".to_string()));
        assert!(flv_args.contains(&"bgra".to_string()));
        assert!(flv_args.contains(&"1920x1080".to_string()));
        assert!(flv_args.contains(&"f32le".to_string()));
        assert!(flv_args.contains(&"tcp://127.0.0.1:45555".to_string()));
        assert!(flv_args.contains(&"pipe:1".to_string()));
        assert!(flv_args.contains(&"1:a:0".to_string()));

        let direct_args = build_ffmpeg_native_args(
            "h264_qsv",
            1080,
            1920,
            30,
            3500,
            None,
            false,
            &["rtmp://live.twitch.tv/app/key".to_string()],
        );
        assert!(direct_args.contains(&"1080x1920".to_string()));
        assert!(direct_args
            .iter()
            .any(|a| a.starts_with("anullsrc=channel_layout=stereo")));
        assert!(direct_args.contains(&"rtmp://live.twitch.tv/app/key".to_string()));
        assert!(!direct_args.contains(&"pipe:1".to_string()));
    }

    #[tokio::test]
    async fn test_abr_telemetry_aggregation_and_summary() {
        let mut engine = MultistreamEngine::new();
        assert_eq!(engine.abr_summary(), (false, 0, 0.0));
        assert!(engine.abr_telemetry().is_empty());

        let mut abr_cfg = DynamicBitrateSettings::default();
        abr_cfg.min_bitrate_kbps = 1200;
        engine.set_dynamic_bitrate(abr_cfg.clone());
        assert_eq!(engine.dynamic_bitrate().min_bitrate_kbps, 1200);

        let rt_h = Arc::new(Mutex::new(GroupRuntimeState {
            dest_ids: vec!["twitch".to_string(), "youtube".to_string()],
            orientation: CanvasOrientation::Horizontal,
            state: "live".to_string(),
            started_at: Some(Instant::now()),
            bytes_sent: 100_000,
            dropped_input_chunks: 0,
            progress: FfmpegProgress::default(),
            abr: AbrDestinationTelemetry {
                id: "twitch".to_string(),
                configured_bitrate_kbps: 6000,
                current_bitrate_kbps: 4500,
                observed_bitrate_kbps: 4480,
                congestion_level: 0.62,
                write_latency_ms: 195.0,
                adjustments: 1,
                state: "congested".to_string(),
            },
            error: None,
        }));
        let rt_v = Arc::new(Mutex::new(GroupRuntimeState {
            dest_ids: vec!["tiktok".to_string()],
            orientation: CanvasOrientation::Vertical,
            state: "live".to_string(),
            started_at: Some(Instant::now()),
            bytes_sent: 50_000,
            dropped_input_chunks: 0,
            progress: FfmpegProgress::default(),
            abr: AbrDestinationTelemetry {
                id: "tiktok".to_string(),
                configured_bitrate_kbps: 3000,
                current_bitrate_kbps: 3000,
                observed_bitrate_kbps: 2990,
                congestion_level: 0.15,
                write_latency_ms: 22.0,
                adjustments: 0,
                state: "nominal".to_string(),
            },
            error: None,
        }));

        let (tx1, _rx1) = mpsc::channel(4);
        let (tx2, _rx2) = mpsc::channel(4);
        let (abr_tx1, _abr_rx1) = watch::channel(abr_cfg.clone());
        let (abr_tx2, _abr_rx2) = watch::channel(abr_cfg);
        let (stop_tx1, _stop_rx1) = watch::channel(false);
        let (stop_tx2, _stop_rx2) = watch::channel(false);

        engine.groups.push(PipelineGroup {
            orientation: CanvasOrientation::Horizontal,
            input_mode: MultistreamInputMode::WebmChunks,
            canvas_size: DEFAULT_CANVAS_H,
            tx: tx1,
            audio_tx: None,
            abr_settings_tx: abr_tx1,
            stop_tx: stop_tx1,
            task: tokio::spawn(async {}),
            runtime: rt_h,
        });
        engine.groups.push(PipelineGroup {
            orientation: CanvasOrientation::Vertical,
            input_mode: MultistreamInputMode::NativeRaw,
            canvas_size: DEFAULT_CANVAS_V,
            tx: tx2,
            audio_tx: None,
            abr_settings_tx: abr_tx2,
            stop_tx: stop_tx2,
            task: tokio::spawn(async {}),
            runtime: rt_v,
        });
        engine.active = true;

        let telemetry = engine.abr_telemetry();
        assert_eq!(telemetry.len(), 3);
        let ids: Vec<&str> = telemetry.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&"twitch"));
        assert!(ids.contains(&"youtube"));
        assert!(ids.contains(&"tiktok"));

        let (abr_active, target_kbps, max_cong) = engine.abr_summary();
        assert!(abr_active);
        assert_eq!(target_kbps, 7500);
        assert!((max_cong - 0.62).abs() < 0.001);

        // Desabilitar ABR mantém target_kbps visível mas marca abr_active = false
        let mut disabled = engine.dynamic_bitrate().clone();
        disabled.enabled = false;
        engine.set_dynamic_bitrate(disabled);
        let (abr_active_off, target_off, _) = engine.abr_summary();
        assert!(!abr_active_off);
        assert_eq!(target_off, 7500);
    }

    #[test]
    fn test_parse_progress_line() {
        let mut prog = FfmpegProgress::default();
        assert!(parse_progress_line("fps=59.94", &mut prog));
        assert!(parse_progress_line("bitrate=6120.5kbits/s", &mut prog));
        assert!(parse_progress_line("drop_frames=3", &mut prog));
        assert!(parse_progress_line("total_size=1048576", &mut prog));
        assert!(parse_progress_line("progress=continue", &mut prog));
        assert!(!parse_progress_line("[flv @ 0x1] Connection reset", &mut prog));

        assert!((prog.fps - 59.94).abs() < 0.01);
        assert_eq!(prog.bitrate_kbps, 6121);
        assert_eq!(prog.dropped_frames, 3);
        assert_eq!(prog.total_size_bytes, 1_048_576);
    }

    #[test]
    fn test_redact_secrets() {
        let msg = "Error opening rtmp://live.twitch.tv/app/live_999_secretkey: I/O error";
        let safe = redact_secrets(msg, &["live_999_secretkey"]);
        assert!(!safe.contains("live_999_secretkey"));
        assert!(safe.contains("••••"));
    }
}
