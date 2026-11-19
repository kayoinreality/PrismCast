use crate::protocol::RecorderStatus;
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::process::Command;

/// Assinatura EBML do elemento Cluster (`0x1F 0x43 0xB6 0x75`) em WebM/Matroska.
const EBML_CLUSTER_ID: [u8; 4] = [0x1F, 0x43, 0xB6, 0x75];

/// Teto padrão de memória do Replay Buffer circular (128 MiB).
pub const DEFAULT_MAX_REPLAY_BYTES: usize = 128 * 1024 * 1024;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone)]
struct ReplayChunk {
    timestamp_ms: u64,
    starts_cluster: bool,
    data: Vec<u8>,
}

/// Verifica se o chunk contém (ou inicia) uma fronteira de EBML Cluster WebM (`1F 43 B6 75`).
pub fn contains_ebml_cluster(bytes: &[u8]) -> bool {
    bytes.windows(4).any(|w| w == EBML_CLUSTER_ID)
}

/// Verifica se o chunk começa exatamente com um Cluster EBML ou contém seu marcador no cabeçalho inicial.
pub fn starts_with_ebml_cluster(bytes: &[u8]) -> bool {
    bytes.starts_with(&EBML_CLUSTER_ID) || contains_ebml_cluster(&bytes[..bytes.len().min(64)])
}

/// Gera carimbo de data/hora com precisão de milissegundos evitando colisão no mesmo segundo.
pub fn unique_timestamp_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d_%H-%M-%S-%3f").to_string()
}

/// Remuxa um `.webm` para `.mp4` (cópia de vídeo sem re-encode + áudio AAC `+faststart`).
/// Só remove o `.webm` original se o FFmpeg terminar com exit code 0 e o `.mp4` existir com tamanho > 0.
/// Retorna o caminho final real (`.mp4` em sucesso, `.webm` em caso de falha ou ausência de FFmpeg).
pub async fn remux_to_mp4(webm_path: PathBuf, ffmpeg_path: Option<String>) -> PathBuf {
    let mp4_path = webm_path.with_extension("mp4");
    let bin = ffmpeg_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("ffmpeg")
        .to_string();

    let mut cmd = Command::new(&bin);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-i",
    ])
    .arg(&webm_path)
    .args([
        "-c:v",
        "copy",
        "-c:a",
        "aac",
        "-b:a",
        "160k",
        "-movflags",
        "+faststart",
    ])
    .arg(&mp4_path)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .kill_on_drop(true);

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    match cmd.status().await {
        Ok(st) if st.success() => {
            let valid_mp4 = fs::metadata(&mp4_path).map(|m| m.len() > 0).unwrap_or(false);
            if valid_mp4 {
                let _ = fs::remove_file(&webm_path);
                return mp4_path;
            }
            let _ = fs::remove_file(&mp4_path);
            webm_path
        }
        _ => {
            let _ = fs::remove_file(&mp4_path);
            webm_path
        }
    }
}

/// Gravador local + Replay Buffer circular em Rust.
///
/// Correções estruturais (Fases 1, 2 e 3):
/// - Escrita com `BufWriter` propagando erros reais de disco (disco cheio / permissão).
/// - Poda do Replay Buffer respeitando fronteiras de **EBML Cluster** (`0x1F 0x43 0xB6 0x75`),
///   garantindo que `init_segment + replay_chunks` produza um `.webm` íntegro e reproduzível.
/// - Teto explícito de RAM (`max_replay_bytes`) e `shrink_to_fit()` ao desativar.
/// - `reset_stream` preserva gravação em curso e não quebra gravação ativa.
/// - Pasta de gravação configurável (`set_recording_dir`) + nomes únicos com milissegundos.
/// - `buffered_sec` preciso mesmo após poda circular contínua.
pub struct RecorderEngine {
    output_dir: PathBuf,
    recording_writer: Option<BufWriter<File>>,
    recording_path: Option<PathBuf>,
    recording_started: Option<Instant>,
    last_error: Option<String>,

    replay_active: bool,
    replay_max_seconds: u32,
    max_replay_bytes: usize,
    replay_bytes_used: usize,
    replay_started: Option<Instant>,
    init_segment: Option<Vec<u8>>,
    replay_chunks: VecDeque<ReplayChunk>,
    last_saved_replay: Option<PathBuf>,
}

