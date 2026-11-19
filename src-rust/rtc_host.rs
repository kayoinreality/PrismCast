use crate::protocol::{IceCandidatePayload, QualitySettings, VideoCodec};
use std::collections::{HashMap, HashSet};

/// Injeta/sobrescreve parâmetros Opus estéreo + FEC + bitrate no `a=fmtp` de Offer e Answer (RFC 7587).
///
/// Correção da auditoria (#12): ao contrário da versão TS que ignorava chaves já existentes
/// (`stereo=0`), esta função **atualiza ou acrescenta** `stereo=1`, `sprop-stereo=1`,
/// `maxaveragebitrate=<bps>` e `useinbandfec=1`, garantindo áudio estéreo de alta qualidade
/// tanto na Offer do Host quanto na Answer do Viewer.
pub fn prefer_opus_stereo(sdp: &str, bitrate_bps: u32) -> String {
    let eol = if sdp.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = sdp.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();

    let mut opus_pt: Option<String> = None;
    let mut rtpmap_idx: Option<usize> = None;

    for (idx, line) in lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("a=rtpmap:") && lower.contains("opus/48000/2") {
            if let Some(rest) = line.strip_prefix("a=rtpmap:") {
                if let Some((pt, _)) = rest.split_once(' ') {
                    opus_pt = Some(pt.to_string());
                    rtpmap_idx = Some(idx);
                    break;
                }
            }
        }
    }

    let (Some(pt), Some(r_idx)) = (opus_pt, rtpmap_idx) else {
        return sdp.to_string();
    };

    let desired: [(&str, String); 4] = [
        ("stereo", "1".to_string()),
        ("sprop-stereo", "1".to_string()),
        ("maxaveragebitrate", bitrate_bps.max(32_000).to_string()),
        ("useinbandfec", "1".to_string()),
    ];

    let fmtp_prefix = format!("a=fmtp:{pt} ");
    if let Some(f_idx) = lines.iter().position(|l| l.starts_with(&fmtp_prefix)) {
        let existing = &lines[f_idx][fmtp_prefix.len()..];
        let mut pairs: Vec<(String, String)> = Vec::new();
        for part in existing.split(';') {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some((k, v)) = trimmed.split_once('=') {
                pairs.push((k.trim().to_string(), v.trim().to_string()));
            } else {
                pairs.push((trimmed.to_string(), String::new()));
            }
        }

        for (dk, dv) in &desired {
            if let Some((_, val)) = pairs.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(dk)) {
                *val = dv.clone();
            } else {
                pairs.push(((*dk).to_string(), dv.clone()));
            }
        }

        let formatted = pairs
            .into_iter()
            .map(|(k, v)| if v.is_empty() { k } else { format!("{k}={v}") })
            .collect::<Vec<_>>()
            .join(";");
        lines[f_idx] = format!("{fmtp_prefix}{formatted}");
    } else {
        let formatted = desired
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(";");
        lines.insert(r_idx + 1, format!("{fmtp_prefix}{formatted}"));
    }

    // RFC / WebRTC exige CRLF no SDP.
    let _ = eol;
    lines.join("\r\n")
}

