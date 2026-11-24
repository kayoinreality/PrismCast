//! Serviço de atualização automática via GitHub Releases (`kayoinreality/PrismCast`).
//!
//! - Consulta a API de Releases do GitHub (`releases/latest` ou `releases` para pré-releases).
//! - Compara versões SemVer com `env!("CARGO_PKG_VERSION")`.
//! - Detecta automaticamente o tipo de distribuição:
//!   - Windows Instalado (NSIS / MSI) vs Windows Portable (`.exe` autônomo).
//!   - Linux AppImage (`$APPIMAGE`) vs pacotes `.deb` / `.rpm` / `.tar.gz`.
//! - Baixa o artefato em streaming com progresso em tempo real e cancelamento.
//! - Valida domínio HTTPS confiável (`github.com` / `*.githubusercontent.com`) e integridade
//!   SHA-256 contra `SHA256SUMS.txt` publicado na release.
//! - Executa troca segura do executável (portable/AppImage) ou inicia o instalador (NSIS/MSI).

use crate::protocol::{UpdateAssetInfo, UpdateState, UpdateStatus};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use url::Url;

pub const GITHUB_REPO: &str = "kayoinreality/PrismCast";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ------------------------------------------------------------------ SemVer

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl SemVer {
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim().trim_start_matches(['v', 'V']);
        if trimmed.is_empty() {
            return None;
        }
        let (core, pre) = match trimmed.split_once('-') {
            Some((c, p)) => (c, Some(p.split('+').next().unwrap_or(p).to_string())),
            None => (trimmed.split('+').next().unwrap_or(trimmed), None),
        };
        let mut parts = core.split('.');
        let major = parts.next()?.parse::<u64>().ok()?;
        let minor = parts.next().unwrap_or("0").parse::<u64>().ok()?;
        let patch = parts.next().unwrap_or("0").parse::<u64>().ok()?;
        Some(Self {
            major,
            minor,
            patch,
            pre: pre.filter(|s| !s.is_empty()),
        })
    }

    pub fn normalized(&self) -> String {
        match &self.pre {
            Some(p) => format!("{}.{}.{}-{p}", self.major, self.minor, self.patch),
            None => format!("{}.{}.{}", self.major, self.minor, self.patch),
        }
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    match (SemVer::parse(current), SemVer::parse(candidate)) {
        (Some(cur), Some(cand)) => cand > cur,
        _ => false,
    }
}

// ------------------------------------------------------------------ Tipo de instalação & Seleção de Asset

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    WindowsInstaller,
    WindowsPortable,
    LinuxAppImage,
    LinuxPackage,
    External,
}

