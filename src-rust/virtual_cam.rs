//! Cross-Platform Virtual Camera Output Engine (`virtual_cam`).
//!
//! Exports the PrismCast compositor output as a virtual camera consumable by
//! Discord, Zoom, Google Meet, Microsoft Teams, OBS, and browsers:
//! - **Color-space conversion (pure Rust, reusable scratch buffer, zero steady-state allocs)**:
//!   - [`bgra_to_yuyv`]: BT.601 limited-range YUY2 (4:2:2 packed) for Linux `v4l2loopback` and Windows DirectShow.
//!   - [`bgra_to_nv12`]: BT.601 limited-range NV12 (Y plane + interleaved UV plane) for Windows Media Foundation and PipeWire.
//! - **Linux**: scans `/dev/video0..63` via `VIDIOC_QUERYCAP` ioctl to discover `v4l2loopback`
//!   output devices and probes PipeWire availability; configures output format with `VIDIOC_S_FMT`
//!   (`V4L2_BUF_TYPE_VIDEO_OUTPUT`, `V4L2_PIX_FMT_YUYV`) or falls back cleanly to a software
//!   virtual-sink mode with an actionable `modprobe v4l2loopback` hint.
//! - **Windows**: dynamically probes `MFCreateVirtualCamera` in `mfsensorgroup.dll` (Windows 11 22H2+)
//!   via `libloading` and publishes frames into a named shared-memory section
//!   (`Local\PrismCastVirtualCamFrame`) with an explicit versioned header for DirectShow / MF consumers.

use crate::protocol::{
    CanvasOrientation, VirtualCameraDevice, VirtualCameraSettings, VirtualCameraStatus,
};

/// Named Windows shared-memory section exposing live virtual camera frames.
pub const VIRTUAL_CAM_SHM_NAME: &str = r"Local\PrismCastVirtualCamFrame";
/// FourCC-like magic header (`"PCVC"`) stored at byte offset 0 of shared memory.
pub const VIRTUAL_CAM_SHM_MAGIC: u32 = 0x5043_5643;
/// Shared-memory wire layout version.
pub const VIRTUAL_CAM_SHM_VERSION: u32 = 1;
/// FourCC `'YUYV'` (`0x56595559`).
pub const VIRTUAL_CAM_FMT_YUYV: u32 = 0x5659_5559;
/// FourCC `'NV12'` (`0x3231564E`).
pub const VIRTUAL_CAM_FMT_NV12: u32 = 0x3231_564E;
/// Exact byte size of [`SharedFrameHeader`] preceding the pixel payload in shared memory.
pub const VIRTUAL_CAM_SHM_HEADER_BYTES: usize = std::mem::size_of::<SharedFrameHeader>();

/// Actionable shell command shown on Linux when `v4l2loopback` is not loaded.
pub const LINUX_V4L2LOOPBACK_HINT: &str =
    r#"sudo modprobe v4l2loopback devices=1 video_nr=10 card_label="PrismCast Virtual Camera" exclusive_caps=1"#;

/// Binary header written at the beginning of `Local\PrismCastVirtualCamFrame`.
///
/// Field layout is strictly `#[repr(C)]` with an explicit 32-bit `reserved` pad so that
/// `seq` and `timestamp_ms` are naturally 8-byte aligned across Rust and C/C++ consumers.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedFrameHeader {
    pub magic: u32,
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub reserved: u32,
    pub seq: u64,
    pub timestamp_ms: u64,
}

impl SharedFrameHeader {
    /// Serializes the header into its 40-byte little-endian representation.
    pub fn to_le_bytes(self) -> [u8; 40] {
        let mut buf = [0u8; 40];
        buf[0..4].copy_from_slice(&self.magic.to_le_bytes());
        buf[4..8].copy_from_slice(&self.version.to_le_bytes());
        buf[8..12].copy_from_slice(&self.width.to_le_bytes());
        buf[12..16].copy_from_slice(&self.height.to_le_bytes());
        buf[16..20].copy_from_slice(&self.format.to_le_bytes());
        buf[20..24].copy_from_slice(&self.reserved.to_le_bytes());
        buf[24..32].copy_from_slice(&self.seq.to_le_bytes());
        buf[32..40].copy_from_slice(&self.timestamp_ms.to_le_bytes());
        buf
    }

    /// Parses a 40-byte little-endian header slice.
    pub fn from_le_bytes(buf: &[u8; 40]) -> Self {
        Self {
            magic: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            version: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            width: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            height: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
            format: u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]),
            reserved: u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]),
            seq: u64::from_le_bytes([
                buf[24], buf[25], buf[26], buf[27], buf[28], buf[29], buf[30], buf[31],
            ]),
            timestamp_ms: u64::from_le_bytes([
                buf[32], buf[33], buf[34], buf[35], buf[36], buf[37], buf[38], buf[39],
            ]),
        }
    }
}

/// Returns the canonical virtual camera output resolution for a canvas orientation.
#[inline]
pub fn dimensions_for_orientation(orientation: CanvasOrientation) -> (u32, u32) {
    match orientation {
        CanvasOrientation::Horizontal => (1920, 1080),
        CanvasOrientation::Vertical => (1080, 1920),
    }
}

// ------------------------------------------------------------------ Color-space conversion

#[inline(always)]
fn rgb_to_y_bt601(r: i32, g: i32, b: i32) -> u8 {
    let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
    y.clamp(0, 255) as u8
}

#[inline(always)]
fn rgb_pair_to_uv_bt601(r_sum: i32, g_sum: i32, b_sum: i32) -> (u8, u8) {
    let u = ((-38 * r_sum - 74 * g_sum + 112 * b_sum + 256) >> 9) + 128;
    let v = ((112 * r_sum - 94 * g_sum - 18 * b_sum + 256) >> 9) + 128;
    (u.clamp(0, 255) as u8, v.clamp(0, 255) as u8)
}

