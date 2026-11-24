//! Publicador RTMP / RTMPS persistente em Rust puro + demuxer FLV incremental.
//!
//! Permite **Dynamic Bitrate (ABR) com Zero-Disconnect**:
//! - Mantém a conexão TCP/TLS + sessão RTMP (`connect` + `publish`) aberta continuamente com o
//!   servidor de ingest (Twitch, YouTube, Kick, TikTok ou RTMP custom).
//! - Consome a saída `-f flv pipe:1` do processo FFmpeg local via `FlvDemuxer`, respeitando
//!   fronteiras arbitrárias de leitura no pipe e identificando headers de sequência AVC/AAC e
//!   keyframes IDR.
//! - Rebaseia timestamps via `TimestampRebaser` a cada hot-swap do encoder local para que o
//!   servidor remoto receba uma linha do tempo estritamente contínua sem desconexão.
//! - Mede a duração real de escrita (`Duration`) no socket TCP/TLS em `send_flv_tag`, fornecendo
//!   sinal autêntico de congestionamento de rede para `DynamicBitrateController`.

use bytes::{Buf, Bytes, BytesMut};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::rml_amf0::{self, Amf0Value};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult,
    PublishRequestType, StreamMetadata,
};
use rml_rtmp::time::RtmpTimestamp;
use std::io::Cursor;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::rustls;
use tokio_rustls::TlsConnector;
use url::Url;

pub const FLV_TAG_AUDIO: u8 = 8;
pub const FLV_TAG_VIDEO: u8 = 9;
pub const FLV_TAG_SCRIPT_DATA: u8 = 18;

const FLV_HEADER_MIN_LEN: usize = 9;
const FLV_PREV_TAG_SIZE_LEN: usize = 4;
const FLV_TAG_HEADER_LEN: usize = 11;
const MAX_FLV_HEADER_OFFSET: usize = 65_536;
const MAX_FLV_TAG_DATA_LEN: usize = 32 * 1024 * 1024;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(12);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const INBOUND_POLL_INTERVAL_TAGS: u64 = 16;

/// Endpoint RTMP/RTMPS decomposto e pronto para conexão TCP/TLS + sessão RTMP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtmpEndpoint {
    pub use_tls: bool,
    pub host: String,
    pub port: u16,
    pub app: String,
    pub stream_key: String,
    pub tc_url: String,
}

/// Faz o parsing de uma URL RTMP/RTMPS completa (`rtmp://host[:port]/app/stream_key[?query]`).
pub fn parse_rtmp_url(raw_url: &str) -> Result<RtmpEndpoint, String> {
    let trimmed = raw_url.trim();
    let parsed = Url::parse(trimmed).map_err(|e| format!("URL RTMP inválida: {e}"))?;

    let scheme = parsed.scheme().to_ascii_lowercase();
    let (use_tls, default_port) = match scheme.as_str() {
        "rtmp" => (false, 1935u16),
        "rtmps" => (true, 443u16),
        other => {
            return Err(format!(
                "Protocolo não suportado ({other}): use rtmp:// ou rtmps://"
            ));
        }
    };

    let host = parsed
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| "Host RTMP ausente na URL.".to_string())?
        .to_string();
    let port = parsed.port().unwrap_or(default_port);

    let path = parsed.path().trim_start_matches('/');
    let (app_raw, key_raw) = path
        .split_once('/')
        .ok_or_else(|| "URL RTMP deve conter /<app>/<stream_key>.".to_string())?;

    let app = app_raw.trim().to_string();
    let key_base = key_raw.trim_start_matches('/').trim();
    if app.is_empty() || key_base.is_empty() {
        return Err("App ou stream key vazio na URL RTMP.".to_string());
    }

    let stream_key = match parsed.query().filter(|q| !q.is_empty()) {
        Some(q) => format!("{key_base}?{q}"),
        None => key_base.to_string(),
    };

    let tc_url = if parsed.port().is_some() && port != default_port {
        format!("{scheme}://{host}:{port}/{app}")
    } else {
        format!("{scheme}://{host}/{app}")
    };

    Ok(RtmpEndpoint {
        use_tls,
        host,
        port,
        app,
        stream_key,
        tc_url,
    })
}

/// Faz o parsing quando servidor (`rtmp(s)://host[:port]/app...`) e `stream_key` já estão separados.
pub fn parse_rtmp_server_and_key(server: &str, stream_key: &str) -> Result<RtmpEndpoint, String> {
    let server_clean = server.trim().trim_end_matches('/');
    let key_clean = stream_key.trim().trim_start_matches('/');
    if key_clean.is_empty() {
        return Err("Chave de transmissão RTMP vazia.".to_string());
    }

    let parsed = Url::parse(server_clean).map_err(|e| format!("URL do servidor RTMP inválida: {e}"))?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    let (use_tls, default_port) = match scheme.as_str() {
        "rtmp" => (false, 1935u16),
        "rtmps" => (true, 443u16),
        other => {
            return Err(format!(
                "Protocolo não suportado ({other}): use rtmp:// ou rtmps://"
            ));
        }
    };

    let host = parsed
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| "Host RTMP ausente.".to_string())?
        .to_string();
    let port = parsed.port().unwrap_or(default_port);

    let app = parsed.path().trim_matches('/').to_string();
    if app.is_empty() {
        return Err("Caminho de aplicação RTMP (/app) ausente no servidor.".to_string());
    }

    let tc_url = if parsed.port().is_some() && port != default_port {
        format!("{scheme}://{host}:{port}/{app}")
    } else {
        format!("{scheme}://{host}/{app}")
    };

    Ok(RtmpEndpoint {
        use_tls,
        host,
        port,
        app,
        stream_key: key_clean.to_string(),
        tc_url,
    })
}