/// Reordena os payload types da mídia `m=video` no SDP priorizando o codec escolhido,
/// dando preferência a perfis H.264 High (`64`) / Main (`4d`) sobre Baseline (`42`),
/// e mantendo cada `video/rtx` (`apt=<pt>`) pareado imediatamente após seu codec principal.
pub fn apply_codec_preference(sdp: &str, preferred_codec: &str) -> String {
    let target_codec = match preferred_codec.trim().to_ascii_lowercase().as_str() {
        "auto" | "h264" => "h264",
        "vp9" => "vp9",
        "av1" => "av1",
        "vp8" => "vp8",
        _ => return sdp.to_string(),
    };

    let mut lines: Vec<String> = sdp
        .split('\n')
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();

    let Some(m_video_idx) = lines.iter().position(|l| l.starts_with("m=video ")) else {
        return sdp.to_string();
    };
    let next_m_idx = lines
        .iter()
        .enumerate()
        .skip(m_video_idx + 1)
        .find(|(_, l)| l.starts_with("m="))
        .map(|(i, _)| i)
        .unwrap_or(lines.len());

    let m_parts: Vec<&str> = lines[m_video_idx].split_whitespace().collect();
    if m_parts.len() < 4 {
        return sdp.to_string();
    }
    let orig_pts: Vec<String> = m_parts[3..].iter().map(|s| (*s).to_string()).collect();

    // Mapeia pt -> codec_name, pt -> fmtp, e apt_pt -> rtx_pt
    let mut codec_by_pt: HashMap<String, String> = HashMap::new();
    let mut fmtp_by_pt: HashMap<String, String> = HashMap::new();
    let mut rtx_by_apt: HashMap<String, Vec<String>> = HashMap::new();
    let mut rtx_pts: HashSet<String> = HashSet::new();

    for line in &lines[m_video_idx + 1..next_m_idx] {
        if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            if let Some((pt, codec_part)) = rest.split_once(' ') {
                let cname = codec_part
                    .split('/')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase();
                codec_by_pt.insert(pt.to_string(), cname.clone());
                if cname == "rtx" {
                    rtx_pts.insert(pt.to_string());
                }
            }
        } else if let Some(rest) = line.strip_prefix("a=fmtp:") {
            if let Some((pt, params)) = rest.split_once(' ') {
                fmtp_by_pt.insert(pt.to_string(), params.to_string());
            }
        }
    }

    for (rtx_pt, params) in &fmtp_by_pt {
        if !rtx_pts.contains(rtx_pt) {
            continue;
        }
        for kv in params.split(';') {
            let trimmed = kv.trim();
            if let Some(apt) = trimmed.strip_prefix("apt=") {
                rtx_by_apt
                    .entry(apt.trim().to_string())
                    .or_default()
                    .push(rtx_pt.clone());
            }
        }
    }

    // Score para priorizar H.264 High/Main sobre Baseline
    let h264_rank = |pt: &str| -> u8 {
        let params = fmtp_by_pt
            .get(pt)
            .map(String::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        if let Some(pos) = params.find("profile-level-id=") {
            let id = &params[pos + "profile-level-id=".len()..];
            if id.starts_with("64") {
                return 0; // High
            }
            if id.starts_with("4d") {
                return 1; // Main
            }
            if id.starts_with("42") {
                return 2; // Baseline / Constrained Baseline
            }
        }
        3
    };

    let mut primary_preferred: Vec<String> = Vec::new();
    let mut primary_rest: Vec<String> = Vec::new();

    for pt in &orig_pts {
        if rtx_pts.contains(pt) {
            continue; // será reinserido logo após seu `apt`
        }
        let cname = codec_by_pt.get(pt).map(String::as_str).unwrap_or("");
        if cname == target_codec {
            primary_preferred.push(pt.clone());
        } else {
            primary_rest.push(pt.clone());
        }
    }

    if primary_preferred.is_empty() {
        return sdp.to_string();
    }

    if target_codec == "h264" {
        primary_preferred.sort_by_key(|pt| h264_rank(pt));
    }

    let mut ordered_pts: Vec<String> = Vec::with_capacity(orig_pts.len());
    let mut used: HashSet<String> = HashSet::new();

    for pt in primary_preferred.into_iter().chain(primary_rest.into_iter()) {
        if used.insert(pt.clone()) {
            ordered_pts.push(pt.clone());
        }
        if let Some(rtx_list) = rtx_by_apt.get(&pt) {
            for rtx in rtx_list {
                if orig_pts.contains(rtx) && used.insert(rtx.clone()) {
                    ordered_pts.push(rtx.clone());
                }
            }
        }
    }

    // Garante que qualquer PT residual não pareado não se perca.
    for pt in &orig_pts {
        if used.insert(pt.clone()) {
            ordered_pts.push(pt.clone());
        }
    }

    lines[m_video_idx] = format!(
        "{} {} {} {}",
        m_parts[0],
        m_parts[1],
        m_parts[2],
        ordered_pts.join(" ")
    );
    lines.join("\r\n")
}

/// Converte `VideoCodec` do `protocol.rs` para identificador SDP.
pub fn codec_name(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::Auto | VideoCodec::H264 => "h264",
        VideoCodec::Vp9 => "vp9",
        VideoCodec::Av1 => "av1",
    }
}

