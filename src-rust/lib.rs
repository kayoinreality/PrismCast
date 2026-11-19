//! PrismCast Studio — Core engine library (`prismcast_core`).
//!
//! Arquitetura modular Rust-first:
//! - `protocol`: fonte única da verdade para contratos de rede, schema v3 de
//!   configurações, áudio, cenas, multichat, kit e telemetria.
//! - `store`: persistência atômica, deep-merge, migração v2->v3 e cofre seguro.
//! - `auth_twitch`: OAuth Device Flow + Helix API + validação de stream keys.
//! - `multichat`: Twitch IRC + YouTube Live Chat + Kick Centrifugo + 7TV/BTTV/FFZ.
//! - `kit`: parser/serializer estático seguro para kits OBS sem execução JS.
//! - `audio_mixer`: DSP 48kHz f32 estéreo, EQ 3 bandas, Ducking e PartyVoice.
//! - `capture` / `compositor` / `rtc_host`: captura nativa, composição Dual Output e WebRTC Host.
//! - `multistream` / `recorder`: pipelines FFmpeg, replay buffer EBML e remux MP4.
//! - `net` / `discovery` / `signaling` / `firewall`: rede LAN P2P e sistema.
//! - `hotkeys`: atalhos globais nativos ligados a `AppSettings.hotkeys`.
//! - `rpc`: `AppEngine` unificado + servidor WebSocket autenticado com frames binários.

pub mod audio_mixer;
pub mod auth_twitch;
pub mod capture;
pub mod compositor;
pub mod discovery;
pub mod firewall;
pub mod hotkeys;
pub mod kit;
pub mod multichat;
pub mod multistream;
pub mod net;
pub mod protocol;
pub mod recorder;
pub mod rpc;
pub mod rtc_host;
pub mod signaling;
pub mod store;
