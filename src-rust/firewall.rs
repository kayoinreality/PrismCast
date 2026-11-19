//! Verificação e autorização de regra de firewall de entrada (Windows & Linux).
//!
//! - Regra oficial: `PrismCast Studio` (substitui a legada `LAN ScreenShare`).
//! - Windows `check_status()`: consulta UTF-8 via `Get-NetFirewallRule` +
//!   `Get-NetConnectionProfile` (imune a quebra de CP850/idioma PT-BR).
//! - Windows `authorize()`: elevação UAC via Win32 `ShellExecuteExW` (`runas`)
//!   invocando `netsh.exe` diretamente com argumentos validados — zero `cmd.exe`,
//!   zero arquivo `.cmd` em `%TEMP%`, zero command injection.
//! - Linux `check_status()`: inspeciona `ufw` / `firewall-cmd` com `LC_ALL=C`.

use serde::{Deserialize, Serialize};
use std::env;
use std::path::Path;
use std::process::Command;

pub const RULE_NAME: &str = "PrismCast Studio";
pub const LEGACY_RULE_NAME: &str = "LAN ScreenShare";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirewallStatus {
    pub rule_exists: bool,
    pub exec_path: String,
    pub command: String,
    pub profile: String, // "private" | "public" | "domain" | "unknown"
    pub checked: bool,
}

pub fn validate_exec_path(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Caminho do executável vazio.".to_string());
    }
    if trimmed
        .chars()
        .any(|c| c == '"' || c == '\0' || c == '\r' || c == '\n' || c.is_control())
    {
        return Err("Caminho do executável contém caracteres inválidos para netsh.".to_string());
    }
    Ok(trimmed.to_string())
}

pub fn build_add_rule_args(exec_path: &str) -> Result<String, String> {
    let clean = validate_exec_path(exec_path)?;
    Ok(format!(
        "advfirewall firewall add rule name=\"{RULE_NAME}\" dir=in action=allow program=\"{clean}\" enable=yes profile=any"
    ))
}

pub fn build_command(exec_path: &str) -> String {
    if cfg!(target_os = "linux") {
        return "sudo ufw allow 41234/udp && sudo ufw allow 41235/tcp".to_string();
    }
    match build_add_rule_args(exec_path) {
        Ok(args) => format!("netsh {args}"),
        Err(_) => format!(
            "netsh advfirewall firewall add rule name=\"{RULE_NAME}\" dir=in action=allow program=\"{exec_path}\" enable=yes profile=any"
        ),
    }
}

#[derive(Debug, Deserialize, Default)]
struct WinProbeOutput {
    #[serde(default)]
    programs: serde_json::Value,
    #[serde(default)]
    categories: serde_json::Value,
}

