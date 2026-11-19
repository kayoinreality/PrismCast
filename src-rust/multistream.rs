use crate::protocol::{
    CanvasOrientation, Destination, DestinationStatus, MultistreamEngineStatus, MultistreamStatus,
    MultistreamTarget, MultistreamTargetStatus,
};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use url::Url;

const CHUNK_QUEUE_CAP: usize = 64;
const STDERR_RING_CAP: usize = 60;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const STOP_GRACE: Duration = Duration::from_millis(1500);
const RECONNECT_BACKOFF_SECS: [u64; 5] = [1, 2, 4, 8, 15];

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

/// Monta os argumentos FFmpeg para um grupo de destinos com mesma orientação e mesmo bitrate.
/// Quando há 2+ destinos no mesmo grupo, usa o muxer `tee` para evitar codificação H.264/AAC duplicada.
pub fn build_ffmpeg_args(
    encoder: &str,
    endpoints: &[String],
    fps: u32,
    bitrate_kbps: u32,
) -> Vec<String> {
    let kbps = bitrate_kbps.clamp(1000, 20_000);
    let fps_clamped = fps.clamp(15, 60);
    let gop = (fps_clamped * 2).to_string();

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
            args.extend(["-bf".to_string(), "2".to_string(), "-pix_fmt".to_string(), "yuv420p".to_string()]);
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

    if endpoints.len() <= 1 {
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

/// Estado interno compartilhado entre a task supervisora do pipeline e `MultistreamEngine`.
#[derive(Debug, Clone)]
struct GroupRuntimeState {
    dest_ids: Vec<String>,
    orientation: CanvasOrientation,
    state: String, // "connecting" | "live" | "reconnecting" | "error" | "idle"
    started_at: Option<Instant>,
    bytes_sent: u64,
    progress: FfmpegProgress,
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
    tx: mpsc::Sender<Vec<u8>>,
    stop_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
    runtime: Arc<Mutex<GroupRuntimeState>>,
}

/// Motor de Multistreaming + Dual Output (16:9 + 9:16) em Rust.
///
/// Características:
/// - Agrupa destinos da mesma orientação + bitrate usando muxer `tee` (zero re-encode duplicado).
/// - Backpressure real via `tokio::sync::mpsc::channel(64)` sem travar `Mutex` durante I/O.
/// - Reconexão automática com backoff exponencial por grupo em quedas transitórias.
/// - Leitura de `-progress pipe:2` e `stderr` em ring buffer com redação de `streamKey`.
/// - `impl Drop` garantindo encerramento limpo de processos filhos sem órfãos.
pub struct MultistreamEngine {
    encoder: String,
    ffmpeg_found: bool,
    ffmpeg_path: Option<String>,
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
            let dest_ids: Vec<String> = group_targets.iter().map(|t| t.id.clone()).collect();

            let runtime = Arc::new(Mutex::new(GroupRuntimeState {
                dest_ids,
                orientation,
                state: "connecting".to_string(),
                started_at: Some(Instant::now()),
                bytes_sent: 0,
                progress: FfmpegProgress::default(),
                error: None,
            }));

            let (tx, rx) = mpsc::channel::<Vec<u8>>(CHUNK_QUEUE_CAP);
            let (stop_tx, stop_rx) = watch::channel(false);

            let task = tokio::spawn(run_pipeline_supervisor(
                bin.clone(),
                encoder.clone(),
                fps,
                bitrate_kbps,
                group_targets,
                runtime.clone(),
                rx,
                stop_rx,
            ));

            self.groups.push(PipelineGroup {
                orientation,
                tx,
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
            if group.orientation != orientation {
                continue;
            }
            let payload = chunk.to_vec();
            if let Err(err) = group.tx.try_send(payload) {
                match err {
                    mpsc::error::TrySendError::Full(_) => {
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

#[allow(clippy::too_many_arguments)]
async fn run_pipeline_supervisor(
    bin: String,
    encoder: String,
    fps: u32,
    bitrate_kbps: u32,
    targets: Vec<ResolvedTarget>,
    runtime: Arc<Mutex<GroupRuntimeState>>,
    mut rx: mpsc::Receiver<Vec<u8>>,
    mut stop_rx: watch::Receiver<bool>,
) {
    let endpoints: Vec<String> = targets.iter().map(|t| t.endpoint.clone()).collect();
    let secrets: Vec<String> = targets.iter().map(|t| t.secret.clone()).collect();
    let secret_refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
    let names = targets
        .iter()
        .map(|t| t.name.as_str())
        .collect::<Vec<_>>()
        .join(" + ");
    let args = build_ffmpeg_args(&encoder, &endpoints, fps, bitrate_kbps);

    let mut attempt: usize = 0;
    let mut cached_init: Option<Vec<u8>> = None;

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

        let mut cmd = Command::new(&bin);
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        configure_no_window(&mut cmd);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                if let Ok(mut rt) = runtime.lock() {
                    rt.state = "error".to_string();
                    rt.error = Some(format!("Falha ao abrir FFmpeg: {e}"));
                }
                break;
            }
        };

        let mut stdin = child.stdin.take();
        let stderr = child.stderr.take();
        let log_ring = Arc::new(Mutex::new(VecDeque::<String>::with_capacity(STDERR_RING_CAP)));

        let stderr_task = stderr.map(|err_pipe| {
            let log_ring = log_ring.clone();
            let runtime_ref = runtime.clone();
            let secrets_owned = secrets.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(err_pipe).lines();
                let mut prog = FfmpegProgress::default();
                while let Ok(Some(line)) = lines.next_line().await {
                    if parse_progress_line(&line, &mut prog) {
                        if let Ok(mut rt) = runtime_ref.lock() {
                            rt.progress = prog.clone();
                        }
                        continue;
                    }
                    let secret_slices: Vec<&str> =
                        secrets_owned.iter().map(String::as_str).collect();
                    let safe = redact_secrets(&line, &secret_slices);
                    if let Ok(mut ring) = log_ring.lock() {
                        if ring.len() >= STDERR_RING_CAP {
                            ring.pop_front();
                        }
                        ring.push_back(safe);
                    }
                }
            })
        });

        // Se for reconexão e já guardamos o init segment, reenvia primeiro para destravar o demuxer WebM.
        if attempt > 0 {
            if let (Some(ref init), Some(ref mut w)) = (&cached_init, &mut stdin) {
                let _ = w.write_all(init).await;
            }
        }

        let live_timer = tokio::time::sleep(Duration::from_secs(4));
        tokio::pin!(live_timer);
        let mut live_promoted = false;

        let exit_reason: Result<bool, String> = loop {
            tokio::select! {
                _ = &mut live_timer, if !live_promoted => {
                    live_promoted = true;
                    attempt = 0;
                    if let Ok(mut rt) = runtime.lock() {
                        rt.state = "live".to_string();
                        rt.error = None;
                    }
                }
                _ = stop_rx.changed() => {
                    if *stop_rx.borrow() {
                        break Ok(true); // parada intencional
                    }
                }
                maybe_chunk = rx.recv() => {
                    match maybe_chunk {
                        Some(chunk) => {
                            if cached_init.is_none() {
                                cached_init = Some(chunk.clone());
                            }
                            if let Some(ref mut w) = stdin {
                                if let Err(e) = w.write_all(&chunk).await {
                                    break Err(redact_secrets(&e.to_string(), &secret_refs));
                                }
                                if let Ok(mut rt) = runtime.lock() {
                                    rt.bytes_sent = rt.bytes_sent.saturating_add(chunk.len() as u64);
                                }
                            }
                        }
                        None => break Ok(true),
                    }
                }
                status = child.wait() => {
                    match status {
                        Ok(st) if st.success() => break Ok(false),
                        Ok(st) => break Err(format!("FFmpeg encerrou (código {:?})", st.code())),
                        Err(e) => break Err(e.to_string()),
                    }
                }
            }
        };

        drop(stdin);
        terminate_child(&mut child).await;
        if let Some(t) = stderr_task {
            let _ = t.await;
        }

        match exit_reason {
            Ok(true) => break, // cancelado ou canal fechado
            Ok(false) | Err(_) => {
                attempt += 1;
                let friendly = log_ring
                    .lock()
                    .map(|r| explain_ffmpeg_error(&r, &names, &endpoints))
                    .unwrap_or_else(|_| format!("A transmissão para {names} caiu."));

                if let Ok(mut rt) = runtime.lock() {
                    rt.error = Some(friendly);
                }

                // Se passou do limite de tentativas consecutivas sem estabilizar, marca error final.
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