#[inline(always)]
fn rgb_quad_to_uv_bt601(r_sum: i32, g_sum: i32, b_sum: i32) -> (u8, u8) {
    let u = ((-38 * r_sum - 74 * g_sum + 112 * b_sum + 512) >> 10) + 128;
    let v = ((112 * r_sum - 94 * g_sum - 18 * b_sum + 512) >> 10) + 128;
    (u.clamp(0, 255) as u8, v.clamp(0, 255) as u8)
}

/// Converts a BGRA (`[B, G, R, A]`) frame into packed YUYV / YUY2 (4:2:2, BT.601 studio range).
///
/// Reuses `out`'s existing allocation without reallocating when `out.capacity()` is already
/// at least `(width & !1) * height * 2` bytes. Each horizontal pixel pair produces 4 bytes:
/// `[Y0, U, Y1, V]`.
pub fn bgra_to_yuyv(bgra: &[u8], width: u32, height: u32, out: &mut Vec<u8>) {
    let w = width as usize;
    let h = height as usize;
    let w_even = w & !1;
    let required_src = match w.checked_mul(h).and_then(|px| px.checked_mul(4)) {
        Some(n) => n,
        None => {
            out.clear();
            return;
        }
    };
    if w_even == 0 || h == 0 || bgra.len() < required_src {
        out.clear();
        return;
    }

    let pairs_per_row = w_even / 2;
    let dst_row_bytes = pairs_per_row * 4;
    let total_dst = dst_row_bytes * h;
    out.clear();
    out.resize(total_dst, 0);

    let src_stride = w * 4;
    let src_used_row = pairs_per_row * 8;

    for y in 0..h {
        let src_row = &bgra[y * src_stride..y * src_stride + src_used_row];
        let dst_row = &mut out[y * dst_row_bytes..(y + 1) * dst_row_bytes];

        for (src_pair, dst_quad) in src_row.chunks_exact(8).zip(dst_row.chunks_exact_mut(4)) {
            let b0 = src_pair[0] as i32;
            let g0 = src_pair[1] as i32;
            let r0 = src_pair[2] as i32;
            let b1 = src_pair[4] as i32;
            let g1 = src_pair[5] as i32;
            let r1 = src_pair[6] as i32;

            let y0 = rgb_to_y_bt601(r0, g0, b0);
            let y1 = rgb_to_y_bt601(r1, g1, b1);
            let (u, v) = rgb_pair_to_uv_bt601(r0 + r1, g0 + g1, b0 + b1);

            dst_quad[0] = y0;
            dst_quad[1] = u;
            dst_quad[2] = y1;
            dst_quad[3] = v;
        }
    }
}

/// Converts a BGRA (`[B, G, R, A]`) frame into planar/semi-planar NV12 (4:2:0, BT.601 studio range).
///
/// Output layout for even `width` and `height`:
/// - Y plane: `width * height` bytes (1 byte per pixel)
/// - Interleaved UV plane: `width * height / 2` bytes (`[U, V]` per 2x2 pixel block)
///
/// Reuses `out`'s existing allocation without reallocating when capacity is sufficient.
pub fn bgra_to_nv12(bgra: &[u8], width: u32, height: u32, out: &mut Vec<u8>) {
    let w = width as usize;
    let h = height as usize;
    let w_even = w & !1;
    let h_even = h & !1;
    let required_src = match w.checked_mul(h).and_then(|px| px.checked_mul(4)) {
        Some(n) => n,
        None => {
            out.clear();
            return;
        }
    };
    if w_even == 0 || h_even == 0 || bgra.len() < required_src {
        out.clear();
        return;
    }

    let y_plane_len = w_even * h_even;
    let uv_plane_len = y_plane_len / 2;
    let total_dst = y_plane_len + uv_plane_len;
    out.clear();
    out.resize(total_dst, 0);

    let (y_plane, uv_plane) = out.split_at_mut(y_plane_len);
    let src_stride = w * 4;
    let src_used_row = w_even * 4;

    for row_pair in 0..(h_even / 2) {
        let y_top = row_pair * 2;
        let y_bot = y_top + 1;

        let src_top = &bgra[y_top * src_stride..y_top * src_stride + src_used_row];
        let src_bot = &bgra[y_bot * src_stride..y_bot * src_stride + src_used_row];

        let (dst_y_top, dst_y_bot) =
            y_plane[y_top * w_even..(y_bot + 1) * w_even].split_at_mut(w_even);
        let dst_uv = &mut uv_plane[row_pair * w_even..(row_pair + 1) * w_even];

        for (((top_pair, bot_pair), (yt_out, yb_out)), uv_out) in src_top
            .chunks_exact(8)
            .zip(src_bot.chunks_exact(8))
            .zip(dst_y_top.chunks_exact_mut(2).zip(dst_y_bot.chunks_exact_mut(2)))
            .zip(dst_uv.chunks_exact_mut(2))
        {
            let b00 = top_pair[0] as i32;
            let g00 = top_pair[1] as i32;
            let r00 = top_pair[2] as i32;
            let b01 = top_pair[4] as i32;
            let g01 = top_pair[5] as i32;
            let r01 = top_pair[6] as i32;

            let b10 = bot_pair[0] as i32;
            let g10 = bot_pair[1] as i32;
            let r10 = bot_pair[2] as i32;
            let b11 = bot_pair[4] as i32;
            let g11 = bot_pair[5] as i32;
            let r11 = bot_pair[6] as i32;

            yt_out[0] = rgb_to_y_bt601(r00, g00, b00);
            yt_out[1] = rgb_to_y_bt601(r01, g01, b01);
            yb_out[0] = rgb_to_y_bt601(r10, g10, b10);
            yb_out[1] = rgb_to_y_bt601(r11, g11, b11);

            let (u, v) = rgb_quad_to_uv_bt601(
                r00 + r01 + r10 + r11,
                g00 + g01 + g10 + g11,
                b00 + b01 + b10 + b11,
            );
            uv_out[0] = u;
            uv_out[1] = v;
        }
    }
}

