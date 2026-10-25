use serde::{Deserialize, Serialize};
use std::env;
use std::process::Command;

pub const RULE_NAME: &str = "PrismCast Studio";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirewallStatus {
    pub rule_exists: bool,
    pub exec_path: String,
    pub command: String,
    pub profile: String, // "private" | "public" | "domain" | "unknown"
    pub checked: bool,
}

pub fn build_command(exec_path: &str) -> String {
    format!(
        "netsh advfirewall firewall add rule name=\"{}\" dir=in action=allow program=\"{}\" enable=yes profile=any",
        RULE_NAME, exec_path
    )
}

pub fn check_status() -> FirewallStatus {
    let exec_path = env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "prismcast.exe".to_string());
    let command = build_command(&exec_path);

    if !cfg!(target_os = "windows") {
        return FirewallStatus {
            rule_exists: true,
            exec_path,
            command,
            profile: "private".to_string(),
            checked: true,
        };
    }

    let rule_output = Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "show",
            "rule",
            &format!("name={}", RULE_NAME),
            "verbose",
        ])
        .output();

    let (rule_exists, checked) = match rule_output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_lowercase();
            let exists = out.status.success() && stdout.contains(&exec_path.to_lowercase());
            (exists, true)
        }
        Err(_) => (false, false),
    };

    FirewallStatus {
        rule_exists,
        exec_path,
        command,
        profile: "private".to_string(),
        checked,
    }
}

pub fn authorize() -> Result<(), String> {
    if !cfg!(target_os = "windows") {
        return Ok(());
    }
    let exec_path = env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())?;

    let escaped = exec_path.replace('\'', "''");
    let netsh_args = format!(
        "advfirewall firewall delete rule name=\"{0}\" & netsh advfirewall firewall add rule name=\"{0}\" dir=in action=allow program=\"{1}\" enable=yes profile=any",
        RULE_NAME, escaped
    );
    let ps = format!(
        "Start-Process -FilePath 'cmd.exe' -ArgumentList '/c netsh {}' -Verb RunAs -WindowStyle Hidden -Wait",
        netsh_args
    );

    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .status()
        .map_err(|e| e.to_string())?;

    if status.success() {
        Ok(())
    } else {
        Err("Permissão de administrador negada ou cancelada.".to_string())
    }
}
