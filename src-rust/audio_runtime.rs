//! Live real-time Audio Runtime supervisor (10 ms DSP quantum loop, isolated per-app capture,
//! OS microphone & desktop loopback capture, low-latency monitor playback, and Stinger one-shot
//! sound injection).
//!
//! Cross-platform architecture:
//! - **Per-app capture (`AudioChannelKind::App`)**: spawns sibling `prismcast-audio capture --exe <exe>`
//!   and streams 48 kHz f32le stereo PCM directly into [`AudioMixerEngine::push_external_pcm_bytes`],
//!   automatically backing off and retrying on exit code 2/3 (app not yet open / closed & reopened).
//! - **Linux (`#[cfg(target_os = "linux")]`)**: enumerates PipeWire/PulseAudio apps and devices via
//!   `pactl -f json` (+ text fallback) + `/proc` + `wmctrl`, captures mic & desktop loopback via
//!   `parec`, and plays the monitor bus via `pacat --playback`.
//! - **Windows (`#[cfg(windows)]`)**: runs dedicated MTA-initialized `wasapi` capture threads
//!   (microphone `Direction::Capture`, desktop loopback `Direction::Render` -> `Direction::Capture`)
//!   and a `wasapi` `Direction::Render` playback thread for the monitor bus.
//! - **10 ms RT DSP quantum loop**: ticks every 10 ms (`480` frames @ 48 kHz stereo), runs
//!   [`AudioMixerEngine::process_quantum`], mixes Stinger one-shot sound effects into Program and
//!   Monitor buses, broadcasts Program PCM via `broadcast::Sender<Arc<Vec<f32>>>`, and feeds Monitor
//!   PCM into the active monitor sink.

use crate::audio_mixer::{
    db_to_linear, AudioDeviceInfo, AudioMixerEngine, FRAME_BYTES, QUANTUM_FRAMES, QUANTUM_SAMPLES,
};
use crate::protocol::{
    AudioApp, AudioChannelKind, AudioChannelStatus, AudioMixerSettings, AudioMonitorMode,
};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

#[cfg(windows)]
const AUDIO_HELPER_BIN: &str = "prismcast-audio.exe";
#[cfg(not(windows))]
const AUDIO_HELPER_BIN: &str = "prismcast-audio";

const PROGRAM_BROADCAST_CAP: usize = 64;
const MONITOR_QUEUE_CAP: usize = 16;
const MAX_ACTIVE_ONESHOTS: usize = 16;
const CAPTURE_READ_CHUNK_BYTES: usize = QUANTUM_FRAMES * FRAME_BYTES; // 3840 bytes = 10 ms @ 48 kHz f32 stereo

type StatusCallback = Arc<dyn Fn(Vec<AudioChannelStatus>) + Send + Sync>;

// ------------------------------------------------------------------ Linux PipeWire / PulseAudio Enumeration