impl InstallKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            InstallKind::WindowsInstaller => "windows-installer",
            InstallKind::WindowsPortable => "windows-portable",
            InstallKind::LinuxAppImage => "linux-appimage",
            InstallKind::LinuxPackage => "linux-package",
            InstallKind::External => "external",
        }
    }

    pub fn detect() -> Self {
        #[cfg(windows)]
        {
            if let Ok(exe) = std::env::current_exe() {
                if looks_like_windows_installed(&exe) {
                    return InstallKind::WindowsInstaller;
                }
            }
            return InstallKind::WindowsPortable;
        }
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("APPIMAGE")
                .map(|v| !v.is_empty())
                .unwrap_or(false)
            {
                return InstallKind::LinuxAppImage;
            }
            return InstallKind::LinuxPackage;
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            InstallKind::External
        }
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn looks_like_windows_installed(exe: &Path) -> bool {
    let lower = exe.to_string_lossy().to_lowercase();
    if lower.contains("\\program files\\")
        || lower.contains("\\program files (x86)\\")
        || lower.contains("\\appdata\\local\\programs\\")
    {
        return true;
    }
    if let Some(dir) = exe.parent() {
        if dir.join("uninstall.exe").exists() || dir.join("unins000.exe").exists() {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: String,
    published_at: Option<String>,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

/// Valida que a URL de download aponta estritamente para HTTPS no GitHub.
pub fn validate_github_download_url(raw: &str) -> Result<Url, String> {
    let parsed = Url::parse(raw.trim()).map_err(|e| format!("URL de download inválida: {e}"))?;
    if parsed.scheme() != "https" {
        return Err("Apenas downloads HTTPS do GitHub são permitidos.".to_string());
    }
    let host = parsed
        .host_str()
        .map(|h| h.to_ascii_lowercase())
        .unwrap_or_default();
    let allowed = host == "github.com"
        || host == "api.github.com"
        || host.ends_with(".githubusercontent.com");
    if !allowed {
        return Err(format!("Host de download não confiável: {host}"));
    }
    Ok(parsed)
}

/// Faz parsing de `SHA256SUMS.txt` (`<hex64>  <arquivo>` ou `<hex64> *<arquivo>`).
pub fn parse_sha256_sums(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let Some(hash_raw) = parts.next() else {
            continue;
        };
        let Some(name_raw) = parts.next() else {
            continue;
        };
        let hash = hash_raw.trim().to_ascii_lowercase();
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let name = name_raw
            .trim()
            .trim_start_matches('*')
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .trim();
        if !name.is_empty() {
            map.insert(name.to_string(), hash);
        }
    }
    map
}

fn select_asset_for_kind(
    assets: &[GithubAsset],
    kind: InstallKind,
    checksums: &HashMap<String, String>,
) -> Option<UpdateAssetInfo> {
    let find_matching = |pred: fn(&str) -> bool| -> Option<&GithubAsset> {
        assets.iter().find(|a| pred(&a.name.to_ascii_lowercase()))
    };

    let chosen = match kind {
        InstallKind::WindowsInstaller => find_matching(|n| n.ends_with("-setup.exe"))
            .or_else(|| find_matching(|n| n.ends_with(".msi")))
            .or_else(|| find_matching(|n| n.ends_with("portable.exe"))),
        InstallKind::WindowsPortable => find_matching(|n| n.ends_with("portable.exe"))
            .or_else(|| find_matching(|n| n.ends_with("-setup.exe")))
            .or_else(|| find_matching(|n| n.ends_with(".msi"))),
        InstallKind::LinuxAppImage => find_matching(|n| n.ends_with(".appimage")),
        InstallKind::LinuxPackage => find_matching(|n| n.ends_with(".appimage"))
            .or_else(|| find_matching(|n| n.ends_with(".deb")))
            .or_else(|| find_matching(|n| n.ends_with(".rpm")))
            .or_else(|| find_matching(|n| n.ends_with(".tar.gz"))),
        InstallKind::External => None,
    }?;

    Some(UpdateAssetInfo {
        name: chosen.name.clone(),
        download_url: chosen.browser_download_url.clone(),
        size_bytes: chosen.size,
        sha256: checksums.get(&chosen.name).cloned(),
    })
}

pub fn compute_file_sha256(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("Falha ao abrir arquivo para SHA-256: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Falha ao ler arquivo para SHA-256: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{b:02x}");
    }
    Ok(hex)
}

fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("prismcast-update");
    let clean: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() || clean == "." || clean == ".." {
        "prismcast-update.bin".to_string()
    } else {
        clean
    }
}

