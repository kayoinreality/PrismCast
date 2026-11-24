//! Zero-Copy GPU Capture Pipeline (`GpuCaptureEngine`).
//!
//! Abstrai captura nativa em GPU sem readback desnecessário para CPU:
//! - **Windows (`#[cfg(windows)]`)**: DXGI Desktop Duplication (`IDXGIOutput1::DuplicateOutput`)
//!   com textura compartilhada `D3D11_RESOURCE_MISC_SHARED` e `CopyResource` estritamente
//!   GPU-para-GPU, produzindo `GpuFrameHandle::SharedDxgiHandle` (0 cópias CPU), mais helper
//!   opcional de readback staging para Virtual Cam / CPU fallback.
//! - **Linux (`#[cfg(target_os = "linux")]`)**: PipeWire + `xdg-desktop-portal` + DMA-BUF
//!   (`GpuFrameHandle::LinuxDmaBuf`) com sondagem não-bloqueante de `$WAYLAND_DISPLAY`,
//!   PipeWire runtime e render nodes `/dev/dri/renderD128+`, fazendo fallback limpo para
//!   `SoftwareFallback` em ambientes headless (CI).

use crate::capture::FrameBuffer;
use crate::protocol::{GpuCaptureBackendKind, GpuPipelineSettings, GpuPipelineStatus};
use std::collections::{HashMap, HashSet};

/// FourCC DRM padrão para ARGB8888 (equivalente a BGRA8 little-endian).
pub const DRM_FORMAT_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
/// FourCC DRM padrão para XRGB8888 (equivalente a BGRX8 little-endian).
pub const DRM_FORMAT_XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
/// Código `DXGI_FORMAT_B8G8R8A8_UNORM` (87) para metadados cross-platform.
pub const DXGI_FORMAT_BGRA8_UNORM: u32 = 87;

/// Handle de frame capturado pelo pipeline GPU zero-copy (ou fallback CPU).
#[derive(Debug, Clone)]
pub enum GpuFrameHandle {
    /// Windows D3D11/DXGI shared texture handle — zero CPU readback.
    SharedDxgiHandle {
        handle: usize,
        width: u32,
        height: u32,
        format: u32,
        seq: u64,
        updated_at_ms: u64,
    },
    /// Linux PipeWire / xdg-desktop-portal DMA-BUF descriptor — zero CPU readback.
    LinuxDmaBuf {
        fd: i32,
        width: u32,
        height: u32,
        stride: u32,
        offset: u32,
        drm_format: u32,
        modifier: u64,
        seq: u64,
        updated_at_ms: u64,
    },
    /// Textura já residente no dispositivo GPU do compositor.
    GpuTextureReady {
        width: u32,
        height: u32,
        seq: u64,
        updated_at_ms: u64,
    },
    /// Fallback CPU BGRA (requer 1 cópia para upload GPU).
    CpuFallback(FrameBuffer),
}

impl GpuFrameHandle {
    /// Retorna `true` se o frame trafega sem readback CPU (zero-copy).
    pub fn is_zero_copy(&self) -> bool {
        !matches!(self, GpuFrameHandle::CpuFallback(_))
    }

    /// Número de cópias CPU necessárias para este handle (`0` para DXGI/DMA-BUF/GPU, `1` para CPU fallback).
    pub fn frame_copy_count(&self) -> u32 {
        match self {
            GpuFrameHandle::SharedDxgiHandle { .. }
            | GpuFrameHandle::LinuxDmaBuf { .. }
            | GpuFrameHandle::GpuTextureReady { .. } => 0,
            GpuFrameHandle::CpuFallback(_) => 1,
        }
    }

    /// Dimensões `(width, height)` do frame.
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            GpuFrameHandle::SharedDxgiHandle { width, height, .. }
            | GpuFrameHandle::LinuxDmaBuf { width, height, .. }
            | GpuFrameHandle::GpuTextureReady { width, height, .. } => (*width, *height),
            GpuFrameHandle::CpuFallback(fb) => (fb.width, fb.height),
        }
    }

    /// Número de sequência monotônico do frame.
    pub fn seq(&self) -> u64 {
        match self {
            GpuFrameHandle::SharedDxgiHandle { seq, .. }
            | GpuFrameHandle::LinuxDmaBuf { seq, .. }
            | GpuFrameHandle::GpuTextureReady { seq, .. } => *seq,
            GpuFrameHandle::CpuFallback(fb) => fb.seq,
        }
    }

    /// Timestamp em milissegundos da última atualização do frame.
    pub fn updated_at_ms(&self) -> u64 {
        match self {
            GpuFrameHandle::SharedDxgiHandle { updated_at_ms, .. }
            | GpuFrameHandle::LinuxDmaBuf { updated_at_ms, .. }
            | GpuFrameHandle::GpuTextureReady { updated_at_ms, .. } => *updated_at_ms,
            GpuFrameHandle::CpuFallback(fb) => fb.updated_at_ms,
        }
    }
}