/// Lists running Linux applications with active PipeWire/PulseAudio audio streams plus
/// open X11/XWayland windows (`wmctrl -lp`) enriched from `/proc`.
pub fn list_linux_audio_apps() -> Vec<AudioApp> {
    #[cfg(target_os = "linux")]
    {
        linux_enum::list_audio_apps()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// Lists Linux PulseAudio/PipeWire microphone inputs (excluding `.monitor` loopback sources)
/// and output sinks for monitor playback selection.
pub fn list_linux_devices() -> Vec<AudioDeviceInfo> {
    #[cfg(target_os = "linux")]
    {
        linux_enum::list_devices()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
mod linux_enum {
    use super::*;
    use serde_json::Value;
    use std::collections::{BTreeMap, HashSet};
    use std::fs;
    use std::process::Command as StdCommand;

    struct SinkInput {
        index: u32,
        pid: u32,
        binary: String,
        app_name: String,
    }

    fn parse_pactl_sink_inputs() -> Vec<SinkInput> {
        let mut out = Vec::new();
        if let Ok(output) = StdCommand::new("pactl")
            .env("LC_ALL", "C")
            .args(["-f", "json", "list", "sink-inputs"])
            .output()
        {
            if output.status.success() {
                if let Ok(Value::Array(items)) = serde_json::from_slice::<Value>(&output.stdout) {
                    for item in items {
                        let Some(index) = item
                            .get("index")
                            .and_then(|v| v.as_u64())
                            .map(|v| v as u32)
                        else {
                            continue;
                        };
                        let props = item.get("properties");
                        let pid = props
                            .and_then(|p| p.get("application.process.id"))
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(0);
                        let binary = props
                            .and_then(|p| p.get("application.process.binary"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let app_name = props
                            .and_then(|p| p.get("application.name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or(&binary)
                            .to_string();
                        if pid > 0 || !binary.is_empty() {
                            out.push(SinkInput {
                                index,
                                pid,
                                binary,
                                app_name,
                            });
                        }
                    }
                    return out;
                }
            }
        }

        // Text fallback (`pactl list sink-inputs`) for older PulseAudio daemons
        if let Ok(output) = StdCommand::new("pactl")
            .env("LC_ALL", "C")
            .args(["list", "sink-inputs"])
            .output()
        {
            if let Ok(text) = String::from_utf8(output.stdout) {
                let mut cur_idx: Option<u32> = None;
                let mut cur_pid = 0u32;
                let mut cur_bin = String::new();
                let mut cur_name = String::new();

                for line in text.lines() {
                    let trimmed = line.trim();
                    if let Some(rest) = trimmed.strip_prefix("Sink Input #") {
                        if let Some(idx) = cur_idx {
                            if cur_pid > 0 || !cur_bin.is_empty() {
                                out.push(SinkInput {
                                    index: idx,
                                    pid: cur_pid,
                                    binary: cur_bin.clone(),
                                    app_name: if cur_name.is_empty() {
                                        cur_bin.clone()
                                    } else {
                                        cur_name.clone()
                                    },
                                });
                            }
                        }
                        cur_idx = rest.trim().parse::<u32>().ok();
                        cur_pid = 0;
                        cur_bin.clear();
                        cur_name.clear();
                    } else if let Some(val) = trimmed.strip_prefix("application.process.id = ") {
                        cur_pid = val.trim_matches('"').parse::<u32>().unwrap_or(0);
                    } else if let Some(val) = trimmed.strip_prefix("application.process.binary = ")
                    {
                        cur_bin = val.trim_matches('"').to_string();
                    } else if let Some(val) = trimmed.strip_prefix("application.name = ") {
                        cur_name = val.trim_matches('"').to_string();
                    }
                }
                if let Some(idx) = cur_idx {
                    if cur_pid > 0 || !cur_bin.is_empty() {
                        out.push(SinkInput {
                            index: idx,
                            pid: cur_pid,
                            binary: cur_bin.clone(),
                            app_name: if cur_name.is_empty() {
                                cur_bin
                            } else {
                                cur_name
                            },
                        });
                    }
                }
            }
        }
        out
    }

    fn proc_table() -> BTreeMap<u32, (String, u32)> {
        let mut map = BTreeMap::new();
        let Ok(entries) = fs::read_dir("/proc") else {
            return map;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(pid) = name.parse::<u32>() else {
                continue;
            };
            let exe_link = fs::read_link(format!("/proc/{pid}/exe"))
                .ok()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let comm = fs::read_to_string(format!("/proc/{pid}/comm"))
                .ok()
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let exe_name = if !exe_link.is_empty() {
                exe_link
                    .rsplit('/')
                    .next()
                    .unwrap_or(&comm)
                    .trim_end_matches(" (deleted)")
                    .to_string()
            } else {
                comm
            };
            if exe_name.is_empty() {
                continue;
            }
            let ppid = fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| {
                    let after_comm = stat.rfind(')')?;
                    let fields: Vec<&str> = stat[after_comm + 1..].split_whitespace().collect();
                    fields.get(1)?.parse::<u32>().ok()
                })
                .unwrap_or(0);
            map.insert(pid, (exe_name, ppid));
        }
        map
    }

    pub fn list_audio_apps() -> Vec<AudioApp> {
        let me = std::process::id();
        let sinks = parse_pactl_sink_inputs();
        let procs = proc_table();

        let mut seen = HashSet::new();
        let mut items = Vec::new();

        // 1. Active PipeWire / PulseAudio streams
        for s in &sinks {
            if s.pid == me {
                continue;
            }
            let exe = if !s.binary.is_empty() {
                s.binary.rsplit('/').next().unwrap_or(&s.binary).to_string()
            } else if let Some((e, _)) = procs.get(&s.pid) {
                e.clone()
            } else {
                s.app_name.clone()
            };
            let lower = exe.to_lowercase();
            if lower.is_empty()
                || lower == "prismcast"
                || lower == "prismcast-audio"
                || seen.contains(&lower)
            {
                continue;
            }
            seen.insert(lower);
            let title = if s.app_name.is_empty() {
                exe.clone()
            } else {
                s.app_name.clone()
            };
            items.push(AudioApp {
                hwnd: s.index as isize,
                pid: s.pid,
                exe,
                title,
                audio: true,
            });
        }

        // 2. Open X11 / XWayland windows via `wmctrl -lp`
        if let Ok(output) = StdCommand::new("wmctrl").args(["-lp"]).output() {
            if let Ok(text) = String::from_utf8(output.stdout) {
                for line in text.lines() {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() < 5 {
                        continue;
                    }
                    let hwnd =
                        isize::from_str_radix(parts[0].trim_start_matches("0x"), 16).unwrap_or(0);
                    let Ok(pid) = parts[2].parse::<u32>() else {
                        continue;
                    };
                    if pid == 0 || pid == me {
                        continue;
                    }
                    let Some((exe, _)) = procs.get(&pid) else {
                        continue;
                    };
                    let lower = exe.to_lowercase();
                    if seen.contains(&lower)
                        || lower == "prismcast"
                        || lower == "prismcast-audio"
                        || lower == "gnome-shell"
                        || lower == "plasmashell"
                    {
                        continue;
                    }
                    seen.insert(lower);
                    let title = parts[4..].join(" ");
                    items.push(AudioApp {
                        hwnd,
                        pid,
                        exe: exe.clone(),
                        title,
                        audio: false,
                    });
                }
            }
        }

        items
    }

    fn pactl_default(subcmd: &str, info_prefix: &str) -> Option<String> {
        if let Ok(out) = StdCommand::new("pactl")
            .env("LC_ALL", "C")
            .arg(subcmd)
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
        if let Ok(out) = StdCommand::new("pactl")
            .env("LC_ALL", "C")
            .arg("info")
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                for line in text.lines() {
                    if let Some(rest) = line.trim().strip_prefix(info_prefix) {
                        let s = rest.trim().to_string();
                        if !s.is_empty() {
                            return Some(s);
                        }
                    }
                }
            }
        }
        None
    }

    fn parse_pactl_devices_json(
        noun: &str,
        kind: &str,
        skip_monitors: bool,
        default_name: Option<&str>,
    ) -> Option<Vec<AudioDeviceInfo>> {
        let output = StdCommand::new("pactl")
            .env("LC_ALL", "C")
            .args(["-f", "json", "list", noun])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let Value::Array(items) = serde_json::from_slice::<Value>(&output.stdout).ok()? else {
            return None;
        };

        let mut out = Vec::new();
        for item in items {
            let Some(id) = item.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            if skip_monitors {
                let is_mon = id.ends_with(".monitor")
                    || item
                        .get("monitor_source")
                        .and_then(|v| v.as_str())
                        .map(|s| !s.is_empty())
                        .unwrap_or(false)
                    || item
                        .get("monitor_of_sink")
                        .and_then(|v| v.as_str())
                        .map(|s| !s.is_empty())
                        .unwrap_or(false)
                    || item
                        .get("properties")
                        .and_then(|p| p.get("device.class"))
                        .and_then(|v| v.as_str())
                        == Some("monitor");
                if is_mon {
                    continue;
                }
            }
            let desc = item
                .get("description")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(id);
            out.push(AudioDeviceInfo {
                id: id.to_string(),
                name: desc.to_string(),
                kind: kind.to_string(),
                is_default: default_name == Some(id),
            });
        }
        Some(out)
    }

    fn parse_pactl_devices_text(
        noun: &str,
        kind: &str,
        skip_monitors: bool,
        default_name: Option<&str>,
    ) -> Vec<AudioDeviceInfo> {
        let mut out = Vec::new();
        if let Ok(output) = StdCommand::new("pactl")
            .env("LC_ALL", "C")
            .args(["list", noun])
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                let mut cur_id = String::new();
                let mut cur_desc = String::new();
                let mut cur_is_mon = false;

                let flush = |out: &mut Vec<AudioDeviceInfo>,
                             id: &mut String,
                             desc: &mut String,
                             is_mon: &mut bool| {
                    if !id.is_empty() && !(skip_monitors && (*is_mon || id.ends_with(".monitor"))) {
                        out.push(AudioDeviceInfo {
                            is_default: default_name == Some(id.as_str()),
                            name: if desc.is_empty() {
                                id.clone()
                            } else {
                                desc.clone()
                            },
                            id: id.clone(),
                            kind: kind.to_string(),
                        });
                    }
                    id.clear();
                    desc.clear();
                    *is_mon = false;
                };

                for line in text.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("Source #") || trimmed.starts_with("Sink #") {
                        flush(&mut out, &mut cur_id, &mut cur_desc, &mut cur_is_mon);
                    } else if let Some(val) = trimmed.strip_prefix("Name:") {
                        cur_id = val.trim().to_string();
                    } else if let Some(val) = trimmed.strip_prefix("Description:") {
                        cur_desc = val.trim().to_string();
                    } else if let Some(val) = trimmed.strip_prefix("Monitor of Sink:") {
                        if val.trim() != "n/a" && !val.trim().is_empty() {
                            cur_is_mon = true;
                        }
                    }
                }
                flush(&mut out, &mut cur_id, &mut cur_desc, &mut cur_is_mon);
            }
        }
        out
    }

    pub fn list_devices() -> Vec<AudioDeviceInfo> {
        let def_source = pactl_default("get-default-source", "Default Source:");
        let def_sink = pactl_default("get-default-sink", "Default Sink:");

        let mut devices = parse_pactl_devices_json(
            "sources",
            "input",
            true,
            def_source.as_deref(),
        )
        .unwrap_or_else(|| {
            parse_pactl_devices_text("sources", "input", true, def_source.as_deref())
        });

        let mut outputs = parse_pactl_devices_json(
            "sinks",
            "output",
            false,
            def_sink.as_deref(),
        )
        .unwrap_or_else(|| {
            parse_pactl_devices_text("sinks", "output", false, def_sink.as_deref())
        });

        devices.append(&mut outputs);
        devices
    }
}

// ------------------------------------------------------------------ Helper Binary Resolution

/// Resolves the `prismcast-audio` helper binary next to `current_exe()` (or parent dir for
/// Cargo `target/*/deps` test executables), falling back to `PATH`.
pub fn resolve_audio_helper_binary() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let path_env = std::env::var_os("PATH");
    resolve_audio_helper_in(exe_dir.as_deref(), path_env.as_deref())
}

/// Deterministic helper resolver suitable for both production lookups and unit tests.
pub fn resolve_audio_helper_in(
    exe_dir: Option<&Path>,
    path_env: Option<&OsStr>,
) -> Option<PathBuf> {
    if let Some(dir) = exe_dir {
        let sibling = dir.join(AUDIO_HELPER_BIN);
        if sibling.is_file() {
            return Some(sibling);
        }
        // Cargo test binaries live inside `target/<profile>/deps/` while `prismcast-audio`
        // is placed one directory up in `target/<profile>/`.
        if let Some(parent) = dir.parent() {
            let up = parent.join(AUDIO_HELPER_BIN);
            if up.is_file() {
                return Some(up);
            }
        }
    }

    if let Some(paths) = path_env {
        for dir in std::env::split_paths(paths) {
            let candidate = dir.join(AUDIO_HELPER_BIN);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

// ------------------------------------------------------------------ Capture Worker Specs & State

#[derive(Debug, Clone, PartialEq, Eq)]
enum CaptureSpec {
    App { exe: String },
    Mic { device_id: Option<String> },
    Desktop { device_id: Option<String> },
}

struct CaptureWorkerEntry {
    spec: CaptureSpec,
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

struct MonitorWorkerEntry {
    device_id: Option<String>,
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

struct OneshotAudio {
    samples: Vec<f32>,
    cursor: usize,
    gain_lin: f32,
}

fn update_channel_status(
    mixer: &Arc<Mutex<AudioMixerEngine>>,
    status_cb: &Arc<Mutex<Option<StatusCallback>>>,
    channel_id: &str,
    state: &str,
    message: Option<String>,
) {
    let snapshot = {
        let mut g = mixer.lock().unwrap_or_else(|e| e.into_inner());
        g.set_channel_status(channel_id, state, message);
        g.statuses()
    };
    let cb = status_cb
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(cb) = cb {
        cb(snapshot);
    }
}

// ------------------------------------------------------------------ Per-App Capture Supervisor

async fn run_app_capture_worker(
    channel_id: String,
    exe: String,
    mixer: Arc<Mutex<AudioMixerEngine>>,
    status_cb: Arc<Mutex<Option<StatusCallback>>>,
    cancel: CancellationToken,
) {
    let Some(helper_bin) = resolve_audio_helper_binary() else {
        update_channel_status(
            &mixer,
            &status_cb,
            &channel_id,
            "error",
            Some(format!("Binário '{AUDIO_HELPER_BIN}' não encontrado.")),
        );
        return;
    };

    let mut backoff_ms: u64 = 750;
    let mut read_buf = vec![0u8; CAPTURE_READ_CHUNK_BYTES];

    loop {
        if cancel.is_cancelled() {
            break;
        }

        update_channel_status(
            &mixer,
            &status_cb,
            &channel_id,
            "waiting",
            Some(format!("Aguardando {exe}...")),
        );

        let mut cmd = Command::new(&helper_bin);
        cmd.args(["capture", "--exe", &exe])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                update_channel_status(
                    &mixer,
                    &status_cb,
                    &channel_id,
                    "error",
                    Some(format!("Falha ao iniciar captura de {exe}: {e}")),
                );
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(backoff_ms.max(2000))) => {}
                }
                backoff_ms = (backoff_ms * 2).min(5000);
                continue;
            }
        };

        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill().await;
            break;
        };
        let stderr = child.stderr.take();

        let last_err_msg = Arc::new(Mutex::new(None::<String>));
        let ready_flag = Arc::new(AtomicBool::new(false));

        let stderr_task = stderr.map(|err_pipe| {
            let last_err = last_err_msg.clone();
            let ready = ready_flag.clone();
            let mix = mixer.clone();
            let cb = status_cb.clone();
            let cid = channel_id.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(err_pipe).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let trimmed = line.trim();
                    if trimmed.starts_with("ready") {
                        if !ready.swap(true, Ordering::SeqCst) {
                            update_channel_status(&mix, &cb, &cid, "ok", None);
                        }
                    } else if let Some(msg) = trimmed.strip_prefix("error ") {
                        *last_err.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(msg.trim().to_string());
                    }
                }
            })
        });

        let mut received_audio = false;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = child.kill().await;
                    if let Some(t) = stderr_task {
                        t.abort();
                    }
                    return;
                }
                res = stdout.read(&mut read_buf) => {
                    match res {
                        Ok(0) => break,
                        Ok(n) => {
                            if !received_audio {
                                received_audio = true;
                                backoff_ms = 750;
                                if !ready_flag.swap(true, Ordering::SeqCst) {
                                    update_channel_status(&mixer, &status_cb, &channel_id, "ok", None);
                                }
                            }
                            let mut g = mixer.lock().unwrap_or_else(|e| e.into_inner());
                            g.push_external_pcm_bytes(&channel_id, &read_buf[..n]);
                        }
                        Err(_) => break,
                    }
                }
            }
        }

        let exit_code = tokio::select! {
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                if let Some(t) = stderr_task {
                    t.abort();
                }
                return;
            }
            status = child.wait() => status.ok().and_then(|s| s.code()),
        };

        if let Some(t) = stderr_task {
            let _ = tokio::time::timeout(Duration::from_millis(100), t).await;
        }

        if cancel.is_cancelled() {
            break;
        }

        match exit_code {
            Some(2) | Some(3) => {
                // 2 = app not open or not playing audio yet; 3 = app exited
                update_channel_status(
                    &mixer,
                    &status_cb,
                    &channel_id,
                    "waiting",
                    Some(format!("Aguardando {exe}...")),
                );
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
                }
                backoff_ms = (backoff_ms + 750).min(3000);
            }
            Some(0) => {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                }
            }
            code => {
                let err_detail = last_err_msg
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                    .unwrap_or_else(|| format!("Captura de {exe} encerrou (código {code:?})"));
                update_channel_status(
                    &mixer,
                    &status_cb,
                    &channel_id,
                    "error",
                    Some(err_detail),
                );
                let wait = backoff_ms.max(2000);
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
                }
                backoff_ms = (wait * 2).min(5000);
            }
        }
    }
}