fn resolve_download_dir(kind: InstallKind) -> PathBuf {
    #[cfg(windows)]
    if kind == InstallKind::WindowsPortable {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let probe = parent.join(".prismcast-write-probe.tmp");
                if std::fs::write(&probe, b"ok").is_ok() {
                    let _ = std::fs::remove_file(&probe);
                    return parent.to_path_buf();
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    if kind == InstallKind::LinuxAppImage {
        if let Some(appimage) = std::env::var_os("APPIMAGE") {
            let p = PathBuf::from(appimage);
            if let Some(parent) = p.parent() {
                let probe = parent.join(".prismcast-write-probe.tmp");
                if std::fs::write(&probe, b"ok").is_ok() {
                    let _ = std::fs::remove_file(&probe);
                    return parent.to_path_buf();
                }
            }
        }
    }
    let _ = kind;
    dirs::download_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("PrismCast-Updates")
}

// ------------------------------------------------------------------ UpdateManager

#[derive(Debug, Clone)]
pub struct InstallOutcome {
    pub ok: bool,
    pub should_exit: bool,
    pub message: String,
}

pub struct UpdateManager {
    repo: String,
    current_version: String,
    install_kind: InstallKind,
    client: reqwest::Client,
    status: Mutex<UpdateStatus>,
    download_cancel: Mutex<Option<CancellationToken>>,
}

impl UpdateManager {
    pub fn new() -> Self {
        Self::with_repo(GITHUB_REPO, CURRENT_VERSION)
    }

    pub fn with_repo(repo: &str, current_version: &str) -> Self {
        let install_kind = InstallKind::detect();
        let client = reqwest::Client::builder()
            .user_agent(format!("PrismCast-Studio/{current_version}"))
            .timeout(Duration::from_secs(600))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let initial = UpdateStatus {
            current_version: current_version.to_string(),
            install_kind: install_kind.as_str().to_string(),
            ..Default::default()
        };

        Self {
            repo: repo.to_string(),
            current_version: current_version.to_string(),
            install_kind,
            client,
            status: Mutex::new(initial),
            download_cancel: Mutex::new(None),
        }
    }

    pub fn status(&self) -> UpdateStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn update_status<F>(&self, mutator: F) -> UpdateStatus
    where
        F: FnOnce(&mut UpdateStatus),
    {
        let mut guard = self.status.lock().unwrap_or_else(|e| e.into_inner());
        mutator(&mut guard);
        guard.clone()
    }

    pub fn cancel_download(&self) -> UpdateStatus {
        if let Some(tok) = self
            .download_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            tok.cancel();
        }
        self.update_status(|st| {
            if st.state == UpdateState::Downloading {
                st.state = if st.latest_version.is_some() {
                    UpdateState::Available
                } else {
                    UpdateState::Idle
                };
                st.downloaded_bytes = 0;
                st.progress_percent = 0.0;
                st.error = None;
            }
        })
    }

    /// Consulta o GitHub Releases para descobrir se há uma versão mais nova.
    pub async fn check<F>(&self, include_prereleases: bool, mut on_status: F) -> UpdateStatus
    where
        F: FnMut(&UpdateStatus),
    {
        {
            let cur = self.status();
            if cur.state == UpdateState::Downloading || cur.state == UpdateState::Installing {
                return cur;
            }
        }

        let checking = self.update_status(|st| {
            st.state = UpdateState::Checking;
            st.error = None;
        });
        on_status(&checking);

        let url = if include_prereleases {
            format!("https://api.github.com/repos/{}/releases?per_page=10", self.repo)
        } else {
            format!("https://api.github.com/repos/{}/releases/latest", self.repo)
        };

        let resp = match self
            .client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .timeout(Duration::from_secs(20))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let err_st = self.update_status(|st| {
                    st.state = UpdateState::Error;
                    st.last_checked_at = Some(now_ms());
                    st.error = Some(format!("Não foi possível conectar ao GitHub: {e}"));
                });
                on_status(&err_st);
                return err_st;
            }
        };

        if !resp.status().is_success() {
            let code = resp.status();
            let err_st = self.update_status(|st| {
                st.state = UpdateState::Error;
                st.last_checked_at = Some(now_ms());
                st.error = Some(format!("GitHub retornou HTTP {code} ao verificar atualizações."));
            });
            on_status(&err_st);
            return err_st;
        }

        let release: Option<GithubRelease> = if include_prereleases {
            match resp.json::<Vec<GithubRelease>>().await {
                Ok(list) => list.into_iter().find(|r| !r.draft && (include_prereleases || !r.prerelease)),
                Err(e) => {
                    let err_st = self.update_status(|st| {
                        st.state = UpdateState::Error;
                        st.last_checked_at = Some(now_ms());
                        st.error = Some(format!("Resposta inválida do GitHub: {e}"));
                    });
                    on_status(&err_st);
                    return err_st;
                }
            }
        } else {
            match resp.json::<GithubRelease>().await {
                Ok(rel) if !rel.draft && !rel.prerelease => Some(rel),
                Ok(_) => None,
                Err(e) => {
                    let err_st = self.update_status(|st| {
                        st.state = UpdateState::Error;
                        st.last_checked_at = Some(now_ms());
                        st.error = Some(format!("Resposta inválida do GitHub: {e}"));
                    });
                    on_status(&err_st);
                    return err_st;
                }
            }
        };

        let Some(rel) = release else {
            let up = self.update_status(|st| {
                st.state = UpdateState::UpToDate;
                st.last_checked_at = Some(now_ms());
                st.error = None;
            });
            on_status(&up);
            return up;
        };

        let latest_ver = SemVer::parse(&rel.tag_name)
            .map(|v| v.normalized())
            .unwrap_or_else(|| rel.tag_name.trim_start_matches(['v', 'V']).to_string());

        if !is_newer_version(&self.current_version, &latest_ver) {
            let up = self.update_status(|st| {
                st.state = UpdateState::UpToDate;
                st.latest_version = Some(latest_ver);
                st.release_name = rel.name.clone();
                st.release_notes = rel.body.clone();
                st.release_url = Some(rel.html_url.clone());
                st.published_at = rel.published_at.clone();
                st.asset = None;
                st.downloaded_bytes = 0;
                st.total_bytes = 0;
                st.progress_percent = 0.0;
                st.downloaded_path = None;
                st.verified_sha256 = false;
                st.last_checked_at = Some(now_ms());
                st.error = None;
            });
            on_status(&up);
            return up;
        }

        // Busca SHA256SUMS.txt opcional da release para validação criptográfica do download
        let mut checksums = HashMap::new();
        if let Some(sum_asset) = rel
            .assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case("SHA256SUMS.txt"))
        {
            if validate_github_download_url(&sum_asset.browser_download_url).is_ok() {
                if let Ok(r) = self
                    .client
                    .get(&sum_asset.browser_download_url)
                    .timeout(Duration::from_secs(15))
                    .send()
                    .await
                {
                    if r.status().is_success() {
                        if let Ok(txt) = r.text().await {
                            checksums = parse_sha256_sums(&txt);
                        }
                    }
                }
            }
        }

        let asset = select_asset_for_kind(&rel.assets, self.install_kind, &checksums);

        let avail = self.update_status(|st| {
            let same_ready = st.state == UpdateState::Ready
                && st.latest_version.as_deref() == Some(latest_ver.as_str())
                && st
                    .downloaded_path
                    .as_deref()
                    .map(|p| Path::new(p).exists())
                    .unwrap_or(false);

            if !same_ready {
                st.state = UpdateState::Available;
                st.downloaded_bytes = 0;
                st.total_bytes = asset.as_ref().map(|a| a.size_bytes).unwrap_or(0);
                st.progress_percent = 0.0;
                st.downloaded_path = None;
                st.verified_sha256 = false;
            }
            st.latest_version = Some(latest_ver);
            st.release_name = rel.name.or_else(|| Some(rel.tag_name.clone()));
            st.release_notes = rel.body;
            st.release_url = Some(rel.html_url);
            st.published_at = rel.published_at;
            st.asset = asset;
            st.last_checked_at = Some(now_ms());
            st.error = None;
        });
        on_status(&avail);
        avail
    }

    /// Baixa o artefato da atualização disponível e valida o SHA-256 (quando presente).
    pub async fn download<F>(&self, mut on_status: F) -> UpdateStatus
    where
        F: FnMut(&UpdateStatus),
    {
        let cur = self.status();
        if cur.state == UpdateState::Downloading || cur.state == UpdateState::Installing {
            return cur;
        }
        if cur.state == UpdateState::Ready {
            if cur
                .downloaded_path
                .as_deref()
                .map(|p| Path::new(p).exists())
                .unwrap_or(false)
            {
                return cur;
            }
        }
        let Some(asset) = cur.asset.clone() else {
            let err_st = self.update_status(|st| {
                st.state = UpdateState::Error;
                st.error = Some(
                    "Nenhum instalador compatível foi encontrado para esta plataforma na release."
                        .to_string(),
                );
            });
            on_status(&err_st);
            return err_st;
        };

        let validated_url = match validate_github_download_url(&asset.download_url) {
            Ok(u) => u,
            Err(err) => {
                let err_st = self.update_status(|st| {
                    st.state = UpdateState::Error;
                    st.error = Some(err);
                });
                on_status(&err_st);
                return err_st;
            }
        };

        let cancel = CancellationToken::new();
        {
            let mut guard = self
                .download_cancel
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(old) = guard.replace(cancel.clone()) {
                old.cancel();
            }
        }

        let start_st = self.update_status(|st| {
            st.state = UpdateState::Downloading;
            st.downloaded_bytes = 0;
            st.total_bytes = asset.size_bytes;
            st.progress_percent = 0.0;
            st.downloaded_path = None;
            st.verified_sha256 = false;
            st.error = None;
        });
        on_status(&start_st);

        let dir = resolve_download_dir(self.install_kind);
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            let err_st = self.update_status(|st| {
                st.state = UpdateState::Error;
                st.error = Some(format!("Não foi possível criar pasta de atualização: {e}"));
            });
            on_status(&err_st);
            return err_st;
        }

        let safe_name = sanitize_filename(&asset.name);
        let final_path = dir.join(&safe_name);
        let part_path = dir.join(format!(".{safe_name}.part"));

        let mut resp = match self.client.get(validated_url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                let err_st = self.update_status(|st| {
                    st.state = UpdateState::Error;
                    st.error = Some(format!(
                        "Falha ao baixar atualização (HTTP {}).",
                        r.status()
                    ));
                });
                on_status(&err_st);
                return err_st;
            }
            Err(e) => {
                let err_st = self.update_status(|st| {
                    st.state = UpdateState::Error;
                    st.error = Some(format!("Erro de rede ao baixar atualização: {e}"));
                });
                on_status(&err_st);
                return err_st;
            }
        };

        let total = resp.content_length().unwrap_or(asset.size_bytes);
        let mut file = match tokio::fs::File::create(&part_path).await {
            Ok(f) => f,
            Err(e) => {
                let err_st = self.update_status(|st| {
                    st.state = UpdateState::Error;
                    st.error = Some(format!("Falha ao gravar arquivo de atualização: {e}"));
                });
                on_status(&err_st);
                return err_st;
            }
        };

        let mut hasher = Sha256::new();
        let mut downloaded: u64 = 0;
        let mut last_emit = Instant::now();

        loop {
            let chunk_res = tokio::select! {
                _ = cancel.cancelled() => {
                    drop(file);
                    let _ = tokio::fs::remove_file(&part_path).await;
                    let canceled = self.update_status(|st| {
                        st.state = UpdateState::Available;
                        st.downloaded_bytes = 0;
                        st.progress_percent = 0.0;
                        st.error = None;
                    });
                    on_status(&canceled);
                    return canceled;
                }
                c = resp.chunk() => c,
            };

            match chunk_res {
                Ok(Some(chunk)) => {
                    if let Err(e) = file.write_all(&chunk).await {
                        drop(file);
                        let _ = tokio::fs::remove_file(&part_path).await;
                        let err_st = self.update_status(|st| {
                            st.state = UpdateState::Error;
                            st.error = Some(format!("Falha ao gravar dados no disco: {e}"));
                        });
                        on_status(&err_st);
                        return err_st;
                    }
                    hasher.update(&chunk);
                    downloaded = downloaded.saturating_add(chunk.len() as u64);

                    if last_emit.elapsed() >= Duration::from_millis(100) {
                        last_emit = Instant::now();
                        let pct = if total > 0 {
                            ((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 99.9) as f32
                        } else {
                            0.0
                        };
                        let prog = self.update_status(|st| {
                            st.downloaded_bytes = downloaded;
                            st.total_bytes = total;
                            st.progress_percent = pct;
                        });
                        on_status(&prog);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    drop(file);
                    let _ = tokio::fs::remove_file(&part_path).await;
                    let err_st = self.update_status(|st| {
                        st.state = UpdateState::Error;
                        st.error = Some(format!("Download interrompido: {e}"));
                    });
                    on_status(&err_st);
                    return err_st;
                }
            }
        }

        if let Err(e) = file.flush().await {
            drop(file);
            let _ = tokio::fs::remove_file(&part_path).await;
            let err_st = self.update_status(|st| {
                st.state = UpdateState::Error;
                st.error = Some(format!("Falha ao finalizar arquivo no disco: {e}"));
            });
            on_status(&err_st);
            return err_st;
        }
        drop(file);

        // Verifica integridade SHA-256 quando o manifesto SHA256SUMS.txt estava disponível
        let digest = hasher.finalize();
        let mut actual_hex = String::with_capacity(64);
        for b in digest {
            use std::fmt::Write;
            let _ = write!(&mut actual_hex, "{b:02x}");
        }

        let mut verified = false;
        if let Some(expected) = asset.sha256.as_deref() {
            if actual_hex != expected.to_ascii_lowercase() {
                let _ = tokio::fs::remove_file(&part_path).await;
                let err_st = self.update_status(|st| {
                    st.state = UpdateState::Error;
                    st.error = Some(
                        "Verificação de integridade SHA-256 falhou — arquivo descartado por segurança."
                            .to_string(),
                    );
                });
                on_status(&err_st);
                return err_st;
            }
            verified = true;
        }

        let _ = tokio::fs::remove_file(&final_path).await;
        if let Err(e) = tokio::fs::rename(&part_path, &final_path).await {
            let _ = tokio::fs::remove_file(&part_path).await;
            let err_st = self.update_status(|st| {
                st.state = UpdateState::Error;
                st.error = Some(format!("Não foi possível salvar arquivo de atualização: {e}"));
            });
            on_status(&err_st);
            return err_st;
        }

        let _ = self
            .download_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();

        let ready = self.update_status(|st| {
            st.state = UpdateState::Ready;
            st.downloaded_bytes = downloaded;
            st.total_bytes = total.max(downloaded);
            st.progress_percent = 100.0;
            st.downloaded_path = Some(final_path.to_string_lossy().to_string());
            st.verified_sha256 = verified;
            st.error = None;
        });
        on_status(&ready);
        ready
    }

    /// Inicia a instalação ou substituição atômica do pacote já baixado.
    pub fn install_prepared<F>(&self, mut on_status: F) -> InstallOutcome
    where
        F: FnMut(&UpdateStatus),
    {
        let cur = self.status();
        if cur.state != UpdateState::Ready {
            return InstallOutcome {
                ok: false,
                should_exit: false,
                message: "Nenhuma atualização baixada pronta para instalar.".to_string(),
            };
        }
        let Some(path_str) = cur.downloaded_path.clone() else {
            return InstallOutcome {
                ok: false,
                should_exit: false,
                message: "Caminho do arquivo baixado não encontrado.".to_string(),
            };
        };
        let path = PathBuf::from(&path_str);
        if !path.exists() {
            let err_st = self.update_status(|st| {
                st.state = UpdateState::Available;
                st.downloaded_path = None;
                st.error = Some("Arquivo de atualização foi removido antes da instalação.".to_string());
            });
            on_status(&err_st);
            return InstallOutcome {
                ok: false,
                should_exit: false,
                message: "Arquivo de atualização não existe mais no disco.".to_string(),
            };
        }

        let installing = self.update_status(|st| {
            st.state = UpdateState::Installing;
            st.error = None;
        });
        on_status(&installing);

        match launch_update_handoff(self.install_kind, &path) {
            Ok(outcome) => outcome,
            Err(err) => {
                let err_st = self.update_status(|st| {
                    st.state = UpdateState::Error;
                    st.error = Some(err.clone());
                });
                on_status(&err_st);
                InstallOutcome {
                    ok: false,
                    should_exit: false,
                    message: err,
                }
            }
        }
    }
}