fn json_to_string_list(val: &serde_json::Value) -> Vec<String> {
    match val {
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        serde_json::Value::String(s) if !s.is_empty() => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn parse_win_probe(stdout: &str, exec_path: &str) -> Option<(bool, String)> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed: WinProbeOutput = serde_json::from_str(trimmed).ok()?;
    let programs = json_to_string_list(&parsed.programs);
    let categories = json_to_string_list(&parsed.categories);

    let profile = if categories.iter().any(|c| c == "Public") {
        "public"
    } else if categories.iter().any(|c| c == "Private") {
        "private"
    } else if categories.iter().any(|c| c == "DomainAuthenticated") {
        "domain"
    } else {
        "unknown"
    };

    let wanted = exec_path.to_lowercase();
    let rule_exists = programs.iter().any(|p| p.to_lowercase() == wanted);
    Some((rule_exists, profile.to_string()))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_ufw_status(stdout: &str) -> Option<bool> {
    let lower = stdout.to_lowercase();
    if lower.contains("status: inactive") {
        return Some(true);
    }
    if lower.contains("status: active") {
        let has_udp = lower.contains("41234/udp") && lower.contains("allow");
        let has_tcp = lower.contains("41235/tcp") && lower.contains("allow");
        return Some(has_udp && has_tcp);
    }
    None
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_firewalld_ports(stdout: &str) -> bool {
    let tokens: Vec<&str> = stdout.split_whitespace().collect();
    tokens.contains(&"41234/udp") && tokens.contains(&"41235/tcp")
}

#[cfg(target_os = "linux")]
fn check_linux_status(exec_path: String, command: String) -> FirewallStatus {
    if let Ok(out) = Command::new("ufw")
        .arg("status")
        .env("LC_ALL", "C")
        .output()
    {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if let Some(exists) = parse_ufw_status(&stdout) {
                return FirewallStatus {
                    rule_exists: exists,
                    exec_path,
                    command,
                    profile: "private".to_string(),
                    checked: true,
                };
            }
        }
    }

    if let Ok(state_out) = Command::new("firewall-cmd")
        .arg("--state")
        .env("LC_ALL", "C")
        .output()
    {
        let state_txt = String::from_utf8_lossy(&state_out.stdout).to_lowercase();
        if !state_out.status.success() && state_txt.contains("not running") {
            return FirewallStatus {
                rule_exists: true,
                exec_path,
                command,
                profile: "private".to_string(),
                checked: true,
            };
        }
        if state_out.status.success() {
            if let Ok(ports_out) = Command::new("firewall-cmd")
                .arg("--list-ports")
                .env("LC_ALL", "C")
                .output()
            {
                if ports_out.status.success() {
                    let ports_txt = String::from_utf8_lossy(&ports_out.stdout);
                    return FirewallStatus {
                        rule_exists: parse_firewalld_ports(&ports_txt),
                        exec_path,
                        command,
                        profile: "private".to_string(),
                        checked: true,
                    };
                }
            }
        }
    }

    FirewallStatus {
        rule_exists: false,
        exec_path,
        command,
        profile: "unknown".to_string(),
        checked: false,
    }
}

pub fn check_status() -> FirewallStatus {
    let exec_path = env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "prismcast.exe".to_string());
    let command = build_command(&exec_path);

    #[cfg(target_os = "linux")]
    {
        return check_linux_status(exec_path, command);
    }

    #[cfg(windows)]
    {
        let script = format!(
            "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; \
$ErrorActionPreference = 'SilentlyContinue'; \
$programs = @(Get-NetFirewallRule -DisplayName '{RULE_NAME}' | \
Where-Object {{ $_.Direction -eq 'Inbound' -and ($_.Enabled -eq 'True' -or $_.Enabled -eq $true) }} | \
ForEach-Object {{ ($_ | Get-NetFirewallApplicationFilter).Program }}); \
$cats = @(Get-NetConnectionProfile | ForEach-Object {{ $_.NetworkCategory.ToString() }}); \
[pscustomobject]@{{ programs = $programs; categories = $cats }} | ConvertTo-Json -Compress"
        );

        let output = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Some((rule_exists, profile)) = parse_win_probe(&stdout, &exec_path) {
                    return FirewallStatus {
                        rule_exists,
                        exec_path,
                        command,
                        profile,
                        checked: true,
                    };
                }
            }
        }

        FirewallStatus {
            rule_exists: false,
            exec_path,
            command,
            profile: "unknown".to_string(),
            checked: false,
        }
    }

    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        FirewallStatus {
            rule_exists: true,
            exec_path,
            command,
            profile: "private".to_string(),
            checked: true,
        }
    }
}