// ------------------------------------------------------------------ Platform Mic / Desktop / Monitor Backends

#[cfg(target_os = "linux")]
mod platform_io {
    use super::*;
    use tokio::io::AsyncWriteExt;

    pub fn spawn_mic_capture(
        channel_id: String,
        device_id: Option<String>,
        mixer: Arc<Mutex<AudioMixerEngine>>,
        status_cb: Arc<Mutex<Option<StatusCallback>>>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::spawn(run_parec_worker(
            channel_id,
            device_id.filter(|d| !d.trim().is_empty() && d != "default"),
            mixer,
            status_cb,
            cancel,
        ))
    }

    pub fn spawn_desktop_capture(
        channel_id: String,
        device_id: Option<String>,
        mixer: Arc<Mutex<AudioMixerEngine>>,
        status_cb: Arc<Mutex<Option<StatusCallback>>>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        let target_device = match device_id
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty() && *d != "default")
        {
            Some(d) if d.ends_with(".monitor") || d.starts_with('@') => Some(d.to_string()),
            Some(d) => Some(format!("{d}.monitor")),
            None => Some("@DEFAULT_MONITOR@".to_string()),
        };
        tokio::spawn(run_parec_worker(
            channel_id,
            target_device,
            mixer,
            status_cb,
            cancel,
        ))
    }

    async fn run_parec_worker(
        channel_id: String,
        device: Option<String>,
        mixer: Arc<Mutex<AudioMixerEngine>>,
        status_cb: Arc<Mutex<Option<StatusCallback>>>,
        cancel: CancellationToken,
    ) {
        let mut backoff_ms: u64 = 1000;
        let mut buf = vec![0u8; CAPTURE_READ_CHUNK_BYTES];

        loop {
            if cancel.is_cancelled() {
                break;
            }

            let mut cmd = Command::new("parec");
            cmd.args([
                "--format=float32le",
                "--rate=48000",
                "--channels=2",
                "--raw",
                "--latency-msec=15",
            ]);
            if let Some(ref dev) = device {
                cmd.arg(format!("--device={dev}"));
            }
            cmd.stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    update_channel_status(
                        &mixer,
                        &status_cb,
                        &channel_id,
                        "error",
                        Some(format!("parec indisponível (instale pulseaudio-utils/pipewire-pulse): {e}")),
                    );
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
                    }
                    backoff_ms = (backoff_ms * 2).min(5000);
                    continue;
                }
            };

            let Some(mut stdout) = child.stdout.take() else {
                let _ = child.kill().await;
                break;
            };

            update_channel_status(&mixer, &status_cb, &channel_id, "ok", None);
            backoff_ms = 1000;

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        let _ = child.kill().await;
                        return;
                    }
                    res = stdout.read(&mut buf) => {
                        match res {
                            Ok(0) => break,
                            Ok(n) => {
                                let mut g = mixer.lock().unwrap_or_else(|e| e.into_inner());
                                g.push_external_pcm_bytes(&channel_id, &buf[..n]);
                            }
                            Err(_) => break,
                        }
                    }
                }
            }

            let _ = child.kill().await;
            let _ = child.wait().await;

            if cancel.is_cancelled() {
                break;
            }
            update_channel_status(
                &mixer,
                &status_cb,
                &channel_id,
                "waiting",
                Some("Reconectando captura de áudio...".to_string()),
            );
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
            }
            backoff_ms = (backoff_ms * 2).min(4000);
        }
    }

    pub fn spawn_monitor_sink(
        device_id: Option<String>,
        mut rx: mpsc::Receiver<Arc<Vec<f32>>>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut raw_bytes: Vec<u8> = Vec::with_capacity(QUANTUM_SAMPLES * 4);
            let dev_clean = device_id.filter(|d| !d.trim().is_empty() && d != "default");

            loop {
                if cancel.is_cancelled() {
                    break;
                }

                let mut cmd = Command::new("pacat");
                cmd.args([
                    "--playback",
                    "--format=float32le",
                    "--rate=48000",
                    "--channels=2",
                    "--raw",
                    "--latency-msec=20",
                ]);
                if let Some(ref dev) = dev_clean {
                    cmd.arg(format!("--device={dev}"));
                }
                cmd.stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);

                let mut child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        log::warn!("audio_runtime: pacat monitor indisponível: {e}");
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = tokio::time::sleep(Duration::from_secs(2)) => continue,
                        }
                    }
                };

                let Some(mut stdin) = child.stdin.take() else {
                    let _ = child.kill().await;
                    break;
                };

                let mut channel_closed = false;
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            let _ = child.kill().await;
                            return;
                        }
                        item = rx.recv() => {
                            let Some(samples) = item else {
                                channel_closed = true;
                                break;
                            };
                            raw_bytes.clear();
                            raw_bytes.reserve(samples.len() * 4);
                            for &s in samples.iter() {
                                raw_bytes.extend_from_slice(&s.to_le_bytes());
                            }
                            if stdin.write_all(&raw_bytes).await.is_err() {
                                break;
                            }
                        }
                    }
                }

                let _ = child.kill().await;
                let _ = child.wait().await;
                if channel_closed || cancel.is_cancelled() {
                    break;
                }
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                }
            }
        })
    }
}