impl RecorderEngine {
    pub fn new() -> Self {
        let output_dir = Self::default_output_dir();
        let _ = fs::create_dir_all(&output_dir);

        Self {
            output_dir,
            recording_writer: None,
            recording_path: None,
            recording_started: None,
            last_error: None,
            replay_active: true,
            replay_max_seconds: 30,
            max_replay_bytes: DEFAULT_MAX_REPLAY_BYTES,
            replay_bytes_used: 0,
            replay_started: Some(Instant::now()),
            init_segment: None,
            replay_chunks: VecDeque::new(),
            last_saved_replay: None,
        }
    }

    pub fn default_output_dir() -> PathBuf {
        dirs::video_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("PrismCast")
    }

    pub fn set_recording_dir(&mut self, dir: Option<&str>) {
        let next = dir
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(Self::default_output_dir);
        self.output_dir = next;
    }

    pub fn recording_dir(&self) -> &Path {
        &self.output_dir
    }

    pub fn set_max_replay_bytes(&mut self, max_bytes: usize) {
        self.max_replay_bytes = max_bytes.max(4 * 1024 * 1024);
        self.prune_replay(self.current_replay_elapsed_ms());
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn start_recording(&mut self) -> Result<RecorderStatus, String> {
        if self.recording_writer.is_some() {
            return Ok(self.status());
        }
        fs::create_dir_all(&self.output_dir).map_err(|e| format!("Falha ao criar pasta: {e}"))?;
        let path = self
            .output_dir
            .join(format!("PrismCast_Live_{}.webm", unique_timestamp_stamp()));
        let file = File::create(&path).map_err(|e| format!("Falha ao criar gravação: {e}"))?;
        let mut writer = BufWriter::with_capacity(256 * 1024, file);

        if let Some(ref init) = self.init_segment {
            writer
                .write_all(init)
                .map_err(|e| format!("Erro ao gravar cabeçalho WebM: {e}"))?;
        }

        self.last_error = None;
        self.recording_writer = Some(writer);
        self.recording_path = Some(path);
        self.recording_started = Some(Instant::now());
        Ok(self.status())
    }

    /// Para a gravação atual e retorna `(status, path_para_remux_se_houver)`.
    pub fn stop_recording_with_path(&mut self) -> (RecorderStatus, Option<PathBuf>) {
        let mut finished_path = None;
        if let Some(mut writer) = self.recording_writer.take() {
            if let Err(e) = writer.flush() {
                self.last_error = Some(format!("Erro ao finalizar gravação: {e}"));
            } else {
                finished_path = self.recording_path.clone();
            }
        }
        self.recording_started = None;
        (self.status(), finished_path)
    }

    pub fn stop_recording(&mut self) -> RecorderStatus {
        self.stop_recording_with_path().0
    }

    pub fn set_replay_buffer(&mut self, enabled: bool, max_seconds: u32) -> RecorderStatus {
        self.replay_max_seconds = max_seconds.clamp(10, 180);
        self.replay_active = enabled;
        if enabled {
            if self.replay_started.is_none() {
                self.replay_started = Some(Instant::now());
            }
            self.prune_replay(self.current_replay_elapsed_ms());
        } else {
            self.replay_started = None;
            self.replay_chunks.clear();
            self.replay_chunks.shrink_to_fit();
            self.replay_bytes_used = 0;
            // Só descarta init_segment se nenhuma gravação estiver dependendo dele.
            if self.recording_writer.is_none() {
                self.init_segment = None;
            }
        }
        self.status()
    }

    fn current_replay_elapsed_ms(&self) -> u64 {
        self.replay_started
            .map(|s| s.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }

    pub fn push_media_chunk(&mut self, bytes: &[u8]) {
        self.push_media_chunk_at(bytes, None);
    }

    /// Recebe um chunk WebM. `override_elapsed_ms` permite timestamps determinísticos em testes.
    pub fn push_media_chunk_at(&mut self, bytes: &[u8], override_elapsed_ms: Option<u64>) {
        if bytes.is_empty() {
            return;
        }

        if let Some(writer) = self.recording_writer.as_mut() {
            if let Err(e) = writer.write_all(bytes) {
                self.last_error = Some(format!("Erro de escrita no disco: {e}"));
                let _ = self.recording_writer.take();
                self.recording_started = None;
            }
        }

        if self.replay_active {
            if self.init_segment.is_none() {
                self.init_segment = Some(bytes.to_vec());
                if self.replay_started.is_none() {
                    self.replay_started = Some(Instant::now());
                }
                return;
            }

            let elapsed_ms = override_elapsed_ms.unwrap_or_else(|| self.current_replay_elapsed_ms());
            let starts_cluster = starts_with_ebml_cluster(bytes);
            self.replay_bytes_used = self.replay_bytes_used.saturating_add(bytes.len());
            self.replay_chunks.push_back(ReplayChunk {
                timestamp_ms: elapsed_ms,
                starts_cluster,
                data: bytes.to_vec(),
            });

            self.prune_replay(elapsed_ms);
        }
    }

    /// Remove chunks expirados/acima do teto de RAM mantendo a cabeça alinhada em fronteira de EBML Cluster.
    fn prune_replay(&mut self, now_ms: u64) {
        let window_ms = (self.replay_max_seconds as u64) * 1000;

        // Passo 1: remove blocos expirados por tempo ou excesso de memória.
        let mut popped_any = false;
        while self.replay_chunks.len() > 1 {
            let front = match self.replay_chunks.front() {
                Some(f) => f,
                None => break,
            };
            let over_time = now_ms.saturating_sub(front.timestamp_ms) > window_ms;
            let over_mem = self.replay_bytes_used > self.max_replay_bytes;
            if !over_time && !over_mem {
                break;
            }
            if let Some(removed) = self.replay_chunks.pop_front() {
                self.replay_bytes_used = self.replay_bytes_used.saturating_sub(removed.data.len());
                popped_any = true;
            }
        }

        // Passo 2: se houve poda, avança até o próximo chunk que inicia um EBML Cluster
        // (desde que exista algum Cluster restante no buffer) para garantir WebM válido.
        if popped_any && self.replay_chunks.iter().any(|c| c.starts_cluster) {
            while let Some(front) = self.replay_chunks.front() {
                if front.starts_cluster || self.replay_chunks.len() <= 1 {
                    break;
                }
                if let Some(removed) = self.replay_chunks.pop_front() {
                    self.replay_bytes_used =
                        self.replay_bytes_used.saturating_sub(removed.data.len());
                }
            }
        }
    }

    /// Salva o replay buffer no disco de forma atômica via `BufWriter`.
    pub fn save_replay(&mut self) -> Result<String, String> {
        let (path, _) = self.save_replay_webm()?;
        self.last_saved_replay = Some(path.clone());
        Ok(path.to_string_lossy().to_string())
    }

    /// Grava o `.webm` de replay e retorna `(path_webm, status_atual)` para remux assíncrono opcional.
    pub fn save_replay_webm(&mut self) -> Result<(PathBuf, RecorderStatus), String> {
        let Some(ref init) = self.init_segment else {
            return Err(
                "O Replay Buffer ainda está aguardando os primeiros quadros da transmissão."
                    .to_string(),
            );
        };
        fs::create_dir_all(&self.output_dir).map_err(|e| format!("Falha ao criar pasta: {e}"))?;
        let path = self.output_dir.join(format!(
            "PrismCast_Replay_{}s_{}.webm",
            self.replay_max_seconds,
            unique_timestamp_stamp()
        ));

        let file = File::create(&path).map_err(|e| format!("Falha ao criar replay: {e}"))?;
        let mut writer = BufWriter::with_capacity(256 * 1024, file);
        writer
            .write_all(init)
            .map_err(|e| format!("Erro ao gravar cabeçalho do replay: {e}"))?;
        for chunk in &self.replay_chunks {
            writer
                .write_all(&chunk.data)
                .map_err(|e| format!("Erro ao gravar replay: {e}"))?;
        }
        writer
            .flush()
            .map_err(|e| format!("Erro ao finalizar replay: {e}"))?;

        self.last_saved_replay = Some(path.clone());
        Ok((path, self.status()))
    }

    pub fn mark_saved_replay_path(&mut self, final_path: PathBuf) {
        self.last_saved_replay = Some(final_path);
    }

    /// Reinicia o stream do Replay Buffer sem destruir uma gravação em curso;
    /// só reinicializa `init_segment` se não houver gravação ativa escrevendo nesse stream.
    pub fn reset_stream(&mut self) {
        self.replay_chunks.clear();
        self.replay_chunks.shrink_to_fit();
        self.replay_bytes_used = 0;
        self.replay_started = if self.replay_active {
            Some(Instant::now())
        } else {
            None
        };
        if self.recording_writer.is_none() {
            self.init_segment = None;
        }
    }

    pub fn status(&self) -> RecorderStatus {
        let rec_sec = self
            .recording_started
            .map(|s| s.elapsed().as_secs())
            .unwrap_or(0);

        let buffered_sec = if self.replay_chunks.len() >= 2 {
            let first = self.replay_chunks.front().unwrap().timestamp_ms;
            let last = self.replay_chunks.back().unwrap().timestamp_ms;
            ((last.saturating_sub(first)) as f32 / 1000.0).min(self.replay_max_seconds as f32)
        } else if self.init_segment.is_some() {
            1.0
        } else {
            0.0
        };

        RecorderStatus {
            recording: self.recording_writer.is_some(),
            recordingPath: self
                .recording_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            recordingDurationSec: rec_sec,
            replayBufferActive: self.replay_active,
            replayBufferedSeconds: buffered_sec,
            replaySavedPath: self
                .last_saved_replay
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
        }
    }
}

impl Default for RecorderEngine {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedRecorderEngine = Arc<Mutex<RecorderEngine>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "prismcast_rec_test_{name}_{}",
            unique_timestamp_stamp()
        ));
        let _ = fs::create_dir_all(&p);
        p
    }

    #[test]
    fn test_ebml_cluster_alignment_and_save_replay() {
        let dir = temp_dir("cluster");
        let mut rec = RecorderEngine::new();
        rec.set_recording_dir(Some(dir.to_str().unwrap()));
        rec.set_replay_buffer(true, 10);

        let init = [0x1A, 0x45, 0xDF, 0xA3, 0x9F];
        let cluster1 = [0x1F, 0x43, 0xB6, 0x75, 0x11, 0x11];
        let cont1 = [0xAA, 0xBB, 0xCC, 0xDD];
        let cluster2 = [0x1F, 0x43, 0xB6, 0x75, 0x22, 0x22];
        let cont2 = [0xEE, 0xFF];

        rec.push_media_chunk_at(&init, Some(0));
        rec.push_media_chunk_at(&cluster1, Some(1000));
        rec.push_media_chunk_at(&cont1, Some(2000));
        rec.push_media_chunk_at(&cluster2, Some(5000));
        rec.push_media_chunk_at(&cont2, Some(12000)); // > 10s depois do cluster1: deve podar cluster1+cont1

        assert!(rec.replay_chunks.front().unwrap().starts_cluster);
        assert_eq!(rec.replay_chunks.front().unwrap().data, cluster2);

        let st = rec.status();
        assert!(st.replayBufferedSeconds >= 6.9);

        let saved = rec.save_replay().expect("save_replay ok");
        let bytes = fs::read(&saved).expect("replay file readable");
        assert!(bytes.starts_with(&init));
        assert!(bytes[init.len()..].starts_with(&cluster2));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ram_cap_and_shrink_on_disable() {
        let mut rec = RecorderEngine::new();
        rec.set_replay_buffer(true, 120);
        rec.max_replay_bytes = 256;

        rec.push_media_chunk_at(&[0x1A, 0x45, 0xDF, 0xA3], Some(0));
        for i in 1..=10u64 {
            let mut pkt = vec![0x1F, 0x43, 0xB6, 0x75];
            pkt.resize(100, i as u8);
            rec.push_media_chunk_at(&pkt, Some(i * 1000));
        }

        assert!(rec.replay_bytes_used <= 256);
        assert!(rec.status().replayBufferedSeconds >= 1.0);

        let st = rec.set_replay_buffer(false, 30);
        assert!(!st.replayBufferActive);
        assert_eq!(rec.replay_bytes_used, 0);
        assert!(rec.replay_chunks.is_empty());
    }

    #[test]
    fn test_unique_filenames_and_reset_preserves_active_recording() {
        let dir = temp_dir("unique");
        let mut rec = RecorderEngine::new();
        rec.set_recording_dir(Some(dir.to_str().unwrap()));

        let s1 = unique_timestamp_stamp();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let s2 = unique_timestamp_stamp();
        assert_ne!(s1, s2);

        rec.push_media_chunk_at(&[0x1A, 0x45, 0xDF, 0xA3], Some(0));
        let st = rec.start_recording().expect("start recording");
        assert!(st.recording);

        rec.reset_stream();
        // Gravação em andamento não deve perder init_segment nem parar
        assert!(rec.init_segment.is_some());
        assert!(rec.status().recording);

        let (stopped, path) = rec.stop_recording_with_path();
        assert!(!stopped.recording);
        assert!(path.is_some());

        let _ = fs::remove_dir_all(&dir);
    }
}