/// Converte `GpuCaptureBackendKind` para o identificador kebab-case do protocolo.
pub fn backend_kind_str(kind: GpuCaptureBackendKind) -> &'static str {
    match kind {
        GpuCaptureBackendKind::Auto => "auto",
        GpuCaptureBackendKind::DxgiDuplication => "dxgi-duplication",
        GpuCaptureBackendKind::WindowsGraphicsCapture => "windows-graphics-capture",
        GpuCaptureBackendKind::PipewireDmabuf => "pipewire-dmabuf",
        GpuCaptureBackendKind::SoftwareFallback => "software-fallback",
    }
}

// =========================================================================
// Windows: DXGI Desktop Duplication Zero-Copy Backend
// =========================================================================

#[cfg(windows)]
pub mod dxgi {
    use super::*;
    use windows::core::{IUnknown, Interface};
    use windows::Win32::Foundation::{HANDLE, HMODULE};
    use windows::Win32::Graphics::Direct3D::{
        D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_11_1,
    };
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
        D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
        D3D11_RESOURCE_MISC_SHARED, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
        D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
    };
    use windows::Win32::Graphics::Dxgi::{
        IDXGIAdapter, IDXGIDevice, IDXGIOutput, IDXGIOutput1, IDXGIOutputDuplication,
        IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    };

    /// Guard RAII garantindo `ReleaseFrame()` mesmo em caminhos de erro antecipado.
    struct FrameReleaseGuard<'a> {
        duplication: &'a IDXGIOutputDuplication,
    }

    impl Drop for FrameReleaseGuard<'_> {
        fn drop(&mut self) {
            unsafe {
                let _ = self.duplication.ReleaseFrame();
            }
        }
    }

    /// Capturador nativo DXGI Desktop Duplication (GPU-to-GPU via `D3D11_RESOURCE_MISC_SHARED`).
    pub struct DxgiDuplicator {
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        duplication: IDXGIOutputDuplication,
        shared_texture: Option<ID3D11Texture2D>,
        shared_handle: usize,
        shared_width: u32,
        shared_height: u32,
        shared_format: u32,
        staging_texture: Option<ID3D11Texture2D>,
        staging_width: u32,
        staging_height: u32,
        output_index: u32,
        adapter_name: String,
        seq: u64,
    }

    // SAFETY: `ID3D11Device` é criado sem `D3D11_CREATE_DEVICE_SINGLETHREADED` (thread-safe),
    // e todo acesso a `ID3D11DeviceContext` / `IDXGIOutputDuplication` exige `&mut self` exclusivo.
    unsafe impl Send for DxgiDuplicator {}

    impl DxgiDuplicator {
        /// Inicializa dispositivo D3D11 de hardware e duplica o monitor `output_index`.
        pub fn new(output_index: u32) -> Result<Self, String> {
            let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
            let mut device_opt: Option<ID3D11Device> = None;
            let mut context_opt: Option<ID3D11DeviceContext> = None;
            let mut chosen_level = D3D_FEATURE_LEVEL::default();

            unsafe {
                D3D11CreateDevice(
                    None::<&IDXGIAdapter>,
                    D3D_DRIVER_TYPE_HARDWARE,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&feature_levels),
                    D3D11_SDK_VERSION,
                    Some(&mut device_opt),
                    Some(&mut chosen_level),
                    Some(&mut context_opt),
                )
            }
            .map_err(|e| format!("D3D11CreateDevice falhou: {e}"))?;

            let device = device_opt
                .ok_or_else(|| "D3D11CreateDevice retornou device nulo".to_string())?;
            let context = context_opt
                .ok_or_else(|| "D3D11CreateDevice retornou immediate context nulo".to_string())?;

            let dxgi_device: IDXGIDevice = device
                .cast()
                .map_err(|e| format!("Cast para IDXGIDevice falhou: {e}"))?;
            let adapter: IDXGIAdapter = unsafe { dxgi_device.GetAdapter() }
                .map_err(|e| format!("IDXGIDevice::GetAdapter falhou: {e}"))?;

            let adapter_name = unsafe { adapter.GetDesc() }
                .map(|desc| {
                    let end = desc
                        .Description
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(desc.Description.len());
                    String::from_utf16_lossy(&desc.Description[..end])
                        .trim()
                        .to_string()
                })
                .unwrap_or_else(|_| "Direct3D11 Adapter".to_string());

            let output: IDXGIOutput = unsafe { adapter.EnumOutputs(output_index) }
                .map_err(|e| format!("IDXGIAdapter::EnumOutputs({output_index}) falhou: {e}"))?;
            let output1: IDXGIOutput1 = output
                .cast()
                .map_err(|e| format!("Cast para IDXGIOutput1 falhou: {e}"))?;

            let device_unk: IUnknown = device
                .cast()
                .map_err(|e| format!("Cast para IUnknown falhou: {e}"))?;
            let duplication: IDXGIOutputDuplication =
                unsafe { output1.DuplicateOutput(&device_unk) }
                    .map_err(|e| format!("IDXGIOutput1::DuplicateOutput falhou: {e}"))?;

            Ok(Self {
                device,
                context,
                duplication,
                shared_texture: None,
                shared_handle: 0,
                shared_width: 0,
                shared_height: 0,
                shared_format: DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
                staging_texture: None,
                staging_width: 0,
                staging_height: 0,
                output_index,
                adapter_name,
                seq: 0,
            })
        }

        pub fn adapter_name(&self) -> &str {
            &self.adapter_name
        }

        pub fn output_index(&self) -> u32 {
            self.output_index
        }

        /// Garante que a textura `D3D11_RESOURCE_MISC_SHARED` exista com as dimensões e formato atuais.
        fn ensure_shared_texture(
            &mut self,
            width: u32,
            height: u32,
            format: DXGI_FORMAT,
        ) -> Result<(), String> {
            let fmt_raw = format.0 as u32;
            if self.shared_texture.is_some()
                && self.shared_width == width
                && self.shared_height == height
                && self.shared_format == fmt_raw
            {
                return Ok(());
            }

            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 as u32)
                    | (D3D11_BIND_RENDER_TARGET.0 as u32),
                CPUAccessFlags: 0,
                MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
            };

            let mut tex_opt: Option<ID3D11Texture2D> = None;
            unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex_opt)) }
                .map_err(|e| format!("CreateTexture2D(SHARED) falhou: {e}"))?;
            let tex = tex_opt
                .ok_or_else(|| "CreateTexture2D(SHARED) retornou textura nula".to_string())?;

            let shared_res: IDXGIResource = tex
                .cast()
                .map_err(|e| format!("Cast para IDXGIResource falhou: {e}"))?;
            let handle: HANDLE = unsafe { shared_res.GetSharedHandle() }
                .map_err(|e| format!("IDXGIResource::GetSharedHandle falhou: {e}"))?;

            self.shared_texture = Some(tex);
            self.shared_handle = handle.0 as usize;
            self.shared_width = width;
            self.shared_height = height;
            self.shared_format = fmt_raw;
            Ok(())
        }

        /// Adquire o próximo frame da área de trabalho copiando exclusivamente GPU-para-GPU
        /// (`CopyResource`) para a textura `D3D11_RESOURCE_MISC_SHARED` (zero CPU readback).
        ///
        /// Retorna `Ok(None)` em timeout (`DXGI_ERROR_WAIT_TIMEOUT`) quando não houve alteração na tela.
        pub fn acquire_shared_frame(
            &mut self,
            timeout_ms: u32,
            now_ms: u64,
        ) -> Result<Option<GpuFrameHandle>, String> {
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut desktop_res_opt: Option<IDXGIResource> = None;

            let acquire_res = unsafe {
                self.duplication.AcquireNextFrame(
                    timeout_ms,
                    &mut frame_info,
                    &mut desktop_res_opt,
                )
            };

            if let Err(err) = acquire_res {
                if err.code() == DXGI_ERROR_WAIT_TIMEOUT {
                    return Ok(None);
                }
                if err.code() == DXGI_ERROR_ACCESS_LOST {
                    return Err(
                        "DXGI_ERROR_ACCESS_LOST: duplicação invalidada (troca de modo/UAC)"
                            .to_string(),
                    );
                }
                return Err(format!("AcquireNextFrame falhou: {err}"));
            }

            let _guard = FrameReleaseGuard {
                duplication: &self.duplication,
            };

            let desktop_res = desktop_res_opt
                .ok_or_else(|| "AcquireNextFrame retornou recurso desktop nulo".to_string())?;
            let desktop_tex: ID3D11Texture2D = desktop_res
                .cast()
                .map_err(|e| format!("Cast de desktop resource para ID3D11Texture2D falhou: {e}"))?;

            let mut src_desc = D3D11_TEXTURE2D_DESC::default();
            unsafe { desktop_tex.GetDesc(&mut src_desc) };

            if src_desc.Width == 0 || src_desc.Height == 0 {
                return Ok(None);
            }

            self.ensure_shared_texture(src_desc.Width, src_desc.Height, src_desc.Format)?;

            let dst_tex = self
                .shared_texture
                .as_ref()
                .ok_or_else(|| "Textura compartilhada indisponível".to_string())?;
            let dst_res: ID3D11Resource = dst_tex
                .cast()
                .map_err(|e| format!("Cast dst ID3D11Resource falhou: {e}"))?;
            let src_res: ID3D11Resource = desktop_tex
                .cast()
                .map_err(|e| format!("Cast src ID3D11Resource falhou: {e}"))?;

            // Cópia estritamente GPU-para-GPU — zero tráfego pelo barramento PCIe para CPU.
            unsafe {
                self.context.CopyResource(&dst_res, &src_res);
            }

            self.seq = self.seq.saturating_add(1);
            Ok(Some(GpuFrameHandle::SharedDxgiHandle {
                handle: self.shared_handle,
                width: self.shared_width,
                height: self.shared_height,
                format: self.shared_format,
                seq: self.seq,
                updated_at_ms: now_ms,
            }))
        }

        /// Helper opcional de readback via textura staging (`D3D11_USAGE_STAGING`) para consumidores
        /// CPU (ex.: Virtual Camera ou fallback software).
        pub fn readback_shared_to_cpu(&mut self, now_ms: u64) -> Result<FrameBuffer, String> {
            let shared_tex = self
                .shared_texture
                .as_ref()
                .ok_or_else(|| "Nenhum frame DXGI capturado ainda para readback".to_string())?
                .clone();
            let width = self.shared_width;
            let height = self.shared_height;
            if width == 0 || height == 0 {
                return Err("Dimensões inválidas na textura compartilhada DXGI".to_string());
            }

            if self.staging_texture.is_none()
                || self.staging_width != width
                || self.staging_height != height
            {
                let staging_desc = D3D11_TEXTURE2D_DESC {
                    Width: width,
                    Height: height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT(self.shared_format as i32),
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut st_opt: Option<ID3D11Texture2D> = None;
                unsafe {
                    self.device
                        .CreateTexture2D(&staging_desc, None, Some(&mut st_opt))
                }
                .map_err(|e| format!("CreateTexture2D(STAGING) falhou: {e}"))?;
                self.staging_texture = Some(
                    st_opt.ok_or_else(|| "CreateTexture2D(STAGING) retornou nulo".to_string())?,
                );
                self.staging_width = width;
                self.staging_height = height;
            }

            let staging_tex = self.staging_texture.as_ref().unwrap();
            let staging_res: ID3D11Resource = staging_tex
                .cast()
                .map_err(|e| format!("Cast staging ID3D11Resource falhou: {e}"))?;
            let shared_res: ID3D11Resource = shared_tex
                .cast()
                .map_err(|e| format!("Cast shared ID3D11Resource falhou: {e}"))?;

            unsafe {
                self.context.CopyResource(&staging_res, &shared_res);
            }

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            unsafe {
                self.context
                    .Map(&staging_res, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            }
            .map_err(|e| format!("ID3D11DeviceContext::Map(READ) falhou: {e}"))?;

            let row_bytes = (width as usize) * 4;
            let pitch = mapped.RowPitch as usize;
            let mut bgra = vec![0u8; row_bytes.saturating_mul(height as usize)];

            if !mapped.pData.is_null() && pitch >= row_bytes {
                let src_base = mapped.pData as *const u8;
                for y in 0..(height as usize) {
                    unsafe {
                        let src_row = std::slice::from_raw_parts(src_base.add(y * pitch), row_bytes);
                        let dst_off = y * row_bytes;
                        bgra[dst_off..dst_off + row_bytes].copy_from_slice(src_row);
                    }
                }
            }

            unsafe {
                self.context.Unmap(&staging_res, 0);
            }

            let mut fb = FrameBuffer::from_bgra(width, height, bgra, now_ms)?;
            fb.seq = self.seq;
            Ok(fb)
        }
    }
}