impl Default for UpdateManager {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------------------ Handoff por plataforma

fn launch_update_handoff(kind: InstallKind, downloaded: &Path) -> Result<InstallOutcome, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;

        let ext = downloaded
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let file_name_lower = downloaded
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        if kind == InstallKind::WindowsPortable && file_name_lower.ends_with("portable.exe") {
            let current_exe = std::env::current_exe()
                .map_err(|e| format!("Não foi possível localizar o executável atual: {e}"))?;
            let pid = std::process::id();
            let script_path = std::env::temp_dir().join(format!("prismcast-update-{pid}.cmd"));
            let target_str = current_exe.to_string_lossy();
            let src_str = downloaded.to_string_lossy();
            let backup_str = format!("{target_str}.bak");

            let script = format!(
                "@echo off\r\n\
                setlocal\r\n\
                set PID={pid}\r\n\
                set SRC={src_str}\r\n\
                set DST={target_str}\r\n\
                set BAK={backup_str}\r\n\
                set /a TRIES=0\r\n\
                :wait_loop\r\n\
                tasklist /FI \"PID eq %PID%\" 2>NUL | find /I \"%PID%\" >NUL\r\n\
                if not errorlevel 1 (\r\n\
                  set /a TRIES+=1\r\n\
                  if %TRIES% GEQ 30 goto do_swap\r\n\
                  timeout /t 1 /nobreak >NUL\r\n\
                  goto wait_loop\r\n\
                )\r\n\
                :do_swap\r\n\
                timeout /t 1 /nobreak >NUL\r\n\
                if exist \"%BAK%\" del /F /Q \"%BAK%\" >NUL 2>&1\r\n\
                move /Y \"%DST%\" \"%BAK%\" >NUL 2>&1\r\n\
                move /Y \"%SRC%\" \"%DST%\" >NUL 2>&1\r\n\
                if errorlevel 1 (\r\n\
                  if exist \"%BAK%\" move /Y \"%BAK%\" \"%DST%\" >NUL 2>&1\r\n\
                  exit /b 1\r\n\
                )\r\n\
                start \"\" \"%DST%\"\r\n\
                timeout /t 2 /nobreak >NUL\r\n\
                if exist \"%BAK%\" del /F /Q \"%BAK%\" >NUL 2>&1\r\n\
                del /F /Q \"%~f0\" >NUL 2>&1\r\n"
            );

            std::fs::write(&script_path, script)
                .map_err(|e| format!("Falha ao preparar script de atualização: {e}"))?;

            std::process::Command::new("cmd.exe")
                .args(["/C", &script_path.to_string_lossy()])
                .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
                .spawn()
                .map_err(|e| format!("Falha ao iniciar atualização portátil: {e}"))?;

            return Ok(InstallOutcome {
                ok: true,
                should_exit: true,
                message: "Reiniciando para concluir a atualização…".to_string(),
            });
        }