/// Amostra bruta para cálculo de estatísticas WebRTC por delta instantâneo.
#[derive(Debug, Clone, Default)]
pub struct RtpSampleInput {
    pub video_timestamp_ms: f64,
    pub audio_timestamp_ms: f64,
    pub video_bytes: u64,
    pub audio_bytes: u64,
    pub packets_sent_or_received: u64,
    pub packets_lost: i64,
    pub fraction_lost: Option<f32>,
    pub fps: f32,
    pub width: u32,
    pub height: u32,
    pub rtt_ms: Option<u32>,
    pub jitter_ms: Option<u32>,
    pub codec: String,
    pub frames_dropped: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ComputedStreamStats {
    pub bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub loss_pct: f32,
    pub rtt_ms: Option<u32>,
    pub jitter_ms: Option<u32>,
    pub codec: String,
    pub frames_dropped: u64,
}

/// Rastreador de estatísticas WebRTC por delta entre amostras.
///
/// Correções da auditoria (#14):
/// - `loss_pct` calculado pelo **delta** entre amostras (`delta_lost / delta_total`, onde
///   no remetente `packets_sent` já inclui os perdidos, evitando contar `lost` duas vezes!),
///   ou usando `fraction_lost * 100.0` do RTCP RR quando disponível.
/// - Timestamps de vídeo e áudio independentes, e não sobrescreve referência quando timestamp == 0.
#[derive(Debug, Clone, Default)]
pub struct StatsTracker {
    prev_video_ts_ms: Option<f64>,
    prev_audio_ts_ms: Option<f64>,
    prev_video_bytes: u64,
    prev_audio_bytes: u64,
    prev_packets: u64,
    prev_lost: i64,
    has_packet_sample: bool,
}

impl StatsTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, sample: &RtpSampleInput, is_sender: bool) -> ComputedStreamStats {
        let mut out = ComputedStreamStats {
            fps: sample.fps.max(0.0).round() as u32,
            width: sample.width,
            height: sample.height,
            rtt_ms: sample.rtt_ms,
            jitter_ms: sample.jitter_ms,
            codec: if sample.codec.is_empty() {
                "—".to_string()
            } else {
                sample.codec.clone()
            },
            frames_dropped: sample.frames_dropped,
            ..Default::default()
        };

        if sample.video_timestamp_ms > 0.0 {
            if let Some(prev_ts) = self.prev_video_ts_ms {
                if sample.video_timestamp_ms > prev_ts {
                    let secs = (sample.video_timestamp_ms - prev_ts) / 1000.0;
                    let delta_bytes = sample.video_bytes.saturating_sub(self.prev_video_bytes);
                    out.bitrate_kbps = ((delta_bytes as f64 * 8.0) / secs / 1000.0)
                        .round()
                        .max(0.0) as u32;
                }
            }
            self.prev_video_ts_ms = Some(sample.video_timestamp_ms);
            self.prev_video_bytes = sample.video_bytes;
        }

        if sample.audio_timestamp_ms > 0.0 {
            if let Some(prev_ts) = self.prev_audio_ts_ms {
                if sample.audio_timestamp_ms > prev_ts {
                    let secs = (sample.audio_timestamp_ms - prev_ts) / 1000.0;
                    let delta_bytes = sample.audio_bytes.saturating_sub(self.prev_audio_bytes);
                    out.audio_bitrate_kbps = ((delta_bytes as f64 * 8.0) / secs / 1000.0)
                        .round()
                        .max(0.0) as u32;
                }
            }
            self.prev_audio_ts_ms = Some(sample.audio_timestamp_ms);
            self.prev_audio_bytes = sample.audio_bytes;
        }

        if let Some(frac) = sample.fraction_lost {
            out.loss_pct = (frac * 100.0).clamp(0.0, 100.0);
        } else if self.has_packet_sample {
            let delta_pkts = sample.packets_sent_or_received.saturating_sub(self.prev_packets);
            let delta_lost = (sample.packets_lost - self.prev_lost).max(0) as u64;
            let denom = if is_sender {
                // No remetente, `packetsSent` já inclui todos os pacotes enviados (inclusive perdidos).
                delta_pkts.max(delta_lost)
            } else {
                // No receptor, pacotes esperados no intervalo = recebidos + perdidos.
                delta_pkts.saturating_add(delta_lost)
            };
            if denom > 0 {
                out.loss_pct = ((delta_lost as f64 / denom as f64) * 100.0).clamp(0.0, 100.0) as f32;
            }
        }

        self.prev_packets = sample.packets_sent_or_received;
        self.prev_lost = sample.packets_lost;
        self.has_packet_sample = true;

        out
    }
}

/// Estado ICE / Peer WebRTC no Host LAN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtcPeerConnectionState {
    Connecting,
    Connected,
    Disconnected,
    Failed,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IceRestartAction {
    None,
    RestartIce { viewer_id: String },
}