#[cfg(windows)]
pub use dxgi::DxgiDuplicator;

// =========================================================================
// Linux: PipeWire / xdg-desktop-portal / DMA-BUF Backend
// =========================================================================

/// Resultado da sondagem de capacidades de captura zero-copy no Linux.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Default)]
pub struct LinuxCaptureProbeInfo {
    pub wayland_session: bool,
    pub pipewire_available: bool,
    pub portal_available: bool,
    pub pw_cli_found: bool,
    pub drm_render_node: Option<String>,
}

/// Capturador Linux PipeWire / `xdg-desktop-portal` com ingestão de descritores DMA-BUF zero-copy.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
pub struct PipeWireDmaBufCapturer {
    probe: LinuxCaptureProbeInfo,
    latest: Option<GpuFrameHandle>,
    seq: u64,
}

#[cfg(target_os = "linux")]
impl PipeWireDmaBufCapturer {
    /// Sonda disponibilidade do runtime PipeWire, `xdg-desktop-portal`, sessão Wayland e DRM render node
    /// sem bloquear (seguro em CI headless).
    pub fn probe() -> Self {
        let wayland_display = std::env::var("WAYLAND_DISPLAY")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
        let wayland_session =
            wayland_display.is_some() || session_type.eq_ignore_ascii_case("wayland");

        // Procura render node DRM (/dev/dri/renderD128 .. renderD135)
        let mut drm_render_node = None;
        for idx in 128..=135u32 {
            let candidate = format!("/dev/dri/renderD{idx}");
            if std::path::Path::new(&candidate).exists() {
                drm_render_node = Some(candidate);
                break;
            }
        }

        // Procura socket PipeWire em $PIPEWIRE_RUNTIME_DIR ou $XDG_RUNTIME_DIR
        let runtime_dir = std::env::var("PIPEWIRE_RUNTIME_DIR")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                std::env::var("XDG_RUNTIME_DIR")
                    .ok()
                    .filter(|s| !s.trim().is_empty())
            });
        let pipewire_socket_exists = runtime_dir
            .as_deref()
            .map(|dir| std::path::Path::new(dir).join("pipewire-0").exists())
            .unwrap_or(false);

        // Verifica se `pw-cli` existe no $PATH
        let pw_cli_found = find_in_path("pw-cli");

        // Verifica presença do barramento de sessão D-Bus + daemon xdg-desktop-portal
        let has_dbus = std::env::var("DBUS_SESSION_BUS_ADDRESS")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .is_some()
            || runtime_dir
                .as_deref()
                .map(|dir| std::path::Path::new(dir).join("bus").exists())
                .unwrap_or(false);
        let portal_binary_exists = [
            "/usr/libexec/xdg-desktop-portal",
            "/usr/lib/xdg-desktop-portal",
            "/usr/lib/xdg-desktop-portal/xdg-desktop-portal",
        ]
        .iter()
        .any(|p| std::path::Path::new(p).exists());
        let portal_available = has_dbus && portal_binary_exists;

        let pipewire_available = pipewire_socket_exists && (pw_cli_found || portal_available);

        Self {
            probe: LinuxCaptureProbeInfo {
                wayland_session,
                pipewire_available,
                portal_available,
                pw_cli_found,
                drm_render_node,
            },
            latest: None,
            seq: 0,
        }
    }

    /// Construtor determinístico para testes unitários sem depender do host.
    pub fn with_probe_info(probe: LinuxCaptureProbeInfo) -> Self {
        Self {
            probe,
            latest: None,
            seq: 0,
        }
    }

    /// Retorna `true` se o ambiente Linux possui PipeWire + render node DRM para DMA-BUF zero-copy.
    pub fn is_zero_copy_capable(&self) -> bool {
        self.probe.pipewire_available && self.probe.drm_render_node.is_some()
    }

    pub fn diagnostics(&self) -> &LinuxCaptureProbeInfo {
        &self.probe
    }

    /// Ingere um descritor DMA-BUF recebido do fluxo PipeWire ScreenCast (sem copiar pixels para CPU).
    #[allow(clippy::too_many_arguments)]
    pub fn ingest_dmabuf(
        &mut self,
        fd: i32,
        width: u32,
        height: u32,
        stride: u32,
        offset: u32,
        drm_format: u32,
        modifier: u64,
        now_ms: u64,
    ) -> Result<GpuFrameHandle, String> {
        if fd < 0 {
            return Err(format!("Descritor DMA-BUF inválido (fd={fd})"));
        }
        if width == 0 || height == 0 {
            return Err("Dimensões DMA-BUF inválidas (0)".to_string());
        }
        let min_stride = width.saturating_mul(4);
        if stride < min_stride {
            return Err(format!(
                "Stride DMA-BUF insuficiente: {stride} < mínimo {min_stride}"
            ));
        }
        self.seq = self.seq.saturating_add(1);
        let handle = GpuFrameHandle::LinuxDmaBuf {
            fd,
            width,
            height,
            stride,
            offset,
            drm_format,
            modifier,
            seq: self.seq,
            updated_at_ms: now_ms,
        };
        self.latest = Some(handle.clone());
        Ok(handle)
    }

    pub fn latest_frame(&self) -> Option<&GpuFrameHandle> {
        self.latest.as_ref()
    }
}