#[cfg(windows)]
mod platform_io {
    use super::*;
    use std::collections::VecDeque;
    use wasapi::{
        initialize_mta, Device, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat,
    };

    const MAX_MONITOR_QUEUE_BYTES: usize = 48_000 * FRAME_BYTES / 8; // ~125 ms cap

    fn find_wasapi_device(
        enumerator: &DeviceEnumerator,
        dir: &Direction,
        device_id: Option<&str>,
    ) -> Result<Device, String> {
        if let Some(want) = device_id.map(str::trim).filter(|s| !s.is_empty() && *s != "default") {
            if let Ok(col) = enumerator.get_device_collection(dir) {
                for dev in &col {
                    let Ok(d) = dev else { continue };
                    if d.get_id().ok().as_deref() == Some(want) {
                        return Ok(d);
                    }
                }
            }
        }
        enumerator
            .get_default_device(dir)
            .map_err(|e| format!("Dispositivo WASAPI padrão indisponível: {e}"))
    }

    pub fn spawn_mic_capture(
        channel_id: String,
        device_id: Option<String>,
        mixer: Arc<Mutex<AudioMixerEngine>>,
        status_cb: Arc<Mutex<Option<StatusCallback>>>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::task::spawn_blocking(move || {
            run_wasapi_capture_loop(channel_id, device_id, false, mixer, status_cb, cancel);
        })
    }