// ------------------------------------------------------------------ Linux V4L2 + PipeWire backend

#[cfg(target_os = "linux")]
mod linux_vcam {
    use super::VIRTUAL_CAM_FMT_YUYV;
    use crate::protocol::VirtualCameraDevice;
    use std::ffi::CString;
    use std::fs::File;
    use std::io::Write;
    use std::os::unix::io::FromRawFd;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub const VIDIOC_QUERYCAP: u32 = 0x8068_5600;
    pub const V4L2_CAP_VIDEO_OUTPUT: u32 = 0x0000_0002;
    pub const V4L2_CAP_DEVICE_CAPS: u32 = 0x8000_0000;
    pub const V4L2_BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
    pub const V4L2_FIELD_NONE: u32 = 1;
    pub const V4L2_COLORSPACE_SMPTE170M: u32 = 1;

    /// Matches `<linux/videodev2.h>` `struct v4l2_capability` (104 bytes).
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct V4l2Capability {
        pub driver: [u8; 16],
        pub card: [u8; 32],
        pub bus_info: [u8; 32],
        pub version: u32,
        pub capabilities: u32,
        pub device_caps: u32,
        pub reserved: [u32; 3],
    }

    impl Default for V4l2Capability {
        fn default() -> Self {
            Self {
                driver: [0; 16],
                card: [0; 32],
                bus_info: [0; 32],
                version: 0,
                capabilities: 0,
                device_caps: 0,
                reserved: [0; 3],
            }
        }
    }

    /// Matches `<linux/videodev2.h>` `struct v4l2_pix_format` (48 bytes).
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct V4l2PixFormat {
        pub width: u32,
        pub height: u32,
        pub pixelformat: u32,
        pub field: u32,
        pub bytesperline: u32,
        pub sizeimage: u32,
        pub colorspace: u32,
        pub priv_field: u32,
        pub flags: u32,
        pub ycbcr_enc: u32,
        pub quantization: u32,
        pub xfer_func: u32,
    }

    /// Union matching `<linux/videodev2.h>` `v4l2_format.fmt`.
    /// `_align` preserves the pointer alignment of `struct v4l2_window` (8 on 64-bit, 4 on 32-bit)
    /// so `sizeof(V4l2Format)` is 208 on 64-bit and 204 on 32-bit, matching the kernel ABI exactly.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub union V4l2FormatUnion {
        pub pix: V4l2PixFormat,
        pub raw_data: [u8; 200],
        pub _align: usize,
    }

    /// Matches `<linux/videodev2.h>` `struct v4l2_format`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct V4l2Format {
        pub buf_type: u32,
        pub fmt: V4l2FormatUnion,
    }

    /// `_IOWR('V', 5, struct v4l2_format)` computed from exact ABI struct size.
    pub const VIDIOC_S_FMT: u32 =
        (3u32 << 30) | ((std::mem::size_of::<V4l2Format>() as u32) << 16) | ((b'V' as u32) << 8) | 5;

    fn c_bytes_to_string(bytes: &[u8]) -> String {
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).trim().to_string()
    }

    fn is_pipewire_running() -> bool {
        if let Ok(pw_dir) = std::env::var("PIPEWIRE_RUNTIME_DIR") {
            if PathBuf::from(pw_dir).join("pipewire-0").exists() {
                return true;
            }
        }
        if let Ok(xdg_dir) = std::env::var("XDG_RUNTIME_DIR") {
            if PathBuf::from(xdg_dir).join("pipewire-0").exists() {
                return true;
            }
        }
        Command::new("pw-cli")
            .args(["info", "0"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn list_devices() -> Vec<VirtualCameraDevice> {
        let mut devices = Vec::new();

        for idx in 0..=63u32 {
            let path = format!("/dev/video{idx}");
            if !Path::new(&path).exists() {
                continue;
            }
            let Ok(c_path) = CString::new(path.as_str()) else {
                continue;
            };
            let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK) };
            if fd < 0 {
                continue;
            }

            let mut cap = V4l2Capability::default();
            let ret = unsafe { libc::ioctl(fd, VIDIOC_QUERYCAP as libc::Ioctl, &mut cap) };
            unsafe {
                libc::close(fd);
            }
            if ret != 0 {
                continue;
            }

            let driver = c_bytes_to_string(&cap.driver);
            let card = c_bytes_to_string(&cap.card);
            let eff_caps = if (cap.capabilities & V4L2_CAP_DEVICE_CAPS) != 0 {
                cap.device_caps
            } else {
                cap.capabilities
            };
            let is_loopback = driver.eq_ignore_ascii_case("v4l2 loopback")
                || driver.to_ascii_lowercase().contains("loopback");
            let supports_output = (eff_caps & V4L2_CAP_VIDEO_OUTPUT) != 0
                || (cap.capabilities & V4L2_CAP_VIDEO_OUTPUT) != 0;

            if is_loopback || supports_output {
                let label = if card.is_empty() {
                    format!("V4L2 Loopback ({path})")
                } else {
                    format!("{card} ({path})")
                };
                devices.push(VirtualCameraDevice {
                    id: path,
                    name: label,
                    backend: "v4l2loopback".to_string(),
                    available: true,
                });
            }
        }

        if is_pipewire_running() {
            devices.push(VirtualCameraDevice {
                id: "pipewire:prismcast-vcam".to_string(),
                name: "PrismCast Virtual Camera (PipeWire)".to_string(),
                backend: "pipewire".to_string(),
                available: true,
            });
        }

        if devices.is_empty() {
            devices.push(VirtualCameraDevice {
                id: "software-sink".to_string(),
                name: "PrismCast Software Virtual Sink (v4l2loopback required for OS apps)"
                    .to_string(),
                backend: "software".to_string(),
                available: false,
            });
        }

        devices
    }

    pub struct LinuxV4l2Sink {
        file: File,
        pub device_path: String,
    }

    impl LinuxV4l2Sink {
        pub fn open(device_path: &str, width: u32, height: u32) -> Result<Self, String> {
            let c_path = CString::new(device_path)
                .map_err(|_| format!("Caminho de dispositivo V4L2 inválido: {device_path}"))?;
            let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR) };
            if fd < 0 {
                let err = std::io::Error::last_os_error();
                return Err(format!("Falha ao abrir {device_path}: {err}"));
            }

            let mut sink = Self {
                file: unsafe { File::from_raw_fd(fd) },
                device_path: device_path.to_string(),
            };
            sink.configure_format(width, height)?;
            Ok(sink)
        }

        pub fn configure_format(&mut self, width: u32, height: u32) -> Result<(), String> {
            use std::os::unix::io::AsRawFd;
            let bytesperline = width.saturating_mul(2);
            let sizeimage = bytesperline.saturating_mul(height);

            let mut fmt = V4l2Format {
                buf_type: V4L2_BUF_TYPE_VIDEO_OUTPUT,
                fmt: V4l2FormatUnion { raw_data: [0; 200] },
            };
            fmt.fmt.pix = V4l2PixFormat {
                width,
                height,
                pixelformat: VIRTUAL_CAM_FMT_YUYV,
                field: V4L2_FIELD_NONE,
                bytesperline,
                sizeimage,
                colorspace: V4L2_COLORSPACE_SMPTE170M,
                ..Default::default()
            };

            let ret = unsafe {
                libc::ioctl(
                    self.file.as_raw_fd(),
                    VIDIOC_S_FMT as libc::Ioctl,
                    &mut fmt,
                )
            };
            if ret != 0 {
                let err = std::io::Error::last_os_error();
                return Err(format!(
                    "VIDIOC_S_FMT ({width}x{height} YUYV) falhou em {}: {err}",
                    self.device_path
                ));
            }
            Ok(())
        }

        pub fn write_frame(&mut self, yuyv: &[u8]) -> Result<(), String> {
            self.file
                .write_all(yuyv)
                .map_err(|e| format!("Falha ao gravar quadro em {}: {e}", self.device_path))
        }
    }
}