#[cfg(target_os = "linux")]
fn find_in_path(bin: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| dir.join(bin).is_file())
}

// =========================================================================
// Cross-Platform GpuCaptureEngine
// =========================================================================

/// Motor unificado cross-platform de captura GPU zero-copy com gerenciamento de handles
/// por chave de fonte e seleção automática de backend (DXGI / PipeWire DMA-BUF / SoftwareFallback).
pub struct GpuCaptureEngine {
    settings: GpuPipelineSettings,
    active_backend: GpuCaptureBackendKind,
    frames: HashMap<String, GpuFrameHandle>,
    primary_key: String,
    last_error: Option<String>,
    #[cfg(windows)]
    dxgi: Option<DxgiDuplicator>,
    #[cfg(target_os = "linux")]
    pipewire: PipeWireDmaBufCapturer,
}

impl GpuCaptureEngine {
    /// Cria o motor resolvendo automaticamente o melhor backend disponível no OS atual.
    pub fn new(settings: GpuPipelineSettings) -> Self {
        let mut engine = Self {
            settings: settings.clone(),
            active_backend: GpuCaptureBackendKind::SoftwareFallback,
            frames: HashMap::new(),
            primary_key: "display:primary:".to_string(),
            last_error: None,
            #[cfg(windows)]
            dxgi: None,
            #[cfg(target_os = "linux")]
            pipewire: PipeWireDmaBufCapturer::probe(),
        };
        engine.apply_settings(settings);
        engine
    }