/// Tag FLV demuxada da saída `-f flv pipe:1` do FFmpeg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlvTag {
    pub tag_type: u8,
    pub timestamp_ms: u32,
    pub data: Vec<u8>,
    pub is_video_keyframe: bool,
    pub is_sequence_header: bool,
}

/// Demuxer FLV incremental tolerante a leituras fragmentadas em pipes/sockets.
///
/// Consome o cabeçalho FLV (9+ bytes) + `PreviousTagSize0` (4 bytes) e produz tags `FlvTag`
/// completos sem perder bytes entre chamadas parciais.
#[derive(Debug)]
pub struct FlvDemuxer {
    buf: BytesMut,
    header_parsed: bool,
    header_skip_total: Option<usize>,
}

impl FlvDemuxer {
    pub fn new() -> Self {
        Self {
            buf: BytesMut::with_capacity(64 * 1024),
            header_parsed: false,
            header_skip_total: None,
        }
    }

    /// Reseta o parser para consumir um novo stream FLV (ex.: após hot-swap do FFmpeg local).
    pub fn reset(&mut self) {
        self.buf.clear();
        self.header_parsed = false;
        self.header_skip_total = None;
    }

    /// Quantidade de bytes pendentes no buffer interno aguardando conclusão do próximo tag.
    pub fn buffered_len(&self) -> usize {
        self.buf.len()
    }

    /// Alimenta bytes brutos lidos de `pipe:1` e retorna todos os `FlvTag` completos disponíveis.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<FlvTag>, String> {
        if !bytes.is_empty() {
            self.buf.extend_from_slice(bytes);
        }

        if !self.header_parsed {
            let skip_total = match self.header_skip_total {
                Some(total) => total,
                None => {
                    if self.buf.len() < FLV_HEADER_MIN_LEN {
                        return Ok(Vec::new());
                    }
                    if &self.buf[0..3] != b"FLV" {
                        return Err("Assinatura FLV inválida no fluxo do encoder.".to_string());
                    }
                    let data_offset = u32::from_be_bytes([
                        self.buf[5],
                        self.buf[6],
                        self.buf[7],
                        self.buf[8],
                    ]) as usize;
                    if !(FLV_HEADER_MIN_LEN..=MAX_FLV_HEADER_OFFSET).contains(&data_offset) {
                        return Err(format!("DataOffset FLV inválido: {data_offset}"));
                    }
                    let total = data_offset + FLV_PREV_TAG_SIZE_LEN;
                    self.header_skip_total = Some(total);
                    total
                }
            };

            if self.buf.len() < skip_total {
                return Ok(Vec::new());
            }
            self.buf.advance(skip_total);
            self.header_parsed = true;
        }

        let mut tags = Vec::new();
        while self.buf.len() >= FLV_TAG_HEADER_LEN {
            let tag_type = self.buf[0] & 0x1F;
            let data_size = ((self.buf[1] as usize) << 16)
                | ((self.buf[2] as usize) << 8)
                | (self.buf[3] as usize);

            if data_size > MAX_FLV_TAG_DATA_LEN {
                return Err(format!("Tamanho de tag FLV excede o limite: {data_size} bytes"));
            }

            let total_tag_len = FLV_TAG_HEADER_LEN + data_size + FLV_PREV_TAG_SIZE_LEN;
            if self.buf.len() < total_tag_len {
                break;
            }

            // Timestamp FLV: 24 bits inferiores nos bytes 4..7 + 8 bits superiores no byte 7.
            let timestamp_ms = ((self.buf[7] as u32) << 24)
                | ((self.buf[4] as u32) << 16)
                | ((self.buf[5] as u32) << 8)
                | (self.buf[6] as u32);

            let data = self.buf[FLV_TAG_HEADER_LEN..FLV_TAG_HEADER_LEN + data_size].to_vec();
            self.buf.advance(total_tag_len);

            let (is_video_keyframe, is_sequence_header) = classify_flv_tag(tag_type, &data);

            tags.push(FlvTag {
                tag_type,
                timestamp_ms,
                data,
                is_video_keyframe,
                is_sequence_header,
            });
        }

        Ok(tags)
    }
}

impl Default for FlvDemuxer {
    fn default() -> Self {
        Self::new()
    }
}

fn classify_flv_tag(tag_type: u8, data: &[u8]) -> (bool, bool) {
    match tag_type {
        FLV_TAG_VIDEO => {
            if data.is_empty() {
                return (false, false);
            }
            let frame_type = (data[0] >> 4) & 0x0F;
            let codec_id = data[0] & 0x0F;
            let is_keyframe = frame_type == 1;
            // AVC (codec_id = 7) ou HEVC legado (codec_id = 12): packet_type == 0 indica Sequence Header.
            let is_seq_header =
                data.len() >= 2 && (codec_id == 7 || codec_id == 12) && data[1] == 0;
            (is_keyframe, is_seq_header)
        }
        FLV_TAG_AUDIO => {
            if data.len() < 2 {
                return (false, false);
            }
            let sound_format = (data[0] >> 4) & 0x0F;
            // AAC (sound_format = 10): AACPacketType == 0 indica AudioSpecificConfig (Sequence Header).
            let is_seq_header = sound_format == 10 && data[1] == 0;
            (false, is_seq_header)
        }
        _ => (false, false),
    }
}