    pub fn spawn_desktop_capture(
        channel_id: String,
        device_id: Option<String>,
        mixer: Arc<Mutex<AudioMixerEngine>>,
        status_cb: Arc<Mutex<Option<StatusCallback>>>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::task::spawn_blocking(move || {
            run_wasapi_capture_loop(channel_id, device_id, true, mixer, status_cb, cancel);
        })
    }

    fn run_wasapi_capture_loop(
        channel_id: String,
        device_id: Option<String>,
        loopback: bool,
        mixer: Arc<Mutex<AudioMixerEngine>>,
        status_cb: Arc<Mutex<Option<StatusCallback>>>,
        cancel: CancellationToken,
    ) {
        let _ = initialize_mta();
        let mut backoff_ms: u64 = 1000;

        while !cancel.is_cancelled() {
            match run_wasapi_capture_once(
                &channel_id,
                device_id.as_deref(),
                loopback,
                &mixer,
                &status_cb,
                &cancel,
            ) {
                Ok(()) => break,
                Err(err) => {
                    if cancel.is_cancelled() {
                        break;
                    }
                    update_channel_status(&mixer, &status_cb, &channel_id, "error", Some(err));
                    let steps = (backoff_ms / 100).max(1);
                    for _ in 0..steps {
                        if cancel.is_cancelled() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    backoff_ms = (backoff_ms * 2).min(5000);
                }
            }
        }
    }

    fn run_wasapi_capture_once(
        channel_id: &str,
        device_id: Option<&str>,
        loopback: bool,
        mixer: &Arc<Mutex<AudioMixerEngine>>,
        status_cb: &Arc<Mutex<Option<StatusCallback>>>,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        let enumerator =
            DeviceEnumerator::new().map_err(|e| format!("WASAPI DeviceEnumerator: {e}"))?;
        let endpoint_dir = if loopback {
            Direction::Render
        } else {
            Direction::Capture
        };
        let dev = find_wasapi_device(&enumerator, &endpoint_dir, device_id)?;
        let mut client = dev
            .get_iaudioclient()
            .map_err(|e| format!("WASAPI get_iaudioclient: {e}"))?;
        let format = WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None);
        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 0,
        };
        client
            .initialize_client(&format, &Direction::Capture, &mode)
            .map_err(|e| format!("WASAPI initialize_client: {e}"))?;
        let event = client
            .set_get_eventhandle()
            .map_err(|e| format!("WASAPI set_get_eventhandle: {e}"))?;
        let capture = client
            .get_audiocaptureclient()
            .map_err(|e| format!("WASAPI get_audiocaptureclient: {e}"))?;
        client
            .start_stream()
            .map_err(|e| format!("WASAPI start_stream: {e}"))?;

        update_channel_status(mixer, status_cb, channel_id, "ok", None);

        let mut queue: VecDeque<u8> = VecDeque::with_capacity(48000 * FRAME_BYTES / 4);
        while !cancel.is_cancelled() {
            let _ = event.wait_for_event(50);
            loop {
                match capture.get_next_packet_size() {
                    Ok(Some(n)) if n > 0 => {
                        if let Err(e) = capture.read_from_device_to_deque(&mut queue) {
                            let _ = client.stop_stream();
                            return Err(format!("WASAPI leitura falhou: {e}"));
                        }
                    }
                    Ok(_) => break,
                    Err(e) => {
                        let _ = client.stop_stream();
                        return Err(format!("WASAPI pacote falhou: {e}"));
                    }
                }
            }
            if !queue.is_empty() {
                let (a, b) = queue.as_slices();
                {
                    let mut g = mixer.lock().unwrap_or_else(|e| e.into_inner());
                    if !a.is_empty() {
                        g.push_external_pcm_bytes(channel_id, a);
                    }
                    if !b.is_empty() {
                        g.push_external_pcm_bytes(channel_id, b);
                    }
                }
                queue.clear();
            }
        }

        let _ = client.stop_stream();
        Ok(())
    }

    pub fn spawn_monitor_sink(
        device_id: Option<String>,
        mut rx: mpsc::Receiver<Arc<Vec<f32>>>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::task::spawn_blocking(move || {
            let _ = initialize_mta();
            while !cancel.is_cancelled() {
                match run_wasapi_monitor_once(device_id.as_deref(), &mut rx, &cancel) {
                    Ok(()) => break,
                    Err(e) => {
                        log::warn!("audio_runtime: monitor WASAPI falhou: {e}");
                        for _ in 0..10 {
                            if cancel.is_cancelled() {
                                return;
                            }
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }
                }
            }
        })
    }

    fn run_wasapi_monitor_once(
        device_id: Option<&str>,
        rx: &mut mpsc::Receiver<Arc<Vec<f32>>>,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        let enumerator =
            DeviceEnumerator::new().map_err(|e| format!("WASAPI DeviceEnumerator: {e}"))?;
        let dev = find_wasapi_device(&enumerator, &Direction::Render, device_id)?;
        let mut client = dev
            .get_iaudioclient()
            .map_err(|e| format!("WASAPI get_iaudioclient: {e}"))?;
        let format = WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None);
        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 0,
        };
        client
            .initialize_client(&format, &Direction::Render, &mode)
            .map_err(|e| format!("WASAPI render initialize_client: {e}"))?;
        let buffer_frames = client
            .get_buffer_size()
            .map_err(|e| format!("WASAPI get_buffer_size: {e}"))?;
        let event = client
            .set_get_eventhandle()
            .map_err(|e| format!("WASAPI render set_get_eventhandle: {e}"))?;
        let render = client
            .get_audiorenderclient()
            .map_err(|e| format!("WASAPI get_audiorenderclient: {e}"))?;
        client
            .start_stream()
            .map_err(|e| format!("WASAPI render start_stream: {e}"))?;

        let mut pending: VecDeque<u8> = VecDeque::with_capacity(MAX_MONITOR_QUEUE_BYTES);
        let mut scratch: Vec<u8> = Vec::with_capacity(buffer_frames as usize * FRAME_BYTES);

        while !cancel.is_cancelled() {
            loop {
                match rx.try_recv() {
                    Ok(samples) => {
                        for &s in samples.iter() {
                            pending.extend(s.to_le_bytes());
                        }
                        if pending.len() > MAX_MONITOR_QUEUE_BYTES {
                            let excess = pending.len() - MAX_MONITOR_QUEUE_BYTES;
                            let aligned = excess - (excess % FRAME_BYTES);
                            pending.drain(..aligned);
                        }
                    }
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        let _ = client.stop_stream();
                        return Ok(());
                    }
                }
            }

            let _ = event.wait_for_event(25);
            let avail = client
                .get_available_space_in_frames()
                .map_err(|e| format!("WASAPI get_available_space_in_frames: {e}"))?
                as usize;
            if avail == 0 {
                continue;
            }
            let need_bytes = avail * FRAME_BYTES;
            scratch.clear();
            scratch.resize(need_bytes, 0);
            let take = pending.len().min(need_bytes);
            for slot in scratch.iter_mut().take(take) {
                if let Some(b) = pending.pop_front() {
                    *slot = b;
                }
            }
            render
                .write_to_device(avail, &scratch[..need_bytes], None)
                .map_err(|e| format!("WASAPI write_to_device: {e}"))?;
        }

        let _ = client.stop_stream();
        Ok(())
    }
}