    /// Atualiza as configurações do pipeline GPU e re-seleciona o backend de captura ativo.
    pub fn apply_settings(&mut self, settings: GpuPipelineSettings) {
        self.settings = settings;
        self.last_error = None;

        if !self.settings.zero_copy_enabled
            || self.settings.capture_backend == GpuCaptureBackendKind::SoftwareFallback
        {
            #[cfg(windows)]
            {
                self.dxgi = None;
            }
            self.active_backend = GpuCaptureBackendKind::SoftwareFallback;
            return;
        }

        #[cfg(windows)]
        {
            match self.settings.capture_backend {
                GpuCaptureBackendKind::Auto
                | GpuCaptureBackendKind::DxgiDuplication
                | GpuCaptureBackendKind::WindowsGraphicsCapture => {
                    if self.dxgi.is_none() {
                        match DxgiDuplicator::new(0) {
                            Ok(dup) => {
                                self.dxgi = Some(dup);
                                self.active_backend = if self.settings.capture_backend
                                    == GpuCaptureBackendKind::WindowsGraphicsCapture
                                {
                                    GpuCaptureBackendKind::WindowsGraphicsCapture
                                } else {
                                    GpuCaptureBackendKind::DxgiDuplication
                                };
                            }
                            Err(err) => {
                                self.last_error = Some(err);
                                self.active_backend = GpuCaptureBackendKind::SoftwareFallback;
                            }
                        }
                    } else {
                        self.active_backend = if self.settings.capture_backend
                            == GpuCaptureBackendKind::WindowsGraphicsCapture
                        {
                            GpuCaptureBackendKind::WindowsGraphicsCapture
                        } else {
                            GpuCaptureBackendKind::DxgiDuplication
                        };
                    }
                }
                GpuCaptureBackendKind::PipewireDmabuf => {
                    self.dxgi = None;
                    self.last_error = Some(
                        "PipeWire DMA-BUF não está disponível no Windows; usando fallback."
                            .to_string(),
                    );
                    self.active_backend = GpuCaptureBackendKind::SoftwareFallback;
                }
                GpuCaptureBackendKind::SoftwareFallback => {
                    self.dxgi = None;
                    self.active_backend = GpuCaptureBackendKind::SoftwareFallback;
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            self.pipewire = PipeWireDmaBufCapturer::probe();
            match self.settings.capture_backend {
                GpuCaptureBackendKind::Auto | GpuCaptureBackendKind::PipewireDmabuf => {
                    if self.pipewire.is_zero_copy_capable() {
                        self.active_backend = GpuCaptureBackendKind::PipewireDmabuf;
                    } else {
                        self.last_error = Some(
                            "PipeWire/DRM render node indisponível (headless ou X11 sem portal); usando SoftwareFallback."
                                .to_string(),
                        );
                        self.active_backend = GpuCaptureBackendKind::SoftwareFallback;
                    }
                }
                GpuCaptureBackendKind::DxgiDuplication
                | GpuCaptureBackendKind::WindowsGraphicsCapture => {
                    self.last_error = Some(
                        "Backends DXGI/WGC são exclusivos do Windows; usando fallback.".to_string(),
                    );
                    self.active_backend = GpuCaptureBackendKind::SoftwareFallback;
                }
                GpuCaptureBackendKind::SoftwareFallback => {
                    self.active_backend = GpuCaptureBackendKind::SoftwareFallback;
                }
            }
        }

        #[cfg(not(any(windows, target_os = "linux")))]
        {
            self.active_backend = GpuCaptureBackendKind::SoftwareFallback;
        }
    }

    pub fn settings(&self) -> &GpuPipelineSettings {
        &self.settings
    }

    pub fn active_backend(&self) -> GpuCaptureBackendKind {
        self.active_backend
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    #[cfg(target_os = "linux")]
    pub fn linux_probe_info(&self) -> &LinuxCaptureProbeInfo {
        self.pipewire.diagnostics()
    }

    /// Calcula o status consolidado do pipeline GPU (combinando backend de captura + compositor).
    pub fn status(
        &self,
        compositor_backend: &str,
        adapter_name: &str,
        driver_info: &str,
    ) -> GpuPipelineStatus {
        let frame_copy_count = self.compute_frame_copy_count();
        let has_zero_copy_frames = self.frames.values().any(GpuFrameHandle::is_zero_copy);
        let capture_is_zero_copy =
            self.active_backend != GpuCaptureBackendKind::SoftwareFallback || has_zero_copy_frames;
        let compositor_is_gpu = compositor_backend.eq_ignore_ascii_case("wgpu");

        let zero_copy_active = self.settings.zero_copy_enabled
            && frame_copy_count == 0
            && capture_is_zero_copy
            && compositor_is_gpu;

        let resolved_adapter = if !adapter_name.trim().is_empty() {
            adapter_name.to_string()
        } else {
            #[cfg(windows)]
            {
                self.dxgi
                    .as_ref()
                    .map(|d| d.adapter_name().to_string())
                    .unwrap_or_else(|| "CPU Fallback".to_string())
            }
            #[cfg(not(windows))]
            {
                "CPU Fallback".to_string()
            }
        };

        let resolved_driver = if !driver_info.trim().is_empty() {
            driver_info.to_string()
        } else if let Some(ref err) = self.last_error {
            err.clone()
        } else {
            "default".to_string()
        };

        GpuPipelineStatus {
            zero_copy_active,
            compositor_backend: compositor_backend.to_string(),
            capture_backend: backend_kind_str(self.active_backend).to_string(),
            adapter_name: resolved_adapter,
            driver_info: resolved_driver,
            frame_copy_count,
            active_textures: self.frames.len(),
        }
    }

    /// Calcula `frame_copy_count`:
    /// - `0` quando `zero_copy_enabled` e os handles ativos são DXGI/DMA-BUF/GPU (ou backend nativo ativo sem fallback).
    /// - `1` quando em modo `CpuFallback` ou `zero_copy_enabled == false`.
    pub fn compute_frame_copy_count(&self) -> u32 {
        if !self.settings.zero_copy_enabled {
            return 1;
        }
        if !self.frames.is_empty() {
            return self
                .frames
                .values()
                .map(GpuFrameHandle::frame_copy_count)
                .max()
                .unwrap_or(0);
        }
        if self.active_backend == GpuCaptureBackendKind::SoftwareFallback {
            1
        } else {
            0
        }
    }

    /// Registra ou atualiza um handle GPU/fallback associado à chave de fonte `key`.
    pub fn push_gpu_frame(&mut self, key: String, handle: GpuFrameHandle) {
        self.frames.insert(key, handle);
    }

    /// Consulta o handle GPU mais recente associado a `key`.
    pub fn get_gpu_frame(&self, key: &str) -> Option<&GpuFrameHandle> {
        self.frames.get(key)
    }

    /// Remove um handle específico.
    pub fn remove_gpu_frame(&mut self, key: &str) -> Option<GpuFrameHandle> {
        self.frames.remove(key)
    }

    /// Remove handles de fontes que não pertencem mais às cenas ativas.
    pub fn prune(&mut self, active_keys: &HashSet<String>) -> usize {
        let before = self.frames.len();
        self.frames.retain(|k, _| active_keys.contains(k));
        before.saturating_sub(self.frames.len())
    }

    pub fn active_frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Ingere um DMA-BUF Linux vindo do portal PipeWire e registra diretamente no mapa de handles.
    #[cfg(target_os = "linux")]
    #[allow(clippy::too_many_arguments)]
    pub fn ingest_linux_dmabuf(
        &mut self,
        key: String,
        fd: i32,
        width: u32,
        height: u32,
        stride: u32,
        offset: u32,
        drm_format: u32,
        modifier: u64,
        now_ms: u64,
    ) -> Result<GpuFrameHandle, String> {
        let handle = self.pipewire.ingest_dmabuf(
            fd, width, height, stride, offset, drm_format, modifier, now_ms,
        )?;
        self.frames.insert(key, handle.clone());
        Ok(handle)
    }

    /// Tenta capturar um novo frame da tela primária usando o backend GPU nativo.
    /// Retorna `Some(GpuFrameHandle)` quando há frame disponível (novo ou último cacheado).
    pub fn try_capture_primary_screen(&mut self, now_ms: u64) -> Option<GpuFrameHandle> {
        if !self.settings.zero_copy_enabled
            || self.active_backend == GpuCaptureBackendKind::SoftwareFallback
        {
            return self.frames.get(&self.primary_key).cloned();
        }

        #[cfg(windows)]
        {
            if self.dxgi.is_none() {
                match DxgiDuplicator::new(0) {
                    Ok(dup) => self.dxgi = Some(dup),
                    Err(err) => {
                        self.last_error = Some(err);
                        return self.frames.get(&self.primary_key).cloned();
                    }
                }
            }
            if let Some(ref mut dup) = self.dxgi {
                match dup.acquire_shared_frame(0, now_ms) {
                    Ok(Some(handle)) => {
                        self.frames.insert(self.primary_key.clone(), handle.clone());
                        return Some(handle);
                    }
                    Ok(None) => {
                        // Timeout sem mudança visual: reutiliza último handle válido.
                        return self.frames.get(&self.primary_key).cloned();
                    }
                    Err(err) => {
                        self.last_error = Some(err);
                        // Invalida duplicator para recriação limpa no próximo tick (ex.: troca de resolução/UAC).
                        self.dxgi = None;
                        return None;
                    }
                }
            }
            None
        }

        #[cfg(target_os = "linux")]
        {
            let _ = now_ms;
            if let Some(latest) = self.pipewire.latest_frame().cloned() {
                self.frames.insert(self.primary_key.clone(), latest.clone());
                return Some(latest);
            }
            self.frames.get(&self.primary_key).cloned()
        }

        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = now_ms;
            self.frames.get(&self.primary_key).cloned()
        }
    }

    /// Readback CPU opcional da tela primária DXGI (Windows) para Virtual Cam ou encoder CPU.
    #[cfg(windows)]
    pub fn readback_primary_cpu(&mut self, now_ms: u64) -> Result<FrameBuffer, String> {
        let dup = self
            .dxgi
            .as_mut()
            .ok_or_else(|| "DXGI duplicator não inicializado".to_string())?;
        dup.readback_shared_to_cpu(now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send<T: Send>() {}

    #[test]
    fn test_gpu_capture_engine_is_send() {
        assert_send::<GpuCaptureEngine>();
        assert_send::<GpuFrameHandle>();
    }

    #[test]
    fn test_zero_copy_handle_tracking_and_frame_copy_count() {
        let mut engine = GpuCaptureEngine::new(GpuPipelineSettings {
            zero_copy_enabled: true,
            prefer_wgpu: true,
            capture_backend: GpuCaptureBackendKind::Auto,
            native_encoder_pipe: true,
        });

        // 1. DMA-BUF handle => 0 cópias e zero_copy_active com compositor wgpu
        engine.push_gpu_frame(
            "display:screen:0".to_string(),
            GpuFrameHandle::LinuxDmaBuf {
                fd: 42,
                width: 1920,
                height: 1080,
                stride: 1920 * 4,
                offset: 0,
                drm_format: DRM_FORMAT_ARGB8888,
                modifier: 0,
                seq: 1,
                updated_at_ms: 1000,
            },
        );
        let h = engine.get_gpu_frame("display:screen:0").expect("handle exists");
        assert!(h.is_zero_copy());
        assert_eq!(h.frame_copy_count(), 0);
        assert_eq!(h.dimensions(), (1920, 1080));
        assert_eq!(h.seq(), 1);

        let st_dmabuf = engine.status("wgpu", "Radeon RX 7900", "Vulkan 1.3");
        assert_eq!(st_dmabuf.frame_copy_count, 0);
        assert!(st_dmabuf.zero_copy_active);
        assert_eq!(st_dmabuf.active_textures, 1);
        assert_eq!(st_dmabuf.compositor_backend, "wgpu");

        // 2. Shared DXGI handle => também 0 cópias
        engine.push_gpu_frame(
            "display:screen:1".to_string(),
            GpuFrameHandle::SharedDxgiHandle {
                handle: 0xDEAD_BEEF,
                width: 2560,
                height: 1440,
                format: DXGI_FORMAT_BGRA8_UNORM,
                seq: 7,
                updated_at_ms: 1016,
            },
        );
        let st_dxgi = engine.status("wgpu", "NVIDIA RTX 4080", "DX12");
        assert_eq!(st_dxgi.frame_copy_count, 0);
        assert!(st_dxgi.zero_copy_active);
        assert_eq!(st_dxgi.active_textures, 2);

        // 3. Se compositor estiver em software, zero_copy_active deve ser false
        let st_sw_comp = engine.status("software", "CPU BGRA", "software");
        assert!(!st_sw_comp.zero_copy_active);

        // 4. Introduzir um frame CpuFallback eleva frame_copy_count para 1 e desativa zero_copy_active
        let fb = FrameBuffer::new(64, 64);
        engine.push_gpu_frame("camera:default".to_string(), GpuFrameHandle::CpuFallback(fb));
        let st_cpu = engine.status("wgpu", "NVIDIA RTX 4080", "DX12");
        assert_eq!(st_cpu.frame_copy_count, 1);
        assert!(!st_cpu.zero_copy_active);
        assert_eq!(st_cpu.active_textures, 3);

        // 5. Prune removendo o CpuFallback volta frame_copy_count para 0
        let keep: HashSet<String> = ["display:screen:0".to_string()].into_iter().collect();
        assert_eq!(engine.prune(&keep), 2);
        let st_after = engine.status("wgpu", "NVIDIA RTX 4080", "DX12");
        assert_eq!(st_after.frame_copy_count, 0);
        assert!(st_after.zero_copy_active);
    }

    #[test]
    fn test_backend_auto_selection_and_settings_switching() {
        // Desabilitar zero_copy força SoftwareFallback em qualquer OS
        let mut engine = GpuCaptureEngine::new(GpuPipelineSettings {
            zero_copy_enabled: false,
            prefer_wgpu: false,
            capture_backend: GpuCaptureBackendKind::Auto,
            native_encoder_pipe: false,
        });
        assert_eq!(engine.active_backend(), GpuCaptureBackendKind::SoftwareFallback);
        let st = engine.status("wgpu", "GPU", "driver");
        assert_eq!(st.capture_backend, "software-fallback");
        assert_eq!(st.frame_copy_count, 1);
        assert!(!st.zero_copy_active);

        // Pedir explicitamente SoftwareFallback mesmo com zero_copy_enabled=true respeita a escolha
        engine.apply_settings(GpuPipelineSettings {
            zero_copy_enabled: true,
            prefer_wgpu: true,
            capture_backend: GpuCaptureBackendKind::SoftwareFallback,
            native_encoder_pipe: true,
        });
        assert_eq!(engine.active_backend(), GpuCaptureBackendKind::SoftwareFallback);

        // Alternar para Auto resolve sem panicar em qualquer plataforma (CI headless incluído)
        engine.apply_settings(GpuPipelineSettings {
            zero_copy_enabled: true,
            prefer_wgpu: true,
            capture_backend: GpuCaptureBackendKind::Auto,
            native_encoder_pipe: true,
        });
        let resolved = engine.active_backend();
        assert_ne!(resolved, GpuCaptureBackendKind::Auto);

        // try_capture_primary_screen nunca entra em pânico mesmo sem display físico
        let _ = engine.try_capture_primary_screen(500);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_linux_pipewire_dmabuf_validation_and_ingest() {
        let mut pw = PipeWireDmaBufCapturer::with_probe_info(LinuxCaptureProbeInfo {
            wayland_session: true,
            pipewire_available: true,
            portal_available: true,
            pw_cli_found: true,
            drm_render_node: Some("/dev/dri/renderD128".to_string()),
        });
        assert!(pw.is_zero_copy_capable());

        // fd negativo deve falhar
        assert!(pw
            .ingest_dmabuf(-1, 1920, 1080, 1920 * 4, 0, DRM_FORMAT_ARGB8888, 0, 10)
            .is_err());
        // stride menor que width*4 deve falhar
        assert!(pw
            .ingest_dmabuf(15, 1920, 1080, 100, 0, DRM_FORMAT_ARGB8888, 0, 10)
            .is_err());

        // DMA-BUF válido
        let handle = pw
            .ingest_dmabuf(15, 1920, 1080, 1920 * 4, 0, DRM_FORMAT_XRGB8888, 0x0100, 250)
            .expect("valid dmabuf");
        assert!(handle.is_zero_copy());
        assert_eq!(handle.frame_copy_count(), 0);
        assert_eq!(handle.seq(), 1);
        assert_eq!(handle.updated_at_ms(), 250);
    }
}