/// Garante continuidade estrita de timestamps RTMP através de hot-swaps do encoder local.
///
/// Quando um novo processo FFmpeg é iniciado com novo bitrate, seus timestamps FLV reiniciam em
/// `t = 0`. O `TimestampRebaser` desloca o novo segmento a partir do último timestamp emitido +
/// passo mínimo, mantendo a linha do tempo monotônica no servidor RTMP sem desconexão.
#[derive(Debug, Clone)]
pub struct TimestampRebaser {
    segment_index: u32,
    segment_offset_ms: u64,
    segment_raw_origin: Option<u32>,
    last_emitted_ms: u64,
    has_emitted: bool,
    segment_gap_ms: u64,
}

impl TimestampRebaser {
    pub fn new() -> Self {
        Self::with_segment_gap_ms(1)
    }

    pub fn with_segment_gap_ms(gap_ms: u32) -> Self {
        Self {
            segment_index: 0,
            segment_offset_ms: 0,
            segment_raw_origin: None,
            last_emitted_ms: 0,
            has_emitted: false,
            segment_gap_ms: gap_ms.max(1) as u64,
        }
    }

    /// Sinaliza que um novo encoder local assumiu a produção de tags FLV.
    pub fn begin_new_segment(&mut self) {
        self.segment_index = self.segment_index.saturating_add(1);
        self.segment_raw_origin = None;
        if self.has_emitted {
            self.segment_offset_ms = self.last_emitted_ms.saturating_add(self.segment_gap_ms);
        }
    }

    /// Quantidade de hot-swaps já registrados desde o início da sessão persistente.
    pub fn segment_count(&self) -> u32 {
        self.segment_index
    }

    /// Último timestamp (em ms, truncado para u32 conforme especificação RTMP) emitido.
    pub fn last_timestamp_ms(&self) -> u32 {
        (self.last_emitted_ms & 0xFFFF_FFFF) as u32
    }

    /// Converte um timestamp bruto do encoder atual para a linha do tempo contínua da sessão RTMP.
    pub fn rebase(&mut self, raw_timestamp_ms: u32) -> u32 {
        let origin = *self.segment_raw_origin.get_or_insert(raw_timestamp_ms);
        let rel = raw_timestamp_ms.wrapping_sub(origin) as u64;
        // Protege contra um raw_timestamp ligeiramente menor que a origem (ex.: B-frame/DTS negativo)
        let safe_rel = if rel > 0x7FFF_FFFF { 0 } else { rel };

        let candidate = self.segment_offset_ms.saturating_add(safe_rel);
        let out = if self.has_emitted {
            candidate.max(self.last_emitted_ms)
        } else {
            candidate
        };

        self.last_emitted_ms = out;
        self.has_emitted = true;
        (out & 0xFFFF_FFFF) as u32
    }

    /// Rebaseia o campo `timestamp_ms` de um `FlvTag` in-place e retorna o novo timestamp.
    pub fn rebase_tag(&mut self, tag: &mut FlvTag) -> u32 {
        let rebased = self.rebase(tag.timestamp_ms);
        tag.timestamp_ms = rebased;
        rebased
    }
}

impl Default for TimestampRebaser {
    fn default() -> Self {
        Self::new()
    }
}

enum RtmpTransport {
    Tcp(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl RtmpTransport {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            RtmpTransport::Tcp(stream) => stream.read(buf).await,
            RtmpTransport::Tls(stream) => stream.read(buf).await,
        }
    }

    /// Lê bytes imediatamente disponíveis no socket/TLS sem bloquear (`Ok(None)` se `Poll::Pending`).
    async fn try_read_available(&mut self, buf: &mut [u8]) -> std::io::Result<Option<usize>> {
        std::future::poll_fn(|cx| {
            let mut read_buf = ReadBuf::new(buf);
            let poll = match self {
                RtmpTransport::Tcp(stream) => Pin::new(stream).poll_read(cx, &mut read_buf),
                RtmpTransport::Tls(stream) => Pin::new(stream.as_mut()).poll_read(cx, &mut read_buf),
            };
            match poll {
                Poll::Pending => Poll::Ready(Ok(None)),
                Poll::Ready(Ok(())) => Poll::Ready(Ok(Some(read_buf.filled().len()))),
                Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            }
        })
        .await
    }

    async fn write_all_and_flush(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self {
            RtmpTransport::Tcp(stream) => {
                stream.write_all(data).await?;
                stream.flush().await
            }
            RtmpTransport::Tls(stream) => {
                // `tokio-rustls` mantém buffer interno de registros TLS; `flush` garante envio ao TCP.
                stream.write_all(data).await?;
                stream.flush().await
            }
        }
    }

    async fn shutdown(&mut self) {
        let _ = match self {
            RtmpTransport::Tcp(stream) => stream.shutdown().await,
            RtmpTransport::Tls(stream) => stream.shutdown().await,
        };
    }
}

fn build_tls_connector() -> Result<TlsConnector, String> {
    let root_store = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };

    let provider = rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider()));

    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("Falha ao configurar versões TLS para RTMPS: {e}"))?
        .with_root_certificates(root_store)
        .with_no_client_auth();

    Ok(TlsConnector::from(Arc::new(config)))
}