        if ext == "msi" {
            std::process::Command::new("msiexec.exe")
                .args(["/i", &downloaded.to_string_lossy(), "/passive"])
                .creation_flags(DETACHED_PROCESS)
                .spawn()
                .map_err(|e| format!("Falha ao abrir instalador MSI: {e}"))?;
        } else {
            std::process::Command::new(downloaded)
                .creation_flags(DETACHED_PROCESS)
                .spawn()
                .map_err(|e| format!("Falha ao iniciar instalador: {e}"))?;
        }

        return Ok(InstallOutcome {
            ok: true,
            should_exit: true,
            message: "Instalador iniciado — fechando o PrismCast para atualizar…".to_string(),
        });
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;

        if kind == InstallKind::LinuxAppImage {
            if let Some(appimage_os) = std::env::var_os("APPIMAGE") {
                let target = PathBuf::from(appimage_os);
                let _ = std::fs::set_permissions(downloaded, std::fs::Permissions::from_mode(0o755));
                if std::fs::rename(downloaded, &target).is_err() {
                    std::fs::copy(downloaded, &target)
                        .map_err(|e| format!("Falha ao substituir AppImage: {e}"))?;
                    let _ = std::fs::remove_file(downloaded);
                    let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
                }
                std::process::Command::new(&target)
                    .spawn()
                    .map_err(|e| format!("Atualizado, mas falha ao reiniciar AppImage: {e}"))?;
                return Ok(InstallOutcome {
                    ok: true,
                    should_exit: true,
                    message: "AppImage atualizado — reiniciando…".to_string(),
                });
            }
        }

