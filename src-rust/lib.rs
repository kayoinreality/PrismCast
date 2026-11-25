//! PrismCast Studio — Core engine & 100% Native Rust Desktop Library (`prismcast_core`).
//!
//! Arquitetura 100% nativa em Rust (zero Tauri, zero WebView2, zero Electron):
//! - `native_ui`: interface desktop nativa em `eframe` + `egui` + `wgpu 24` com visual
//!   Broadcast Carbon & Glassmorphism espectral.
//! - `native_capture`: captura nativa direta de telas e janelas (DXGI / PrintWindow / PipeWire)
//!   sem popups de navegador, sem barra de compartilhamento e sem roubar foco.
//! - `protocol`: fonte única da verdade para contratos de rede, schema v3 de
//!   configurações, áudio, cenas, multichat, kit e telemetria.
//! - `store`: persistência atômica, deep-merge, migração v2->v3 e cofre seguro.
//! - `auth_twitch`: OAuth Device Flow + Helix API + validação de stream keys.
//! - `multichat`: Twitch IRC + YouTube Live Chat + Kick Centrifugo + 7TV/BTTV/FFZ.
//! - `kit`: parser/serializer estático seguro para kits OBS sem execução JS.
//! - `audio_mixer` / `audio_runtime`: DSP 48kHz f32 estéreo, EQ 3 bandas, Ducking e VST3/CLAP.
//! - `capture` / `gpu_capture` / `compositor` / `gpu_compositor`: captura nativa e composição `wgpu`.
//! - `multistream` / `recorder` / `virtual_cam` / `obs_ws`: pipelines de broadcast profissional.

pub mod abr;
pub mod audio_mixer;
pub mod audio_runtime;
pub mod auth_twitch;
pub mod capture;
pub mod compositor;
pub mod discovery;
pub mod firewall;
pub mod gpu_capture;
pub mod gpu_compositor;
pub mod hotkeys;
pub mod kit;
pub mod multichat;
pub mod multistream;
pub mod native_capture;
#[cfg(feature = "native-ui")]
pub mod native_ui;
pub mod net;
pub mod obs_ws;
pub mod protocol;
pub mod recorder;
pub mod rpc;
pub mod rtc_host;
pub mod rtmp_publisher;
pub mod signaling;
pub mod store;
pub mod updater;
pub mod virtual_cam;
pub mod vst_host;