#[derive(Debug, Clone)]
pub struct RtcPeerSlot {
    pub viewer_id: String,
    pub state: RtcPeerConnectionState,
    pub signaling_stable: bool,
    pub pending_renegotiation: bool,
    pub remote_description_set: bool,
    pub pending_ice: Vec<IceCandidatePayload>,
    pub voice_remote_description_set: bool,
    pub pending_voice_ice: Vec<IceCandidatePayload>,
    pub disconnected_since_ms: Option<u64>,
    pub ice_restart_attempts: u32,
    pub stats: StatsTracker,
}

impl RtcPeerSlot {
    pub fn new(viewer_id: String) -> Self {
        Self {
            viewer_id,
            state: RtcPeerConnectionState::Connecting,
            signaling_stable: true,
            pending_renegotiation: false,
            remote_description_set: false,
            pending_ice: Vec::new(),
            voice_remote_description_set: false,
            pending_voice_ice: Vec::new(),
            disconnected_since_ms: None,
            ice_restart_attempts: 0,
            stats: StatsTracker::new(),
        }
    }
}

/// Máquina de estados do Host WebRTC LAN em Rust (`HostSession` / `sdp` / `stats`).
///
/// Correções da auditoria (#12, #13, #14, #15):
/// - `party_voice_enabled` sincronizado com `AudioMixerSettings` (não fica preso em `false`).
/// - Fila `pending_voice_ice` para ambos Host e Viewer quando candidatos chegam antes do SDP remoto.
/// - Guarda `signaling_stable` / `pending_renegotiation` contra glare em troca rápida de codec.
/// - ICE Restart automático após 3s em `Disconnected` ou imediato em `Failed` (com debounce/limite).
pub struct RtcHostState {
    peers: HashMap<String, RtcPeerSlot>,
    settings: QualitySettings,
    party_voice_enabled: bool,
    ice_restart_delay_ms: u64,
}

impl RtcHostState {
    pub fn new(settings: QualitySettings, party_voice_enabled: bool) -> Self {
        Self {
            peers: HashMap::new(),
            settings,
            party_voice_enabled,
            ice_restart_delay_ms: 3000,
        }
    }

    pub fn set_party_voice_enabled(&mut self, enabled: bool) {
        self.party_voice_enabled = enabled;
        if !enabled {
            for peer in self.peers.values_mut() {
                peer.voice_remote_description_set = false;
                peer.pending_voice_ice.clear();
            }
        }
    }

    pub fn party_voice_enabled(&self) -> bool {
        self.party_voice_enabled
    }

    pub fn add_peer(&mut self, viewer_id: String) {
        self.peers
            .insert(viewer_id.clone(), RtcPeerSlot::new(viewer_id));
    }

    pub fn remove_peer(&mut self, viewer_id: &str) -> Option<RtcPeerSlot> {
        self.peers.remove(viewer_id)
    }