// ------------------------------------------------------------------ Windows MF + Shared Memory backend

#[cfg(windows)]
mod win_vcam {
    use super::{
        SharedFrameHeader, VIRTUAL_CAM_FMT_NV12, VIRTUAL_CAM_FMT_YUYV, VIRTUAL_CAM_SHM_HEADER_BYTES,
        VIRTUAL_CAM_SHM_MAGIC, VIRTUAL_CAM_SHM_NAME, VIRTUAL_CAM_SHM_VERSION,
    };
    use crate::protocol::VirtualCameraDevice;
    use std::ffi::c_void;
    use std::sync::atomic::{fence, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::System::Memory::{
        CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
        MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
    };

    pub fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    /// Dynamically probes Windows 11 22H2+ Media Foundation `MFCreateVirtualCamera`.
    pub fn probe_mf_virtual_camera() -> bool {
        unsafe {
            let Ok(lib) = libloading::Library::new("mfsensorgroup.dll") else {
                return false;
            };
            lib.get::<*const c_void>(b"MFCreateVirtualCamera\0").is_ok()
        }
    }

    pub fn list_devices() -> Vec<VirtualCameraDevice> {
        let mf_available = probe_mf_virtual_camera();
        vec![
            VirtualCameraDevice {
                id: "mf:prismcast-vcam".to_string(),
                name: "PrismCast Virtual Camera (Windows Media Foundation)".to_string(),
                backend: "media-foundation".to_string(),
                available: mf_available,
            },
            VirtualCameraDevice {
                id: VIRTUAL_CAM_SHM_NAME.to_string(),
                name: "PrismCast Virtual Camera (DirectShow / Shared Memory)".to_string(),
                backend: "directshow-shm".to_string(),
                available: true,
            },
        ]
    }

    pub struct WinSharedMemSink {
        handle: HANDLE,
        view: MEMORY_MAPPED_VIEW_ADDRESS,
        capacity: usize,
        pub mf_available: bool,
        pub use_nv12: bool,
    }

    // SAFETY: `WinSharedMemSink` exclusively owns the Win32 section handle and view pointer;
    // mutation requires `&mut self`, synchronized externally (e.g. via `Mutex`).
    unsafe impl Send for WinSharedMemSink {}

    impl WinSharedMemSink {
        pub fn create(payload_capacity: usize) -> Result<Self, String> {
            let mf_available = probe_mf_virtual_camera();
            // Always reserve at least 1920x1080 YUYV (4,147,200 bytes) to avoid remapping on 16:9 <-> 9:16 switches.
            let min_payload = 1920usize * 1080usize * 2usize;
            let total_size =
                VIRTUAL_CAM_SHM_HEADER_BYTES.saturating_add(payload_capacity.max(min_payload));
            let size_u64 = total_size as u64;
            let size_high = (size_u64 >> 32) as u32;
            let size_low = (size_u64 & 0xFFFF_FFFF) as u32;

            let wide_name: Vec<u16> = VIRTUAL_CAM_SHM_NAME
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();

            let handle = unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    None,
                    PAGE_READWRITE,
                    size_high,
                    size_low,
                    PCWSTR(wide_name.as_ptr()),
                )
            }
            .map_err(|e| format!("CreateFileMappingW({VIRTUAL_CAM_SHM_NAME}) falhou: {e}"))?;

            let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, total_size) };
            if view.Value.is_null() {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                return Err(format!(
                    "MapViewOfFile({VIRTUAL_CAM_SHM_NAME}) retornou ponteiro nulo"
                ));
            }

            let init_header = SharedFrameHeader {
                magic: VIRTUAL_CAM_SHM_MAGIC,
                version: VIRTUAL_CAM_SHM_VERSION,
                width: 0,
                height: 0,
                format: if mf_available {
                    VIRTUAL_CAM_FMT_NV12
                } else {
                    VIRTUAL_CAM_FMT_YUYV
                },
                reserved: 0,
                seq: 0,
                timestamp_ms: 0,
            };
            unsafe {
                std::ptr::write_unaligned(view.Value as *mut SharedFrameHeader, init_header);
            }

            Ok(Self {
                handle,
                view,
                capacity: total_size,
                mf_available,
                use_nv12: mf_available,
            })
        }

        pub fn capacity(&self) -> usize {
            self.capacity
        }

        pub fn write_frame(
            &mut self,
            width: u32,
            height: u32,
            format: u32,
            seq: u64,
            timestamp_ms: u64,
            payload: &[u8],
        ) -> Result<(), String> {
            let needed = VIRTUAL_CAM_SHM_HEADER_BYTES.saturating_add(payload.len());
            if needed > self.capacity {
                return Err(format!(
                    "Quadro ({needed} bytes) excede capacidade do shared memory ({} bytes)",
                    self.capacity
                ));
            }
            let header = SharedFrameHeader {
                magic: VIRTUAL_CAM_SHM_MAGIC,
                version: VIRTUAL_CAM_SHM_VERSION,
                width,
                height,
                format,
                reserved: 0,
                seq,
                timestamp_ms,
            };
            unsafe {
                let base = self.view.Value as *mut u8;
                let dst_payload = base.add(VIRTUAL_CAM_SHM_HEADER_BYTES);
                std::ptr::copy_nonoverlapping(payload.as_ptr(), dst_payload, payload.len());
                // Publish pixel bytes before incrementing sequence number in header.
                fence(Ordering::Release);
                std::ptr::write_unaligned(base as *mut SharedFrameHeader, header);
            }
            Ok(())
        }
    }

    impl Drop for WinSharedMemSink {
        fn drop(&mut self) {
            unsafe {
                if !self.view.Value.is_null() {
                    let _ = UnmapViewOfFile(self.view);
                }
                if !self.handle.is_invalid() {
                    let _ = CloseHandle(self.handle);
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Cross-platform discovery

/// Discovers available virtual camera backends/devices on the current operating system.
/// Never panics, including in headless CI environments without video hardware.
pub fn list_virtual_camera_devices() -> Vec<VirtualCameraDevice> {
    #[cfg(target_os = "linux")]
    {
        linux_vcam::list_devices()
    }
    #[cfg(windows)]
    {
        win_vcam::list_devices()
    }
    #[cfg(all(not(target_os = "linux"), not(windows)))]
    {
        vec![VirtualCameraDevice {
            id: "software-sink".to_string(),
            name: "PrismCast Virtual Camera (Software Fallback)".to_string(),
            backend: "software".to_string(),
            available: true,
        }]
    }
}

// ------------------------------------------------------------------ VirtualCameraEngine

enum ActiveBackend {
    None,
    SoftwareSink,
    #[cfg(target_os = "linux")]
    LinuxV4l2(linux_vcam::LinuxV4l2Sink),
    #[cfg(windows)]
    WindowsShm(win_vcam::WinSharedMemSink),
}

/// Stateful virtual camera output engine managing device setup, reusable pixel
/// conversion buffers, and frame delivery.
pub struct VirtualCameraEngine {
    settings: VirtualCameraSettings,
    active: bool,
    backend_name: String,
    device_label: String,
    orientation: CanvasOrientation,
    width: u32,
    height: u32,
    fps: u32,
    frames_sent: u64,
    error: Option<String>,
    mf_virtual_cam_available: bool,
    scratch_buf: Vec<u8>,
    backend: ActiveBackend,
}

impl VirtualCameraEngine {
    /// Creates a new stopped [`VirtualCameraEngine`] initialized from normalized settings.
    pub fn new(mut settings: VirtualCameraSettings) -> Self {
        settings.normalize();
        let (width, height) = dimensions_for_orientation(settings.orientation);
        #[cfg(windows)]
        let mf_available = win_vcam::probe_mf_virtual_camera();
        #[cfg(not(windows))]
        let mf_available = false;

        Self {
            orientation: settings.orientation,
            fps: settings.fps,
            device_label: settings.device_name.clone(),
            settings,
            active: false,
            backend_name: "idle".to_string(),
            width,
            height,
            frames_sent: 0,
            error: None,
            mf_virtual_cam_available: mf_available,
            scratch_buf: Vec::new(),
            backend: ActiveBackend::None,
        }
    }

    /// Applies updated virtual camera settings. Takes effect immediately for FPS/device label
    /// and on next `start()` for active backend/orientation changes.
    pub fn apply_settings(&mut self, mut settings: VirtualCameraSettings) {
        settings.normalize();
        self.fps = settings.fps;
        if !self.active {
            self.orientation = settings.orientation;
            let (w, h) = dimensions_for_orientation(settings.orientation);
            self.width = w;
            self.height = h;
            self.device_label = settings.device_name.clone();
        }
        self.settings = settings;
    }

    /// Starts virtual camera output using the specified orientation (or configured default).
    pub fn start(&mut self, orientation: Option<CanvasOrientation>) -> VirtualCameraStatus {
        if self.active {
            self.stop();
        }

        let target_orient = orientation.unwrap_or(self.settings.orientation);
        let (width, height) = dimensions_for_orientation(target_orient);
        self.orientation = target_orient;
        self.width = width;
        self.height = height;
        self.fps = self.settings.fps;
        self.frames_sent = 0;
        self.error = None;

        #[cfg(target_os = "linux")]
        {
            self.start_linux(width, height);
        }

        #[cfg(windows)]
        {
            self.start_windows(width, height);
        }

        #[cfg(all(not(target_os = "linux"), not(windows)))]
        {
            self.active = true;
            self.backend = ActiveBackend::SoftwareSink;
            self.backend_name = "software-sink".to_string();
            self.device_label = self.settings.device_name.clone();
            self.error = None;
        }

        self.status()
    }

    #[cfg(target_os = "linux")]
    fn start_linux(&mut self, width: u32, height: u32) {
        let configured_path = self
            .settings
            .linux_device_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let candidate_path = configured_path.or_else(|| {
            linux_vcam::list_devices()
                .into_iter()
                .find(|d| d.backend == "v4l2loopback" && d.available)
                .map(|d| d.id)
        });

        if let Some(dev_path) = candidate_path {
            match linux_vcam::LinuxV4l2Sink::open(&dev_path, width, height) {
                Ok(sink) => {
                    self.active = true;
                    self.backend_name = "v4l2loopback".to_string();
                    self.device_label = dev_path;
                    self.error = None;
                    self.backend = ActiveBackend::LinuxV4l2(sink);
                    return;
                }
                Err(open_err) => {
                    log::warn!("[virtual_cam] {open_err} — entrando em modo software-sink");
                    self.active = true;
                    self.backend = ActiveBackend::SoftwareSink;
                    self.backend_name = "software-sink".to_string();
                    self.device_label = self.settings.device_name.clone();
                    self.error = Some(format!(
                        "{open_err}. Para ativar a câmera virtual no sistema, execute: {LINUX_V4L2LOOPBACK_HINT}"
                    ));
                    return;
                }
            }
        }

        // Kernel module not loaded: run in software virtual-sink mode with clear actionable hint.
        self.active = true;
        self.backend = ActiveBackend::SoftwareSink;
        self.backend_name = "software-sink".to_string();
        self.device_label = self.settings.device_name.clone();
        self.error = Some(format!(
            "Módulo v4l2loopback não detectado (modo software-sink ativo). Para expor a câmera virtual em apps (Discord, Zoom, Meet), execute: {LINUX_V4L2LOOPBACK_HINT}"
        ));
    }

    #[cfg(windows)]
    fn start_windows(&mut self, width: u32, height: u32) {
        let initial_bytes = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(2);
        match win_vcam::WinSharedMemSink::create(initial_bytes) {
            Ok(sink) => {
                self.mf_virtual_cam_available = sink.mf_available;
                self.active = true;
                self.backend_name = if sink.mf_available {
                    "media-foundation+shm".to_string()
                } else {
                    "directshow-shm".to_string()
                };
                self.device_label = VIRTUAL_CAM_SHM_NAME.to_string();
                self.error = None;
                self.backend = ActiveBackend::WindowsShm(sink);
            }
            Err(err) => {
                log::warn!("[virtual_cam] Windows shared memory falhou ({err}) — usando software-sink");
                self.active = true;
                self.backend = ActiveBackend::SoftwareSink;
                self.backend_name = "software-sink".to_string();
                self.device_label = self.settings.device_name.clone();
                self.error = Some(err);
            }
        }
    }

    /// Converts an incoming BGRA frame into YUYV/NV12 inside a reusable scratch buffer and
    /// writes it to the active virtual camera backend.
    pub fn push_frame_bgra(&mut self, width: u32, height: u32, bgra: &[u8]) -> Result<(), String> {
        if !self.active {
            return Err("Câmera virtual não está ativa.".to_string());
        }
        if width < 2 || height < 2 {
            return Err(format!("Dimensões de quadro inválidas: {width}x{height}"));
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| format!("Overflow nas dimensões do quadro: {width}x{height}"))?;
        if bgra.len() < expected {
            return Err(format!(
                "Buffer BGRA menor que o esperado: recebido {}, esperado {expected}",
                bgra.len()
            ));
        }

        let dims_changed = self.width != width || self.height != height;

        match &mut self.backend {
            ActiveBackend::None => {
                return Err("Backend da câmera virtual não inicializado.".to_string());
            }
            ActiveBackend::SoftwareSink => {
                bgra_to_yuyv(bgra, width, height, &mut self.scratch_buf);
                self.width = width;
                self.height = height;
            }
            #[cfg(target_os = "linux")]
            ActiveBackend::LinuxV4l2(sink) => {
                if dims_changed {
                    sink.configure_format(width, height)?;
                    self.width = width;
                    self.height = height;
                }
                bgra_to_yuyv(bgra, width, height, &mut self.scratch_buf);
                sink.write_frame(&self.scratch_buf)?;
            }
            #[cfg(windows)]
            ActiveBackend::WindowsShm(sink) => {
                let fmt = if sink.use_nv12 {
                    bgra_to_nv12(bgra, width, height, &mut self.scratch_buf);
                    VIRTUAL_CAM_FMT_NV12
                } else {
                    bgra_to_yuyv(bgra, width, height, &mut self.scratch_buf);
                    VIRTUAL_CAM_FMT_YUYV
                };

                let needed = VIRTUAL_CAM_SHM_HEADER_BYTES.saturating_add(self.scratch_buf.len());
                if needed > sink.capacity() {
                    let rebuilt = win_vcam::WinSharedMemSink::create(self.scratch_buf.len())?;
                    *sink = rebuilt;
                }

                let next_seq = self.frames_sent.saturating_add(1);
                sink.write_frame(
                    width,
                    height,
                    fmt,
                    next_seq,
                    win_vcam::now_ms(),
                    &self.scratch_buf,
                )?;
                if dims_changed {
                    self.width = width;
                    self.height = height;
                }
            }
        }

        // Prevent unused variable warning on platforms without a branch checking `dims_changed`.
        let _ = dims_changed;

        self.frames_sent = self.frames_sent.saturating_add(1);
        Ok(())
    }

    /// Stops virtual camera output and releases OS handles (V4L2 fd or Windows shared memory).
    pub fn stop(&mut self) -> VirtualCameraStatus {
        self.backend = ActiveBackend::None;
        self.active = false;
        self.backend_name = "idle".to_string();
        self.error = None;
        self.status()
    }

    /// Returns a snapshot of the current virtual camera status.
    pub fn status(&self) -> VirtualCameraStatus {
        VirtualCameraStatus {
            active: self.active,
            backend: self.backend_name.clone(),
            device: self.device_label.clone(),
            orientation: self.orientation,
            width: self.width,
            height: self.height,
            fps: self.fps,
            frames_sent: self.frames_sent,
            error: self.error.clone(),
        }
    }

    /// Returns `true` when the virtual camera is currently streaming frames.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Reports whether Windows 11 22H2+ Media Foundation `MFCreateVirtualCamera` was detected.
    pub fn mf_virtual_cam_available(&self) -> bool {
        self.mf_virtual_cam_available
    }

    /// Returns the most recently converted pixel buffer (YUYV or NV12) for diagnostics/tests.
    pub fn last_frame_bytes(&self) -> &[u8] {
        &self.scratch_buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_bgra(width: u32, height: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
        let px = (width * height) as usize;
        let mut buf = Vec::with_capacity(px * 4);
        for _ in 0..px {
            buf.extend_from_slice(&[b, g, r, 255]);
        }
        buf
    }

    #[test]
    fn test_virtual_camera_engine_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<VirtualCameraEngine>();
    }

    #[test]
    fn test_shared_frame_header_abi_and_roundtrip() {
        assert_eq!(VIRTUAL_CAM_SHM_HEADER_BYTES, 40);
        let hdr = SharedFrameHeader {
            magic: VIRTUAL_CAM_SHM_MAGIC,
            version: VIRTUAL_CAM_SHM_VERSION,
            width: 1920,
            height: 1080,
            format: VIRTUAL_CAM_FMT_YUYV,
            reserved: 0,
            seq: 42,
            timestamp_ms: 1_700_000_000_123,
        };
        let bytes = hdr.to_le_bytes();
        let decoded = SharedFrameHeader::from_le_bytes(&bytes);
        assert_eq!(decoded, hdr);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_linux_v4l2_struct_sizes_match_kernel_abi() {
        assert_eq!(std::mem::size_of::<linux_vcam::V4l2Capability>(), 104);
        assert_eq!(std::mem::size_of::<linux_vcam::V4l2PixFormat>(), 48);
        assert_eq!(linux_vcam::VIDIOC_QUERYCAP, 0x8068_5600);
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(std::mem::size_of::<linux_vcam::V4l2Format>(), 208);
            assert_eq!(linux_vcam::VIDIOC_S_FMT, 0xc0d0_5605);
        }
        #[cfg(target_pointer_width = "32")]
        {
            assert_eq!(std::mem::size_of::<linux_vcam::V4l2Format>(), 204);
            assert_eq!(linux_vcam::VIDIOC_S_FMT, 0xc0cc_5605);
        }
    }

    #[test]
    fn test_bgra_to_yuyv_primary_colors_and_buffer_reuse() {
        let (w, h) = (8u32, 4u32);
        let expected_len = (w * h * 2) as usize;
        let mut out = Vec::with_capacity(expected_len);
        let initial_ptr = out.as_ptr();
        let initial_cap = out.capacity();

        // White (255, 255, 255) -> Y=235, U=128, V=128
        let white = solid_bgra(w, h, 255, 255, 255);
        bgra_to_yuyv(&white, w, h, &mut out);
        assert_eq!(out.len(), expected_len);
        assert_eq!(out.as_ptr(), initial_ptr, "scratch buffer must not reallocate");
        assert_eq!(out.capacity(), initial_cap);
        for chunk in out.chunks_exact(4) {
            assert_eq!(chunk, &[235, 128, 235, 128]);
        }

        // Black (0, 0, 0) -> Y=16, U=128, V=128
        let black = solid_bgra(w, h, 0, 0, 0);
        bgra_to_yuyv(&black, w, h, &mut out);
        assert_eq!(out.len(), expected_len);
        for chunk in out.chunks_exact(4) {
            assert_eq!(chunk, &[16, 128, 16, 128]);
        }

        // Red (255, 0, 0) -> BT.601 Y≈81..82, U≈90, V=240
        let red = solid_bgra(w, h, 255, 0, 0);
        bgra_to_yuyv(&red, w, h, &mut out);
        assert_eq!(out.len(), expected_len);
        assert!((81..=82).contains(&out[0]), "red Y={}", out[0]);
        assert!((89..=91).contains(&out[1]), "red U={}", out[1]);
        assert_eq!(out[2], out[0]);
        assert!((239..=240).contains(&out[3]), "red V={}", out[3]);

        // Green (0, 255, 0) -> BT.601 Y≈144..145, U≈54, V≈34
        let green = solid_bgra(w, h, 0, 255, 0);
        bgra_to_yuyv(&green, w, h, &mut out);
        assert!((144..=145).contains(&out[0]), "green Y={}", out[0]);
        assert!((53..=55).contains(&out[1]), "green U={}", out[1]);
        assert!((33..=35).contains(&out[3]), "green V={}", out[3]);

        // Blue (0, 0, 255) -> BT.601 Y≈41, U=240, V≈110
        let blue = solid_bgra(w, h, 0, 0, 255);
        bgra_to_yuyv(&blue, w, h, &mut out);
        assert!((40..=42).contains(&out[0]), "blue Y={}", out[0]);
        assert!((239..=240).contains(&out[1]), "blue U={}", out[1]);
        assert!((109..=111).contains(&out[3]), "blue V={}", out[3]);
    }

    #[test]
    fn test_bgra_to_nv12_primary_colors_and_planes() {
        let (w, h) = (8u32, 4u32);
        let y_len = (w * h) as usize;
        let uv_len = y_len / 2;
        let expected_len = y_len + uv_len;
        let mut out = Vec::with_capacity(expected_len);
        let initial_ptr = out.as_ptr();

        // White -> Y plane all 235, UV plane all 128
        let white = solid_bgra(w, h, 255, 255, 255);
        bgra_to_nv12(&white, w, h, &mut out);
        assert_eq!(out.len(), expected_len);
        assert_eq!(out.as_ptr(), initial_ptr);
        assert!(out[..y_len].iter().all(|&v| v == 235));
        assert!(out[y_len..].iter().all(|&v| v == 128));

        // Black -> Y plane all 16, UV plane all 128
        let black = solid_bgra(w, h, 0, 0, 0);
        bgra_to_nv12(&black, w, h, &mut out);
        assert!(out[..y_len].iter().all(|&v| v == 16));
        assert!(out[y_len..].iter().all(|&v| v == 128));

        // Red -> Y≈82, U≈90, V=240
        let red = solid_bgra(w, h, 255, 0, 0);
        bgra_to_nv12(&red, w, h, &mut out);
        assert!(out[..y_len].iter().all(|&y| (81..=82).contains(&y)));
        for uv in out[y_len..].chunks_exact(2) {
            assert!((89..=91).contains(&uv[0]), "red NV12 U={}", uv[0]);
            assert!((239..=240).contains(&uv[1]), "red NV12 V={}", uv[1]);
        }

        // Green -> Y≈144, U≈54, V≈34
        let green = solid_bgra(w, h, 0, 255, 0);
        bgra_to_nv12(&green, w, h, &mut out);
        assert!(out[..y_len].iter().all(|&y| (144..=145).contains(&y)));
        for uv in out[y_len..].chunks_exact(2) {
            assert!((53..=55).contains(&uv[0]), "green NV12 U={}", uv[0]);
            assert!((33..=35).contains(&uv[1]), "green NV12 V={}", uv[1]);
        }

        // Blue -> Y≈41, U=240, V≈110
        let blue = solid_bgra(w, h, 0, 0, 255);
        bgra_to_nv12(&blue, w, h, &mut out);
        assert!(out[..y_len].iter().all(|&y| (40..=42).contains(&y)));
        for uv in out[y_len..].chunks_exact(2) {
            assert!((239..=240).contains(&uv[0]), "blue NV12 U={}", uv[0]);
            assert!((109..=111).contains(&uv[1]), "blue NV12 V={}", uv[1]);
        }
    }

    #[test]
    fn test_engine_lifecycle_orientation_switching_and_device_discovery() {
        // Device discovery must never panic even in headless CI containers.
        let devices = list_virtual_camera_devices();
        assert!(!devices.is_empty());

        #[allow(unused_mut)]
        let mut cfg = VirtualCameraSettings::default();
        #[cfg(target_os = "linux")]
        {
            // Avoid touching a live developer /dev/videoN loopback device during unit tests.
            cfg.linux_device_path = Some("/dev/prismcast-nonexistent-test-vcam".to_string());
        }
        let mut engine = VirtualCameraEngine::new(cfg);
        assert!(!engine.is_active());
        assert_eq!(engine.status().width, 1920);
        assert_eq!(engine.status().height, 1080);

        // Pushing a frame while stopped returns an error cleanly.
        let small = solid_bgra(4, 4, 255, 255, 255);
        assert!(engine.push_frame_bgra(4, 4, &small).is_err());

        // Start in 16:9 Horizontal mode
        let st_h = engine.start(Some(CanvasOrientation::Horizontal));
        assert!(st_h.active);
        assert!(engine.is_active());
        assert_eq!(st_h.orientation, CanvasOrientation::Horizontal);
        assert_eq!((st_h.width, st_h.height), (1920, 1080));

        // On Linux without v4l2loopback loaded, software-sink error hint includes modprobe command.
        #[cfg(target_os = "linux")]
        if st_h.backend == "software-sink" {
            let err = st_h.error.as_deref().unwrap_or("");
            assert!(
                err.contains("modprobe v4l2loopback"),
                "expected actionable modprobe hint, got: {err}"
            );
        }

        // Push two frames and verify counter increments
        engine.push_frame_bgra(4, 4, &small).unwrap();
        engine.push_frame_bgra(4, 4, &small).unwrap();
        assert_eq!(engine.status().frames_sent, 2);
        assert!(!engine.last_frame_bytes().is_empty());

        // Switch to 9:16 Vertical orientation
        let st_v = engine.start(Some(CanvasOrientation::Vertical));
        assert!(st_v.active);
        assert_eq!(st_v.orientation, CanvasOrientation::Vertical);
        assert_eq!((st_v.width, st_v.height), (1080, 1920));
        assert_eq!(st_v.frames_sent, 0);

        // Apply settings while stopped
        let st_stopped = engine.stop();
        assert!(!st_stopped.active);
        assert!(!engine.is_active());

        let mut updated = VirtualCameraSettings::default();
        updated.fps = 60;
        updated.orientation = CanvasOrientation::Vertical;
        engine.apply_settings(updated);
        let st_after = engine.status();
        assert_eq!(st_after.fps, 60);
        assert_eq!((st_after.width, st_after.height), (1080, 1920));
    }
}