        let target_dir = downloaded.parent().unwrap_or_else(|| Path::new("/tmp"));
        let _ = std::process::Command::new("xdg-open").arg(target_dir).spawn();
        return Ok(InstallOutcome {
            ok: true,
            should_exit: false,
            message: format!(
                "Pacote salvo em {}. Instale o pacote para concluir a atualização.",
                downloaded.display()
            ),
        });
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (kind, downloaded);
        Ok(InstallOutcome {
            ok: true,
            should_exit: false,
            message: "Pacote baixado com sucesso.".to_string(),
        })
    }
}

// ------------------------------------------------------------------ Testes

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semver_ordering_and_prefix() {
        assert!(is_newer_version("2.5.0", "2.5.1"));
        assert!(is_newer_version("2.5.0", "v2.6.0"));
        assert!(is_newer_version("2.9.9", "2.10.0"));
        assert!(is_newer_version("2.5.1-beta.1", "2.5.1"));
        assert!(!is_newer_version("2.5.1", "2.5.1"));
        assert!(!is_newer_version("2.5.1", "2.5.0"));
        assert!(!is_newer_version("2.5.1", "2.5.1-rc.1"));
        assert!(!is_newer_version("2.5.0", "invalid"));
    }

    #[test]
    fn test_validate_github_download_url() {
        assert!(validate_github_download_url(
            "https://github.com/kayoinreality/PrismCast/releases/download/v2.5.1/PrismCast-Studio_2.5.1_x64-setup.exe"
        )
        .is_ok());
        assert!(validate_github_download_url(
            "https://objects.githubusercontent.com/github-production-release-asset/foo.exe"
        )
        .is_ok());
        assert!(validate_github_download_url("http://github.com/foo/bar.exe").is_err());
        assert!(validate_github_download_url("https://evil.example.com/foo.exe").is_err());
        assert!(validate_github_download_url("file:///C:/Windows/calc.exe").is_err());
    }

    #[test]
    fn test_parse_sha256_sums_and_asset_selection() {
        let manifest = format!(
            "{}  PrismCast-Studio_2.5.1_x64-setup.exe\n{} *PrismCast-Studio-2.5.1-windows-x64-portable.exe\ninvalid line\n",
            "a".repeat(64),
            "b".repeat(64)
        );
        let sums = parse_sha256_sums(&manifest);
        assert_eq!(sums.len(), 2);
        assert_eq!(
            sums.get("PrismCast-Studio_2.5.1_x64-setup.exe").map(String::as_str),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );

        let assets = vec![
            GithubAsset {
                name: "PrismCast-Studio-2.5.1-windows-x64-portable.exe".to_string(),
                browser_download_url: "https://github.com/kayoinreality/PrismCast/releases/download/v2.5.1/PrismCast-Studio-2.5.1-windows-x64-portable.exe".to_string(),
                size: 30_000_000,
            },
            GithubAsset {
                name: "PrismCast-Studio_2.5.1_x64-setup.exe".to_string(),
                browser_download_url: "https://github.com/kayoinreality/PrismCast/releases/download/v2.5.1/PrismCast-Studio_2.5.1_x64-setup.exe".to_string(),
                size: 8_000_000,
            },
            GithubAsset {
                name: "PrismCast-Studio_2.5.1_amd64.AppImage".to_string(),
                browser_download_url: "https://github.com/kayoinreality/PrismCast/releases/download/v2.5.1/PrismCast-Studio_2.5.1_amd64.AppImage".to_string(),
                size: 90_000_000,
            },
        ];

        let win_inst = select_asset_for_kind(&assets, InstallKind::WindowsInstaller, &sums).unwrap();
        assert_eq!(win_inst.name, "PrismCast-Studio_2.5.1_x64-setup.exe");
        assert!(win_inst.sha256.is_some());

        let win_port = select_asset_for_kind(&assets, InstallKind::WindowsPortable, &sums).unwrap();
        assert_eq!(win_port.name, "PrismCast-Studio-2.5.1-windows-x64-portable.exe");
        assert_eq!(win_port.sha256.as_deref(), Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));

        let linux_app = select_asset_for_kind(&assets, InstallKind::LinuxAppImage, &sums).unwrap();
        assert_eq!(linux_app.name, "PrismCast-Studio_2.5.1_amd64.AppImage");
    }

    #[test]
    fn test_compute_file_sha256() {
        let path = std::env::temp_dir().join(format!(
            "prismcast-sha-test-{}.bin",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, b"prismcast-update").unwrap();
        let hash = compute_file_sha256(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(hash.len(), 64);
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
