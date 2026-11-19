use crate::protocol::{
    ChatFragment, ChatPlatform, ChatSettings, ChatSourceState, ChatSourceStatus, MultiChatEvent,
    MultiChatMessage, MultiChatMessageKind,
};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use regex_lite::Regex;
use reqwest::Client;
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

const KICK_API: &str = "https://web.kick.com/api/v1/realtime";
const KICK_REALTIME: &str = "wss://realtime.us-east-1.platform.kick.com/connection/websocket";
const TWITCH_IRC_WS: &str = "wss://irc-ws.chat.twitch.tv:443";

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const TWITCH_PING_INTERVAL: Duration = Duration::from_secs(60);
const TWITCH_PONG_TIMEOUT: Duration = Duration::from_secs(15);
const BATCH_FLUSH_MS: u64 = 120;
const MAX_RING_MESSAGES: usize = 400;
const MAX_SEEN_IDS: usize = 2000;

fn build_http_client() -> Client {
    Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(UA)
        .build()
        .unwrap_or_else(|_| Client::new())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn text_only(text: &str) -> Vec<ChatFragment> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![ChatFragment::Text {
            text: text.to_string(),
        }]
    }
}

pub fn merge_fragments(frags: Vec<ChatFragment>) -> Vec<ChatFragment> {
    let mut out: Vec<ChatFragment> = Vec::with_capacity(frags.len());
    for f in frags {
        match f {
            ChatFragment::Text { text } => {
                if text.is_empty() {
                    continue;
                }
                if let Some(ChatFragment::Text { text: last }) = out.last_mut() {
                    last.push_str(&text);
                } else {
                    out.push(ChatFragment::Text { text });
                }
            }
            emote @ ChatFragment::Emote { .. } => out.push(emote),
        }
    }
    out
}

/// Verifica menção com fronteira de palavra: `@ana` não casa `@anaclara`.
pub fn contains_mention(text: &str, channel: &str) -> bool {
    let ch = channel.trim().trim_start_matches('#').trim_start_matches('@');
    if ch.is_empty() {
        return false;
    }
    let needle = format!("@{}", ch.to_lowercase());
    let lower = text.to_lowercase();
    let bytes = lower.as_bytes();
    let mut start = 0usize;
    while let Some(rel) = lower[start..].find(&needle) {
        let idx = start + rel;
        let before_ok = idx == 0 || !is_ident_byte(bytes[idx - 1]);
        let after = idx + needle.len();
        let after_ok = after >= bytes.len() || !is_ident_byte(bytes[after]);
        if before_ok && after_ok {
            return true;
        }
        start = idx + 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

// ------------------------------------------------------------------ Cache de emotes 7TV / BTTV / FFZ

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalEmote {
    pub name: String,
    pub url: String,
    pub src_set: Option<String>,
    pub provider: String,
}

#[derive(Debug, Default, Clone)]
pub struct EmoteCatalog {
    pub by_name: HashMap<String, ExternalEmote>,
}

impl EmoteCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, emote: ExternalEmote) {
        self.by_name.entry(emote.name.clone()).or_insert(emote);
    }

    /// Expande um fragmento `Text` em palavras substituindo tokens de emotes externos (7TV/BTTV/FFZ).
    pub fn expand_text(&self, text: &str) -> Vec<ChatFragment> {
        if self.by_name.is_empty() || text.is_empty() {
            return text_only(text);
        }
        let mut out = Vec::new();
        let mut buf = String::new();

        for part in split_preserving_spaces(text) {
            if let Some(emote) = self.by_name.get(part) {
                if !buf.is_empty() {
                    out.push(ChatFragment::Text {
                        text: std::mem::take(&mut buf),
                    });
                }
                out.push(ChatFragment::Emote {
                    url: emote.url.clone(),
                    name: emote.name.clone(),
                    src_set: emote.src_set.clone(),
                    provider: Some(emote.provider.clone()),
                });
            } else {
                buf.push_str(part);
            }
        }
        if !buf.is_empty() {
            out.push(ChatFragment::Text { text: buf });
        }
        merge_fragments(out)
    }

    pub fn expand_fragments(&self, frags: Vec<ChatFragment>) -> Vec<ChatFragment> {
        if self.by_name.is_empty() {
            return frags;
        }
        let mut out = Vec::with_capacity(frags.len());
        for f in frags {
            match f {
                ChatFragment::Text { text } => out.extend(self.expand_text(&text)),
                other => out.push(other),
            }
        }
        merge_fragments(out)
    }
}

fn split_preserving_spaces(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut last = 0;
    let mut in_space = false;
    for (idx, ch) in s.char_indices() {
        let is_sp = ch.is_whitespace();
        if idx > last && is_sp != in_space {
            out.push(&s[last..idx]);
            last = idx;
        }
        in_space = is_sp;
    }
    if last < s.len() {
        out.push(&s[last..]);
    }
    out
}

#[derive(Debug, Deserialize)]
struct SeventvUserResp {
    emote_set: Option<SeventvEmoteSet>,
}

#[derive(Debug, Deserialize)]
struct SeventvGlobalSetResp {
    emotes: Option<Vec<SeventvEmote>>,
}

#[derive(Debug, Deserialize)]
struct SeventvEmoteSet {
    emotes: Option<Vec<SeventvEmote>>,
}

#[derive(Debug, Deserialize)]
struct SeventvEmote {
    id: String,
    name: String,
    data: Option<SeventvEmoteData>,
}

#[derive(Debug, Deserialize)]
struct SeventvEmoteData {
    host: Option<SeventvHost>,
}

#[derive(Debug, Deserialize)]
struct SeventvHost {
    url: Option<String>,
    files: Option<Vec<SeventvFile>>,
}