/// Sessão RTMP/RTMPS persistente (TCP ou TLS + handshake + `connect` + `publish`).
///
/// Mantida viva através de trocas de bitrate do encoder local, fornecendo a latência exata de
/// escrita de cada tag FLV para o controlador ABR.
pub struct PersistentRtmpSession {
    endpoint: RtmpEndpoint,
    transport: RtmpTransport,
    session: ClientSession,
    bytes_sent: u64,
    tags_sent: u64,
    read_buf: Vec<u8>,
}

impl PersistentRtmpSession {
    /// Abre conexão RTMP/RTMPS completa a partir da URL completa (`rtmp(s)://host/app/key`).
    pub async fn connect(url: &str) -> Result<Self, String> {
        let endpoint = parse_rtmp_url(url)?;
        Self::connect_endpoint(endpoint).await
    }

    /// Abre conexão RTMP/RTMPS completa a partir de um `RtmpEndpoint` já validado.
    pub async fn connect_endpoint(endpoint: RtmpEndpoint) -> Result<Self, String> {
        let addr = format!("{}:{}", endpoint.host, endpoint.port);
        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr))
            .await
            .map_err(|_| format!("Timeout ao conectar TCP em {}:{}", endpoint.host, endpoint.port))?
            .map_err(|e| format!("Falha TCP ao conectar em {}:{}: {e}", endpoint.host, endpoint.port))?;

        let _ = tcp.set_nodelay(true);

        let mut transport = if endpoint.use_tls {
            let connector = build_tls_connector()?;
            let sni_host = endpoint
                .host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string();
            let server_name = rustls::pki_types::ServerName::try_from(sni_host)
                .map_err(|_| format!("Nome de host TLS inválido: {}", endpoint.host))?;
            let tls_stream = tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(server_name, tcp))
                .await
                .map_err(|_| format!("Timeout no handshake TLS com {}", endpoint.host))?
                .map_err(|e| format!("Falha no handshake TLS com {}: {e}", endpoint.host))?;
            RtmpTransport::Tls(Box::new(tls_stream))
        } else {
            RtmpTransport::Tcp(tcp)
        };

        let deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;
        let mut read_buf = vec![0u8; 8192];

        // 1. Handshake RTMP (C0/C1 <-> S0/S1/S2 <-> C2)
        let mut handshake = Handshake::new(PeerType::Client);
        let c0_c1 = handshake
            .generate_outbound_p0_and_p1()
            .map_err(|e| format!("Falha ao gerar C0/C1 RTMP: {e:?}"))?;
        transport
            .write_all_and_flush(&c0_c1)
            .await
            .map_err(|e| format!("Falha ao enviar C0/C1 RTMP: {e}"))?;

        let remaining_after_handshake = loop {
            let n = read_until_deadline(&mut transport, &mut read_buf, deadline, "handshake RTMP")
                .await?;
            match handshake
                .process_bytes(&read_buf[..n])
                .map_err(|e| format!("Handshake RTMP rejeitado: {e:?}"))?
            {
                HandshakeProcessResult::InProgress { response_bytes } => {
                    if !response_bytes.is_empty() {
                        transport
                            .write_all_and_flush(&response_bytes)
                            .await
                            .map_err(|e| format!("Falha ao responder handshake RTMP: {e}"))?;
                    }
                }
                HandshakeProcessResult::Completed {
                    response_bytes,
                    remaining_bytes,
                } => {
                    if !response_bytes.is_empty() {
                        transport
                            .write_all_and_flush(&response_bytes)
                            .await
                            .map_err(|e| format!("Falha ao enviar C2 RTMP: {e}"))?;
                    }
                    break remaining_bytes;
                }
            }
        };

        // 2. Cria `ClientSession` e envia `connect(app)`
        let mut config = ClientSessionConfig::new();
        config.chunk_size = 4096;
        config.tc_url = Some(endpoint.tc_url.clone());

        let (mut session, initial_results) =
            ClientSession::new(config).map_err(|e| format!("Falha ao iniciar ClientSession: {e:?}"))?;
        write_outbound_results(&mut transport, &initial_results).await?;

        if !remaining_after_handshake.is_empty() {
            let extra = session
                .handle_input(&remaining_after_handshake)
                .map_err(|e| format!("Erro ao processar bytes pós-handshake: {e:?}"))?;
            write_outbound_results(&mut transport, &extra).await?;
        }

        let connect_req = session
            .request_connection(endpoint.app.clone())
            .map_err(|e| format!("Falha ao montar comando RTMP connect: {e:?}"))?;
        write_outbound_results(&mut transport, &[connect_req]).await?;

        // Aguarda `ConnectionRequestAccepted` garantindo o envio de TODOS os pacotes de resposta
        // (inclusive `WindowAcknowledgement` e `SetChunkSize` que vêm no mesmo lote!).
        loop {
            let n = read_until_deadline(&mut transport, &mut read_buf, deadline, "RTMP connect")
                .await?;
            let results = session
                .handle_input(&read_buf[..n])
                .map_err(|e| format!("Erro RTMP durante connect: {e:?}"))?;

            let mut connected = false;
            let mut rejected: Option<String> = None;

            for res in results {
                match res {
                    ClientSessionResult::OutboundResponse(packet) => {
                        transport
                            .write_all_and_flush(&packet.bytes)
                            .await
                            .map_err(|e| format!("Falha ao enviar resposta de connect RTMP: {e}"))?;
                    }
                    ClientSessionResult::RaisedEvent(ClientSessionEvent::ConnectionRequestAccepted) => {
                        connected = true;
                    }
                    ClientSessionResult::RaisedEvent(
                        ClientSessionEvent::ConnectionRequestRejected { description },
                    ) => {
                        rejected = Some(description);
                    }
                    ClientSessionResult::RaisedEvent(_)
                    | ClientSessionResult::UnhandleableMessageReceived(_) => {}
                }
            }

            if let Some(desc) = rejected {
                return Err(format!("Conexão RTMP rejeitada pelo servidor: {desc}"));
            }
            if connected {
                break;
            }
        }

        // 3. Solicita `createStream` + `publish(stream_key, Live)`
        let pub_req = session
            .request_publishing(endpoint.stream_key.clone(), PublishRequestType::Live)
            .map_err(|e| format!("Falha ao montar comando RTMP publish: {e:?}"))?;
        write_outbound_results(&mut transport, &[pub_req]).await?;

        loop {
            let n = read_until_deadline(&mut transport, &mut read_buf, deadline, "RTMP publish")
                .await?;
            let results = session
                .handle_input(&read_buf[..n])
                .map_err(|e| format!("Erro RTMP durante publish: {e:?}"))?;

            let mut publishing = false;
            let mut status_err: Option<String> = None;

            for res in results {
                match res {
                    ClientSessionResult::OutboundResponse(packet) => {
                        transport
                            .write_all_and_flush(&packet.bytes)
                            .await
                            .map_err(|e| format!("Falha ao enviar comando publish RTMP: {e}"))?;
                    }
                    ClientSessionResult::RaisedEvent(ClientSessionEvent::PublishRequestAccepted) => {
                        publishing = true;
                    }
                    ClientSessionResult::RaisedEvent(
                        ClientSessionEvent::UnhandleableOnStatusCode { code },
                    ) => {
                        let lower = code.to_ascii_lowercase();
                        if lower.contains("badname")
                            || lower.contains("failed")
                            || lower.contains("rejected")
                            || lower.contains("unauthorized")
                            || lower.contains("error")
                        {
                            status_err = Some(code);
                        }
                    }
                    ClientSessionResult::RaisedEvent(_)
                    | ClientSessionResult::UnhandleableMessageReceived(_) => {}
                }
            }

            if let Some(code) = status_err {
                return Err(format!("Servidor RTMP rejeitou publish ({code})."));
            }
            if publishing {
                break;
            }
        }

        Ok(Self {
            endpoint,
            transport,
            session,
            bytes_sent: 0,
            tags_sent: 0,
            read_buf,
        })
    }

    pub fn endpoint(&self) -> &RtmpEndpoint {
        &self.endpoint
    }

    pub fn bytes_sent(&self) -> u64 {
        self.bytes_sent
    }

    pub fn tags_sent(&self) -> u64 {
        self.tags_sent
    }

    /// Processa sem bloquear mensagens de controle vindas do servidor (ex.: `PingRequest`, `WindowAck`,
    /// `SetChunkSize`) para manter a sessão persistente saudável em transmissões longas.
    pub async fn poll_server_messages(&mut self) -> Result<(), String> {
        loop {
            match self.transport.try_read_available(&mut self.read_buf).await {
                Ok(None) => return Ok(()),
                Ok(Some(0)) => {
                    return Err("Servidor RTMP encerrou a conexão (EOF).".to_string());
                }
                Ok(Some(n)) => {
                    let results = self
                        .session
                        .handle_input(&self.read_buf[..n])
                        .map_err(|e| format!("Erro ao processar mensagem RTMP do servidor: {e:?}"))?;
                    for res in results {
                        if let ClientSessionResult::OutboundResponse(packet) = res {
                            tokio::time::timeout(
                                WRITE_TIMEOUT,
                                self.transport.write_all_and_flush(&packet.bytes),
                            )
                            .await
                            .map_err(|_| "Timeout ao responder controle RTMP.".to_string())?
                            .map_err(|e| format!("Erro ao enviar controle RTMP: {e}"))?;
                        }
                    }
                }
                Err(e) => return Err(format!("Erro de leitura no socket RTMP: {e}")),
            }
        }
    }

    /// Envia um `FlvTag` (vídeo, áudio ou metadados `onMetaData`) sobre a sessão RTMP persistente
    /// e retorna a **duração exata medida** da escrita TCP/TLS como sinal real de congestionamento.
    pub async fn send_flv_tag(&mut self, tag: &FlvTag) -> Result<Duration, String> {
        if self.tags_sent % INBOUND_POLL_INTERVAL_TAGS == 0 {
            self.poll_server_messages().await?;
        }

        let outbound = match tag.tag_type {
            FLV_TAG_VIDEO => {
                if tag.data.is_empty() {
                    return Ok(Duration::ZERO);
                }
                let can_be_dropped = !tag.is_video_keyframe && !tag.is_sequence_header;
                self.session
                    .publish_video_data(
                        Bytes::copy_from_slice(&tag.data),
                        RtmpTimestamp::new(tag.timestamp_ms),
                        can_be_dropped,
                    )
                    .map_err(|e| format!("Falha ao serializar pacote de vídeo RTMP: {e:?}"))?
            }
            FLV_TAG_AUDIO => {
                if tag.data.is_empty() {
                    return Ok(Duration::ZERO);
                }
                let can_be_dropped = !tag.is_sequence_header;
                self.session
                    .publish_audio_data(
                        Bytes::copy_from_slice(&tag.data),
                        RtmpTimestamp::new(tag.timestamp_ms),
                        can_be_dropped,
                    )
                    .map_err(|e| format!("Falha ao serializar pacote de áudio RTMP: {e:?}"))?
            }
            FLV_TAG_SCRIPT_DATA => {
                let Some(meta) = parse_flv_script_metadata(&tag.data) else {
                    return Ok(Duration::ZERO);
                };
                self.session
                    .publish_metadata(&meta)
                    .map_err(|e| format!("Falha ao serializar metadados RTMP: {e:?}"))?
            }
            _ => return Ok(Duration::ZERO),
        };

        let ClientSessionResult::OutboundResponse(packet) = outbound else {
            return Ok(Duration::ZERO);
        };

        let start = Instant::now();
        tokio::time::timeout(WRITE_TIMEOUT, self.transport.write_all_and_flush(&packet.bytes))
            .await
            .map_err(|_| "Timeout ao enviar dados no socket RTMP.".to_string())?
            .map_err(|e| format!("Falha ao escrever no socket RTMP: {e}"))?;
        let elapsed = start.elapsed();

        self.bytes_sent = self.bytes_sent.saturating_add(packet.bytes.len() as u64);
        self.tags_sent = self.tags_sent.saturating_add(1);
        Ok(elapsed)
    }

    /// Encerra a publicação RTMP enviando `deleteStream` e fechando o socket graciosamente.
    pub async fn close(&mut self) {
        if let Ok(results) = self.session.stop_publishing() {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                write_outbound_results(&mut self.transport, &results),
            )
            .await;
        }
        self.transport.shutdown().await;
    }
}