#[cfg(windows)]
fn run_elevated_netsh(args: &str) -> Result<(), String> {
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED, HANDLE, HINSTANCE, HWND};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::SEE_MASK_NOCLOSEPROCESS;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    #[repr(C)]
    #[allow(non_snake_case)]
    struct ShellExecuteInfoW {
        cbSize: u32,
        fMask: u32,
        hwnd: HWND,
        lpVerb: PCWSTR,
        lpFile: PCWSTR,
        lpParameters: PCWSTR,
        lpDirectory: PCWSTR,
        nShow: i32,
        hInstApp: HINSTANCE,
        lpIDList: *mut c_void,
        lpClass: PCWSTR,
        hkeyClass: *mut c_void,
        dwHotKey: u32,
        hIconOrMonitor: HANDLE,
        hProcess: HANDLE,
    }

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteExW(pExecInfo: *mut ShellExecuteInfoW) -> i32;
    }

    let wide_args: Vec<u16> = OsStr::new(args)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut sei = ShellExecuteInfoW {
        cbSize: std::mem::size_of::<ShellExecuteInfoW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        hwnd: HWND::default(),
        lpVerb: w!("runas"),
        lpFile: w!("netsh.exe"),
        lpParameters: PCWSTR(wide_args.as_ptr()),
        lpDirectory: PCWSTR::null(),
        nShow: SW_HIDE.0,
        hInstApp: HINSTANCE::default(),
        lpIDList: std::ptr::null_mut(),
        lpClass: PCWSTR::null(),
        hkeyClass: std::ptr::null_mut(),
        dwHotKey: 0,
        hIconOrMonitor: HANDLE::default(),
        hProcess: HANDLE::default(),
    };

    unsafe {
        if ShellExecuteExW(&mut sei) == 0 {
            let err = GetLastError();
            if err == ERROR_CANCELLED {
                return Err(
                    "Você cancelou a permissão do Windows. Sem ela, ninguém consegue se conectar."
                        .to_string(),
                );
            }
            return Err(format!("Falha ao solicitar elevação UAC (código {}).", err.0));
        }

        if !sei.hProcess.is_invalid() {
            let _ = WaitForSingleObject(sei.hProcess, 30_000);
            let mut exit_code = 1u32;
            let got = GetExitCodeProcess(sei.hProcess, &mut exit_code).is_ok();
            let _ = CloseHandle(sei.hProcess);
            if !got || exit_code != 0 {
                return Err(format!(
                    "netsh.exe falhou ao criar a regra de firewall (código {exit_code})."
                ));
            }
        }
    }
    Ok(())
}

pub fn authorize() -> Result<(), String> {
    #[cfg(not(windows))]
    {
        return Err("Autorização automática UAC está disponível apenas no Windows.".to_string());
    }

    #[cfg(windows)]
    {
        let exe_path = env::current_exe().map_err(|e| e.to_string())?;
        let exe_str = path_to_clean_string(&exe_path)?;
        let args = build_add_rule_args(&exe_str)?;
        run_elevated_netsh(&args)
    }
}

#[allow(dead_code)]
fn path_to_clean_string(path: &Path) -> Result<String, String> {
    let s = path
        .to_str()
        .ok_or_else(|| "Caminho do executável contém caracteres não UTF-8.".to_string())?;
    validate_exec_path(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_exec_path_rejects_injection() {
        assert!(validate_exec_path("").is_err());
        assert!(validate_exec_path("C:\\PrismCast\\prismcast.exe\"").is_err());
        assert!(validate_exec_path("C:\\a.exe\nwhoami").is_err());
        assert!(validate_exec_path("C:\\Users\\João Silva\\prismcast.exe").is_ok());
    }

    #[test]
    fn test_build_add_rule_args_is_clean() {
        let args = build_add_rule_args(r"C:\Games & Apps\PrismCast\prismcast.exe").unwrap();
        assert_eq!(
            args,
            r#"advfirewall firewall add rule name="PrismCast Studio" dir=in action=allow program="C:\Games & Apps\PrismCast\prismcast.exe" enable=yes profile=any"#
        );
    }

    #[test]
    fn test_parse_win_probe_utf8_accented_user() {
        let json = r#"{"programs":["C:\\Users\\João\\PrismCast\\prismcast.exe"],"categories":["Private"]}"#;
        let (exists, profile) =
            parse_win_probe(json, r"C:\Users\João\PrismCast\prismcast.exe").unwrap();
        assert!(exists);
        assert_eq!(profile, "private");
    }

    #[test]
    fn test_parse_linux_firewalls() {
        assert_eq!(parse_ufw_status("Status: inactive\n"), Some(true));
        assert_eq!(
            parse_ufw_status("Status: active\n41234/udp ALLOW Anywhere\n41235/tcp ALLOW Anywhere\n"),
            Some(true)
        );
        assert_eq!(
            parse_ufw_status("Status: active\n41234/udp ALLOW Anywhere\n"),
            Some(false)
        );
        assert!(parse_firewalld_ports("41234/udp 41235/tcp"));
        assert!(!parse_firewalld_ports("41234/udp"));
    }
}
