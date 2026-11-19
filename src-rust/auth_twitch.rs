use crate::protocol::{
    LiveInfo, PlatformAccount, PlatformId, TwitchActionResult, TwitchCapabilities, TwitchCategory,
    TwitchLoginState, PRISMCAST_TWITCH_CLIENT_ID,
};
use regex_lite::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use url::Url;

pub const SCOPE_STREAM_KEY: &str = "channel:read:stream_key";
pub const SCOPE_INFO: &str = "channel:manage:broadcast";
pub const SCOPE_CHAT: &str = "user:write:chat";

const TWITCH_TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const TWITCH_DEVICE_URL: &str = "https://id.twitch.tv/oauth2/device";
const TWITCH_HELIX_BASE: &str = "https://api.twitch.tv/helix";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

fn twitch_scopes() -> String {
    format!("{SCOPE_STREAM_KEY} {SCOPE_INFO} {SCOPE_CHAT}")
}

/// Erro tipado das operações de autenticação e API da Twitch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    NeedsLogin(String),
    ScopeMissing(String),
    Cancelled,
    Network(String),
    Rejected(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::NeedsLogin(m)
            | AuthError::ScopeMissing(m)
            | AuthError::Network(m)
            | AuthError::Rejected(m) => write!(f, "{m}"),
            AuthError::Cancelled => write!(f, "Login cancelado."),
        }
    }
}

impl std::error::Error for AuthError {}

/// Valida o formato da chave de transmissão por plataforma.
pub fn validate_stream_key(platform: PlatformId, raw_key: &str) -> Result<Option<String>, String> {
    let key = raw_key.trim();
    if key.is_empty() {
        return Ok(None);
    }
    if key.chars().any(char::is_whitespace) {
        return Err("A chave não pode ter espaços.".to_string());
    }

    static TWITCH_RE: OnceLock<Regex> = OnceLock::new();
    static YOUTUBE_RE: OnceLock<Regex> = OnceLock::new();
    static KICK_RE: OnceLock<Regex> = OnceLock::new();

    match platform {
        PlatformId::Twitch => {
            let re = TWITCH_RE.get_or_init(|| {
                Regex::new(r"^live_\d{3,}_[A-Za-z0-9]{16,}$").expect("valid twitch key regex")
            });
            if !re.is_match(key) {
                return Err(
                    "A chave da Twitch tem o formato live_123456789_xxxxxxxx… (copie inteira do painel)."
                        .to_string(),
                );
            }
        }
        PlatformId::Youtube => {
            let re = YOUTUBE_RE.get_or_init(|| {
                Regex::new(r"(?i)^[a-z0-9]{4}(-[a-z0-9]{4}){3,4}$")
                    .expect("valid youtube key regex")
            });
            if !re.is_match(key) {
                return Err("A chave do YouTube tem o formato xxxx-xxxx-xxxx-xxxx-xxxx.".to_string());
            }
        }
        PlatformId::Kick => {
            let re = KICK_RE
                .get_or_init(|| Regex::new(r"^sk_[A-Za-z0-9_.-]{10,}$").expect("valid kick regex"));
            if !re.is_match(key) {
                return Err("A chave do Kick começa com \"sk_\".".to_string());
            }
        }
        PlatformId::Tiktok | PlatformId::Custom => {}
    }

    Ok(Some(key.to_string()))
}