async fn read_until_deadline(
    transport: &mut RtmpTransport,
    buf: &mut [u8],
    deadline: tokio::time::Instant,
    stage: &str,
) -> Result<usize, String> {
    let n = tokio::time::timeout_at(deadline, transport.read(buf))
        .await
        .map_err(|_| format!("Timeout durante {stage}."))?
        .map_err(|e| format!("Erro de I/O durante {stage}: {e}"))?;
    if n == 0 {
        return Err(format!("Servidor RTMP fechou a conexão durante {stage}."));
    }
    Ok(n)
}

async fn write_outbound_results(
    transport: &mut RtmpTransport,
    results: &[ClientSessionResult],
) -> Result<(), String> {
    for res in results {
        if let ClientSessionResult::OutboundResponse(packet) = res {
            transport
                .write_all_and_flush(&packet.bytes)
                .await
                .map_err(|e| format!("Erro ao escrever pacote RTMP: {e}"))?;
        }
    }
    Ok(())
}

fn parse_flv_script_metadata(data: &[u8]) -> Option<StreamMetadata> {
    let mut cursor = Cursor::new(data);
    let values = rml_amf0::deserialize(&mut cursor).ok()?;
    for val in values {
        if let Amf0Value::Object(props) = val {
            if props.is_empty() {
                continue;
            }
            let mut metadata = StreamMetadata::new();
            metadata.apply_metadata_values(props);
            return Some(metadata);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_flv_header() -> Vec<u8> {
        // "FLV", version=1, flags=audio|video (0x05), data_offset=9, PreviousTagSize0=0
        vec![b'F', b'L', b'V', 0x01, 0x05, 0, 0, 0, 9, 0, 0, 0, 0]
    }

    fn build_flv_tag_bytes(tag_type: u8, timestamp_ms: u32, payload: &[u8]) -> Vec<u8> {
        let len = payload.len();
        let mut out = Vec::with_capacity(11 + len + 4);
        out.push(tag_type);
        out.push(((len >> 16) & 0xFF) as u8);
        out.push(((len >> 8) & 0xFF) as u8);
        out.push((len & 0xFF) as u8);
        out.push(((timestamp_ms >> 16) & 0xFF) as u8);
        out.push(((timestamp_ms >> 8) & 0xFF) as u8);
        out.push((timestamp_ms & 0xFF) as u8);
        out.push(((timestamp_ms >> 24) & 0xFF) as u8); // TimestampExtended
        out.extend_from_slice(&[0, 0, 0]); // StreamID
        out.extend_from_slice(payload);
        let prev_size = (11 + len) as u32;
        out.extend_from_slice(&prev_size.to_be_bytes());
        out
    }

    #[test]
    fn test_flv_demuxer_odd_chunk_boundaries_and_avc_aac_flags() {
        let mut stream = build_flv_header();
        // 1. AVC Sequence Header (FrameType=1 keyframe, CodecID=7 AVC -> 0x17; AVCPacketType=0 -> seq header)
        let avc_seq = vec![0x17, 0x00, 0x00, 0x00, 0x00, 0x01, 0x64, 0x00, 0x2A];
        stream.extend(build_flv_tag_bytes(FLV_TAG_VIDEO, 0, &avc_seq));

        // 2. AAC Sequence Header (SoundFormat=10 AAC -> 0xAF; AACPacketType=0 -> seq header)
        let aac_seq = vec![0xAF, 0x00, 0x11, 0x90];
        stream.extend(build_flv_tag_bytes(FLV_TAG_AUDIO, 0, &aac_seq));

        // 3. AVC IDR Keyframe (0x17, AVCPacketType=1 NALU)
        let avc_idr = vec![0x17, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x65, 0x88];
        stream.extend(build_flv_tag_bytes(FLV_TAG_VIDEO, 33, &avc_idr));

        // 4. AVC Inter-frame (FrameType=2 -> 0x27, AVCPacketType=1) com timestamp estendido > 24 bits!
        let avc_inter = vec![0x27, 0x01, 0x00, 0x00, 0x00, 0x41, 0x9A];
        let ext_ts = 0x01_00_00_42u32; // > 16.777.215 ms (~4.6h de live)
        stream.extend(build_flv_tag_bytes(FLV_TAG_VIDEO, ext_ts, &avc_inter));

        // 5. AAC Raw Audio Frame (0xAF, AACPacketType=1)
        let aac_raw = vec![0xAF, 0x01, 0xDE, 0xAD, 0xBE, 0xEF];
        stream.extend(build_flv_tag_bytes(FLV_TAG_AUDIO, ext_ts + 21, &aac_raw));

        // Testa corte em fronteiras ímpares (1, 3, 7 bytes por vez)
        for chunk_size in [1usize, 3, 7, 13] {
            let mut demuxer = FlvDemuxer::new();
            let mut parsed = Vec::new();
            for slice in stream.chunks(chunk_size) {
                parsed.extend(demuxer.push(slice).expect("valid FLV chunk"));
            }
            assert_eq!(demuxer.buffered_len(), 0);
            assert_eq!(parsed.len(), 5, "chunk_size={chunk_size}");

            // Tag 0: AVC sequence header
            assert_eq!(parsed[0].tag_type, FLV_TAG_VIDEO);
            assert_eq!(parsed[0].timestamp_ms, 0);
            assert!(parsed[0].is_video_keyframe);
            assert!(parsed[0].is_sequence_header);
            assert_eq!(parsed[0].data, avc_seq);

            // Tag 1: AAC sequence header
            assert_eq!(parsed[1].tag_type, FLV_TAG_AUDIO);
            assert_eq!(parsed[1].timestamp_ms, 0);
            assert!(!parsed[1].is_video_keyframe);
            assert!(parsed[1].is_sequence_header);
            assert_eq!(parsed[1].data, aac_seq);

            // Tag 2: AVC IDR keyframe
            assert_eq!(parsed[2].tag_type, FLV_TAG_VIDEO);
            assert_eq!(parsed[2].timestamp_ms, 33);
            assert!(parsed[2].is_video_keyframe);
            assert!(!parsed[2].is_sequence_header);

            // Tag 3: AVC P-frame com timestamp estendido de 32 bits
            assert_eq!(parsed[3].tag_type, FLV_TAG_VIDEO);
            assert_eq!(parsed[3].timestamp_ms, ext_ts);
            assert!(!parsed[3].is_video_keyframe);
            assert!(!parsed[3].is_sequence_header);

            // Tag 4: AAC raw audio
            assert_eq!(parsed[4].tag_type, FLV_TAG_AUDIO);
            assert_eq!(parsed[4].timestamp_ms, ext_ts + 21);
            assert!(!parsed[4].is_video_keyframe);
            assert!(!parsed[4].is_sequence_header);
            assert_eq!(parsed[4].data, aac_raw);
        }
    }

    #[test]
    fn test_timestamp_rebaser_continuity_across_three_bitrate_hot_swaps() {
        let mut rebaser = TimestampRebaser::new();
        let mut timeline = Vec::new();

        // Segmento 0: 6000 kbps (t = 0, 33, 66, 100)
        for ts in [0u32, 33, 66, 100] {
            timeline.push(rebaser.rebase(ts));
        }
        assert_eq!(timeline, vec![0, 33, 66, 100]);

        // Hot-swap #1: 6000 -> 4500 kbps (novo FFmpeg reinicia em t = 0)
        rebaser.begin_new_segment();
        assert_eq!(rebaser.segment_count(), 1);
        for ts in [0u32, 0, 33, 66, 100] {
            timeline.push(rebaser.rebase(ts));
        }

        // Hot-swap #2: 4500 -> 3000 kbps
        rebaser.begin_new_segment();
        assert_eq!(rebaser.segment_count(), 2);
        for ts in [0u32, 16, 33, 50] {
            timeline.push(rebaser.rebase(ts));
        }

        // Hot-swap #3: 3000 -> 3500 kbps (recuperação AIMD)
        rebaser.begin_new_segment();
        assert_eq!(rebaser.segment_count(), 3);
        for ts in [0u32, 33, 66] {
            timeline.push(rebaser.rebase(ts));
        }

        // Toda a linha do tempo deve ser estritamente não-decrescente sem saltos para zero!
        for pair in timeline.windows(2) {
            assert!(
                pair[1] >= pair[0],
                "timestamp regressou de {} para {} em {:?}",
                pair[0],
                pair[1],
                timeline
            );
        }

        // Cada hot-swap avança a linha do tempo a partir do final do segmento anterior
        assert_eq!(timeline[4], 101); // início 4500 kbps
        assert_eq!(timeline[8], 201); // fim 4500 kbps
        assert_eq!(timeline[9], 202); // início 3000 kbps
        assert_eq!(timeline[12], 252); // fim 3000 kbps
        assert_eq!(timeline[13], 253); // início 3500 kbps
        assert_eq!(*timeline.last().unwrap(), 319);
    }

    #[test]
    fn test_parse_rtmp_url_splitting() {
        let twitch = parse_rtmp_url("rtmp://live.twitch.tv/app/live_12345_secret").unwrap();
        assert!(!twitch.use_tls);
        assert_eq!(twitch.host, "live.twitch.tv");
        assert_eq!(twitch.port, 1935);
        assert_eq!(twitch.app, "app");
        assert_eq!(twitch.stream_key, "live_12345_secret");
        assert_eq!(twitch.tc_url, "rtmp://live.twitch.tv/app");

        let kick = parse_rtmp_url("rtmps://host.example.net/app/sk_us_xxx").unwrap();
        assert!(kick.use_tls);
        assert_eq!(kick.host, "host.example.net");
        assert_eq!(kick.port, 443);
        assert_eq!(kick.app, "app");
        assert_eq!(kick.stream_key, "sk_us_xxx");
        assert_eq!(kick.tc_url, "rtmps://host.example.net/app");

        let custom = parse_rtmp_url("rtmp://127.0.0.1:1935/live/key").unwrap();
        assert!(!custom.use_tls);
        assert_eq!(custom.host, "127.0.0.1");
        assert_eq!(custom.port, 1935);
        assert_eq!(custom.app, "live");
        assert_eq!(custom.stream_key, "key");

        let custom_port = parse_rtmp_url("rtmp://127.0.0.1:2935/live/key?token=abc").unwrap();
        assert_eq!(custom_port.port, 2935);
        assert_eq!(custom_port.stream_key, "key?token=abc");
        assert_eq!(custom_port.tc_url, "rtmp://127.0.0.1:2935/live");

        assert!(parse_rtmp_url("https://example.com/app/key").is_err());
        assert!(parse_rtmp_url("rtmp://example.com/onlyapp").is_err());
    }

    #[tokio::test]
    async fn test_persistent_rtmp_session_loopback_handshake_and_publish() {
        use rml_rtmp::sessions::{
            ServerSession, ServerSessionConfig, ServerSessionEvent, ServerSessionResult,
        };
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let port = listener.local_addr().unwrap().port();

        let server_task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut hs = Handshake::new(PeerType::Server);
            let mut buf = [0u8; 8192];

            let remaining = loop {
                let n = socket.read(&mut buf).await.expect("server hs read");
                assert!(n > 0);
                match hs.process_bytes(&buf[..n]).expect("server hs") {
                    HandshakeProcessResult::InProgress { response_bytes } => {
                        if !response_bytes.is_empty() {
                            socket.write_all(&response_bytes).await.unwrap();
                        }
                    }
                    HandshakeProcessResult::Completed {
                        response_bytes,
                        remaining_bytes,
                    } => {
                        if !response_bytes.is_empty() {
                            socket.write_all(&response_bytes).await.unwrap();
                        }
                        break remaining_bytes;
                    }
                }
            };

            let (mut session, init_out) =
                ServerSession::new(ServerSessionConfig::new()).expect("server session");
            for r in init_out {
                if let ServerSessionResult::OutboundResponse(p) = r {
                    socket.write_all(&p.bytes).await.unwrap();
                }
            }

            let mut received_video = 0usize;
            let mut received_audio = 0usize;
            let mut pending = remaining;

            loop {
                let results = if !pending.is_empty() {
                    let data = std::mem::take(&mut pending);
                    session.handle_input(&data).expect("server input")
                } else {
                    let n = socket.read(&mut buf).await.expect("server read");
                    if n == 0 {
                        break;
                    }
                    session.handle_input(&buf[..n]).expect("server input")
                };

                for res in results {
                    match res {
                        ServerSessionResult::OutboundResponse(p) => {
                            socket.write_all(&p.bytes).await.unwrap();
                        }
                        ServerSessionResult::RaisedEvent(ev) => match ev {
                            ServerSessionEvent::ConnectionRequested { request_id, .. }
                            | ServerSessionEvent::PublishStreamRequested { request_id, .. } => {
                                let acks = session.accept_request(request_id).unwrap();
                                for a in acks {
                                    if let ServerSessionResult::OutboundResponse(p) = a {
                                        socket.write_all(&p.bytes).await.unwrap();
                                    }
                                }
                            }
                            ServerSessionEvent::VideoDataReceived { .. } => {
                                received_video += 1;
                            }
                            ServerSessionEvent::AudioDataReceived { .. } => {
                                received_audio += 1;
                            }
                            ServerSessionEvent::PublishStreamFinished { .. } => {
                                return (received_video, received_audio);
                            }
                            _ => {}
                        },
                        ServerSessionResult::UnhandleableMessageReceived(_) => {}
                    }
                }
            }
            (received_video, received_audio)
        });

        let url = format!("rtmp://127.0.0.1:{port}/live/test_stream_key");
        let mut client = PersistentRtmpSession::connect(&url)
            .await
            .expect("persistent RTMP connect");

        let video_tag = FlvTag {
            tag_type: FLV_TAG_VIDEO,
            timestamp_ms: 0,
            data: vec![0x17, 0x00, 0x00, 0x00, 0x00, 0x01, 0x64, 0x00, 0x2A],
            is_video_keyframe: true,
            is_sequence_header: true,
        };
        let audio_tag = FlvTag {
            tag_type: FLV_TAG_AUDIO,
            timestamp_ms: 21,
            data: vec![0xAF, 0x00, 0x11, 0x90],
            is_video_keyframe: false,
            is_sequence_header: true,
        };

        let lat_v = client.send_flv_tag(&video_tag).await.expect("send video");
        let lat_a = client.send_flv_tag(&audio_tag).await.expect("send audio");
        assert!(lat_v < Duration::from_secs(2));
        assert!(lat_a < Duration::from_secs(2));
        assert!(client.bytes_sent() > 0);
        assert_eq!(client.tags_sent(), 2);

        client.close().await;

        let (v_count, a_count) = tokio::time::timeout(Duration::from_secs(5), server_task)
            .await
            .expect("server join timeout")
            .expect("server task panic");
        assert_eq!(v_count, 1);
        assert_eq!(a_count, 1);
    }
}