#[cfg(all(not(windows), not(target_os = "linux")))]
mod platform_io {
    use super::*;

    pub fn spawn_mic_capture(
        channel_id: String,
        _device_id: Option<String>,
        mixer: Arc<Mutex<AudioMixerEngine>>,
        status_cb: Arc<Mutex<Option<StatusCallback>>>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            update_channel_status(
                &mixer,
                &status_cb,
                &channel_id,
                "waiting",
                Some("Captura nativa de microfone aguardando entrada PCM externa.".to_string()),
            );
            cancel.cancelled().await;
        })
    }

    pub fn spawn_desktop_capture(
        channel_id: String,
        _device_id: Option<String>,
        mixer: Arc<Mutex<AudioMixerEngine>>,
        status_cb: Arc<Mutex<Option<StatusCallback>>>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            update_channel_status(
                &mixer,
                &status_cb,
                &channel_id,
                "waiting",
                Some("Loopback nativo de desktop aguardando entrada PCM externa.".to_string()),
            );
            cancel.cancelled().await;
        })
    }

    pub fn spawn_monitor_sink(
        _device_id: Option<String>,
        mut rx: mpsc::Receiver<Arc<Vec<f32>>>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    item = rx.recv() => {
                        if item.is_none() {
                            break;
                        }
                    }
                }
            }
        })
    }
}

// ------------------------------------------------------------------ AudioRuntime

/// Real-time background supervisor managing live OS audio capture, per-app isolation,
/// the 10 ms DSP quantum loop, Stinger one-shot sound mixing, and monitor playback.
pub struct AudioRuntime {
    mixer: Arc<Mutex<AudioMixerEngine>>,
    program_tx: broadcast::Sender<Arc<Vec<f32>>>,
    monitor_tx: Arc<Mutex<Option<mpsc::Sender<Arc<Vec<f32>>>>>>,
    oneshots: Arc<Mutex<Vec<OneshotAudio>>>,
    running: Arc<AtomicBool>,
    master_cancel: Arc<Mutex<Option<CancellationToken>>>,
    dsp_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    workers: Arc<Mutex<HashMap<String, CaptureWorkerEntry>>>,
    monitor_worker: Arc<Mutex<Option<MonitorWorkerEntry>>>,
    status_cb: Arc<Mutex<Option<StatusCallback>>>,
}

impl AudioRuntime {
    pub fn new(mixer: Arc<Mutex<AudioMixerEngine>>) -> Self {
        let (program_tx, _) = broadcast::channel(PROGRAM_BROADCAST_CAP);
        Self {
            mixer,
            program_tx,
            monitor_tx: Arc::new(Mutex::new(None)),
            oneshots: Arc::new(Mutex::new(Vec::new())),
            running: Arc::new(AtomicBool::new(false)),
            master_cancel: Arc::new(Mutex::new(None)),
            dsp_task: Arc::new(Mutex::new(None)),
            workers: Arc::new(Mutex::new(HashMap::new())),
            monitor_worker: Arc::new(Mutex::new(None)),
            status_cb: Arc::new(Mutex::new(None)),
        }
    }