    pub fn get_peer(&self, viewer_id: &str) -> Option<&RtcPeerSlot> {
        self.peers.get(viewer_id)
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Inicia uma oferta SDP apenas se o canal de sinalização estiver estável;
    /// caso contrário, agenda `pending_renegotiation` evitando glare.
    pub fn begin_offer(&mut self, viewer_id: &str) -> bool {
        let Some(peer) = self.peers.get_mut(viewer_id) else {
            return false;
        };
        if !peer.signaling_stable {
            peer.pending_renegotiation = true;
            return false;
        }
        peer.signaling_stable = false;
        peer.pending_renegotiation = false;
        true
    }

    /// Registra Answer remota, libera ICE pendentes e retorna se há renegociação enfileirada.
    pub fn on_remote_answer(&mut self, viewer_id: &str) -> (Vec<IceCandidatePayload>, bool) {
        let Some(peer) = self.peers.get_mut(viewer_id) else {
            return (Vec::new(), false);
        };
        peer.remote_description_set = true;
        peer.signaling_stable = true;
        let drained = std::mem::take(&mut peer.pending_ice);
        let need_renegotiate = peer.pending_renegotiation;
        peer.pending_renegotiation = false;
        (drained, need_renegotiate)
    }

    /// Enfileira ou retorna imediatamente candidato ICE de vídeo/áudio principal.
    pub fn on_ice_candidate(
        &mut self,
        viewer_id: &str,
        candidate: IceCandidatePayload,
    ) -> Option<IceCandidatePayload> {
        let peer = self.peers.get_mut(viewer_id)?;
        if peer.remote_description_set {
            Some(candidate)
        } else {
            peer.pending_ice.push(candidate);
            None
        }
    }

    /// Ao receber `voice-offer` (quando `party_voice_enabled == true`), marca `voice_remote_description_set`
    /// e drena candidatos `voice-ice` que chegaram antes do SDP!
    pub fn on_voice_offer(&mut self, viewer_id: &str) -> Option<Vec<IceCandidatePayload>> {
        if !self.party_voice_enabled {
            return None;
        }
        let peer = self.peers.get_mut(viewer_id)?;
        peer.voice_remote_description_set = true;
        Some(std::mem::take(&mut peer.pending_voice_ice))
    }

    /// Enfileira `voice-ice` se `voice_remote_description_set == false` em vez de descartar!
    pub fn on_voice_ice_candidate(
        &mut self,
        viewer_id: &str,
        candidate: IceCandidatePayload,
    ) -> Option<IceCandidatePayload> {
        if !self.party_voice_enabled {
            return None;
        }
        let peer = self.peers.get_mut(viewer_id)?;
        if peer.voice_remote_description_set {
            Some(candidate)
        } else {
            peer.pending_voice_ice.push(candidate);
            None
        }
    }

    pub fn set_connection_state(
        &mut self,
        viewer_id: &str,
        state: RtcPeerConnectionState,
        now_ms: u64,
    ) {
        let Some(peer) = self.peers.get_mut(viewer_id) else {
            return;
        };
        peer.state = state;
        match state {
            RtcPeerConnectionState::Connected => {
                peer.disconnected_since_ms = None;
                peer.ice_restart_attempts = 0;
            }
            RtcPeerConnectionState::Disconnected | RtcPeerConnectionState::Failed => {
                if peer.disconnected_since_ms.is_none() {
                    peer.disconnected_since_ms = Some(now_ms);
                }
            }
            RtcPeerConnectionState::Connecting | RtcPeerConnectionState::Closed => {}
        }
    }

    /// Verifica quais peers em `Disconnected` (> 3s) ou `Failed` precisam de `iceRestart: true`.
    pub fn poll_ice_restarts(&mut self, now_ms: u64) -> Vec<IceRestartAction> {
        let mut actions = Vec::new();
        for peer in self.peers.values_mut() {
            if peer.ice_restart_attempts >= 3 {
                continue;
            }
            let trigger = match peer.state {
                RtcPeerConnectionState::Failed => true,
                RtcPeerConnectionState::Disconnected => peer
                    .disconnected_since_ms
                    .map(|since| now_ms.saturating_sub(since) >= self.ice_restart_delay_ms)
                    .unwrap_or(false),
                _ => false,
            };
            if trigger {
                peer.ice_restart_attempts += 1;
                peer.disconnected_since_ms = Some(now_ms);
                actions.push(IceRestartAction::RestartIce {
                    viewer_id: peer.viewer_id.clone(),
                });
            }
        }
        actions
    }

    /// Atualiza configurações de qualidade e retorna se houve troca de codec (exigindo renegociação).
    pub fn update_settings(&mut self, next: QualitySettings) -> bool {
        let codec_changed = next.codec != self.settings.codec;
        self.settings = next;
        codec_changed
    }

    pub fn settings(&self) -> &QualitySettings {
        &self.settings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prefer_opus_stereo_overwrites_existing_fmtp_keys() {
        let sdp = [
            "v=0",
            "m=audio 9 UDP/TLS/RTP/SAVPF 111",
            "a=rtpmap:111 opus/48000/2",
            "a=fmtp:111 minptime=10;stereo=0;maxaveragebitrate=64000",
        ]
        .join("\r\n");

        let patched = prefer_opus_stereo(&sdp, 192_000);
        assert!(patched.contains("stereo=1"));
        assert!(!patched.contains("stereo=0"));
        assert!(patched.contains("sprop-stereo=1"));
        assert!(patched.contains("maxaveragebitrate=192000"));
        assert!(!patched.contains("maxaveragebitrate=64000"));
        assert!(patched.contains("useinbandfec=1"));
        assert!(patched.contains("\r\n"));
    }

    #[test]
    fn test_apply_codec_preference_h264_high_first_and_rtx_paired() {
        let sdp = [
            "v=0",
            "m=video 9 UDP/TLS/RTP/SAVPF 96 97 98 99 100 101",
            "a=rtpmap:96 VP8/90000",
            "a=rtpmap:97 rtx/90000",
            "a=fmtp:97 apt=96",
            "a=rtpmap:98 H264/90000",
            "a=fmtp:98 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f",
            "a=rtpmap:99 rtx/90000",
            "a=fmtp:99 apt=98",
            "a=rtpmap:100 H264/90000",
            "a=fmtp:100 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=64001f",
            "a=rtpmap:101 rtx/90000",
            "a=fmtp:101 apt=100",
        ]
        .join("\r\n");

        let reordered = apply_codec_preference(&sdp, "h264");
        let m_line = reordered
            .lines()
            .find(|l| l.starts_with("m=video "))
            .unwrap();
        // Deve colocar 100 (H264 High) + 101 (rtx apt=100), depois 98 (Baseline) + 99 (rtx apt=98), depois 96 + 97
        assert_eq!(m_line, "m=video 9 UDP/TLS/RTP/SAVPF 100 101 98 99 96 97");
    }

    #[test]
    fn test_stats_tracker_delta_loss_and_separate_timestamps() {
        let mut tracker = StatsTracker::new();

        // Amostra 1: 1000 pacotes enviados, 0 perdidos
        let s1 = tracker.update(
            &RtpSampleInput {
                video_timestamp_ms: 1000.0,
                audio_timestamp_ms: 1000.0,
                video_bytes: 100_000,
                audio_bytes: 10_000,
                packets_sent_or_received: 1000,
                packets_lost: 0,
                fps: 60.0,
                width: 1920,
                height: 1080,
                codec: "H264".to_string(),
                ..Default::default()
            },
            true,
        );
        assert_eq!(s1.loss_pct, 0.0);

        // Amostra 2 (1s depois): +100 pacotes enviados, +50 perdidos -> no remetente é 50/100 = 50% exatos!
        let s2 = tracker.update(
            &RtpSampleInput {
                video_timestamp_ms: 2000.0,
                audio_timestamp_ms: 2000.0,
                video_bytes: 225_000, // +125 KB/s = 1000 kbps
                audio_bytes: 26_000,  // +16 KB/s = 128 kbps
                packets_sent_or_received: 1100,
                packets_lost: 50,
                fps: 60.0,
                width: 1920,
                height: 1080,
                codec: "H264".to_string(),
                ..Default::default()
            },
            true,
        );
        assert!((s2.loss_pct - 50.0).abs() < 0.01);
        assert_eq!(s2.bitrate_kbps, 1000);
        assert_eq!(s2.audio_bitrate_kbps, 128);

        // Amostra 3: janela limpa (+100 pacotes, 0 novos perdidos) -> perda volta para 0% em vez de ficar presa!
        let s3 = tracker.update(
            &RtpSampleInput {
                video_timestamp_ms: 3000.0,
                audio_timestamp_ms: 0.0, // áudio sem timestamp não zera referência
                video_bytes: 350_000,
                audio_bytes: 26_000,
                packets_sent_or_received: 1200,
                packets_lost: 50,
                ..Default::default()
            },
            true,
        );
        assert_eq!(s3.loss_pct, 0.0);
        assert_eq!(s3.bitrate_kbps, 1000);
    }

    #[test]
    fn test_pending_voice_ice_glare_guard_and_auto_ice_restart() {
        let mut host = RtcHostState::new(QualitySettings::default(), true);
        host.add_peer("v1".to_string());

        // voice-ice chega antes do voice-offer -> deve enfileirar, não descartar!
        let cand = IceCandidatePayload {
            candidate: "candidate:1 1 udp 2122260223 192.168.0.10 50000 typ host".to_string(),
            sdp_mid: Some("0".to_string()),
            sdp_m_line_index: Some(0),
            username_fragment: None,
        };
        assert!(host.on_voice_ice_candidate("v1", cand.clone()).is_none());
        let drained = host.on_voice_offer("v1").expect("party voice enabled");
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].candidate, cand.candidate);

        // Troca rápida de codec durante offer pendente usa pending_renegotiation
        assert!(host.begin_offer("v1"));
        assert!(!host.begin_offer("v1"));
        let (_, need_renegotiate) = host.on_remote_answer("v1");
        assert!(need_renegotiate);

        // ICE Restart após 3000ms em Disconnected
        host.set_connection_state("v1", RtcPeerConnectionState::Disconnected, 1000);
        assert!(host.poll_ice_restarts(2500).is_empty());
        let restarts = host.poll_ice_restarts(4100);
        assert_eq!(
            restarts,
            vec![IceRestartAction::RestartIce {
                viewer_id: "v1".to_string()
            }]
        );
    }
}
