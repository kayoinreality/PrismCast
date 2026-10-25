//! prismcast-audio — áudio de um aplicativo só (o "Captura de áudio de aplicativo" do OBS).
//!
//! Usa o loopback por processo do WASAPI (Windows 10 2004+): pega só o som que
//! um programa e os filhos dele tocam, não o PC inteiro.
//!
//!   prismcast-audio list
//!       JSON com as janelas abertas (e programas tocando som) para o seletor.
//!   prismcast-audio capture --exe jogo.exe | --pid 1234
//!       Escreve no stdout PCM f32 little-endian, 48 kHz, estéreo intercalado.
//!       Saída 2 = programa não está aberto; 3 = o programa fechou.

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("list") => win::list(),
        Some("capture") => {
            let get = |flag: &str| {
                args.iter()
                    .position(|a| a == flag)
                    .and_then(|i| args.get(i + 1))
                    .cloned()
            };
            match (get("--exe"), get("--pid").and_then(|p| p.parse::<u32>().ok())) {
                (_, Some(pid)) => win::capture(pid),
                (Some(exe), None) => match win::find_root_pid(&exe) {
                    Some(pid) => win::capture(pid),
                    None => {
                        eprintln!("not-found {exe}");
                        2
                    }
                },
                _ => usage(),
            }
        }
        _ => usage(),
    };
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}

fn usage() -> i32 {
    eprintln!("uso: prismcast-audio list | capture --exe <nome.exe> | capture --pid <pid>");
    1
}

#[cfg(not(windows))]
mod win {
    pub fn list() -> i32 {
        println!("[]");
        0
    }
    pub fn capture(_pid: u32) -> i32 {
        eprintln!("unsupported");
        1
    }
    pub fn find_root_pid(_exe: &str) -> Option<u32> {
        None
    }
}

#[cfg(windows)]
mod win {
    use std::collections::{BTreeMap, HashSet, VecDeque};
    use std::io::Write;

    use serde_json::json;
    use wasapi::{
        initialize_mta, AudioClient, DeviceEnumerator, Direction, SampleType, SessionState, StreamMode,
        WaveFormat,
    };
    use windows::core::{BOOL, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowLongW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
        GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW,
    };

    const STILL_ACTIVE: u32 = 259;