/// Garante que uma URL externa de verificação OAuth usa estritamente `https://` e host esperado.
pub fn validate_https_url(raw: &str) -> Result<Url, String> {
    let parsed = Url::parse(raw.trim()).map_err(|_| "URL inválida.".to_string())?;
    if parsed.scheme() != "https" {
        return Err("URL externa deve usar https://.".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("URL externa sem host.".to_string());
    }
    Ok(parsed)
}

/// Resultado completo do fluxo OAuth Device Code da Twitch (segredos ficam no cofre).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceLoginOutcome {
    pub destination_id: String,
    pub account: PlatformAccount,
    pub stream_key: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    scope: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    message: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HelixUsersResponse {
    data: Vec<HelixUser>,
}

#[derive(Debug, Deserialize)]
struct HelixUser {
    id: String,
    login: String,
    display_name: String,
    profile_image_url: String,
}

#[derive(Debug, Deserialize)]
struct HelixStreamKeyResponse {
    data: Vec<HelixStreamKey>,
}

#[derive(Debug, Deserialize)]
struct HelixStreamKey {
    stream_key: String,
}

#[derive(Debug, Deserialize)]
struct HelixChannelsResponse {
    data: Vec<HelixChannel>,
}

#[derive(Debug, Deserialize)]
struct HelixChannel {
    title: String,
    game_id: String,
    game_name: String,
}

#[derive(Debug, Deserialize)]
struct HelixCategoriesResponse {
    data: Vec<HelixCategory>,
}

#[derive(Debug, Deserialize)]
struct HelixCategory {
    id: String,
    name: String,
    box_art_url: String,
}

#[derive(Debug, Deserialize)]
struct HelixChatResponse {
    data: Option<Vec<HelixChatItem>>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HelixChatItem {
    is_sent: bool,
    drop_reason: Option<HelixDropReason>,
}

#[derive(Debug, Deserialize)]
struct HelixDropReason {
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HelixErrorBody {
    message: Option<String>,
}

/// Abstração para leitura/gravação do refresh token no cofre seguro (DPAPI/keyring).
pub trait TokenVault: Send + Sync {
    fn get_refresh_token(&self, destination_id: &str) -> Option<String>;
    fn set_refresh_token(&self, destination_id: &str, token: Option<&str>);
}

/// Implementação em memória para testes unitários ou uso isolado.
#[derive(Default)]
pub struct MemoryTokenVault {
    tokens: std::sync::Mutex<HashMap<String, String>>,
}

impl TokenVault for MemoryTokenVault {
    fn get_refresh_token(&self, destination_id: &str) -> Option<String> {
        self.tokens.lock().ok()?.get(destination_id).cloned()
    }

    fn set_refresh_token(&self, destination_id: &str, token: Option<&str>) {
        if let Ok(mut guard) = self.tokens.lock() {
            match token {
                Some(t) if !t.is_empty() => {
                    guard.insert(destination_id.to_string(), t.to_string());
                }
                _ => {
                    guard.remove(destination_id);
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct CachedToken {
    destination_id: String,
    token: String,
    expires_at: Instant,
}

/// Gerencia o fluxo OAuth Device Code da Twitch com cancelamento seguro.
pub struct TwitchDeviceLoginManager {
    http: Client,
    active_cancel: Mutex<Option<CancellationToken>>,
}

impl TwitchDeviceLoginManager {
    pub fn new() -> Self {
        let http = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            http,
            active_cancel: Mutex::new(None),
        }
    }

    pub async fn cancel_device_login(&self) {
        let mut guard = self.active_cancel.lock().await;
        if let Some(tok) = guard.take() {
            tok.cancel();
        }
    }

    /// Inicia o fluxo Device Code Flow, emitindo atualizações de `TwitchLoginState` via `on_state`.
    pub async fn start_device_login<F>(
        &self,
        destination_id: &str,
        custom_client_id: &str,
        mut on_state: F,
    ) -> Result<DeviceLoginOutcome, AuthError>
    where
        F: FnMut(TwitchLoginState) + Send,
    {
        let token = CancellationToken::new();
        {
            let mut guard = self.active_cancel.lock().await;
            if let Some(prev) = guard.replace(token.clone()) {
                prev.cancel();
            }
        }

        let client_id = resolve_client_id(custom_client_id);
        let result = self
            .run_device_login(destination_id, &client_id, &token, &mut on_state)
            .await;

        let mut guard = self.active_cancel.lock().await;
        if guard.as_ref().is_some_and(|t| t.is_cancelled() || token.is_cancelled()) {
            // Limpa apenas se for o token desta tentativa.
            *guard = None;
        } else {
            *guard = None;
        }

        match &result {
            Err(AuthError::Cancelled) => {}
            Err(e) => {
                on_state(TwitchLoginState::Error {
                    message: e.to_string(),
                });
            }
            Ok(outcome) => {
                on_state(TwitchLoginState::Done {
                    account: outcome.account.clone(),
                });
            }
        }

        result
    }

    async fn run_device_login<F>(
        &self,
        destination_id: &str,
        client_id: &str,
        cancel: &CancellationToken,
        on_state: &mut F,
    ) -> Result<DeviceLoginOutcome, AuthError>
    where
        F: FnMut(TwitchLoginState) + Send,
    {
        let scopes = twitch_scopes();
        let device_res = tokio::select! {
            _ = cancel.cancelled() => return Err(AuthError::Cancelled),
            res = self.http
                .post(TWITCH_DEVICE_URL)
                .form(&[("client_id", client_id), ("scopes", scopes.as_str())])
                .send() => res.map_err(|e| AuthError::Network(e.to_string()))?,
        };

        if !device_res.status().is_success() {
            let status = device_res.status().as_u16();
            let msg = if status == 400 {
                if client_id == PRISMCAST_TWITCH_CLIENT_ID {
                    "A Twitch recusou o login do PrismCast agora. Tente de novo em instantes ou cole a chave abaixo.".to_string()
                } else {
                    "A Twitch recusou o seu Client ID. Confira se o app é do tipo \"Público\", ou apague o campo para usar o do PrismCast.".to_string()
                }
            } else {
                format!("A Twitch respondeu {status} ao pedir o código.")
            };
            return Err(AuthError::Rejected(msg));
        }

        let device: DeviceCodeResponse = device_res
            .json()
            .await
            .map_err(|e| AuthError::Network(e.to_string()))?;

        let verified_url = validate_https_url(&device.verification_uri)
            .map_err(|e| AuthError::Rejected(format!("URL de ativação insegura: {e}")))?;

        let expires_at_ms = now_ms() + device.expires_in.saturating_mul(1000);
        let deadline = Instant::now() + Duration::from_secs(device.expires_in);
        on_state(TwitchLoginState::Code {
            user_code: device.user_code,
            verification_uri: verified_url.to_string(),
            expires_at: expires_at_ms,
        });

        let mut interval = Duration::from_secs(device.interval.max(1));
        let token_res: TokenResponse = loop {
            if Instant::now() >= deadline {
                return Err(AuthError::Rejected("O código expirou. Tente de novo.".to_string()));
            }
            tokio::select! {
                _ = cancel.cancelled() => return Err(AuthError::Cancelled),
                _ = tokio::time::sleep(interval) => {}
            }

            let res = tokio::select! {
                _ = cancel.cancelled() => return Err(AuthError::Cancelled),
                res = self.http
                    .post(TWITCH_TOKEN_URL)
                    .form(&[
                        ("client_id", client_id),
                        ("scopes", scopes.as_str()),
                        ("device_code", device.device_code.as_str()),
                        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ])
                    .send() => res.map_err(|e| AuthError::Network(e.to_string()))?,
            };

            if res.status().is_success() {
                break res
                    .json()
                    .await
                    .map_err(|e| AuthError::Network(e.to_string()))?;
            }

            let status = res.status().as_u16();
            let err_body = res.json::<TokenErrorResponse>().await.ok();
            let code = err_body
                .as_ref()
                .and_then(|b| b.message.as_deref().or(b.error.as_deref()))
                .unwrap_or("");
            match code {
                "authorization_pending" => continue,
                "slow_down" => {
                    interval += Duration::from_secs(5);
                    continue;
                }
                "access_denied" => {
                    return Err(AuthError::Rejected("Você recusou o acesso na Twitch.".to_string()));
                }
                other => {
                    let detail = if other.is_empty() {
                        status.to_string()
                    } else {
                        other.to_string()
                    };
                    return Err(AuthError::Rejected(format!(
                        "A Twitch não liberou o login ({detail})."
                    )));
                }
            }
        };

        let user_res = tokio::select! {
            _ = cancel.cancelled() => return Err(AuthError::Cancelled),
            res = self.http
                .get(format!("{TWITCH_HELIX_BASE}/users"))
                .bearer_auth(&token_res.access_token)
                .header("Client-Id", client_id)
                .send() => res.map_err(|e| AuthError::Network(e.to_string()))?,
        };
        if !user_res.status().is_success() {
            return Err(AuthError::Rejected(format!(
                "Não deu para ler sua conta ({}).",
                user_res.status().as_u16()
            )));
        }
        let user = user_res
            .json::<HelixUsersResponse>()
            .await
            .map_err(|e| AuthError::Network(e.to_string()))?
            .data
            .into_iter()
            .next()
            .ok_or_else(|| AuthError::Rejected("A Twitch não devolveu os dados da conta.".to_string()))?;

        let key_res = tokio::select! {
            _ = cancel.cancelled() => return Err(AuthError::Cancelled),
            res = self.http
                .get(format!("{TWITCH_HELIX_BASE}/streams/key"))
                .query(&[("broadcaster_id", user.id.as_str())])
                .bearer_auth(&token_res.access_token)
                .header("Client-Id", client_id)
                .send() => res.map_err(|e| AuthError::Network(e.to_string()))?,
        };
        if !key_res.status().is_success() {
            return Err(AuthError::Rejected(format!(
                "Não deu para buscar a chave de transmissão ({}).",
                key_res.status().as_u16()
            )));
        }
        let stream_key = key_res
            .json::<HelixStreamKeyResponse>()
            .await
            .map_err(|e| AuthError::Network(e.to_string()))?
            .data
            .into_iter()
            .next()
            .map(|k| k.stream_key)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                AuthError::Rejected("A Twitch não devolveu a chave de transmissão.".to_string())
            })?;

        let account = PlatformAccount {
            login: user.login,
            display_name: user.display_name,
            avatar_url: user.profile_image_url,
            id: Some(user.id),
            scopes: Some(token_res.scope.unwrap_or_else(|| {
                vec![
                    SCOPE_STREAM_KEY.to_string(),
                    SCOPE_INFO.to_string(),
                    SCOPE_CHAT.to_string(),
                ]
            })),
        };

        Ok(DeviceLoginOutcome {
            destination_id: destination_id.to_string(),
            account,
            stream_key,
            access_token: token_res.access_token,
            refresh_token: token_res.refresh_token,
            expires_in: token_res.expires_in.unwrap_or(3600),
        })
    }
}

impl Default for TwitchDeviceLoginManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Cliente Twitch Helix com cache em memória, deduplicação de refresh e retry único em 401.
pub struct TwitchApi {
    http: Client,
    helix_base: String,
    token_url: String,
    vault: Arc<dyn TokenVault>,
    cached: Mutex<Option<CachedToken>>,
    refresh_lock: Mutex<()>,
    broadcaster_cache: Mutex<HashMap<String, String>>,
}

impl TwitchApi {
    pub fn new(vault: Arc<dyn TokenVault>) -> Self {
        Self::with_endpoints(vault, TWITCH_HELIX_BASE, TWITCH_TOKEN_URL)
    }

    pub fn with_endpoints(vault: Arc<dyn TokenVault>, helix_base: &str, token_url: &str) -> Self {
        let http = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            http,
            helix_base: helix_base.trim_end_matches('/').to_string(),
            token_url: token_url.to_string(),
            vault,
            cached: Mutex::new(None),
            refresh_lock: Mutex::new(()),
            broadcaster_cache: Mutex::new(HashMap::new()),
        }
    }

    pub async fn remember_access_token(&self, destination_id: &str, token: &str, expires_in: u64) {
        let ttl = expires_in.saturating_sub(60).max(5);
        let mut guard = self.cached.lock().await;
        *guard = Some(CachedToken {
            destination_id: destination_id.to_string(),
            token: token.to_string(),
            expires_at: Instant::now() + Duration::from_secs(ttl),
        });
    }

    pub async fn clear_cached_token(&self) {
        *self.cached.lock().await = None;
        self.broadcaster_cache.lock().await.clear();
    }

    pub fn capabilities(account: Option<&PlatformAccount>) -> TwitchCapabilities {
        let Some(acc) = account else {
            return TwitchCapabilities::default();
        };
        let scopes = acc.scopes.as_deref().unwrap_or(&[]);
        TwitchCapabilities {
            linked: true,
            login: Some(acc.login.clone()),
            can_edit_info: scopes.iter().any(|s| s == SCOPE_INFO),
            can_chat: scopes.iter().any(|s| s == SCOPE_CHAT),
        }
    }

    async fn get_access_token(
        &self,
        destination_id: &str,
        custom_client_id: &str,
        force_refresh: bool,
    ) -> Result<String, AuthError> {
        if !force_refresh {
            if let Some(c) = self.cached.lock().await.as_ref() {
                if c.destination_id == destination_id && Instant::now() < c.expires_at {
                    return Ok(c.token.clone());
                }
            }
        }

        // Serializa refreshes concorrentes: quem esperar na trava reutiliza o novo token do cache.
        let _refresh_guard = self.refresh_lock.lock().await;
        if !force_refresh {
            if let Some(c) = self.cached.lock().await.as_ref() {
                if c.destination_id == destination_id && Instant::now() < c.expires_at {
                    return Ok(c.token.clone());
                }
            }
        }

        let refresh = self.vault.get_refresh_token(destination_id).ok_or_else(|| {
            AuthError::NeedsLogin("O login da Twitch expirou. Entre de novo em Destinos.".to_string())
        })?;

        let cid = resolve_client_id(custom_client_id);
        let res = self
            .http
            .post(&self.token_url)
            .form(&[
                ("client_id", cid.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh.as_str()),
            ])
            .send()
            .await
            .map_err(|e| AuthError::Network(e.to_string()))?;

        if !res.status().is_success() {
            *self.cached.lock().await = None;
            return Err(AuthError::NeedsLogin(
                "O login da Twitch expirou. Entre de novo em Destinos.".to_string(),
            ));
        }

        let json: TokenResponse = res
            .json()
            .await
            .map_err(|e| AuthError::Network(e.to_string()))?;
        if let Some(next_refresh) = json.refresh_token.as_deref() {
            self.vault
                .set_refresh_token(destination_id, Some(next_refresh));
        }
        self.remember_access_token(destination_id, &json.access_token, json.expires_in.unwrap_or(3600))
            .await;
        Ok(json.access_token)
    }

    async fn helix_request(
        &self,
        destination_id: &str,
        custom_client_id: &str,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&serde_json::Value>,
    ) -> Result<reqwest::Response, AuthError> {
        let cid = resolve_client_id(custom_client_id);
        let url = format!("{}/{}", self.helix_base, path.trim_start_matches('/'));

        for attempt in 0..2 {
            let token = self
                .get_access_token(destination_id, &cid, attempt == 1)
                .await?;
            let mut req = self
                .http
                .request(method.clone(), &url)
                .bearer_auth(&token)
                .header("Client-Id", &cid);
            if !query.is_empty() {
                req = req.query(query);
            }
            if let Some(json_body) = body {
                req = req.json(json_body);
            }
            let res = req.send().await.map_err(|e| AuthError::Network(e.to_string()))?;
            if res.status().as_u16() == 401 && attempt == 0 {
                *self.cached.lock().await = None;
                continue;
            }
            if res.status().as_u16() == 401 {
                *self.cached.lock().await = None;
                return Err(AuthError::NeedsLogin(
                    "O login da Twitch expirou. Entre de novo em Destinos.".to_string(),
                ));
            }
            return Ok(res);
        }

        Err(AuthError::NeedsLogin(
            "O login da Twitch expirou. Entre de novo em Destinos.".to_string(),
        ))
    }

    /// Resolve o `broadcaster_id` da conta autenticada (checa `res.status().is_success()`).
    pub async fn own_broadcaster_id(
        &self,
        destination_id: &str,
        account: &PlatformAccount,
        custom_client_id: &str,
    ) -> Result<String, AuthError> {
        if let Some(id) = account.id.as_deref().filter(|s| !s.is_empty()) {
            return Ok(id.to_string());
        }
        let res = self
            .helix_request(
                destination_id,
                custom_client_id,
                reqwest::Method::GET,
                "users",
                &[],
                None,
            )
            .await?;
        if !res.status().is_success() {
            return Err(AuthError::Rejected(format!(
                "Não foi possível obter a conta da Twitch ({}).",
                res.status().as_u16()
            )));
        }
        let users: HelixUsersResponse = res
            .json()
            .await
            .map_err(|e| AuthError::Network(e.to_string()))?;
        users
            .data
            .into_iter()
            .next()
            .map(|u| u.id)
            .ok_or_else(|| AuthError::Rejected("A Twitch não devolveu a conta.".to_string()))
    }

    /// Resolve o `broadcaster_id` de um canal alvo pelo login (ex.: chat aberto de outro canal).
    pub async fn resolve_channel_id(
        &self,
        destination_id: &str,
        account: &PlatformAccount,
        target_channel: &str,
        custom_client_id: &str,
    ) -> Result<String, AuthError> {
        let normalized = target_channel.trim().trim_start_matches('#').to_lowercase();
        if normalized.is_empty() || normalized == account.login.to_lowercase() {
            return self
                .own_broadcaster_id(destination_id, account, custom_client_id)
                .await;
        }
        if let Some(cached) = self.broadcaster_cache.lock().await.get(&normalized).cloned() {
            return Ok(cached);
        }
        let res = self
            .helix_request(
                destination_id,
                custom_client_id,
                reqwest::Method::GET,
                "users",
                &[("login", normalized.as_str())],
                None,
            )
            .await?;
        if !res.status().is_success() {
            return Err(AuthError::Rejected(format!(
                "Canal #{normalized} não encontrado na Twitch ({}).",
                res.status().as_u16()
            )));
        }
        let users: HelixUsersResponse = res
            .json()
            .await
            .map_err(|e| AuthError::Network(e.to_string()))?;
        let id = users
            .data
            .into_iter()
            .next()
            .map(|u| u.id)
            .ok_or_else(|| AuthError::Rejected(format!("Canal #{normalized} não encontrado.")))?;
        self.broadcaster_cache.lock().await.insert(normalized, id.clone());
        Ok(id)
    }

    pub async fn get_channel_info(
        &self,
        destination_id: &str,
        account: &PlatformAccount,
        custom_client_id: &str,
    ) -> Result<Option<LiveInfo>, AuthError> {
        let id = self
            .own_broadcaster_id(destination_id, account, custom_client_id)
            .await?;
        let res = self
            .helix_request(
                destination_id,
                custom_client_id,
                reqwest::Method::GET,
                "channels",
                &[("broadcaster_id", id.as_str())],
                None,
            )
            .await?;
        if !res.status().is_success() {
            return Ok(None);
        }
        let body: HelixChannelsResponse = res
            .json()
            .await
            .map_err(|e| AuthError::Network(e.to_string()))?;
        Ok(body.data.into_iter().next().map(|ch| LiveInfo {
            title: ch.title,
            category_id: ch.game_id,
            category_name: ch.game_name,
        }))
    }

    pub async fn update_channel_info(
        &self,
        destination_id: &str,
        account: &PlatformAccount,
        info: &LiveInfo,
        custom_client_id: &str,
    ) -> TwitchActionResult {
        let id = match self
            .own_broadcaster_id(destination_id, account, custom_client_id)
            .await
        {
            Ok(id) => id,
            Err(e) => return action_err(e),
        };
        let title: String = info.title.chars().take(140).collect();
        let mut payload = serde_json::json!({ "title": title });
        if !info.category_id.trim().is_empty() {
            payload["game_id"] = serde_json::Value::String(info.category_id.clone());
        }

        let res = match self
            .helix_request(
                destination_id,
                custom_client_id,
                reqwest::Method::PATCH,
                "channels",
                &[("broadcaster_id", id.as_str())],
                Some(&payload),
            )
            .await
        {
            Ok(r) => r,
            Err(e) => return action_err(e),
        };

        let status = res.status().as_u16();
        if status == 204 || res.status().is_success() {
            return TwitchActionResult {
                ok: true,
                error: None,
                needs_relogin: None,
            };
        }
        if status == 403 {
            return TwitchActionResult {
                ok: false,
                error: Some(
                    "O login atual da Twitch não deu essa permissão. Clique em “Entrar de novo com a Twitch”."
                        .to_string(),
                ),
                needs_relogin: Some(true),
            };
        }
        let err_msg = res
            .json::<HelixErrorBody>()
            .await
            .ok()
            .and_then(|b| b.message)
            .unwrap_or_else(|| status.to_string());
        TwitchActionResult {
            ok: false,
            error: Some(format!("A Twitch recusou: {err_msg}")),
            needs_relogin: None,
        }
    }

    pub async fn search_categories(
        &self,
        destination_id: &str,
        query: &str,
        custom_client_id: &str,
    ) -> Vec<TwitchCategory> {
        let q = query.trim();
        if q.is_empty() {
            return Vec::new();
        }
        let Ok(res) = self
            .helix_request(
                destination_id,
                custom_client_id,
                reqwest::Method::GET,
                "search/categories",
                &[("first", "8"), ("query", q)],
                None,
            )
            .await
        else {
            return Vec::new();
        };
        if !res.status().is_success() {
            return Vec::new();
        }
        let Ok(body) = res.json::<HelixCategoriesResponse>().await else {
            return Vec::new();
        };
        body.data
            .into_iter()
            .map(|c| TwitchCategory {
                id: c.id,
                name: c.name,
                box_art_url: c
                    .box_art_url
                    .replace("{width}", "52")
                    .replace("{height}", "72"),
            })
            .collect()
    }

    /// Envia mensagem no chat do canal `target_channel` (ou do próprio broadcaster se vazio).
    pub async fn send_chat(
        &self,
        destination_id: &str,
        account: &PlatformAccount,
        target_channel: &str,
        text: &str,
        custom_client_id: &str,
    ) -> TwitchActionResult {
        let message: String = text.trim().chars().take(500).collect();
        if message.is_empty() {
            return TwitchActionResult {
                ok: true,
                error: None,
                needs_relogin: None,
            };
        }

        let sender_id = match self
            .own_broadcaster_id(destination_id, account, custom_client_id)
            .await
        {
            Ok(id) => id,
            Err(e) => return action_err(e),
        };
        let broadcaster_id = match self
            .resolve_channel_id(destination_id, account, target_channel, custom_client_id)
            .await
        {
            Ok(id) => id,
            Err(e) => return action_err(e),
        };

        let payload = serde_json::json!({
            "broadcaster_id": broadcaster_id,
            "sender_id": sender_id,
            "message": message,
        });

        let res = match self
            .helix_request(
                destination_id,
                custom_client_id,
                reqwest::Method::POST,
                "chat/messages",
                &[],
                Some(&payload),
            )
            .await
        {
            Ok(r) => r,
            Err(e) => return action_err(e),
        };

        parse_helix_chat_response(res, account.scopes.as_deref()).await
    }
}

pub(crate) async fn parse_helix_chat_response(
    res: reqwest::Response,
    scopes: Option<&[String]>,
) -> TwitchActionResult {
    let status = res.status().as_u16();
    let ok = res.status().is_success();
    let body = res.json::<HelixChatResponse>().await.ok();
    parse_helix_chat_parts(status, ok, body.as_ref(), scopes)
}

fn parse_helix_chat_parts(
    status: u16,
    ok: bool,
    body: Option<&HelixChatResponse>,
    scopes: Option<&[String]>,
) -> TwitchActionResult {
    if ok {
        if let Some(item) = body.and_then(|b| b.data.as_ref()).and_then(|d| d.first()) {
            if item.is_sent {
                return TwitchActionResult {
                    ok: true,
                    error: None,
                    needs_relogin: None,
                };
            }
            let reason = item
                .drop_reason
                .as_ref()
                .and_then(|r| r.message.clone())
                .unwrap_or_else(|| "Mensagem recusada pelo chat da Twitch.".to_string());
            return TwitchActionResult {
                ok: false,
                error: Some(reason),
                needs_relogin: None,
            };
        }
    }

    if status == 403 {
        let server_msg = body.and_then(|b| b.message.as_deref()).unwrap_or("");
        let has_scope = scopes
            .map(|s| s.iter().any(|x| x == SCOPE_CHAT))
            .unwrap_or(false);
        let lower = server_msg.to_lowercase();
        let looks_like_scope = !has_scope || lower.contains("missing scope") || lower.contains("insufficient");
        if looks_like_scope {
            return TwitchActionResult {
                ok: false,
                error: Some(
                    "O login atual da Twitch não deu permissão para escrever no chat. Entre novamente com a Twitch."
                        .to_string(),
                ),
                needs_relogin: Some(true),
            };
        }
        // Moderação do canal (sub-only, follower-only, ban, timeout) não exige relogin!
        let reason = if server_msg.is_empty() {
            "O chat deste canal recusou a mensagem (modo restrito ou silenciado).".to_string()
        } else {
            server_msg.to_string()
        };
        return TwitchActionResult {
            ok: false,
            error: Some(reason),
            needs_relogin: Some(false),
        };
    }

    let msg = body
        .and_then(|b| b.message.clone())
        .unwrap_or_else(|| format!("A Twitch respondeu {status}."));
    TwitchActionResult {
        ok: false,
        error: Some(msg),
        needs_relogin: None,
    }
}

fn action_err(err: AuthError) -> TwitchActionResult {
    match err {
        AuthError::NeedsLogin(m) | AuthError::ScopeMissing(m) => TwitchActionResult {
            ok: false,
            error: Some(m),
            needs_relogin: Some(true),
        },
        other => TwitchActionResult {
            ok: false,
            error: Some(other.to_string()),
            needs_relogin: None,
        },
    }
}

fn resolve_client_id(custom: &str) -> String {
    let trimmed = custom.trim();
    if trimmed.is_empty() {
        PRISMCAST_TWITCH_CLIENT_ID.to_string()
    } else {
        trimmed.to_string()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_stream_key_platforms() {
        assert_eq!(validate_stream_key(PlatformId::Twitch, "   ").unwrap(), None);
        assert!(validate_stream_key(PlatformId::Twitch, "live_123456_AbCdEfGh12345678").is_ok());
        assert!(validate_stream_key(PlatformId::Twitch, "live_twitch_abc").is_err());
        assert!(validate_stream_key(PlatformId::Twitch, "live_123456_abc def1234567890").is_err());

        assert!(validate_stream_key(PlatformId::Youtube, "abcd-efgh-ijkl-mnop-qrst").is_ok());
        assert!(validate_stream_key(PlatformId::Youtube, "invalid").is_err());

        assert!(validate_stream_key(PlatformId::Kick, "sk_live_1234567890").is_ok());
        assert!(validate_stream_key(PlatformId::Kick, "pk_wrong_1234567890").is_err());

        assert_eq!(
            validate_stream_key(PlatformId::Tiktok, "any-key").unwrap(),
            Some("any-key".to_string())
        );
    }

    #[test]
    fn test_validate_https_url() {
        assert!(validate_https_url("https://www.twitch.tv/activate").is_ok());
        assert!(validate_https_url("http://www.twitch.tv/activate").is_err());
        assert!(validate_https_url("file:///C:/Windows/cmd.exe").is_err());
        assert!(validate_https_url("ms-msdt:/id").is_err());
    }

    #[test]
    fn test_parse_helix_chat_403_distinguishes_moderation_from_scope() {
        let scopes = vec![SCOPE_CHAT.to_string()];
        let mod_body = HelixChatResponse {
            data: None,
            message: Some("This room is in subscribers-only mode.".to_string()),
        };
        let mod_res = parse_helix_chat_parts(403, false, Some(&mod_body), Some(&scopes));
        assert!(!mod_res.ok);
        assert_eq!(mod_res.needs_relogin, Some(false));
        assert!(mod_res.error.unwrap().contains("subscribers-only"));

        let missing_scope = parse_helix_chat_parts(403, false, None, Some(&[]));
        assert!(!missing_scope.ok);
        assert_eq!(missing_scope.needs_relogin, Some(true));
    }

    #[test]
    fn test_parse_helix_chat_dropped_message() {
        let body = HelixChatResponse {
            data: Some(vec![HelixChatItem {
                is_sent: false,
                drop_reason: Some(HelixDropReason {
                    message: Some("Message blocked by AutoMod".to_string()),
                }),
            }]),
            message: None,
        };
        let res = parse_helix_chat_parts(200, true, Some(&body), None);
        assert!(!res.ok);
        assert_eq!(res.error.as_deref(), Some("Message blocked by AutoMod"));
    }
}