    /// Registers a listener notified whenever any channel capture status transitions.
    pub fn set_status_callback<F>(&self, cb: F)
    where
        F: Fn(Vec<AudioChannelStatus>) + Send + Sync + 'static,
    {
        *self.status_cb.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(cb));
    }

    /// Returns `true` while the 10 ms DSP loop and capture supervisor are active.
    pub fn is_active(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Subscribes to the live mixed Program Bus PCM stream (`480` frames = `960` f32 stereo
    /// samples every 10 ms @ 48 kHz) for native encoders and WebRTC hosts.
    pub fn subscribe_program_pcm(&self) -> broadcast::Receiver<Arc<Vec<f32>>> {
        self.program_tx.subscribe()
    }

    /// Queues a one-shot interleaved 48 kHz stereo f32 sound effect (e.g. Stinger transition SFX)
    /// to be mixed cleanly into BOTH the Program Bus and Monitor Bus across subsequent 10 ms quantums.
    pub fn inject_oneshot_pcm(&self, mut interleaved_f32: Vec<f32>, gain_db: f32) {
        if interleaved_f32.len() < 2 {
            return;
        }
        // Keep stereo frame alignment strictly intact
        if interleaved_f32.len() % 2 != 0 {
            interleaved_f32.pop();
        }
        let safe_db = if gain_db.is_finite() {
            gain_db.clamp(-60.0, 24.0)
        } else {
            0.0
        };
        let gain_lin = db_to_linear(safe_db);
        if gain_lin <= 0.0 {
            return;
        }
        let mut guard = self.oneshots.lock().unwrap_or_else(|e| e.into_inner());
        if guard.len() >= MAX_ACTIVE_ONESHOTS {
            guard.remove(0);
        }
        guard.push(OneshotAudio {
            samples: interleaved_f32,
            cursor: 0,
            gain_lin,
        });
    }

    /// Runs a single 10 ms quantum through [`AudioMixerEngine::process_quantum`] and mixes any
    /// queued Stinger one-shot audio into both `program_out` and `monitor_out`.
    pub fn process_single_quantum_into(&self, program_out: &mut [f32], monitor_out: &mut [f32]) {
        Self::step_quantum(&self.mixer, &self.oneshots, program_out, monitor_out);
    }

    fn step_quantum(
        mixer: &Arc<Mutex<AudioMixerEngine>>,
        oneshots: &Arc<Mutex<Vec<OneshotAudio>>>,
        program_out: &mut [f32],
        monitor_out: &mut [f32],
    ) {
        {
            let mut g = mixer.lock().unwrap_or_else(|e| e.into_inner());
            g.process_quantum(program_out, monitor_out);
        }

        let mut shots = oneshots.lock().unwrap_or_else(|e| e.into_inner());
        if shots.is_empty() {
            return;
        }
        for shot in shots.iter_mut() {
            let rem = shot.samples.len().saturating_sub(shot.cursor);
            let n = rem.min(QUANTUM_SAMPLES);
            for i in 0..n {
                let raw = shot.samples[shot.cursor + i];
                if raw.is_finite() {
                    let s = raw * shot.gain_lin;
                    program_out[i] = (program_out[i] + s).clamp(-1.0, 1.0);
                    monitor_out[i] = (monitor_out[i] + s).clamp(-1.0, 1.0);
                }
            }
            shot.cursor += n;
        }
        shots.retain(|s| s.cursor < s.samples.len());
    }

    /// Starts the 10 ms real-time DSP loop and reconciles all channel capture/monitor workers.
    /// Safe to call repeatedly (idempotent: refreshes channels if already started).
    pub fn start(&self, settings: &AudioMixerSettings) {
        if self.running.swap(true, Ordering::SeqCst) {
            self.sync_channels(settings);
            return;
        }

        let Ok(rt_handle) = tokio::runtime::Handle::try_current() else {
            self.running.store(false, Ordering::SeqCst);
            log::warn!("audio_runtime: start() chamado fora de um runtime Tokio ativo.");
            return;
        };

        let cancel = CancellationToken::new();
        *self.master_cancel.lock().unwrap_or_else(|e| e.into_inner()) = Some(cancel.clone());

        let mixer_ref = self.mixer.clone();
        let oneshots_ref = self.oneshots.clone();
        let prog_tx = self.program_tx.clone();
        let mon_tx_ref = self.monitor_tx.clone();
        let cancel_dsp = cancel.clone();

        let dsp_handle = rt_handle.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(10));
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

            let mut prog = vec![0.0_f32; QUANTUM_SAMPLES];
            let mut mon = vec![0.0_f32; QUANTUM_SAMPLES];

            loop {
                tokio::select! {
                    _ = cancel_dsp.cancelled() => break,
                    _ = tick.tick() => {}
                }

                Self::step_quantum(&mixer_ref, &oneshots_ref, &mut prog, &mut mon);

                if prog_tx.receiver_count() > 0 {
                    let _ = prog_tx.send(Arc::new(prog.clone()));
                }

                let mon_sender = mon_tx_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                if let Some(tx) = mon_sender {
                    let _ = tx.try_send(Arc::new(mon.clone()));
                }
            }
        });

        *self.dsp_task.lock().unwrap_or_else(|e| e.into_inner()) = Some(dsp_handle);
        self.sync_channels(settings);
    }

    /// Reconciles active capture workers (`App`, `Mic`, `Desktop`) and monitor playback sink
    /// against the latest [`AudioMixerSettings`] without restarting unchanged channels.
    pub fn sync_channels(&self, settings: &AudioMixerSettings) {
        if !self.is_active() {
            return;
        }
        let Some(master_cancel) = self
            .master_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }

        // Build desired specs for each channel
        let mut desired: HashMap<String, CaptureSpec> = HashMap::new();
        for ch in &settings.channels {
            match ch.kind {
                AudioChannelKind::App => {
                    if let Some(exe) = ch
                        .app_exe
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        desired.insert(ch.id.clone(), CaptureSpec::App { exe: exe.to_string() });
                    } else {
                        update_channel_status(
                            &self.mixer,
                            &self.status_cb,
                            &ch.id,
                            "off",
                            None,
                        );
                    }
                }
                AudioChannelKind::Mic => {
                    desired.insert(
                        ch.id.clone(),
                        CaptureSpec::Mic {
                            device_id: ch.device_id.clone(),
                        },
                    );
                }
                AudioChannelKind::Desktop => {
                    desired.insert(
                        ch.id.clone(),
                        CaptureSpec::Desktop {
                            device_id: ch.device_id.clone(),
                        },
                    );
                }
                AudioChannelKind::PartyVoice => {
                    let st = if settings.party_voice_enabled {
                        "ok"
                    } else {
                        "off"
                    };
                    update_channel_status(&self.mixer, &self.status_cb, &ch.id, st, None);
                }
            }
        }

        // Reconcile per-channel capture workers
        {
            let mut workers = self.workers.lock().unwrap_or_else(|e| e.into_inner());
            let stale_ids: Vec<String> = workers
                .iter()
                .filter(|(id, entry)| desired.get(*id) != Some(&entry.spec))
                .map(|(id, _)| id.clone())
                .collect();

            for id in stale_ids {
                if let Some(entry) = workers.remove(&id) {
                    entry.cancel.cancel();
                    entry.handle.abort();
                }
            }

            for (id, spec) in desired {
                if workers.contains_key(&id) {
                    continue;
                }
                let child_cancel = master_cancel.child_token();
                let handle = match &spec {
                    CaptureSpec::App { exe } => tokio::spawn(run_app_capture_worker(
                        id.clone(),
                        exe.clone(),
                        self.mixer.clone(),
                        self.status_cb.clone(),
                        child_cancel.clone(),
                    )),
                    CaptureSpec::Mic { device_id } => platform_io::spawn_mic_capture(
                        id.clone(),
                        device_id.clone(),
                        self.mixer.clone(),
                        self.status_cb.clone(),
                        child_cancel.clone(),
                    ),
                    CaptureSpec::Desktop { device_id } => platform_io::spawn_desktop_capture(
                        id.clone(),
                        device_id.clone(),
                        self.mixer.clone(),
                        self.status_cb.clone(),
                        child_cancel.clone(),
                    ),
                };
                workers.insert(
                    id,
                    CaptureWorkerEntry {
                        spec,
                        cancel: child_cancel,
                        handle,
                    },
                );
            }
        }

        // Reconcile monitor output bus
        let monitor_needed = settings
            .channels
            .iter()
            .any(|c| c.monitor != AudioMonitorMode::Off);
        let mut mon_guard = self.monitor_worker.lock().unwrap_or_else(|e| e.into_inner());

        if !monitor_needed {
            if let Some(existing) = mon_guard.take() {
                existing.cancel.cancel();
                existing.handle.abort();
            }
            *self.monitor_tx.lock().unwrap_or_else(|e| e.into_inner()) = None;
        } else {
            let needs_restart = match mon_guard.as_ref() {
                None => true,
                Some(entry) => entry.device_id != settings.monitor_device_id,
            };
            if needs_restart {
                if let Some(existing) = mon_guard.take() {
                    existing.cancel.cancel();
                    existing.handle.abort();
                }
                let (tx, rx) = mpsc::channel(MONITOR_QUEUE_CAP);
                *self.monitor_tx.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
                let child_cancel = master_cancel.child_token();
                let handle = platform_io::spawn_monitor_sink(
                    settings.monitor_device_id.clone(),
                    rx,
                    child_cancel.clone(),
                );
                *mon_guard = Some(MonitorWorkerEntry {
                    device_id: settings.monitor_device_id.clone(),
                    cancel: child_cancel,
                    handle,
                });
            }
        }
    }

    /// Stops the 10 ms DSP loop and cleanly terminates every capture and monitor worker without
    /// leaking tasks or child processes.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);

        if let Some(cancel) = self
            .master_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            cancel.cancel();
        }

        {
            let mut workers = self.workers.lock().unwrap_or_else(|e| e.into_inner());
            for (_, entry) in workers.drain() {
                entry.cancel.cancel();
                entry.handle.abort();
            }
        }

        *self.monitor_tx.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if let Some(mon) = self
            .monitor_worker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            mon.cancel.cancel();
            mon.handle.abort();
        }

        if let Some(dsp) = self
            .dsp_task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            dsp.abort();
        }

        self.oneshots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