    struct Win {
        hwnd: isize,
        pid: u32,
        title: String,
    }

    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = &mut *(lparam.0 as *mut Vec<Win>);
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        if GetWindow(hwnd, GW_OWNER).map(|h| !h.is_invalid()).unwrap_or(false) {
            return BOOL(1);
        }
        if (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32) & WS_EX_TOOLWINDOW.0 != 0 {
            return BOOL(1);
        }
        // Janelas "escondidas" de apps da loja continuam visíveis para o EnumWindows.
        let mut cloaked: u32 = 0;
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        );
        if cloaked != 0 {
            return BOOL(1);
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n <= 0 {
            return BOOL(1);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        out.push(Win {
            hwnd: hwnd.0 as isize,
            pid,
            title: String::from_utf16_lossy(&buf[..n as usize]),
        });
        BOOL(1)
    }

    fn exe_path(pid: u32) -> Option<String> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut size = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size).is_ok();
            let _ = CloseHandle(h);
            ok.then(|| String::from_utf16_lossy(&buf[..size as usize]))
        }
    }

    fn file_name(path: &str) -> String {
        path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
    }

    /// pid -> (exe, pai)
    fn processes() -> BTreeMap<u32, (String, u32)> {
        let mut out = BTreeMap::new();
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return out;
            };
            let mut e = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut ok = Process32FirstW(snap, &mut e).is_ok();
            while ok {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let exe = String::from_utf16_lossy(&e.szExeFile[..len]);
                out.insert(e.th32ProcessID, (exe, e.th32ParentProcessID));
                ok = Process32NextW(snap, &mut e).is_ok();
            }
            let _ = CloseHandle(snap);
        }
        out
    }

    /// pids com sessão de áudio ativa (programas tocando som agora).
    fn audio_pids() -> HashSet<u32> {
        let mut pids = HashSet::new();
        let Ok(enumerator) = DeviceEnumerator::new() else {
            return pids;
        };
        let Ok(devices) = enumerator.get_device_collection(&Direction::Render) else {
            return pids;
        };
        for device in &devices {
            let Ok(dev) = device else { continue };
            let Ok(manager) = dev.get_iaudiosessionmanager() else { continue };
            let Ok(sessions) = manager.get_audiosessionenumerator() else { continue };
            for i in 0..sessions.get_count().unwrap_or(0) {
                let Ok(control) = sessions.get_session(i) else { continue };
                if control.get_state().ok() != Some(SessionState::Active) {
                    continue;
                }
                if let Ok(pid) = control.get_process_id() {
                    if pid != 0 {
                        pids.insert(pid);
                    }
                }
            }
        }
        pids
    }

    /// O processo mais alto da árvore com o mesmo exe (navegadores e jogos abrem vários).
    /// Com várias árvores do mesmo exe (dois terminais, duas instâncias), fica com a
    /// que está tocando som agora.
    pub fn find_root_pid(exe: &str) -> Option<u32> {
        let procs = processes();
        let want = exe.to_lowercase();
        let roots: Vec<u32> = procs
            .iter()
            .filter(|(_, (e, _))| e.to_lowercase() == want)
            .filter(|(_, (_, parent))| {
                procs
                    .get(parent)
                    .map(|(pe, _)| pe.to_lowercase() != want)
                    .unwrap_or(true)
            })
            .map(|(pid, _)| *pid)
            .collect();
        if roots.len() > 1 {
            let _ = initialize_mta();
            let playing = audio_pids();
            let in_tree = |root: u32, mut pid: u32| {
                for _ in 0..64 {
                    if pid == root {
                        return true;
                    }
                    match procs.get(&pid) {
                        Some((_, parent)) if *parent != pid && *parent != 0 => pid = *parent,
                        _ => return false,
                    }
                }
                false
            };
            if let Some(root) = roots.iter().find(|r| playing.iter().any(|p| in_tree(**r, *p))) {
                return Some(*root);
            }
        }
        roots.first().copied()
    }

    pub fn list() -> i32 {
        let _ = initialize_mta();
        let mut wins: Vec<Win> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut wins as *mut Vec<Win> as isize));
        }
        let me = std::process::id();
        let audio = audio_pids();
        let procs = processes();
        // Som tocando num processo filho (abas do navegador) conta para o exe da janela.
        let audio_exes: HashSet<String> = audio
            .iter()
            .filter_map(|pid| procs.get(pid).map(|(e, _)| e.to_lowercase()))
            .collect();

        let mut seen_exe = HashSet::new();
        let mut items = Vec::new();
        for w in &wins {
            if w.pid == me {
                continue;
            }
            let Some(path) = exe_path(w.pid) else { continue };
            let exe = file_name(&path);
            let lower = exe.to_lowercase();
            if lower == "explorer.exe"
                || lower == "textinputhost.exe"
                || lower == "applicationframehost.exe"
                || lower == "prismcast-audio.exe"
            {
                continue;
            }
            seen_exe.insert(lower.clone());
            items.push(json!({
                "hwnd": w.hwnd,
                "pid": w.pid,
                "exe": exe,
                "path": path,
                "title": w.title,
                "audio": audio_exes.contains(&lower),
            }));
        }
        // Programas tocando som sem janela visível (ex.: player minimizado na bandeja).
        for pid in &audio {
            let Some((exe, _)) = procs.get(pid) else { continue };
            let lower = exe.to_lowercase();
            if seen_exe.contains(&lower) || lower == "audiodg.exe" || lower == "prismcast-audio.exe" || *pid == me {
                continue;
            }
            seen_exe.insert(lower);
            items.push(json!({
                "hwnd": 0,
                "pid": pid,
                "exe": exe,
                "path": exe_path(*pid).unwrap_or_default(),
                "title": exe.trim_end_matches(".exe"),
                "audio": true,
            }));
        }
        println!("{}", serde_json::Value::Array(items));
        0
    }

    fn alive(pid: u32) -> bool {
        unsafe {
            let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return false;
            };
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code).is_ok();
            let _ = CloseHandle(h);
            ok && code == STILL_ACTIVE
        }
    }

    pub fn capture(pid: u32) -> i32 {
        if !alive(pid) {
            eprintln!("not-found {pid}");
            return 2;
        }
        let _ = initialize_mta();
        let format = WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None);
        let mut client = match AudioClient::new_application_loopback_client(pid, true) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error {e}");
                return 1;
            }
        };
        let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 };
        if let Err(e) = client.initialize_client(&format, &Direction::Capture, &mode) {
            eprintln!("error {e}");
            return 1;
        }
        let (Ok(event), Ok(capture)) = (client.set_get_eventhandle(), client.get_audiocaptureclient()) else {
            eprintln!("error sem cliente de captura");
            return 1;
        };
        if let Err(e) = client.start_stream() {
            eprintln!("error {e}");
            return 1;
        }
        eprintln!("ready {pid}");

        let mut out = std::io::stdout().lock();
        let mut queue: VecDeque<u8> = VecDeque::with_capacity(48000 * 8);
        let mut last_check = std::time::Instant::now();
        loop {
            // Programa calado não gera eventos: o timeout é a hora de ver se ele ainda existe.
            let _ = event.wait_for_event(250);
            if last_check.elapsed().as_millis() > 1000 {
                last_check = std::time::Instant::now();
                if !alive(pid) {
                    eprintln!("exited {pid}");
                    let _ = client.stop_stream();
                    return 3;
                }
            }
            loop {
                match capture.get_next_packet_size() {
                    Ok(Some(n)) if n > 0 => {
                        if capture.read_from_device_to_deque(&mut queue).is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            if queue.is_empty() {
                continue;
            }
            let (a, b) = queue.as_slices();
            if out.write_all(a).and_then(|_| out.write_all(b)).and_then(|_| out.flush()).is_err() {
                // O PrismCast fechou o pipe: fim normal.
                let _ = client.stop_stream();
                return 0;
            }
            queue.clear();
        }
    }
}
