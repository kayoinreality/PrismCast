use crate::protocol::RecorderStatus;
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct ReplayChunk {
    timestamp_ms: u64,
    data: Vec<u8>,
}

/// Gravador local + Replay Buffer circular em Rust.
///
/// Inspirado no `SimpleReplayBuffer` e `RecordingModeService` do Streamlabs Desktop:
/// - Preserva o header inicial do container WebM (`init_segment`) para que qualquer
///   recorte do buffer circular gere um arquivo `.webm` integro e reproduzivel.
/// - Permite gravacao continua em disco e salvamento instantaneo dos ultimos N segundos.
pub struct RecorderEngine {
    output_dir: PathBuf,
    recording_file: Option<File>,
    recording_path: Option<PathBuf>,
    recording_started: Option<Instant>,

    replay_active: bool,
    replay_max_seconds: u32,
    replay_started: Option<Instant>,
    init_segment: Option<Vec<u8>>,
    replay_chunks: VecDeque<ReplayChunk>,
    last_saved_replay: Option<PathBuf>,
}

impl RecorderEngine {
    pub fn new() -> Self {
        let output_dir = dirs::video_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("PrismCast");
        let _ = fs::create_dir_all(&output_dir);

        Self {
            output_dir,
            recording_file: None,
            recording_path: None,
            recording_started: None,
            replay_active: false,
            replay_max_seconds: 30,
            replay_started: None,
            init_segment: None,
            replay_chunks: VecDeque::new(),
            last_saved_replay: None,
        }
    }

    pub fn start_recording(&mut self) -> Result<RecorderStatus, String> {
        let _ = fs::create_dir_all(&self.output_dir);
        let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        let path = self.output_dir.join(format!("PrismCast_Live_{}.webm", stamp));
        let mut file = File::create(&path).map_err(|e| e.to_string())?;

        // Se ja temos o init segment do stream, escreve no inicio do arquivo
        if let Some(ref init) = self.init_segment {
            let _ = file.write_all(init);
        }

        self.recording_file = Some(file);
        self.recording_path = Some(path);
        self.recording_started = Some(Instant::now());
        Ok(self.status())
    }

    pub fn stop_recording(&mut self) -> RecorderStatus {
        if let Some(mut f) = self.recording_file.take() {
            let _ = f.flush();
        }
        self.recording_started = None;
        self.status()
    }

    pub fn set_replay_buffer(&mut self, enabled: bool, max_seconds: u32) -> RecorderStatus {
        self.replay_max_seconds = max_seconds.clamp(10, 180);
        self.replay_active = enabled;
        if enabled {
            if self.replay_started.is_none() {
                self.replay_started = Some(Instant::now());
            }
        } else {
            self.replay_started = None;
            self.replay_chunks.clear();
            self.init_segment = None;
        }
        self.status()
    }

    pub fn push_media_chunk(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        if let Some(file) = self.recording_file.as_mut() {
            let _ = file.write_all(bytes);
        }

        if self.replay_active {
            if self.init_segment.is_none() {
                // O primeiro bloco emitido pelo MediaRecorder contem o EBML Header + Segment Info + Tracks
                self.init_segment = Some(bytes.to_vec());
                return;
            }

            let elapsed_ms = self
                .replay_started
                .map(|s| s.elapsed().as_millis() as u64)
                .unwrap_or(0);

            self.replay_chunks.push_back(ReplayChunk {
                timestamp_ms: elapsed_ms,
                data: bytes.to_vec(),
            });

            let window_ms = (self.replay_max_seconds as u64) * 1000;
            while let Some(front) = self.replay_chunks.front() {
                if elapsed_ms.saturating_sub(front.timestamp_ms) > window_ms {
                    self.replay_chunks.pop_front();
                } else {
                    break;
                }
            }
        }
    }

    pub fn save_replay(&mut self) -> Result<String, String> {
        let Some(ref init) = self.init_segment else {
            return Err("O Replay Buffer ainda está aguardando os primeiros quadros da transmissão.".to_string());
        };
        let _ = fs::create_dir_all(&self.output_dir);
        let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        let path = self
            .output_dir
            .join(format!("PrismCast_Replay_{}s_{}.webm", self.replay_max_seconds, stamp));

        let mut file = File::create(&path).map_err(|e| e.to_string())?;
        file.write_all(init).map_err(|e| e.to_string())?;
        for chunk in &self.replay_chunks {
            file.write_all(&chunk.data).map_err(|e| e.to_string())?;
        }
        file.flush().map_err(|e| e.to_string())?;

        let saved_str = path.to_string_lossy().to_string();
        self.last_saved_replay = Some(path);
        Ok(saved_str)
    }

    pub fn status(&self) -> RecorderStatus {
        let rec_sec = self
            .recording_started
            .map(|s| s.elapsed().as_secs())
            .unwrap_or(0);

        let buffered_sec = if let (Some(first), Some(last)) =
            (self.replay_chunks.front(), self.replay_chunks.back())
        {
            ((last.timestamp_ms.saturating_sub(first.timestamp_ms)) as f32 / 1000.0)
                .min(self.replay_max_seconds as f32)
        } else if self.init_segment.is_some() {
            1.0
        } else {
            0.0
        };

        RecorderStatus {
            recording: self.recording_file.is_some(),
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

pub type SharedRecorderEngine = Arc<Mutex<RecorderEngine>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replay_ring_buffer() {
        let mut rec = RecorderEngine::new();
        rec.set_replay_buffer(true, 30);
        rec.push_media_chunk(&[0x1A, 0x45, 0xDF, 0xA3]); // EBML header
        rec.push_media_chunk(&[0x01, 0x02, 0x03, 0x04]);
        let st = rec.status();
        assert!(st.replayBufferActive);
        assert!(st.replayBufferedSeconds >= 0.0);
    }
}