impl Drop for AudioRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

// ------------------------------------------------------------------ Unit Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AudioChannel, AudioFilters};
    use std::ffi::OsString;

    fn party_only_settings() -> AudioMixerSettings {
        // PartyVoice does not spawn OS capture commands, making tests deterministic in headless CI
        AudioMixerSettings {
            channels: vec![AudioChannel {
                id: "party".to_string(),
                kind: AudioChannelKind::PartyVoice,
                name: "Party".to_string(),
                filters: AudioFilters::default(),
                ..Default::default()
            }],
            party_voice_enabled: true,
            monitor_device_id: None,
        }
    }

    #[tokio::test]
    async fn test_audio_runtime_10ms_quantum_pump_and_clean_stop() {
        let settings = party_only_settings();
        let mixer = Arc::new(Mutex::new(AudioMixerEngine::new(settings.clone())));
        let runtime = AudioRuntime::new(mixer.clone());
        assert!(!runtime.is_active());

        let mut rx = runtime.subscribe_program_pcm();
        runtime.start(&settings);
        assert!(runtime.is_active());

        // Feed primed PCM (60 ms) into PartyVoice channel so the 10 ms pump outputs audible samples
        let prime_frames = (48_000 * 60) / 1000;
        let mut samples = Vec::with_capacity(prime_frames * 2);
        for _ in 0..prime_frames {
            samples.push(0.25_f32);
            samples.push(-0.25_f32);
        }
        mixer
            .lock()
            .unwrap()
            .push_external_pcm("party", &samples);

        // Wait for at least one non-zero 10 ms quantum broadcast from the background RT loop
        let mut got_signal = false;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(chunk)) =
                tokio::time::timeout(Duration::from_millis(40), rx.recv()).await
            {
                assert_eq!(chunk.len(), QUANTUM_SAMPLES);
                if chunk.iter().any(|&s| s.abs() > 0.05) {
                    got_signal = true;
                    break;
                }
            }
        }
        assert!(got_signal, "expected live 10ms quantum broadcast with audio signal");

        runtime.stop();
        assert!(!runtime.is_active());

        // Drain any buffered frame already sent before stop, then verify no further ticks arrive
        tokio::time::sleep(Duration::from_millis(25)).await;
        while rx.try_recv().is_ok() {}
        let after_stop = tokio::time::timeout(Duration::from_millis(40), rx.recv()).await;
        assert!(after_stop.is_err(), "expected no quantum broadcasts after stop()");
    }

    #[test]
    fn test_oneshot_stinger_pcm_mixing_and_drain() {
        let settings = party_only_settings();
        let mixer = Arc::new(Mutex::new(AudioMixerEngine::new(settings)));
        let runtime = AudioRuntime::new(mixer);

        // Inject 1.5 quantums of stereo SFX at -6 dB (~0.5012x linear)
        let total_samples = QUANTUM_SAMPLES + QUANTUM_SAMPLES / 2;
        let mut sfx = Vec::with_capacity(total_samples);
        for _ in 0..(total_samples / 2) {
            sfx.push(0.8_f32);
            sfx.push(-0.4_f32);
        }
        runtime.inject_oneshot_pcm(sfx, -6.0);

        let expected_gain = db_to_linear(-6.0);
        let mut prog = vec![0.0_f32; QUANTUM_SAMPLES];
        let mut mon = vec![0.0_f32; QUANTUM_SAMPLES];

        // Quantum 1: full quantum of SFX mixed into BOTH Program and Monitor buses
        runtime.process_single_quantum_into(&mut prog, &mut mon);
        assert!((prog[0] - 0.8 * expected_gain).abs() < 1e-4);
        assert!((prog[1] - (-0.4 * expected_gain)).abs() < 1e-4);
        assert!((mon[0] - 0.8 * expected_gain).abs() < 1e-4);
        assert!((mon[1] - (-0.4 * expected_gain)).abs() < 1e-4);

        // Quantum 2: first half has remaining SFX, second half is silent
        runtime.process_single_quantum_into(&mut prog, &mut mon);
        assert!((prog[0] - 0.8 * expected_gain).abs() < 1e-4);
        assert_eq!(prog[QUANTUM_SAMPLES - 1], 0.0);
        assert_eq!(mon[QUANTUM_SAMPLES - 1], 0.0);

        // Quantum 3: completely drained and removed
        runtime.process_single_quantum_into(&mut prog, &mut mon);
        assert!(prog.iter().all(|&s| s == 0.0));
        assert!(mon.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_resolve_audio_helper_binary_sibling_parent_and_path() {
        let base = std::env::temp_dir().join(format!(
            "prismcast_helper_resolve_{}",
            uuid::Uuid::new_v4()
        ));
        let exe_dir = base.join("bin");
        let deps_dir = exe_dir.join("deps");
        let path_dir = base.join("path_bin");
        std::fs::create_dir_all(&deps_dir).unwrap();
        std::fs::create_dir_all(&path_dir).unwrap();

        // 1. Neither exists -> None
        assert_eq!(
            resolve_audio_helper_in(Some(&exe_dir), Some(OsStr::new(&path_dir))),
            None
        );

        // 2. Exists on PATH -> resolves from PATH
        let path_bin = path_dir.join(AUDIO_HELPER_BIN);
        std::fs::write(&path_bin, b"stub").unwrap();
        assert_eq!(
            resolve_audio_helper_in(Some(&exe_dir), Some(&OsString::from(&path_dir))),
            Some(path_bin.clone())
        );

        // 3. Exists as sibling of current_exe -> takes priority over PATH!
        let sibling_bin = exe_dir.join(AUDIO_HELPER_BIN);
        std::fs::write(&sibling_bin, b"stub").unwrap();
        assert_eq!(
            resolve_audio_helper_in(Some(&exe_dir), Some(&OsString::from(&path_dir))),
            Some(sibling_bin.clone())
        );

        // 4. Called from Cargo `deps/` subdir -> finds parent `sibling_bin` before PATH!
        assert_eq!(
            resolve_audio_helper_in(Some(&deps_dir), Some(&OsString::from(&path_dir))),
            Some(sibling_bin)
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