#[derive(Debug, Deserialize)]
struct SeventvFile {
    name: String,
    format: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BttvUserResp {
    #[serde(rename = "channelEmotes")]
    channel_emotes: Option<Vec<BttvEmote>>,
    #[serde(rename = "sharedEmotes")]
    shared_emotes: Option<Vec<BttvEmote>>,
}

#[derive(Debug, Deserialize)]
struct BttvEmote {
    id: String,
    code: String,
}

#[derive(Debug, Deserialize)]
struct FfzRoomResp {
    sets: Option<HashMap<String, FfzSet>>,
}

#[derive(Debug, Deserialize)]
struct FfzSet {
    emoticons: Option<Vec<FfzEmote>>,
}

#[derive(Debug, Deserialize)]
struct FfzEmote {
    name: String,
    urls: Option<HashMap<String, String>>,
}

/// Busca emotes 7TV, BTTV e FFZ (globais + canal da Twitch) sem falhar o chat se alguma API estiver fora.
pub async fn fetch_third_party_emotes(
    http: &Client,
    twitch_room_id: Option<&str>,
) -> EmoteCatalog {
    let mut catalog = EmoteCatalog::new();

    // 7TV global
    if let Ok(res) = http.get("https://7tv.io/v3/emote-sets/global").send().await {
        if res.status().is_success() {
            if let Ok(set) = res.json::<SeventvGlobalSetResp>().await {
                for e in set.emotes.unwrap_or_default() {
                    if let Some(ext) = parse_7tv_emote(e) {
                        catalog.insert(ext);
                    }
                }
            }
        }
    }

    // BTTV global
    if let Ok(res) = http
        .get("https://api.betterttv.net/3/cached/emotes/global")
        .send()
        .await
    {
        if res.status().is_success() {
            if let Ok(list) = res.json::<Vec<BttvEmote>>().await {
                for e in list {
                    catalog.insert(parse_bttv_emote(e));
                }
            }
        }
    }

    if let Some(room_id) = twitch_room_id.filter(|s| !s.is_empty()) {
        // 7TV canal
        if let Ok(res) = http
            .get(format!("https://7tv.io/v3/users/twitch/{room_id}"))
            .send()
            .await
        {
            if res.status().is_success() {
                if let Ok(u) = res.json::<SeventvUserResp>().await {
                    if let Some(emotes) = u.emote_set.and_then(|s| s.emotes) {
                        for e in emotes {
                            if let Some(ext) = parse_7tv_emote(e) {
                                catalog.insert(ext);
                            }
                        }
                    }
                }
            }
        }

        // BTTV canal
        if let Ok(res) = http
            .get(format!("https://api.betterttv.net/3/cached/users/twitch/{room_id}"))
            .send()
            .await
        {
            if res.status().is_success() {
                if let Ok(u) = res.json::<BttvUserResp>().await {
                    for e in u.channel_emotes.unwrap_or_default() {
                        catalog.insert(parse_bttv_emote(e));
                    }
                    for e in u.shared_emotes.unwrap_or_default() {
                        catalog.insert(parse_bttv_emote(e));
                    }
                }
            }
        }

        // FFZ canal
        if let Ok(res) = http
            .get(format!("https://api.frankerfacez.com/v1/room/id/{room_id}"))
            .send()
            .await
        {
            if res.status().is_success() {
                if let Ok(room) = res.json::<FfzRoomResp>().await {
                    for set in room.sets.unwrap_or_default().into_values() {
                        for e in set.emoticons.unwrap_or_default() {
                            if let Some(ext) = parse_ffz_emote(e) {
                                catalog.insert(ext);
                            }
                        }
                    }
                }
            }
        }
    }

    catalog
}

fn parse_7tv_emote(e: SeventvEmote) -> Option<ExternalEmote> {
    let base = e
        .data
        .and_then(|d| d.host)
        .and_then(|h| h.url)
        .map(|u| {
            if u.starts_with("//") {
                format!("https:{u}")
            } else {
                u
            }
        })
        .unwrap_or_else(|| format!("https://cdn.7tv.app/emote/{}", e.id));
    let url_1x = format!("{base}/1x.webp");
    let url_2x = format!("{base}/2x.webp");
    let url_3x = format!("{base}/3x.webp");
    Some(ExternalEmote {
        name: e.name,
        url: url_1x.clone(),
        src_set: Some(format!("{url_1x} 1x, {url_2x} 2x, {url_3x} 3x")),
        provider: "7tv".to_string(),
    })
}

fn parse_bttv_emote(e: BttvEmote) -> ExternalEmote {
    let u1 = format!("https://cdn.betterttv.net/emote/{}/1x.webp", e.id);
    let u2 = format!("https://cdn.betterttv.net/emote/{}/2x.webp", e.id);
    let u3 = format!("https://cdn.betterttv.net/emote/{}/3x.webp", e.id);
    ExternalEmote {
        name: e.code,
        url: u1.clone(),
        src_set: Some(format!("{u1} 1x, {u2} 2x, {u3} 3x")),
        provider: "bttv".to_string(),
    }
}

fn parse_ffz_emote(e: FfzEmote) -> Option<ExternalEmote> {
    let urls = e.urls?;
    let norm = |u: &str| -> String {
        if u.starts_with("//") {
            format!("https:{u}")
        } else {
            u.to_string()
        }
    };
    let u1 = urls.get("1").or_else(|| urls.values().next()).map(|u| norm(u))?;
    let u2 = urls.get("2").map(|u| norm(u));
    let u4 = urls.get("4").map(|u| norm(u));
    let mut parts = vec![format!("{u1} 1x")];
    if let Some(x2) = u2 {
        parts.push(format!("{x2} 2x"));
    }
    if let Some(x4) = u4 {
        parts.push(format!("{x4} 4x"));
    }
    Some(ExternalEmote {
        name: e.name,
        url: u1,
        src_set: Some(parts.join(", ")),
        provider: "ffz".to_string(),
    })
}

// ------------------------------------------------------------------ Twitch IRC Parser & Reader

/// Passagem única IRCv3 tag unescape (corrige o bug encadeado `\\s` -> `\ `).
pub fn unescape_tag(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(':') => out.push(';'),
            Some('s') => out.push(' '),
            Some('\\') => out.push('\\'),
            Some('r') => {}
            Some('n') => {}
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedIrcLine {
    pub tags: HashMap<String, String>,
    pub prefix: String,
    pub cmd: String,
    pub target: String,
    pub text: String,
}

pub fn parse_irc_line(line: &str) -> Option<ParsedIrcLine> {
    let mut tags = HashMap::new();
    let mut rest = line.trim_end_matches(['\r', '\n']);
    if rest.is_empty() {
        return None;
    }

    if let Some(after_at) = rest.strip_prefix('@') {
        let sp = after_at.find(' ')?;
        let tag_str = &after_at[..sp];
        for kv in tag_str.split(';') {
            if kv.is_empty() {
                continue;
            }
            if let Some((k, v)) = kv.split_once('=') {
                tags.insert(k.to_string(), unescape_tag(v));
            } else {
                tags.insert(kv.to_string(), String::new());
            }
        }
        rest = after_at[sp + 1..].trim_start();
    }

    let prefix = if let Some(after_colon) = rest.strip_prefix(':') {
        let sp = after_colon.find(' ')?;
        let raw_prefix = &after_colon[..sp];
        rest = after_colon[sp + 1..].trim_start();
        raw_prefix.split('!').next().unwrap_or("").to_string()
    } else {
        String::new()
    };

    let (head, text) = if let Some(idx) = rest.find(" :") {
        (&rest[..idx], rest[idx + 2..].to_string())
    } else if let Some(after) = rest.strip_prefix(':') {
        ("", after.to_string())
    } else {
        (rest, String::new())
    };

    let mut parts = head.split_whitespace();
    let cmd = parts.next()?.to_string();
    let target = parts.last().unwrap_or("").to_string();

    Some(ParsedIrcLine {
        tags,
        prefix,
        cmd,
        target,
        text,
    })
}

/// Constrói fragmentos de mensagem da Twitch usando índices Unicode (UTF-32/char) com bounds-check.
pub fn twitch_fragments(text: &str, emotes_tag: Option<&str>, catalog: Option<&EmoteCatalog>) -> Vec<ChatFragment> {
    let chars: Vec<char> = text.chars().collect();
    let base = if let Some(raw_emotes) = emotes_tag.filter(|s| !s.is_empty()) {
        let mut spots: Vec<(usize, usize, String)> = Vec::new();
        for part in raw_emotes.split('/') {
            let Some((id, ranges)) = part.split_once(':') else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            for r in ranges.split(',') {
                let Some((a_str, b_str)) = r.split_once('-') else {
                    continue;
                };
                let (Ok(a), Ok(b)) = (a_str.parse::<usize>(), b_str.parse::<usize>()) else {
                    continue;
                };
                if a <= b && b < chars.len() {
                    spots.push((a, b, id.to_string()));
                }
            }
        }
        spots.sort_by_key(|s| (s.0, s.1));

        let mut out = Vec::new();
        let mut cursor = 0usize;
        for (a, b, id) in spots {
            if a < cursor {
                continue;
            }
            if a > cursor {
                let slice: String = chars[cursor..a].iter().collect();
                out.push(ChatFragment::Text { text: slice });
            }
            let name: String = chars[a..=b].iter().collect();
            let u1 = format!("https://static-cdn.jtvnw.net/emoticons/v2/{id}/default/dark/1.0");
            let u2 = format!("https://static-cdn.jtvnw.net/emoticons/v2/{id}/default/dark/2.0");
            let u3 = format!("https://static-cdn.jtvnw.net/emoticons/v2/{id}/default/dark/3.0");
            out.push(ChatFragment::Emote {
                url: u1.clone(),
                name,
                src_set: Some(format!("{u1} 1x, {u2} 2x, {u3} 3x")),
                provider: Some("twitch".to_string()),
            });
            cursor = b + 1;
        }
        if cursor < chars.len() {
            let tail: String = chars[cursor..].iter().collect();
            out.push(ChatFragment::Text { text: tail });
        }
        if out.is_empty() {
            text_only(text)
        } else {
            out
        }
    } else {
        text_only(text)
    };

    if let Some(cat) = catalog {
        cat.expand_fragments(base)
    } else {
        merge_fragments(base)
    }
}

fn twitch_badges(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in raw.split(',') {
        let name = item.split('/').next().unwrap_or("");
        if matches!(
            name,
            "broadcaster" | "moderator" | "vip" | "subscriber" | "founder" | "partner"
        ) && !out.iter().any(|x| x == name)
        {
            out.push(name.to_string());
        }
    }
    out
}

pub fn normalize_twitch_channel(raw: &str) -> String {
    raw.trim().trim_start_matches('#').trim_start_matches('@').to_lowercase()
}

struct TwitchReader {
    channel: String,
    status: Arc<RwLock<ChatSourceStatus>>,
    cancel: CancellationToken,
    tx: mpsc::Sender<InternalEvent>,
    http: Client,
}

impl TwitchReader {
    fn spawn(
        channel: String,
        tx: mpsc::Sender<InternalEvent>,
        http: Client,
    ) -> Arc<Self> {
        let normalized = normalize_twitch_channel(&channel);
        let status = Arc::new(RwLock::new(ChatSourceStatus {
            platform: ChatPlatform::Twitch,
            state: ChatSourceState::Connecting,
            channel: normalized.clone(),
            message: None,
        }));
        let reader = Arc::new(Self {
            channel: normalized,
            status,
            cancel: CancellationToken::new(),
            tx,
            http,
        });
        let r = reader.clone();
        tokio::spawn(async move { r.run().await });
        reader
    }

    async fn set_status(&self, state: ChatSourceState, msg: Option<String>) {
        {
            let mut g = self.status.write().await;
            g.state = state;
            g.message = msg;
        }
        let _ = self.tx.send(InternalEvent::StatusChanged).await;
    }

    async fn run(self: Arc<Self>) {
        let emotes = Arc::new(RwLock::new(
            fetch_third_party_emotes(&self.http, None).await,
        ));
        let mut backoff_ms = 1000u64;
        let mut room_emotes_loaded = false;

        while !self.cancel.is_cancelled() {
            self.set_status(ChatSourceState::Connecting, None).await;
            let req = match TWITCH_IRC_WS.into_client_request() {
                Ok(r) => r,
                Err(e) => {
                    self.set_status(ChatSourceState::Error, Some(e.to_string()))
                        .await;
                    break;
                }
            };

            let conn = tokio::select! {
                _ = self.cancel.cancelled() => break,
                res = tokio_tungstenite::connect_async(req) => res,
            };

            let (ws_stream, _) = match conn {
                Ok(pair) => pair,
                Err(_) => {
                    self.set_status(
                        ChatSourceState::Error,
                        Some("Falha ao conectar no chat da Twitch. Tentando de novo…".to_string()),
                    )
                    .await;
                    tokio::select! {
                        _ = self.cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
                    }
                    backoff_ms = (backoff_ms * 2).min(30_000);
                    continue;
                }
            };

            let (mut write, mut read) = ws_stream.split();
            let nick_num: u32 = rand::rng().random_range(10_000..99_999);
            let init_cmds = [
                "CAP REQ :twitch.tv/tags twitch.tv/commands".to_string(),
                "PASS SCHMOOPIIE".to_string(),
                format!("NICK justinfan{nick_num}"),
                format!("JOIN #{}", self.channel),
            ];
            let mut init_ok = true;
            for cmd in init_cmds {
                if write.send(Message::Text(cmd.into())).await.is_err() {
                    init_ok = false;
                    break;
                }
            }
            if !init_ok {
                continue;
            }

            let mut ping_ticker = tokio::time::interval(TWITCH_PING_INTERVAL);
            ping_ticker.tick().await;
            let mut awaiting_pong_since: Option<tokio::time::Instant> = None;
            let mut pong_check = tokio::time::interval(Duration::from_secs(2));
            let mut joined = false;

            loop {
                tokio::select! {
                    _ = self.cancel.cancelled() => {
                        let _ = write.close().await;
                        return;
                    }
                    _ = ping_ticker.tick() => {
                        if write.send(Message::Text("PING :prismcast".into())).await.is_err() {
                            break;
                        }
                        if awaiting_pong_since.is_none() {
                            awaiting_pong_since = Some(tokio::time::Instant::now());
                        }
                    }
                    _ = pong_check.tick() => {
                        if let Some(since) = awaiting_pong_since {
                            if since.elapsed() > TWITCH_PONG_TIMEOUT {
                                break;
                            }
                        }
                    }
                    msg_opt = read.next() => {
                        let Some(Ok(msg)) = msg_opt else {
                            break;
                        };
                        let Message::Text(text_payload) = msg else {
                            if matches!(msg, Message::Close(_)) {
                                break;
                            }
                            continue;
                        };
                        for raw in text_payload.split("\r\n") {
                            if raw.is_empty() {
                                continue;
                            }
                            if raw.starts_with("PING") {
                                if write.send(Message::Text("PONG :tmi.twitch.tv".into())).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                            if raw.starts_with("PONG") {
                                awaiting_pong_since = None;
                                continue;
                            }
                            let Some(parsed) = parse_irc_line(raw) else {
                                continue;
                            };
                            match parsed.cmd.as_str() {
                                "JOIN" | "366" => {
                                    if !joined {
                                        joined = true;
                                        backoff_ms = 1000;
                                        self.set_status(ChatSourceState::Connected, None).await;
                                    }
                                }
                                "ROOMSTATE" => {
                                    if !room_emotes_loaded {
                                        if let Some(room_id) = parsed.tags.get("room-id").cloned() {
                                            room_emotes_loaded = true;
                                            let http = self.http.clone();
                                            let emotes_ref = emotes.clone();
                                            tokio::spawn(async move {
                                                let cat = fetch_third_party_emotes(&http, Some(&room_id)).await;
                                                *emotes_ref.write().await = cat;
                                            });
                                        }
                                    }
                                }
                                "PRIVMSG" => {
                                    let msg_text = parsed
                                        .text
                                        .strip_prefix("\u{0001}ACTION ")
                                        .and_then(|s| s.strip_suffix('\u{0001}'))
                                        .unwrap_or(&parsed.text);
                                    let name = parsed
                                        .tags
                                        .get("display-name")
                                        .filter(|s| !s.is_empty())
                                        .cloned()
                                        .unwrap_or_else(|| parsed.prefix.clone());
                                    let id = parsed
                                        .tags
                                        .get("id")
                                        .filter(|s| !s.is_empty())
                                        .cloned()
                                        .unwrap_or_else(|| format!("tw-{}", Uuid::new_v4()));
                                    let is_event = parsed.tags.get("msg-id").map(String::as_str) == Some("highlighted-message")
                                        || parsed.tags.contains_key("bits");
                                    let cat = emotes.read().await;
                                    let frags = twitch_fragments(
                                        msg_text,
                                        parsed.tags.get("emotes").map(String::as_str),
                                        Some(&cat),
                                    );
                                    let ts = parsed
                                        .tags
                                        .get("tmi-sent-ts")
                                        .and_then(|s| s.parse::<u64>().ok())
                                        .unwrap_or_else(now_ms);
                                    let chat_msg = MultiChatMessage {
                                        id,
                                        platform: ChatPlatform::Twitch,
                                        kind: if is_event {
                                            MultiChatMessageKind::Event
                                        } else {
                                            MultiChatMessageKind::Msg
                                        },
                                        user: parsed.prefix.to_lowercase(),
                                        name,
                                        color: parsed.tags.get("color").cloned().unwrap_or_default(),
                                        badges: twitch_badges(parsed.tags.get("badges").map(String::as_str)),
                                        fragments: frags,
                                        ts,
                                    };
                                    let _ = self.tx.send(InternalEvent::Message(chat_msg)).await;
                                }
                                "USERNOTICE" => {
                                    let sys = parsed.tags.get("system-msg").cloned().unwrap_or_default();
                                    let name = parsed
                                        .tags
                                        .get("display-name")
                                        .filter(|s| !s.is_empty())
                                        .or_else(|| parsed.tags.get("login"))
                                        .cloned()
                                        .unwrap_or_default();
                                    let mut frags = text_only(&sys);
                                    if !parsed.text.is_empty() {
                                        if !frags.is_empty() {
                                            frags.push(ChatFragment::Text {
                                                text: " — ".to_string(),
                                            });
                                        }
                                        let cat = emotes.read().await;
                                        frags.extend(twitch_fragments(
                                            &parsed.text,
                                            parsed.tags.get("emotes").map(String::as_str),
                                            Some(&cat),
                                        ));
                                    }
                                    let id = parsed
                                        .tags
                                        .get("id")
                                        .filter(|s| !s.is_empty())
                                        .cloned()
                                        .unwrap_or_else(|| format!("tw-{}", Uuid::new_v4()));
                                    let ts = parsed
                                        .tags
                                        .get("tmi-sent-ts")
                                        .and_then(|s| s.parse::<u64>().ok())
                                        .unwrap_or_else(now_ms);
                                    let chat_msg = MultiChatMessage {
                                        id,
                                        platform: ChatPlatform::Twitch,
                                        kind: MultiChatMessageKind::Event,
                                        user: parsed.tags.get("login").cloned().unwrap_or_default().to_lowercase(),
                                        name,
                                        color: parsed.tags.get("color").cloned().unwrap_or_default(),
                                        badges: twitch_badges(parsed.tags.get("badges").map(String::as_str)),
                                        fragments: merge_fragments(frags),
                                        ts,
                                    };
                                    let _ = self.tx.send(InternalEvent::Message(chat_msg)).await;
                                }
                                "CLEARMSG" => {
                                    if let Some(target) = parsed.tags.get("target-msg-id") {
                                        let _ = self
                                            .tx
                                            .send(InternalEvent::Delete {
                                                platform: ChatPlatform::Twitch,
                                                id: target.clone(),
                                            })
                                            .await;
                                    }
                                }
                                "RECONNECT" => {
                                    backoff_ms = 500;
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }

            if !self.cancel.is_cancelled() {
                self.set_status(
                    ChatSourceState::Error,
                    Some("Conexão com a Twitch caiu. Reconectando…".to_string()),
                )
                .await;
                tokio::select! {
                    _ = self.cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
                }
                backoff_ms = (backoff_ms * 2).min(30_000);
            }
        }
    }

    fn stop(&self) {
        self.cancel.cancel();
    }
}

// ------------------------------------------------------------------ YouTube Reader

pub enum YouTubeTarget {
    VideoId(String),
    LiveUrl(String),
}

pub fn resolve_youtube_target(input: &str) -> Result<YouTubeTarget, String> {
    let v = input.trim();
    if v.is_empty() {
        return Err("Canal do YouTube vazio.".to_string());
    }

    static VID_RE: OnceLock<Regex> = OnceLock::new();
    static EXACT_VID_RE: OnceLock<Regex> = OnceLock::new();
    static HANDLE_RE: OnceLock<Regex> = OnceLock::new();
    static CHANNEL_RE: OnceLock<Regex> = OnceLock::new();

    let vid_re = VID_RE
        .get_or_init(|| Regex::new(r"(?:v=|youtu\.be/|/live/|/shorts/)([\w-]{11})").unwrap());
    let exact_re = EXACT_VID_RE.get_or_init(|| Regex::new(r"^[\w-]{11}$").unwrap());
    let handle_re = HANDLE_RE.get_or_init(|| Regex::new(r"(?:youtube\.com/)?(@[\w.-]+)").unwrap());
    let channel_re = CHANNEL_RE.get_or_init(|| Regex::new(r"(UC[\w-]{22})").unwrap());

    if !v.starts_with('@') {
        if let Some(cap) = vid_re.captures(v).or_else(|| exact_re.captures(v)) {
            return Ok(YouTubeTarget::VideoId(cap[1].to_string()));
        }
    }
    if let Some(cap) = handle_re.captures(v) {
        return Ok(YouTubeTarget::LiveUrl(format!(
            "https://www.youtube.com/{}/live",
            &cap[1]
        )));
    }
    if let Some(cap) = channel_re.captures(v) {
        return Ok(YouTubeTarget::LiveUrl(format!(
            "https://www.youtube.com/channel/{}/live",
            &cap[1]
        )));
    }
    let clean: String = v
        .trim_start_matches('@')
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
        .collect();
    if clean.is_empty() {
        return Err("Canal do YouTube inválido.".to_string());
    }
    Ok(YouTubeTarget::LiveUrl(format!(
        "https://www.youtube.com/@{clean}/live"
    )))
}

struct YouTubeReader {
    channel: String,
    status: Arc<RwLock<ChatSourceStatus>>,
    cancel: CancellationToken,
    tx: mpsc::Sender<InternalEvent>,
    http: Client,
}

impl YouTubeReader {
    fn spawn(channel: String, tx: mpsc::Sender<InternalEvent>, http: Client) -> Arc<Self> {
        let status = Arc::new(RwLock::new(ChatSourceStatus {
            platform: ChatPlatform::Youtube,
            state: ChatSourceState::Connecting,
            channel: channel.clone(),
            message: None,
        }));
        let reader = Arc::new(Self {
            channel,
            status,
            cancel: CancellationToken::new(),
            tx,
            http,
        });
        let r = reader.clone();
        tokio::spawn(async move { r.run().await });
        reader
    }

    async fn set_status(&self, state: ChatSourceState, msg: Option<String>) {
        {
            let mut g = self.status.write().await;
            g.state = state;
            g.message = msg;
        }
        let _ = self.tx.send(InternalEvent::StatusChanged).await;
    }

    async fn get_text(&self, url: &str) -> Result<(u16, String), String> {
        let res = tokio::select! {
            _ = self.cancel.cancelled() => return Err("cancelado".to_string()),
            res = self.http
                .get(url)
                .header("Accept-Language", "pt-BR,pt;q=0.9,en;q=0.8")
                .header("Cookie", "CONSENT=YES+1")
                .send() => res.map_err(|e| e.to_string())?,
        };
        let status = res.status().as_u16();
        let text = tokio::select! {
            _ = self.cancel.cancelled() => return Err("cancelado".to_string()),
            txt = res.text() => txt.map_err(|e| e.to_string())?,
        };
        Ok((status, text))
    }

    async fn find_video_id(&self) -> Result<Option<String>, String> {
        match resolve_youtube_target(&self.channel)? {
            YouTubeTarget::VideoId(id) => Ok(Some(id)),
            YouTubeTarget::LiveUrl(url) => {
                let (status, html) = self.get_text(&url).await?;
                if status == 404 {
                    return Err("Canal do YouTube não encontrado.".to_string());
                }
                static CANON_RE: OnceLock<Regex> = OnceLock::new();
                static LIVE_RE: OnceLock<Regex> = OnceLock::new();
                let canon_re = CANON_RE.get_or_init(|| {
                    Regex::new(r#"<link rel="canonical" href="[^"]*[?&]v=([\w-]{11})"#).unwrap()
                });
                let live_re = LIVE_RE
                    .get_or_init(|| Regex::new(r#""(?:isLiveNow|isUpcoming)"\s*:\s*true"#).unwrap());
                let Some(vid) = canon_re.captures(&html).map(|c| c[1].to_string()) else {
                    return Ok(None);
                };
                if live_re.is_match(&html) {
                    Ok(Some(vid))
                } else {
                    Ok(None)
                }
            }
        }
    }

    async fn run(self: Arc<Self>) {
        let mut failures = 0u32;
        while !self.cancel.is_cancelled() {
            self.set_status(ChatSourceState::Connecting, None).await;
            let video_id = match self.find_video_id().await {
                Ok(Some(v)) => v,
                Ok(None) => {
                    failures = 0;
                    self.set_status(
                        ChatSourceState::Waiting,
                        Some("O canal não está ao vivo no YouTube agora.".to_string()),
                    )
                    .await;
                    tokio::select! {
                        _ = self.cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(60)) => continue,
                    }
                }
                Err(e) => {
                    if self.cancel.is_cancelled() {
                        break;
                    }
                    failures += 1;
                    self.set_status(ChatSourceState::Error, Some(e)).await;
                    let wait = (5 * failures).min(60) as u64;
                    tokio::select! {
                        _ = self.cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(wait)) => continue,
                    }
                }
            };

            let chat_url = format!(
                "https://www.youtube.com/live_chat?is_popout=1&v={}",
                urlencoding_component(&video_id)
            );
            let page_html = match self.get_text(&chat_url).await {
                Ok((_, t)) => t,
                Err(e) => {
                    if self.cancel.is_cancelled() {
                        break;
                    }
                    failures += 1;
                    self.set_status(ChatSourceState::Error, Some(e)).await;
                    let wait = (5 * failures).min(60) as u64;
                    tokio::select! {
                        _ = self.cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(wait)) => continue,
                    }
                }
            };

            static KEY_RE: OnceLock<Regex> = OnceLock::new();
            static VER_RE: OnceLock<Regex> = OnceLock::new();
            static RELOAD_RE: OnceLock<Regex> = OnceLock::new();
            static CONT_RE: OnceLock<Regex> = OnceLock::new();

            let key_re = KEY_RE
                .get_or_init(|| Regex::new(r#""INNERTUBE_API_KEY"\s*:\s*"([^"]+)""#).unwrap());
            let ver_re = VER_RE.get_or_init(|| {
                Regex::new(r#""INNERTUBE_CLIENT_VERSION"\s*:\s*"([^"]+)""#).unwrap()
            });
            let reload_re = RELOAD_RE.get_or_init(|| {
                Regex::new(r#""reloadContinuationData"\s*:\s*\{\s*"continuation"\s*:\s*"([^"]+)""#)
                    .unwrap()
            });
            let cont_re =
                CONT_RE.get_or_init(|| Regex::new(r#""continuation"\s*:\s*"([^"]+)""#).unwrap());

            let api_key = key_re
                .captures(&page_html)
                .map(|c| c[1].to_string())
                .unwrap_or_default();
            let client_version = ver_re
                .captures(&page_html)
                .map(|c| c[1].to_string())
                .unwrap_or_else(|| "2.20250101.00.00".to_string());
            let mut continuation = reload_re
                .captures_iter(&page_html)
                .last()
                .or_else(|| cont_re.captures(&page_html))
                .map(|c| c[1].to_string())
                .unwrap_or_default();

            if api_key.is_empty() || continuation.is_empty() {
                self.set_status(
                    ChatSourceState::Waiting,
                    Some("Esta live não tem chat (ou ele está desligado).".to_string()),
                )
                .await;
                tokio::select! {
                    _ = self.cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(60)) => continue,
                }
            }

            failures = 0;
            self.set_status(ChatSourceState::Connected, None).await;

            // Loop de polling até a live acabar ou erro.
            loop {
                if self.cancel.is_cancelled() {
                    return;
                }
                let api_url = format!(
                    "https://www.youtube.com/youtubei/v1/live_chat/get_live_chat?key={}&prettyPrint=false",
                    urlencoding_component(&api_key)
                );
                let body = serde_json::json!({
                    "context": {
                        "client": {
                            "clientName": "WEB",
                            "clientVersion": client_version,
                            "hl": "pt",
                            "gl": "BR"
                        }
                    },
                    "continuation": continuation
                });

                let res = tokio::select! {
                    _ = self.cancel.cancelled() => return,
                    res = self.http.post(&api_url).json(&body).send() => res,
                };
                let Ok(res) = res else {
                    failures += 1;
                    self.set_status(
                        ChatSourceState::Error,
                        Some("Falha de rede ao ler chat do YouTube.".to_string()),
                    )
                    .await;
                    break;
                };
                if !res.status().is_success() {
                    failures += 1;
                    self.set_status(
                        ChatSourceState::Error,
                        Some(format!("YouTube respondeu {}", res.status().as_u16())),
                    )
                    .await;
                    break;
                }

                let Ok(json) = res.json::<serde_json::Value>().await else {
                    failures += 1;
                    break;
                };
                let Some(chat) = json
                    .pointer("/continuationContents/liveChatContinuation")
                else {
                    self.set_status(
                        ChatSourceState::Waiting,
                        Some("A live do YouTube terminou.".to_string()),
                    )
                    .await;
                    tokio::select! {
                        _ = self.cancel.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                    }
                    break;
                };

                if let Some(actions) = chat.get("actions").and_then(|v| v.as_array()) {
                    for ev in parse_youtube_actions(actions) {
                        let _ = self.tx.send(ev).await;
                    }
                }

                let next_obj = chat
                    .get("continuations")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.as_object())
                    .and_then(|obj| obj.values().next());

                let Some(next_cont) = next_obj
                    .and_then(|v| v.get("continuation"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                else {
                    // Sem próximo continuation: evita loop infinito repetindo token antigo.
                    self.set_status(
                        ChatSourceState::Waiting,
                        Some("A live do YouTube terminou.".to_string()),
                    )
                    .await;
                    tokio::select! {
                        _ = self.cancel.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                    }
                    break;
                };

                continuation = next_cont.to_string();
                let timeout_ms = next_obj
                    .and_then(|v| v.get("timeoutMs"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(3000)
                    .clamp(1000, 8000);
                failures = 0;
                tokio::select! {
                    _ = self.cancel.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => {}
                }
            }

            if failures > 0 && !self.cancel.is_cancelled() {
                let wait = (5 * failures).min(60) as u64;
                tokio::select! {
                    _ = self.cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
                }
            }
        }
    }

    fn stop(&self) {
        self.cancel.cancel();
    }
}

fn parse_youtube_actions(actions: &[serde_json::Value]) -> Vec<InternalEvent> {
    let mut out = Vec::new();
    for action in actions {
        if let Some(target_id) = action
            .pointer("/markChatItemAsDeletedAction/targetItemId")
            .and_then(|v| v.as_str())
        {
            out.push(InternalEvent::Delete {
                platform: ChatPlatform::Youtube,
                id: format!("yt-{target_id}"),
            });
            continue;
        }
        let Some(item_obj) = action
            .pointer("/addChatItemAction/item")
            .and_then(|v| v.as_object())
        else {
            continue;
        };
        let Some((item_type, r)) = item_obj.iter().next() else {
            continue;
        };
        let Some(raw_id) = r.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let author_name = r
            .pointer("/authorName/simpleText")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim_start_matches('@')
            .to_string();
        let user = r
            .get("authorExternalChannelId")
            .and_then(|v| v.as_str())
            .unwrap_or(&author_name)
            .to_string();
        let ts = r
            .get("timestampUsec")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .map(|u| u / 1000)
            .unwrap_or_else(now_ms);
        let badges = yt_badges(r.get("authorBadges"));

        match item_type.as_str() {
            "liveChatTextMessageRenderer" => {
                let frags = yt_runs(r.pointer("/message/runs"));
                out.push(InternalEvent::Message(MultiChatMessage {
                    id: format!("yt-{raw_id}"),
                    platform: ChatPlatform::Youtube,
                    kind: MultiChatMessageKind::Msg,
                    user,
                    name: author_name,
                    color: String::new(),
                    badges,
                    fragments: merge_fragments(frags),
                    ts,
                }));
            }
            "liveChatPaidMessageRenderer" | "liveChatPaidStickerRenderer" => {
                let amount = r
                    .pointer("/purchaseAmountText/simpleText")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let msg_runs = yt_runs(r.pointer("/message/runs"));
                let sep = if msg_runs.is_empty() { "" } else { " — " };
                let mut frags = vec![ChatFragment::Text {
                    text: format!("Super Chat {amount}{sep}"),
                }];
                frags.extend(msg_runs);
                out.push(InternalEvent::Message(MultiChatMessage {
                    id: format!("yt-{raw_id}"),
                    platform: ChatPlatform::Youtube,
                    kind: MultiChatMessageKind::Event,
                    user,
                    name: author_name,
                    color: String::new(),
                    badges,
                    fragments: merge_fragments(frags),
                    ts,
                }));
            }
            "liveChatMembershipItemRenderer" => {
                let head = r
                    .pointer("/headerSubtext/simpleText")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        yt_runs(r.pointer("/headerSubtext/runs"))
                            .into_iter()
                            .map(|f| match f {
                                ChatFragment::Text { text } => text,
                                ChatFragment::Emote { name, .. } => name,
                            })
                            .collect::<String>()
                    });
                let msg_runs = yt_runs(r.pointer("/message/runs"));
                let mut frags = vec![ChatFragment::Text {
                    text: if head.is_empty() {
                        "Novo membro".to_string()
                    } else {
                        head
                    },
                }];
                if !msg_runs.is_empty() {
                    frags.push(ChatFragment::Text {
                        text: " — ".to_string(),
                    });
                    frags.extend(msg_runs);
                }
                out.push(InternalEvent::Message(MultiChatMessage {
                    id: format!("yt-{raw_id}"),
                    platform: ChatPlatform::Youtube,
                    kind: MultiChatMessageKind::Event,
                    user,
                    name: author_name,
                    color: String::new(),
                    badges,
                    fragments: merge_fragments(frags),
                    ts,
                }));
            }
            _ => {}
        }
    }
    out
}

fn yt_runs(val: Option<&serde_json::Value>) -> Vec<ChatFragment> {
    let Some(arr) = val.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(arr.len());
    for r in arr {
        if let Some(emoji) = r.get("emoji") {
            let url = emoji
                .pointer("/image/thumbnails")
                .and_then(|v| v.as_array())
                .and_then(|a| a.last())
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str());
            let emoji_id = emoji.get("emojiId").and_then(|v| v.as_str()).unwrap_or("");
            let shortcut = emoji
                .pointer("/shortcuts/0")
                .and_then(|v| v.as_str())
                .unwrap_or(emoji_id);
            if url.is_none() || (!emoji_id.is_empty() && !emoji_id.contains('/') && emoji_id.chars().count() <= 8) {
                let text = if !emoji_id.is_empty() {
                    emoji_id
                } else {
                    shortcut
                };
                out.push(ChatFragment::Text {
                    text: text.to_string(),
                });
            } else if let Some(u) = url {
                out.push(ChatFragment::Emote {
                    url: u.to_string(),
                    name: shortcut.to_string(),
                    src_set: None,
                    provider: Some("youtube".to_string()),
                });
            }
        } else if let Some(t) = r.get("text").and_then(|v| v.as_str()) {
            out.push(ChatFragment::Text {
                text: t.to_string(),
            });
        }
    }
    out
}

fn yt_badges(val: Option<&serde_json::Value>) -> Vec<String> {
    let Some(arr) = val.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        let Some(badge) = item.get("liveChatAuthorBadgeRenderer") else {
            continue;
        };
        let icon = badge
            .pointer("/icon/iconType")
            .and_then(|v| v.as_str());
        match icon {
            Some("OWNER") => out.push("broadcaster".to_string()),
            Some("MODERATOR") => out.push("moderator".to_string()),
            Some("VERIFIED") => out.push("verified".to_string()),
            None if badge.get("tooltip").and_then(|v| v.as_str()).is_some() => {
                out.push("member".to_string())
            }
            _ => {}
        }
    }
    out
}

// ------------------------------------------------------------------ Kick Reader

pub fn sanitize_kick_slug(input: &str) -> Result<String, String> {
    let v = input.trim();
    static SLUG_RE: OnceLock<Regex> = OnceLock::new();
    let re = SLUG_RE.get_or_init(|| Regex::new(r"(?i)kick\.com/([\w-]+)").unwrap());
    let candidate = if let Some(cap) = re.captures(v) {
        cap[1].to_string()
    } else {
        v.trim_start_matches('@').to_string()
    };
    let clean: String = candidate
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if clean.is_empty() {
        return Err("Canal da Kick inválido.".to_string());
    }
    Ok(clean)
}

pub fn kick_fragments(content: &str) -> Vec<ChatFragment> {
    static EMOTE_RE: OnceLock<Regex> = OnceLock::new();
    let re = EMOTE_RE.get_or_init(|| Regex::new(r"\[emote:(\d+):([^\]]*)\]").unwrap());
    let mut out = Vec::new();
    let mut last = 0usize;
    for cap in re.captures_iter(content) {
        let m = cap.get(0).unwrap();
        if m.start() > last {
            out.push(ChatFragment::Text {
                text: content[last..m.start()].to_string(),
            });
        }
        out.push(ChatFragment::Emote {
            url: format!("https://files.kick.com/emotes/{}/fullsize", &cap[1]),
            name: cap[2].to_string(),
            src_set: None,
            provider: Some("kick".to_string()),
        });
        last = m.end();
    }
    if last < content.len() {
        out.push(ChatFragment::Text {
            text: content[last..].to_string(),
        });
    }
    merge_fragments(out)
}

struct KickReader {
    slug: String,
    client_id: String,
    status: Arc<RwLock<ChatSourceStatus>>,
    cancel: CancellationToken,
    tx: mpsc::Sender<InternalEvent>,
    http: Client,
}

impl KickReader {
    fn spawn(channel: String, tx: mpsc::Sender<InternalEvent>, http: Client) -> Arc<Self> {
        let slug = sanitize_kick_slug(&channel).unwrap_or_default();
        let status = Arc::new(RwLock::new(ChatSourceStatus {
            platform: ChatPlatform::Kick,
            state: ChatSourceState::Connecting,
            channel: slug.clone(),
            message: None,
        }));
        let reader = Arc::new(Self {
            slug,
            client_id: Uuid::new_v4().to_string(),
            status,
            cancel: CancellationToken::new(),
            tx,
            http,
        });
        let r = reader.clone();
        tokio::spawn(async move { r.run().await });
        reader
    }

    async fn set_status(&self, state: ChatSourceState, msg: Option<String>) {
        {
            let mut g = self.status.write().await;
            g.state = state;
            g.message = msg;
        }
        let _ = self.tx.send(InternalEvent::StatusChanged).await;
    }

    async fn fetch_room(&self) -> Result<(u64, u64), String> {
        if self.slug.is_empty() {
            return Err("Canal da Kick inválido.".to_string());
        }
        for ver in ["v2", "v1"] {
            let url = format!("https://kick.com/api/{ver}/channels/{}", self.slug);
            let res = tokio::select! {
                _ = self.cancel.cancelled() => return Err("cancelado".to_string()),
                r = self.http.get(&url).send() => r.map_err(|e| e.to_string())?,
            };
            if res.status().as_u16() == 404 {
                return Err("Canal da Kick não encontrado.".to_string());
            }
            if !res.status().is_success() {
                continue;
            }
            if let Ok(json) = res.json::<serde_json::Value>().await {
                let cid = json.get("id").and_then(|v| v.as_u64());
                let crid = json.pointer("/chatroom/id").and_then(|v| v.as_u64());
                if let (Some(c), Some(cr)) = (cid, crid) {
                    return Ok((c, cr));
                }
            }
        }
        Err("A Kick não respondeu (proteção do site). Tentando de novo…".to_string())
    }

    async fn kick_post(&self, path: &str, body: serde_json::Value) -> Option<serde_json::Value> {
        let res = tokio::select! {
            _ = self.cancel.cancelled() => return None,
            r = self.http
                .post(format!("{KICK_API}{path}"))
                .header("Origin", "https://kick.com")
                .header("Referer", "https://kick.com/")
                .json(&body)
                .send() => r.ok()?,
        };
        if !res.status().is_success() {
            return None;
        }
        let val = res.json::<serde_json::Value>().await.ok()?;
        val.get("data").cloned()
    }

    async fn fetch_token(&self) -> Result<String, String> {
        let data = self
            .kick_post("/auth/connection", serde_json::json!({ "client_id": self.client_id }))
            .await;
        data.and_then(|v| v.get("token").and_then(|t| t.as_str()).map(str::to_string))
            .ok_or_else(|| "A Kick não liberou o chat agora. Tentando de novo…".to_string())
    }

    async fn fetch_realtime_url(&self, channel_id: u64) -> String {
        let body = serde_json::json!({
            "client": { "id": self.client_id, "type": "web" },
            "capabilities": { "accepted_providers": [{ "provider": "centrifugo" }] }
        });
        let data = self
            .kick_post(&format!("/channels/{channel_id}/chat/connection"), body)
            .await;
        data.and_then(|v| {
            v.get("connections")
                .and_then(|c| c.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .find(|c| c.get("provider").and_then(|p| p.as_str()) == Some("centrifugo"))
                })
                .and_then(|c| c.pointer("/credentials/url").and_then(|u| u.as_str()))
                .filter(|u| Url::parse(u).is_ok_and(|parsed| parsed.scheme() == "wss"))
                .map(str::to_string)
        })
        .unwrap_or_else(|| KICK_REALTIME.to_string())
    }

    async fn run(self: Arc<Self>) {
        let mut failures = 0u32;
        while !self.cancel.is_cancelled() {
            self.set_status(ChatSourceState::Connecting, None).await;
            let (channel_id, chatroom_id) = match self.fetch_room().await {
                Ok(pair) => pair,
                Err(e) => {
                    if self.cancel.is_cancelled() {
                        break;
                    }
                    failures += 1;
                    self.set_status(ChatSourceState::Error, Some(e)).await;
                    let wait = (3 * failures).min(60) as u64;
                    tokio::select! {
                        _ = self.cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(wait)) => continue,
                    }
                }
            };

            let (token_res, ws_url) =
                tokio::join!(self.fetch_token(), self.fetch_realtime_url(channel_id));
            let token = match token_res {
                Ok(t) => t,
                Err(e) => {
                    if self.cancel.is_cancelled() {
                        break;
                    }
                    failures += 1;
                    self.set_status(ChatSourceState::Error, Some(e)).await;
                    let wait = (3 * failures).min(60) as u64;
                    tokio::select! {
                        _ = self.cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(wait)) => continue,
                    }
                }
            };

            let Ok(mut req) = ws_url.into_client_request() else {
                break;
            };
            let _ = req.headers_mut().insert("Origin", "https://kick.com".parse().unwrap());
            let _ = req.headers_mut().insert("User-Agent", UA.parse().unwrap());

            let conn = tokio::select! {
                _ = self.cancel.cancelled() => break,
                r = tokio_tungstenite::connect_async(req) => r,
            };
            let Ok((ws_stream, _)) = conn else {
                failures += 1;
                self.set_status(
                    ChatSourceState::Error,
                    Some("Conexão com a Kick falhou. Reconectando…".to_string()),
                )
                .await;
                let wait = (3 * failures).min(60) as u64;
                tokio::select! {
                    _ = self.cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(wait)) => continue,
                }
            };

            let (mut write, mut read) = ws_stream.split();
            let mut next_id = 1u64;
            let connect_msg = serde_json::json!({ "id": next_id, "connect": { "token": token, "name": "js" } });
            next_id += 1;
            let sub_msg = serde_json::json!({ "id": next_id, "subscribe": { "channel": format!("chatrooms.{chatroom_id}.v2") } });
            next_id += 1;

            if write.send(Message::Text(connect_msg.to_string().into())).await.is_err()
                || write.send(Message::Text(sub_msg.to_string().into())).await.is_err()
            {
                continue;
            }

            let mut refresh_deadline: Option<tokio::time::Instant> = None;
            let mut refresh_tick = tokio::time::interval(Duration::from_secs(5));
            let mut last_activity = tokio::time::Instant::now();
            let mut ws_open = true;

            while ws_open {
                tokio::select! {
                    _ = self.cancel.cancelled() => {
                        let _ = write.close().await;
                        return;
                    }
                    _ = refresh_tick.tick() => {
                        if last_activity.elapsed() > Duration::from_secs(90) {
                            break;
                        }
                        if let Some(dl) = refresh_deadline {
                            if tokio::time::Instant::now() >= dl {
                                refresh_deadline = None;
                                match self.fetch_token().await {
                                    Ok(new_tok) => {
                                        let msg = serde_json::json!({ "id": next_id, "refresh": { "token": new_tok } });
                                        next_id += 1;
                                        if write.send(Message::Text(msg.to_string().into())).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                    }
                    item = read.next() => {
                        let Some(Ok(msg)) = item else {
                            break;
                        };
                        last_activity = tokio::time::Instant::now();
                        let Message::Text(payload) = msg else {
                            if matches!(msg, Message::Close(_)) {
                                break;
                            }
                            continue;
                        };
                        for line in payload.split('\n') {
                            if line.is_empty() {
                                continue;
                            }
                            if line == "{}" {
                                if write.send(Message::Text("{}".into())).await.is_err() {
                                    ws_open = false;
                                    break;
                                }
                                continue;
                            }
                            let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
                                continue;
                            };
                            if let Some(err_msg) = val.pointer("/error/message").and_then(|v| v.as_str()) {
                                failures += 1;
                                self.set_status(ChatSourceState::Error, Some(format!("Kick: {err_msg}"))).await;
                                ws_open = false;
                                break;
                            }
                            // Renova tanto em `connect` quanto em `refresh` (corrige queda após ~90 min).
                            for key in ["connect", "refresh"] {
                                if let Some(obj) = val.get(key) {
                                    let expires = obj.get("expires").and_then(|v| v.as_bool()).unwrap_or(false);
                                    if expires {
                                        if let Some(ttl) = obj.get("ttl").and_then(|v| v.as_u64()) {
                                            let wait_sec = ttl.saturating_sub(120).max(10);
                                            refresh_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(wait_sec));
                                        }
                                    }
                                }
                            }
                            if val.get("subscribe").is_some() {
                                failures = 0;
                                self.set_status(ChatSourceState::Connected, None).await;
                            }
                            if let Some(pub_data) = val.pointer("/push/pub/data") {
                                if let Some(ev) = parse_kick_push(pub_data) {
                                    let _ = self.tx.send(ev).await;
                                }
                            }
                        }
                    }
                }
            }

            if !self.cancel.is_cancelled() {
                if failures == 0 {
                    failures = 1;
                }
                self.set_status(
                    ChatSourceState::Error,
                    Some("Conexão com a Kick caiu. Reconectando…".to_string()),
                )
                .await;
                let wait = (3 * failures).min(60) as u64;
                tokio::select! {
                    _ = self.cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
                }
            }
        }
    }

    fn stop(&self) {
        self.cancel.cancel();
    }
}

fn parse_kick_push(pub_data: &serde_json::Value) -> Option<InternalEvent> {
    let event = pub_data.get("event")?.as_str()?;
    let raw = pub_data.get("data")?;
    let data: serde_json::Value = if let Some(s) = raw.as_str() {
        serde_json::from_str(s).ok()?
    } else {
        raw.clone()
    };

    if event == r"App\Events\MessageDeletedEvent" {
        let id = data.pointer("/message/id").and_then(|v| v.as_str())?;
        return Some(InternalEvent::Delete {
            platform: ChatPlatform::Kick,
            id: format!("kick-{id}"),
        });
    }
    if event != r"App\Events\ChatMessageEvent" {
        return None;
    }

    let id = data
        .get("id")
        .and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_u64().map(|n| n.to_string())))
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let sender = data.get("sender").unwrap_or(&serde_json::Value::Null);
    let username = sender
        .get("username")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let user = sender
        .get("slug")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| username.to_lowercase());
    let color = sender
        .pointer("/identity/color")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut badges = Vec::new();
    if let Some(arr) = sender.pointer("/identity/badges").and_then(|v| v.as_array()) {
        for b in arr {
            let mapped = match b.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                "broadcaster" => Some("broadcaster"),
                "moderator" => Some("moderator"),
                "vip" | "og" => Some("vip"),
                "subscriber" => Some("subscriber"),
                "founder" => Some("founder"),
                "verified" => Some("verified"),
                _ => None,
            };
            if let Some(m) = mapped {
                if !badges.iter().any(|x| x == m) {
                    badges.push(m.to_string());
                }
            }
        }
    }
    let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let ts = data
        .get("created_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.timestamp_millis().max(0) as u64)
        .unwrap_or_else(now_ms);

    Some(InternalEvent::Message(MultiChatMessage {
        id: format!("kick-{id}"),
        platform: ChatPlatform::Kick,
        kind: MultiChatMessageKind::Msg,
        user,
        name: username,
        color,
        badges,
        fragments: kick_fragments(content),
        ts,
    }))
}

fn urlencoding_component(input: &str) -> String {
    url::form_urlencoded::byte_serialize(input.as_bytes()).collect()
}

// ------------------------------------------------------------------ MultiChatHub

enum InternalEvent {
    Message(MultiChatMessage),
    Delete { platform: ChatPlatform, id: String },
    StatusChanged,
}

#[derive(Default)]
struct HubState {
    ring: VecDeque<MultiChatMessage>,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
}

impl HubState {
    fn key(platform: ChatPlatform, id: &str) -> String {
        let p = match platform {
            ChatPlatform::Twitch => "twitch",
            ChatPlatform::Youtube => "youtube",
            ChatPlatform::Kick => "kick",
        };
        format!("{p}:{id}")
    }

    /// Insere mensagem no ring buffer com deduplicação estrita por `${platform}:${id}`.
    fn insert_message(&mut self, msg: MultiChatMessage) -> bool {
        let k = Self::key(msg.platform, &msg.id);
        if !self.seen.insert(k.clone()) {
            return false;
        }
        self.seen_order.push_back(k);
        while self.seen_order.len() > MAX_SEEN_IDS {
            if let Some(old) = self.seen_order.pop_front() {
                self.seen.remove(&old);
            }
        }
        if self.ring.len() >= MAX_RING_MESSAGES {
            self.ring.pop_front();
        }
        self.ring.push_back(msg);
        true
    }

    fn delete_message(&mut self, platform: ChatPlatform, id: &str) {
        self.ring.retain(|m| !(m.platform == platform && m.id == id));
    }
}

/// Hub unificado de chat para Twitch (IRC), YouTube e Kick.
pub struct MultiChatHub {
    http: Client,
    event_tx: broadcast::Sender<MultiChatEvent>,
    internal_tx: mpsc::Sender<InternalEvent>,
    state: Arc<Mutex<HubState>>,
    twitch: Mutex<Option<(String, Arc<TwitchReader>)>>,
    youtube: Mutex<Option<(String, Arc<YouTubeReader>)>>,
    kick: Mutex<Option<(String, Arc<KickReader>)>>,
    stop_token: CancellationToken,
}

impl MultiChatHub {
    pub fn new() -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(256);
        let (internal_tx, internal_rx) = mpsc::channel(512);
        let hub = Arc::new(Self {
            http: build_http_client(),
            event_tx,
            internal_tx,
            state: Arc::new(Mutex::new(HubState::default())),
            twitch: Mutex::new(None),
            youtube: Mutex::new(None),
            kick: Mutex::new(None),
            stop_token: CancellationToken::new(),
        });
        let h = hub.clone();
        tokio::spawn(async move { h.event_loop(internal_rx).await });
        hub
    }

    pub fn subscribe(&self) -> broadcast::Receiver<MultiChatEvent> {
        self.event_tx.subscribe()
    }

    pub async fn snapshot_messages(&self) -> Vec<MultiChatMessage> {
        self.state.lock().await.ring.iter().cloned().collect()
    }

    pub async fn status(&self) -> Vec<ChatSourceStatus> {
        let tw = match self.twitch.lock().await.as_ref() {
            Some((_, r)) => r.status.read().await.clone(),
            None => ChatSourceStatus {
                platform: ChatPlatform::Twitch,
                state: ChatSourceState::Off,
                channel: String::new(),
                message: None,
            },
        };
        let yt = match self.youtube.lock().await.as_ref() {
            Some((_, r)) => r.status.read().await.clone(),
            None => ChatSourceStatus {
                platform: ChatPlatform::Youtube,
                state: ChatSourceState::Off,
                channel: String::new(),
                message: None,
            },
        };
        let kk = match self.kick.lock().await.as_ref() {
            Some((_, r)) => r.status.read().await.clone(),
            None => ChatSourceStatus {
                platform: ChatPlatform::Kick,
                state: ChatSourceState::Off,
                channel: String::new(),
                message: None,
            },
        };
        vec![tw, yt, kk]
    }

    pub async fn configure(&self, chat: &ChatSettings, linked_twitch_login: Option<&str>, force: bool) {
        let tw_target = if !chat.channel.trim().is_empty() {
            normalize_twitch_channel(&chat.channel)
        } else {
            linked_twitch_login
                .map(normalize_twitch_channel)
                .unwrap_or_default()
        };
        let yt_target = chat.youtube.trim().to_string();
        let kick_target = chat.kick.trim().to_string();

        {
            let mut guard = self.twitch.lock().await;
            let same = guard.as_ref().is_some_and(|(k, _)| k == &tw_target);
            if force || !same {
                if let Some((_, old)) = guard.take() {
                    old.stop();
                }
                if !tw_target.is_empty() {
                    let r = TwitchReader::spawn(tw_target.clone(), self.internal_tx.clone(), self.http.clone());
                    *guard = Some((tw_target, r));
                }
            }
        }
        {
            let mut guard = self.youtube.lock().await;
            let same = guard.as_ref().is_some_and(|(k, _)| k == &yt_target);
            if force || !same {
                if let Some((_, old)) = guard.take() {
                    old.stop();
                }
                if !yt_target.is_empty() {
                    let r = YouTubeReader::spawn(yt_target.clone(), self.internal_tx.clone(), self.http.clone());
                    *guard = Some((yt_target, r));
                }
            }
        }
        {
            let mut guard = self.kick.lock().await;
            let same = guard.as_ref().is_some_and(|(k, _)| k == &kick_target);
            if force || !same {
                if let Some((_, old)) = guard.take() {
                    old.stop();
                }
                if !kick_target.is_empty() {
                    let r = KickReader::spawn(kick_target.clone(), self.internal_tx.clone(), self.http.clone());
                    *guard = Some((kick_target, r));
                }
            }
        }

        let _ = self.internal_tx.send(InternalEvent::StatusChanged).await;
    }

    pub async fn dispose(&self) {
        self.stop_token.cancel();
        if let Some((_, r)) = self.twitch.lock().await.take() {
            r.stop();
        }
        if let Some((_, r)) = self.youtube.lock().await.take() {
            r.stop();
        }
        if let Some((_, r)) = self.kick.lock().await.take() {
            r.stop();
        }
    }

    async fn event_loop(self: Arc<Self>, mut rx: mpsc::Receiver<InternalEvent>) {
        let mut pending: Vec<MultiChatMessage> = Vec::new();
        let mut flush = tokio::time::interval(Duration::from_millis(BATCH_FLUSH_MS));
        flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = self.stop_token.cancelled() => break,
                _ = flush.tick() => {
                    if !pending.is_empty() {
                        let batch = std::mem::take(&mut pending);
                        let _ = self.event_tx.send(MultiChatEvent::Messages { messages: batch });
                    }
                }
                ev = rx.recv() => {
                    let Some(ev) = ev else { break };
                    match ev {
                        InternalEvent::Message(msg) => {
                            let accepted = self.state.lock().await.insert_message(msg.clone());
                            if accepted {
                                pending.push(msg);
                            }
                        }
                        InternalEvent::Delete { platform, id } => {
                            self.state.lock().await.delete_message(platform, &id);
                            let _ = self.event_tx.send(MultiChatEvent::Delete { platform, id });
                        }
                        InternalEvent::StatusChanged => {
                            let st = self.status().await;
                            let _ = self.event_tx.send(MultiChatEvent::Status { status: st });
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unescape_tag_single_pass() {
        assert_eq!(unescape_tag(r"olá\smundo\:teste\\s"), r"olá mundo;teste\s");
        assert_eq!(unescape_tag(r"a\r\nb"), "ab");
        assert_eq!(unescape_tag(r"\\"), r"\");
    }

    #[test]
    fn test_twitch_fragments_and_bounds_check() {
        let frags = twitch_fragments("Oi Kappa 😁!", Some("25:3-7/999:50-80"), None);
        assert_eq!(frags.len(), 3);
        assert!(matches!(&frags[0], ChatFragment::Text { text } if text == "Oi "));
        match &frags[1] {
            ChatFragment::Emote { name, url, src_set, .. } => {
                assert_eq!(name, "Kappa");
                assert!(url.contains("/25/"));
                assert!(src_set.as_deref().unwrap().contains("3.0 3x"));
            }
            _ => panic!("expected emote"),
        }
        assert!(matches!(&frags[2], ChatFragment::Text { text } if text == " 😁!"));
    }

    #[test]
    fn test_contains_mention_word_boundary() {
        assert!(contains_mention("Salve @ana tudo bem?", "ana"));
        assert!(contains_mention("@ANA!", "ana"));
        assert!(!contains_mention("oi @anaclara tudo bem?", "ana"));
        assert!(!contains_mention("email@ana.com", "ana"));
    }

    #[test]
    fn test_dedup_keyed_by_platform_and_id() {
        let mut hub = HubState::default();
        let make = |platform, id: &str| MultiChatMessage {
            id: id.to_string(),
            platform,
            kind: MultiChatMessageKind::Msg,
            user: "u".into(),
            name: "U".into(),
            color: String::new(),
            badges: vec![],
            fragments: vec![],
            ts: 1,
        };
        assert!(hub.insert_message(make(ChatPlatform::Twitch, "123")));
        assert!(!hub.insert_message(make(ChatPlatform::Twitch, "123")));
        // Mesmo ID em plataforma diferente NÃO colide!
        assert!(hub.insert_message(make(ChatPlatform::Kick, "123")));
        assert_eq!(hub.ring.len(), 2);
    }

    #[test]
    fn test_7tv_and_twitch_emote_expansion() {
        let mut cat = EmoteCatalog::new();
        cat.insert(ExternalEmote {
            name: "KEKW".to_string(),
            url: "https://cdn.7tv.app/emote/abc/1x.webp".to_string(),
            src_set: Some("https://cdn.7tv.app/emote/abc/1x.webp 1x".to_string()),
            provider: "7tv".to_string(),
        });
        let frags = twitch_fragments("Kappa KEKW gg", Some("25:0-4"), Some(&cat));
        assert_eq!(frags.len(), 4);
        assert!(matches!(&frags[0], ChatFragment::Emote { name, provider: Some(p), .. } if name == "Kappa" && p == "twitch"));
        assert!(matches!(&frags[1], ChatFragment::Text { text } if text == " "));
        assert!(matches!(&frags[2], ChatFragment::Emote { name, provider: Some(p), .. } if name == "KEKW" && p == "7tv"));
        assert!(matches!(&frags[3], ChatFragment::Text { text } if text == " gg"));
    }

    #[test]
    fn test_kick_slug_and_fragments() {
        assert_eq!(sanitize_kick_slug("https://kick.com/Kayo_Live").unwrap(), "kayo_live");
        assert_eq!(sanitize_kick_slug("@kayo-1").unwrap(), "kayo-1");
        let frags = kick_fragments("bom dia [emote:37226:KEKW]!");
        assert_eq!(frags.len(), 3);
        assert!(matches!(&frags[1], ChatFragment::Emote { name, url, .. } if name == "KEKW" && url.contains("37226")));
    }
}
