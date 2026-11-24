//! Dynamic cross-platform VST3 & CLAP audio plugin scanner and real-time host.
//!
//! Features:
//! - Scans standard system & user VST3 and CLAP plugin directories across Windows, Linux, and macOS,
//!   resolving both `.vst3`/`.clap` bundle directories (`Contents/<arch>-<os>/*`) and flat shared libraries.
//! - Real `#[repr(C)]` C-ABI bindings for CLAP 1.2 (`clap_entry`, factory, plugin, params, state, GUI)
//!   and Steinberg VST3 COM-style ABI (`GetPluginFactory`, `ModuleEntry`/`ModuleExit`, `FUnknown`,
//!   `IPluginFactory`, `IComponent`, `IAudioProcessor`, `IEditController`, `IBStream`, `IPlugView`).
//! - Fault-isolated `PluginChain` per mixer channel: zero-allocation real-time `process_block`,
//!   `catch_unwind` panic shield, `NaN`/`Inf` output sanitization, wet/dry mixing, automatic slot
//!   bypass on plugin fault, live parameter automation, and Base64 state export/restore.

use crate::protocol::{AudioPluginFormat, AudioPluginInfo, AudioPluginParam, AudioPluginSlot};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use libloading::Library;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

// ------------------------------------------------------------------ Helpers

fn c_char_array_to_string(buf: &[c_char]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let bytes: Vec<u8> = buf[..len].iter().map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

unsafe fn c_ptr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    CStr::from_ptr(ptr).to_string_lossy().trim().to_string()
}

fn utf16_array_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len]).trim().to_string()
}

fn format_tuid(uid: &[u8; 16]) -> String {
    let mut s = String::with_capacity(32);
    for b in uid {
        use std::fmt::Write;
        let _ = write!(&mut s, "{b:02X}");
    }
    s
}

fn parse_tuid(s: &str) -> Option<[u8; 16]> {
    let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn file_stem_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Unknown Plugin".to_string())
}

// ------------------------------------------------------------------ CLAP C-ABI (1.2)

pub const CLAP_PLUGIN_FACTORY_ID: &[u8] = b"clap.plugin-factory\0";
pub const CLAP_EXT_PARAMS: &[u8] = b"clap.params\0";
pub const CLAP_EXT_STATE: &[u8] = b"clap.state\0";
pub const CLAP_EXT_GUI: &[u8] = b"clap.gui\0";

pub const CLAP_CORE_EVENT_SPACE_ID: u16 = 0;
pub const CLAP_EVENT_PARAM_VALUE: u16 = 5;

pub const CLAP_PROCESS_ERROR: i32 = 0;
pub const CLAP_NAME_SIZE: usize = 256;
pub const CLAP_PATH_SIZE: usize = 1024;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClapVersion {
    pub major: u32,
    pub minor: u32,
    pub revision: u32,
}

pub const CLAP_VERSION: ClapVersion = ClapVersion {
    major: 1,
    minor: 2,
    revision: 0,
};

#[repr(C)]
pub struct ClapPluginEntry {
    pub clap_version: ClapVersion,
    pub init: Option<unsafe extern "C" fn(plugin_path: *const c_char) -> bool>,
    pub deinit: Option<unsafe extern "C" fn()>,
    pub get_factory: Option<unsafe extern "C" fn(factory_id: *const c_char) -> *const c_void>,
}

#[repr(C)]
pub struct ClapPluginDescriptor {
    pub clap_version: ClapVersion,
    pub id: *const c_char,
    pub name: *const c_char,
    pub vendor: *const c_char,
    pub url: *const c_char,
    pub manual_url: *const c_char,
    pub support_url: *const c_char,
    pub version: *const c_char,
    pub description: *const c_char,
    pub features: *const *const c_char,
}

#[repr(C)]
pub struct ClapPluginFactory {
    pub get_plugin_count: Option<unsafe extern "C" fn(factory: *const ClapPluginFactory) -> u32>,
    pub get_plugin_descriptor: Option<
        unsafe extern "C" fn(
            factory: *const ClapPluginFactory,
            index: u32,
        ) -> *const ClapPluginDescriptor,
    >,
    pub create_plugin: Option<
        unsafe extern "C" fn(
            factory: *const ClapPluginFactory,
            host: *const ClapHost,
            plugin_id: *const c_char,
        ) -> *const ClapPlugin,
    >,
}

#[repr(C)]
pub struct ClapHost {
    pub clap_version: ClapVersion,
    pub host_data: *mut c_void,
    pub name: *const c_char,
    pub vendor: *const c_char,
    pub url: *const c_char,
    pub version: *const c_char,
    pub get_extension:
        Option<unsafe extern "C" fn(host: *const ClapHost, extension_id: *const c_char) -> *const c_void>,
    pub request_restart: Option<unsafe extern "C" fn(host: *const ClapHost)>,
    pub request_process: Option<unsafe extern "C" fn(host: *const ClapHost)>,
    pub request_callback: Option<unsafe extern "C" fn(host: *const ClapHost)>,
}

#[repr(C)]
pub struct ClapPlugin {
    pub desc: *const ClapPluginDescriptor,
    pub plugin_data: *mut c_void,
    pub init: Option<unsafe extern "C" fn(plugin: *const ClapPlugin) -> bool>,
    pub destroy: Option<unsafe extern "C" fn(plugin: *const ClapPlugin)>,
    pub activate: Option<
        unsafe extern "C" fn(
            plugin: *const ClapPlugin,
            sample_rate: f64,
            min_frames_count: u32,
            max_frames_count: u32,
        ) -> bool,
    >,
    pub deactivate: Option<unsafe extern "C" fn(plugin: *const ClapPlugin)>,
    pub start_processing: Option<unsafe extern "C" fn(plugin: *const ClapPlugin) -> bool>,
    pub stop_processing: Option<unsafe extern "C" fn(plugin: *const ClapPlugin)>,
    pub reset: Option<unsafe extern "C" fn(plugin: *const ClapPlugin)>,
    pub process:
        Option<unsafe extern "C" fn(plugin: *const ClapPlugin, process: *const ClapProcess) -> i32>,
    pub get_extension:
        Option<unsafe extern "C" fn(plugin: *const ClapPlugin, id: *const c_char) -> *const c_void>,
    pub on_main_thread: Option<unsafe extern "C" fn(plugin: *const ClapPlugin)>,
}

#[repr(C)]
pub struct ClapAudioBuffer {
    pub data32: *mut *mut f32,
    pub data64: *mut *mut f64,
    pub channel_count: u32,
    pub latency: u32,
    pub constant_mask: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ClapEventHeader {
    pub size: u32,
    pub time: u32,
    pub space_id: u16,
    pub event_type: u16,
    pub flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ClapEventParamValue {
    pub header: ClapEventHeader,
    pub param_id: u32,
    pub cookie: *mut c_void,
    pub note_id: i32,
    pub port_index: i16,
    pub channel: i16,
    pub key: i16,
    pub value: f64,
}

#[repr(C)]
pub struct ClapInputEvents {
    pub ctx: *mut c_void,
    pub size: Option<unsafe extern "C" fn(list: *const ClapInputEvents) -> u32>,
    pub get: Option<
        unsafe extern "C" fn(list: *const ClapInputEvents, index: u32) -> *const ClapEventHeader,
    >,
}

#[repr(C)]
pub struct ClapOutputEvents {
    pub ctx: *mut c_void,
    pub try_push: Option<
        unsafe extern "C" fn(list: *const ClapOutputEvents, event: *const ClapEventHeader) -> bool,
    >,
}

#[repr(C)]
pub struct ClapProcess {
    pub steady_time: i64,
    pub frames_count: u32,
    pub transport: *const c_void,
    pub audio_inputs: *const ClapAudioBuffer,
    pub audio_outputs: *mut ClapAudioBuffer,
    pub audio_inputs_count: u32,
    pub audio_outputs_count: u32,
    pub in_events: *const ClapInputEvents,
    pub out_events: *const ClapOutputEvents,
}

#[repr(C)]
pub struct ClapParamInfo {
    pub id: u32,
    pub flags: u32,
    pub cookie: *mut c_void,
    pub name: [c_char; CLAP_NAME_SIZE],
    pub module: [c_char; CLAP_PATH_SIZE],
    pub min_value: f64,
    pub max_value: f64,
    pub default_value: f64,
}

#[repr(C)]
pub struct ClapPluginParams {
    pub count: Option<unsafe extern "C" fn(plugin: *const ClapPlugin) -> u32>,
    pub get_info: Option<
        unsafe extern "C" fn(
            plugin: *const ClapPlugin,
            param_index: u32,
            param_info: *mut ClapParamInfo,
        ) -> bool,
    >,
    pub get_value: Option<
        unsafe extern "C" fn(
            plugin: *const ClapPlugin,
            param_id: u32,
            out_value: *mut f64,
        ) -> bool,
    >,
    pub value_to_text: Option<
        unsafe extern "C" fn(
            plugin: *const ClapPlugin,
            param_id: u32,
            value: f64,
            out_buffer: *mut c_char,
            out_buffer_capacity: u32,
        ) -> bool,
    >,
    pub text_to_value: Option<
        unsafe extern "C" fn(
            plugin: *const ClapPlugin,
            param_id: u32,
            param_value_text: *const c_char,
            out_value: *mut f64,
        ) -> bool,
    >,
    pub flush: Option<
        unsafe extern "C" fn(
            plugin: *const ClapPlugin,
            in_events: *const ClapInputEvents,
            out_events: *const ClapOutputEvents,
        ),
    >,
}

#[repr(C)]
pub struct ClapIStream {
    pub ctx: *mut c_void,
    pub read: Option<
        unsafe extern "C" fn(stream: *const ClapIStream, buffer: *mut c_void, size: u64) -> i64,
    >,
}

#[repr(C)]
pub struct ClapOStream {
    pub ctx: *mut c_void,
    pub write: Option<
        unsafe extern "C" fn(stream: *const ClapOStream, buffer: *const c_void, size: u64) -> i64,
    >,
}

#[repr(C)]
pub struct ClapPluginState {
    pub save:
        Option<unsafe extern "C" fn(plugin: *const ClapPlugin, stream: *const ClapOStream) -> bool>,
    pub load:
        Option<unsafe extern "C" fn(plugin: *const ClapPlugin, stream: *const ClapIStream) -> bool>,
}

#[repr(C)]
pub struct ClapWindow {
    pub api: *const c_char,
    pub ptr: *mut c_void,
}

#[repr(C)]
pub struct ClapPluginGui {
    pub is_api_supported: Option<
        unsafe extern "C" fn(
            plugin: *const ClapPlugin,
            api: *const c_char,
            is_floating: bool,
        ) -> bool,
    >,
    pub get_preferred_api: Option<
        unsafe extern "C" fn(
            plugin: *const ClapPlugin,
            api: *mut *const c_char,
            is_floating: *mut bool,
        ) -> bool,
    >,
    pub create: Option<
        unsafe extern "C" fn(
            plugin: *const ClapPlugin,
            api: *const c_char,
            is_floating: bool,
        ) -> bool,
    >,
    pub destroy: Option<unsafe extern "C" fn(plugin: *const ClapPlugin)>,
    pub set_scale: Option<unsafe extern "C" fn(plugin: *const ClapPlugin, scale: f64) -> bool>,
    pub get_size: Option<
        unsafe extern "C" fn(
            plugin: *const ClapPlugin,
            width: *mut u32,
            height: *mut u32,
        ) -> bool,
    >,
    pub can_resize: Option<unsafe extern "C" fn(plugin: *const ClapPlugin) -> bool>,
    pub get_resize_hints:
        Option<unsafe extern "C" fn(plugin: *const ClapPlugin, hints: *mut c_void) -> bool>,
    pub adjust_size: Option<
        unsafe extern "C" fn(
            plugin: *const ClapPlugin,
            width: *mut u32,
            height: *mut u32,
        ) -> bool,
    >,
    pub set_size:
        Option<unsafe extern "C" fn(plugin: *const ClapPlugin, width: u32, height: u32) -> bool>,
    pub set_parent: Option<
        unsafe extern "C" fn(plugin: *const ClapPlugin, window: *const ClapWindow) -> bool,
    >,
    pub set_transient: Option<
        unsafe extern "C" fn(plugin: *const ClapPlugin, window: *const ClapWindow) -> bool,
    >,
    pub suggest_title:
        Option<unsafe extern "C" fn(plugin: *const ClapPlugin, title: *const c_char)>,
    pub show: Option<unsafe extern "C" fn(plugin: *const ClapPlugin) -> bool>,
    pub hide: Option<unsafe extern "C" fn(plugin: *const ClapPlugin) -> bool>,
}

unsafe extern "C" fn clap_host_get_extension(
    _host: *const ClapHost,
    _ext_id: *const c_char,
) -> *const c_void {
    ptr::null()
}

unsafe extern "C" fn clap_host_noop(_host: *const ClapHost) {}

fn make_clap_host() -> Box<ClapHost> {
    Box::new(ClapHost {
        clap_version: CLAP_VERSION,
        host_data: ptr::null_mut(),
        name: b"PrismCast Studio\0".as_ptr() as *const c_char,
        vendor: b"PrismCast\0".as_ptr() as *const c_char,
        url: b"https://github.com/kayoinreality/PrismCast\0".as_ptr() as *const c_char,
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char,
        get_extension: Some(clap_host_get_extension),
        request_restart: Some(clap_host_noop),
        request_process: Some(clap_host_noop),
        request_callback: Some(clap_host_noop),
    })
}

unsafe extern "C" fn clap_in_events_size(list: *const ClapInputEvents) -> u32 {
    if list.is_null() {
        return 0;
    }
    let ctx = (*list).ctx as *const Vec<ClapEventParamValue>;
    if ctx.is_null() {
        return 0;
    }
    (*ctx).len() as u32
}

unsafe extern "C" fn clap_in_events_get(
    list: *const ClapInputEvents,
    index: u32,
) -> *const ClapEventHeader {
    if list.is_null() {
        return ptr::null();
    }
    let ctx = (*list).ctx as *const Vec<ClapEventParamValue>;
    if ctx.is_null() {
        return ptr::null();
    }
    let vec = &*ctx;
    match vec.get(index as usize) {
        Some(ev) => &ev.header as *const ClapEventHeader,
        None => ptr::null(),
    }
}

unsafe extern "C" fn clap_out_events_try_push(
    _list: *const ClapOutputEvents,
    _event: *const ClapEventHeader,
) -> bool {
    true
}

unsafe extern "C" fn clap_ostream_write(
    stream: *const ClapOStream,
    buffer: *const c_void,
    size: u64,
) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    let ctx = (*stream).ctx as *mut Vec<u8>;
    if ctx.is_null() {
        return -1;
    }
    let slice = std::slice::from_raw_parts(buffer as *const u8, size as usize);
    (*ctx).extend_from_slice(slice);
    size as i64
}

struct ClapReadCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

unsafe extern "C" fn clap_istream_read(
    stream: *const ClapIStream,
    buffer: *mut c_void,
    size: u64,
) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    let ctx = (*stream).ctx as *mut ClapReadCursor<'_>;
    if ctx.is_null() {
        return -1;
    }
    let cursor = &mut *ctx;
    let remaining = cursor.data.len().saturating_sub(cursor.pos);
    let to_read = remaining.min(size as usize);
    if to_read > 0 {
        ptr::copy_nonoverlapping(
            cursor.data.as_ptr().add(cursor.pos),
            buffer as *mut u8,
            to_read,
        );
        cursor.pos += to_read;
    }
    to_read as i64
}

// ------------------------------------------------------------------ VST3 COM C-ABI

pub type Tuid = [u8; 16];
pub type TResult = i32;

pub const K_RESULT_OK: TResult = 0;
pub const K_RESULT_FALSE: TResult = 1;

#[cfg(windows)]
pub const K_NO_INTERFACE: TResult = 0x8000_4002_u32 as i32;
#[cfg(not(windows))]
pub const K_NO_INTERFACE: TResult = -1;

pub const VST3_MEDIA_AUDIO: i32 = 0;
pub const VST3_BUS_INPUT: i32 = 0;
pub const VST3_BUS_OUTPUT: i32 = 1;
pub const VST3_REALTIME: i32 = 0;
pub const VST3_SAMPLE_32: i32 = 0;
pub const VST3_SPEAKER_STEREO: u64 = 0x3; // kSpeakerL | kSpeakerR

/// Encodes a Steinberg VST3 IID matching `INLINE_UID(l1, l2, l3, l4)` on both Windows COM
/// byte order and POSIX big-endian byte order.
pub const fn vst3_iid(l1: u32, l2: u32, l3: u32, l4: u32) -> Tuid {
    #[cfg(windows)]
    {
        [
            (l1 & 0x0000_00FF) as u8,
            ((l1 & 0x0000_FF00) >> 8) as u8,
            ((l1 & 0x00FF_0000) >> 16) as u8,
            ((l1 & 0xFF00_0000) >> 24) as u8,
            ((l2 & 0x00FF_0000) >> 16) as u8,
            ((l2 & 0xFF00_0000) >> 24) as u8,
            (l2 & 0x0000_00FF) as u8,
            ((l2 & 0x0000_FF00) >> 8) as u8,
            ((l3 & 0xFF00_0000) >> 24) as u8,
            ((l3 & 0x00FF_0000) >> 16) as u8,
            ((l3 & 0x0000_FF00) >> 8) as u8,
            (l3 & 0x0000_00FF) as u8,
            ((l4 & 0xFF00_0000) >> 24) as u8,
            ((l4 & 0x00FF_0000) >> 16) as u8,
            ((l4 & 0x0000_FF00) >> 8) as u8,
            (l4 & 0x0000_00FF) as u8,
        ]
    }
    #[cfg(not(windows))]
    {
        [
            ((l1 & 0xFF00_0000) >> 24) as u8,
            ((l1 & 0x00FF_0000) >> 16) as u8,
            ((l1 & 0x0000_FF00) >> 8) as u8,
            (l1 & 0x0000_00FF) as u8,
            ((l2 & 0xFF00_0000) >> 24) as u8,
            ((l2 & 0x00FF_0000) >> 16) as u8,
            ((l2 & 0x0000_FF00) >> 8) as u8,
            (l2 & 0x0000_00FF) as u8,
            ((l3 & 0xFF00_0000) >> 24) as u8,
            ((l3 & 0x00FF_0000) >> 16) as u8,
            ((l3 & 0x0000_FF00) >> 8) as u8,
            (l3 & 0x0000_00FF) as u8,
            ((l4 & 0xFF00_0000) >> 24) as u8,
            ((l4 & 0x00FF_0000) >> 16) as u8,
            ((l4 & 0x0000_FF00) >> 8) as u8,
            (l4 & 0x0000_00FF) as u8,
        ]
    }
}

pub const IID_FUNKNOWN: Tuid = vst3_iid(0x00000000, 0x00000000, 0xC0000000, 0x00000046);
pub const IID_IBSTREAM: Tuid = vst3_iid(0xC3BF6EA2, 0x30994EF5, 0x9B6BF990, 0x1EE33E9B);
pub const IID_ICOMPONENT: Tuid = vst3_iid(0xE831FF31, 0xF2D54301, 0x928EBBEE, 0x25697802);
pub const IID_IAUDIO_PROCESSOR: Tuid = vst3_iid(0x42043F99, 0xB7DA453C, 0xA569E79D, 0x9AAEC33D);
pub const IID_IEDIT_CONTROLLER: Tuid = vst3_iid(0xDCD7BBE3, 0x7742448D, 0xA874AACC, 0x979C759E);
pub const IID_IPARAMETER_CHANGES: Tuid = vst3_iid(0xA4779663, 0x0BB64A56, 0xB44384A8, 0x466FEB9D);
pub const IID_IPARAM_VALUE_QUEUE: Tuid = vst3_iid(0x01263A18, 0xED074F6F, 0x98C9D356, 0x4686F9BA);

#[repr(C)]
pub struct FUnknownVtbl {
    pub query_interface: unsafe extern "system" fn(
        this: *mut FUnknown,
        iid: *const Tuid,
        obj: *mut *mut c_void,
    ) -> TResult,
    pub add_ref: unsafe extern "system" fn(this: *mut FUnknown) -> u32,
    pub release: unsafe extern "system" fn(this: *mut FUnknown) -> u32,
}

#[repr(C)]
pub struct FUnknown {
    pub vtbl: *const FUnknownVtbl,
}

#[repr(C)]
pub struct PFactoryInfo {
    pub vendor: [c_char; 64],
    pub url: [c_char; 256],
    pub email: [c_char; 128],
    pub flags: i32,
}

#[repr(C)]
pub struct PClassInfo {
    pub cid: Tuid,
    pub cardinality: i32,
    pub category: [c_char; 32],
    pub name: [c_char; 64],
}

#[repr(C)]
pub struct IPluginFactoryVtbl {
    pub base: FUnknownVtbl,
    pub get_factory_info:
        unsafe extern "system" fn(this: *mut IPluginFactory, info: *mut PFactoryInfo) -> TResult,
    pub count_classes: unsafe extern "system" fn(this: *mut IPluginFactory) -> i32,
    pub get_class_info: unsafe extern "system" fn(
        this: *mut IPluginFactory,
        index: i32,
        info: *mut PClassInfo,
    ) -> TResult,
    pub create_instance: unsafe extern "system" fn(
        this: *mut IPluginFactory,
        cid: *const c_char,
        iid: *const c_char,
        obj: *mut *mut c_void,
    ) -> TResult,
}

#[repr(C)]
pub struct IPluginFactory {
    pub vtbl: *const IPluginFactoryVtbl,
}

#[repr(C)]
pub struct IBStreamVtbl {
    pub base: FUnknownVtbl,
    pub read: unsafe extern "system" fn(
        this: *mut IBStream,
        buffer: *mut c_void,
        num_bytes: i32,
        num_bytes_read: *mut i32,
    ) -> TResult,
    pub write: unsafe extern "system" fn(
        this: *mut IBStream,
        buffer: *mut c_void,
        num_bytes: i32,
        num_bytes_written: *mut i32,
    ) -> TResult,
    pub seek: unsafe extern "system" fn(
        this: *mut IBStream,
        pos: i64,
        mode: i32,
        result: *mut i64,
    ) -> TResult,
    pub tell: unsafe extern "system" fn(this: *mut IBStream, pos: *mut i64) -> TResult,
}

#[repr(C)]
pub struct IBStream {
    pub vtbl: *const IBStreamVtbl,
}

#[repr(C)]
pub struct IPluginBaseVtbl {
    pub base: FUnknownVtbl,
    pub initialize:
        unsafe extern "system" fn(this: *mut IPluginBase, context: *mut FUnknown) -> TResult,
    pub terminate: unsafe extern "system" fn(this: *mut IPluginBase) -> TResult,
}

#[repr(C)]
pub struct IPluginBase {
    pub vtbl: *const IPluginBaseVtbl,
}

#[repr(C)]
pub struct IComponentVtbl {
    pub base: IPluginBaseVtbl,
    pub get_controller_class_id:
        unsafe extern "system" fn(this: *mut IComponent, class_id: *mut Tuid) -> TResult,
    pub set_io_mode: unsafe extern "system" fn(this: *mut IComponent, mode: i32) -> TResult,
    pub get_bus_count:
        unsafe extern "system" fn(this: *mut IComponent, media_type: i32, dir: i32) -> i32,
    pub get_bus_info: unsafe extern "system" fn(
        this: *mut IComponent,
        media_type: i32,
        dir: i32,
        index: i32,
        bus: *mut c_void,
    ) -> TResult,
    pub get_routing_info: unsafe extern "system" fn(
        this: *mut IComponent,
        in_info: *mut c_void,
        out_info: *mut c_void,
    ) -> TResult,
    pub activate_bus: unsafe extern "system" fn(
        this: *mut IComponent,
        media_type: i32,
        dir: i32,
        index: i32,
        state: u8,
    ) -> TResult,
    pub set_active: unsafe extern "system" fn(this: *mut IComponent, state: u8) -> TResult,
    pub set_state:
        unsafe extern "system" fn(this: *mut IComponent, state: *mut IBStream) -> TResult,
    pub get_state:
        unsafe extern "system" fn(this: *mut IComponent, state: *mut IBStream) -> TResult,
}

#[repr(C)]
pub struct IComponent {
    pub vtbl: *const IComponentVtbl,
}

#[repr(C)]
pub struct ProcessSetup {
    pub process_mode: i32,
    pub symbolic_sample_size: i32,
    pub max_samples_per_block: i32,
    pub sample_rate: f64,
}

#[repr(C)]
pub struct AudioBusBuffers {
    pub num_channels: i32,
    pub silence_flags: u64,
    pub channel_buffers_32: *mut *mut f32,
}

#[repr(C)]
pub struct ProcessData {
    pub process_mode: i32,
    pub symbolic_sample_size: i32,
    pub num_samples: i32,
    pub num_inputs: i32,
    pub num_outputs: i32,
    pub inputs: *mut AudioBusBuffers,
    pub outputs: *mut AudioBusBuffers,
    pub input_parameter_changes: *mut IParameterChanges,
    pub output_parameter_changes: *mut IParameterChanges,
    pub input_events: *mut c_void,
    pub output_events: *mut c_void,
    pub process_context: *mut c_void,
}

#[repr(C)]
pub struct IAudioProcessorVtbl {
    pub base: FUnknownVtbl,
    pub set_bus_arrangements: unsafe extern "system" fn(
        this: *mut IAudioProcessor,
        inputs: *mut u64,
        num_ins: i32,
        outputs: *mut u64,
        num_outs: i32,
    ) -> TResult,
    pub get_bus_arrangement: unsafe extern "system" fn(
        this: *mut IAudioProcessor,
        dir: i32,
        index: i32,
        arr: *mut u64,
    ) -> TResult,
    pub can_process_sample_size:
        unsafe extern "system" fn(this: *mut IAudioProcessor, symbolic_sample_size: i32) -> TResult,
    pub get_latency_samples: unsafe extern "system" fn(this: *mut IAudioProcessor) -> u32,
    pub setup_processing:
        unsafe extern "system" fn(this: *mut IAudioProcessor, setup: *mut ProcessSetup) -> TResult,
    pub set_processing: unsafe extern "system" fn(this: *mut IAudioProcessor, state: u8) -> TResult,
    pub process:
        unsafe extern "system" fn(this: *mut IAudioProcessor, data: *mut ProcessData) -> TResult,
    pub get_tail_samples: unsafe extern "system" fn(this: *mut IAudioProcessor) -> u32,
}

#[repr(C)]
pub struct IAudioProcessor {
    pub vtbl: *const IAudioProcessorVtbl,
}

#[repr(C)]
pub struct Vst3ParameterInfo {
    pub id: u32,
    pub title: [u16; 128],
    pub short_title: [u16; 128],
    pub units: [u16; 128],
    pub step_count: i32,
    pub default_normalized_value: f64,
    pub unit_id: i32,
    pub flags: i32,
}

#[repr(C)]
pub struct ViewRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
pub struct IPlugViewVtbl {
    pub base: FUnknownVtbl,
    pub is_platform_type_supported:
        unsafe extern "system" fn(this: *mut IPlugView, ty: *const c_char) -> TResult,
    pub attached: unsafe extern "system" fn(
        this: *mut IPlugView,
        parent: *mut c_void,
        ty: *const c_char,
    ) -> TResult,
    pub removed: unsafe extern "system" fn(this: *mut IPlugView) -> TResult,
    pub on_wheel: unsafe extern "system" fn(this: *mut IPlugView, distance: f32) -> TResult,
    pub on_key_down: unsafe extern "system" fn(
        this: *mut IPlugView,
        key: u16,
        key_code: i16,
        modifiers: i16,
    ) -> TResult,
    pub on_key_up: unsafe extern "system" fn(
        this: *mut IPlugView,
        key: u16,
        key_code: i16,
        modifiers: i16,
    ) -> TResult,
    pub get_size: unsafe extern "system" fn(this: *mut IPlugView, size: *mut ViewRect) -> TResult,
    pub on_size: unsafe extern "system" fn(this: *mut IPlugView, new_size: *mut ViewRect) -> TResult,
    pub on_focus: unsafe extern "system" fn(this: *mut IPlugView, state: u8) -> TResult,
    pub set_frame:
        unsafe extern "system" fn(this: *mut IPlugView, frame: *mut c_void) -> TResult,
    pub can_resize: unsafe extern "system" fn(this: *mut IPlugView) -> TResult,
    pub check_size_constraint:
        unsafe extern "system" fn(this: *mut IPlugView, rect: *mut ViewRect) -> TResult,
}

#[repr(C)]
pub struct IPlugView {
    pub vtbl: *const IPlugViewVtbl,
}

#[repr(C)]
pub struct IEditControllerVtbl {
    pub base: IPluginBaseVtbl,
    pub set_component_state:
        unsafe extern "system" fn(this: *mut IEditController, state: *mut IBStream) -> TResult,
    pub set_state:
        unsafe extern "system" fn(this: *mut IEditController, state: *mut IBStream) -> TResult,
    pub get_state:
        unsafe extern "system" fn(this: *mut IEditController, state: *mut IBStream) -> TResult,
    pub get_parameter_count: unsafe extern "system" fn(this: *mut IEditController) -> i32,
    pub get_parameter_info: unsafe extern "system" fn(
        this: *mut IEditController,
        param_index: i32,
        info: *mut Vst3ParameterInfo,
    ) -> TResult,
    pub get_param_string_by_value: unsafe extern "system" fn(
        this: *mut IEditController,
        id: u32,
        value_normalized: f64,
        string: *mut u16,
    ) -> TResult,
    pub get_param_value_by_string: unsafe extern "system" fn(
        this: *mut IEditController,
        id: u32,
        string: *const u16,
        value_normalized: *mut f64,
    ) -> TResult,
    pub normalized_param_to_plain: unsafe extern "system" fn(
        this: *mut IEditController,
        id: u32,
        value_normalized: f64,
    ) -> f64,
    pub plain_param_to_normalized:
        unsafe extern "system" fn(this: *mut IEditController, id: u32, plain_value: f64) -> f64,
    pub get_param_normalized:
        unsafe extern "system" fn(this: *mut IEditController, id: u32) -> f64,
    pub set_param_normalized: unsafe extern "system" fn(
        this: *mut IEditController,
        id: u32,
        value: f64,
    ) -> TResult,
    pub set_component_handler:
        unsafe extern "system" fn(this: *mut IEditController, handler: *mut c_void) -> TResult,
    pub create_view:
        unsafe extern "system" fn(this: *mut IEditController, name: *const c_char) -> *mut IPlugView,
}

#[repr(C)]
pub struct IEditController {
    pub vtbl: *const IEditControllerVtbl,
}

#[repr(C)]
pub struct IParamValueQueueVtbl {
    pub base: FUnknownVtbl,
    pub get_parameter_id: unsafe extern "system" fn(this: *mut IParamValueQueue) -> u32,
    pub get_point_count: unsafe extern "system" fn(this: *mut IParamValueQueue) -> i32,
    pub get_point: unsafe extern "system" fn(
        this: *mut IParamValueQueue,
        index: i32,
        sample_offset: *mut i32,
        value: *mut f64,
    ) -> TResult,
    pub add_point: unsafe extern "system" fn(
        this: *mut IParamValueQueue,
        sample_offset: i32,
        value: f64,
        index: *mut i32,
    ) -> TResult,
}

#[repr(C)]
pub struct IParamValueQueue {
    pub vtbl: *const IParamValueQueueVtbl,
}

#[repr(C)]
pub struct IParameterChangesVtbl {
    pub base: FUnknownVtbl,
    pub get_parameter_count: unsafe extern "system" fn(this: *mut IParameterChanges) -> i32,
    pub get_parameter_data:
        unsafe extern "system" fn(this: *mut IParameterChanges, index: i32) -> *mut IParamValueQueue,
    pub add_parameter_data: unsafe extern "system" fn(
        this: *mut IParameterChanges,
        id: *const u32,
        index: *mut i32,
    ) -> *mut IParamValueQueue,
}

#[repr(C)]
pub struct IParameterChanges {
    pub vtbl: *const IParameterChangesVtbl,
}

// ------------------------------------------------------------------ VST3 COM Host Objects

#[repr(C)]
struct Vst3HostApp {
    vtbl: *const FUnknownVtbl,
    refs: AtomicU32,
}

static VST3_HOST_VTBL: FUnknownVtbl = FUnknownVtbl {
    query_interface: vst3_host_qi,
    add_ref: vst3_host_add_ref,
    release: vst3_host_release,
};

unsafe extern "system" fn vst3_host_qi(
    this: *mut FUnknown,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if this.is_null() || iid.is_null() || obj.is_null() {
        return K_NO_INTERFACE;
    }
    if *iid == IID_FUNKNOWN {
        vst3_host_add_ref(this);
        *obj = this as *mut c_void;
        K_RESULT_OK
    } else {
        *obj = ptr::null_mut();
        K_NO_INTERFACE
    }
}

unsafe extern "system" fn vst3_host_add_ref(this: *mut FUnknown) -> u32 {
    let host = &*(this as *const Vst3HostApp);
    host.refs.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn vst3_host_release(this: *mut FUnknown) -> u32 {
    let host = &*(this as *const Vst3HostApp);
    let prev = host.refs.fetch_sub(1, Ordering::AcqRel);
    prev.saturating_sub(1)
}

impl Vst3HostApp {
    fn new_boxed() -> Box<Self> {
        Box::new(Self {
            vtbl: &VST3_HOST_VTBL,
            refs: AtomicU32::new(1),
        })
    }

    fn as_funknown(&mut self) -> *mut FUnknown {
        self as *mut Self as *mut FUnknown
    }
}

/// In-memory COM `IBStream` implementation for VST3 state save/load.
#[repr(C)]
struct MemoryBStream {
    vtbl: *const IBStreamVtbl,
    refs: AtomicU32,
    data: Vec<u8>,
    pos: usize,
}

static MEMORY_BSTREAM_VTBL: IBStreamVtbl = IBStreamVtbl {
    base: FUnknownVtbl {
        query_interface: bstream_qi,
        add_ref: bstream_add_ref,
        release: bstream_release,
    },
    read: bstream_read,
    write: bstream_write,
    seek: bstream_seek,
    tell: bstream_tell,
};

unsafe extern "system" fn bstream_qi(
    this: *mut FUnknown,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if this.is_null() || iid.is_null() || obj.is_null() {
        return K_NO_INTERFACE;
    }
    if *iid == IID_FUNKNOWN || *iid == IID_IBSTREAM {
        bstream_add_ref(this);
        *obj = this as *mut c_void;
        K_RESULT_OK
    } else {
        *obj = ptr::null_mut();
        K_NO_INTERFACE
    }
}

unsafe extern "system" fn bstream_add_ref(this: *mut FUnknown) -> u32 {
    let s = &*(this as *const MemoryBStream);
    s.refs.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn bstream_release(this: *mut FUnknown) -> u32 {
    let s = &*(this as *const MemoryBStream);
    s.refs.fetch_sub(1, Ordering::AcqRel).saturating_sub(1)
}

unsafe extern "system" fn bstream_read(
    this: *mut IBStream,
    buffer: *mut c_void,
    num_bytes: i32,
    num_bytes_read: *mut i32,
) -> TResult {
    if this.is_null() || buffer.is_null() || num_bytes < 0 {
        return K_RESULT_FALSE;
    }
    let s = &mut *(this as *mut MemoryBStream);
    let rem = s.data.len().saturating_sub(s.pos);
    let n = rem.min(num_bytes as usize);
    if n > 0 {
        ptr::copy_nonoverlapping(s.data.as_ptr().add(s.pos), buffer as *mut u8, n);
        s.pos += n;
    }
    if !num_bytes_read.is_null() {
        *num_bytes_read = n as i32;
    }
    K_RESULT_OK
}

unsafe extern "system" fn bstream_write(
    this: *mut IBStream,
    buffer: *mut c_void,
    num_bytes: i32,
    num_bytes_written: *mut i32,
) -> TResult {
    if this.is_null() || buffer.is_null() || num_bytes < 0 {
        return K_RESULT_FALSE;
    }
    let s = &mut *(this as *mut MemoryBStream);
    let n = num_bytes as usize;
    let slice = std::slice::from_raw_parts(buffer as *const u8, n);
    let end = s.pos + n;
    if end > s.data.len() {
        s.data.resize(end, 0);
    }
    s.data[s.pos..end].copy_from_slice(slice);
    s.pos = end;
    if !num_bytes_written.is_null() {
        *num_bytes_written = n as i32;
    }
    K_RESULT_OK
}

unsafe extern "system" fn bstream_seek(
    this: *mut IBStream,
    pos: i64,
    mode: i32,
    result: *mut i64,
) -> TResult {
    if this.is_null() {
        return K_RESULT_FALSE;
    }
    let s = &mut *(this as *mut MemoryBStream);
    let base: i64 = match mode {
        0 => 0,                  // kIBSeekSet
        1 => s.pos as i64,       // kIBSeekCur
        2 => s.data.len() as i64, // kIBSeekEnd
        _ => return K_RESULT_FALSE,
    };
    let next = (base + pos).clamp(0, s.data.len() as i64);
    s.pos = next as usize;
    if !result.is_null() {
        *result = next;
    }
    K_RESULT_OK
}

unsafe extern "system" fn bstream_tell(this: *mut IBStream, pos: *mut i64) -> TResult {
    if this.is_null() || pos.is_null() {
        return K_RESULT_FALSE;
    }
    let s = &*(this as *const MemoryBStream);
    *pos = s.pos as i64;
    K_RESULT_OK
}

impl MemoryBStream {
    fn new(data: Vec<u8>) -> Self {
        Self {
            vtbl: &MEMORY_BSTREAM_VTBL,
            refs: AtomicU32::new(1),
            data,
            pos: 0,
        }
    }

    fn as_ibstream(&mut self) -> *mut IBStream {
        self as *mut Self as *mut IBStream
    }
}

/// Minimal COM `IParamValueQueue` + `IParameterChanges` for VST3 automation updates.
#[repr(C)]
struct SingleParamQueue {
    vtbl: *const IParamValueQueueVtbl,
    refs: AtomicU32,
    param_id: u32,
    value: f64,
}

static PARAM_QUEUE_VTBL: IParamValueQueueVtbl = IParamValueQueueVtbl {
    base: FUnknownVtbl {
        query_interface: pq_qi,
        add_ref: pq_add_ref,
        release: pq_release,
    },
    get_parameter_id: pq_get_id,
    get_point_count: pq_get_count,
    get_point: pq_get_point,
    add_point: pq_add_point,
};

unsafe extern "system" fn pq_qi(
    this: *mut FUnknown,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if this.is_null() || iid.is_null() || obj.is_null() {
        return K_NO_INTERFACE;
    }
    if *iid == IID_FUNKNOWN || *iid == IID_IPARAM_VALUE_QUEUE {
        pq_add_ref(this);
        *obj = this as *mut c_void;
        K_RESULT_OK
    } else {
        *obj = ptr::null_mut();
        K_NO_INTERFACE
    }
}

unsafe extern "system" fn pq_add_ref(this: *mut FUnknown) -> u32 {
    (*(this as *const SingleParamQueue))
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}

unsafe extern "system" fn pq_release(this: *mut FUnknown) -> u32 {
    (*(this as *const SingleParamQueue))
        .refs
        .fetch_sub(1, Ordering::AcqRel)
        .saturating_sub(1)
}

unsafe extern "system" fn pq_get_id(this: *mut IParamValueQueue) -> u32 {
    (*(this as *const SingleParamQueue)).param_id
}

unsafe extern "system" fn pq_get_count(_this: *mut IParamValueQueue) -> i32 {
    1
}

unsafe extern "system" fn pq_get_point(
    this: *mut IParamValueQueue,
    index: i32,
    sample_offset: *mut i32,
    value: *mut f64,
) -> TResult {
    if index != 0 {
        return K_RESULT_FALSE;
    }
    let q = &*(this as *const SingleParamQueue);
    if !sample_offset.is_null() {
        *sample_offset = 0;
    }
    if !value.is_null() {
        *value = q.value;
    }
    K_RESULT_OK
}

unsafe extern "system" fn pq_add_point(
    this: *mut IParamValueQueue,
    _sample_offset: i32,
    value: f64,
    index: *mut i32,
) -> TResult {
    let q = &mut *(this as *mut SingleParamQueue);
    q.value = value;
    if !index.is_null() {
        *index = 0;
    }
    K_RESULT_OK
}

#[repr(C)]
struct BatchParameterChanges {
    vtbl: *const IParameterChangesVtbl,
    refs: AtomicU32,
    queues: Vec<Box<SingleParamQueue>>,
}

static PARAM_CHANGES_VTBL: IParameterChangesVtbl = IParameterChangesVtbl {
    base: FUnknownVtbl {
        query_interface: pc_qi,
        add_ref: pc_add_ref,
        release: pc_release,
    },
    get_parameter_count: pc_get_count,
    get_parameter_data: pc_get_data,
    add_parameter_data: pc_add_data,
};

unsafe extern "system" fn pc_qi(
    this: *mut FUnknown,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    if this.is_null() || iid.is_null() || obj.is_null() {
        return K_NO_INTERFACE;
    }
    if *iid == IID_FUNKNOWN || *iid == IID_IPARAMETER_CHANGES {
        pc_add_ref(this);
        *obj = this as *mut c_void;
        K_RESULT_OK
    } else {
        *obj = ptr::null_mut();
        K_NO_INTERFACE
    }
}

unsafe extern "system" fn pc_add_ref(this: *mut FUnknown) -> u32 {
    (*(this as *const BatchParameterChanges))
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}

unsafe extern "system" fn pc_release(this: *mut FUnknown) -> u32 {
    (*(this as *const BatchParameterChanges))
        .refs
        .fetch_sub(1, Ordering::AcqRel)
        .saturating_sub(1)
}

unsafe extern "system" fn pc_get_count(this: *mut IParameterChanges) -> i32 {
    (*(this as *const BatchParameterChanges)).queues.len() as i32
}

unsafe extern "system" fn pc_get_data(
    this: *mut IParameterChanges,
    index: i32,
) -> *mut IParamValueQueue {
    let pc = &mut *(this as *mut BatchParameterChanges);
    match pc.queues.get_mut(index as usize) {
        Some(q) => (&mut **q) as *mut SingleParamQueue as *mut IParamValueQueue,
        None => ptr::null_mut(),
    }
}

unsafe extern "system" fn pc_add_data(
    this: *mut IParameterChanges,
    id: *const u32,
    index: *mut i32,
) -> *mut IParamValueQueue {
    if id.is_null() {
        return ptr::null_mut();
    }
    let pc = &mut *(this as *mut BatchParameterChanges);
    let idx = pc.queues.len();
    pc.queues.push(Box::new(SingleParamQueue {
        vtbl: &PARAM_QUEUE_VTBL,
        refs: AtomicU32::new(1),
        param_id: *id,
        value: 0.0,
    }));
    if !index.is_null() {
        *index = idx as i32;
    }
    (&mut *pc.queues[idx]) as *mut SingleParamQueue as *mut IParamValueQueue
}

impl BatchParameterChanges {
    fn new() -> Self {
        Self {
            vtbl: &PARAM_CHANGES_VTBL,
            refs: AtomicU32::new(1),
            queues: Vec::new(),
        }
    }

    fn set_changes(&mut self, changes: &[(u32, f64)]) {
        self.queues.clear();
        for &(id, value) in changes {
            self.queues.push(Box::new(SingleParamQueue {
                vtbl: &PARAM_QUEUE_VTBL,
                refs: AtomicU32::new(1),
                param_id: id,
                value,
            }));
        }
    }

    fn as_ptr(&mut self) -> *mut IParameterChanges {
        self as *mut Self as *mut IParameterChanges
    }
}

// ------------------------------------------------------------------ Plugin Processor Trait

/// Unified stereo DSP processor trait implemented by dynamic CLAP plugins, VST3 plugins,
/// and deterministic test processors.
pub trait AudioPluginProcessor: Send {
    fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32]);
    fn set_param(&mut self, param_id: u32, value: f32) -> Result<(), String>;
    fn get_params(&self) -> Vec<AudioPluginParam>;
    fn save_state_b64(&self) -> Option<String>;
    fn load_state_b64(&mut self, b64: &str) -> Result<(), String>;
    fn open_gui(&mut self) -> Result<(), String>;
    fn has_gui(&self) -> bool {
        false
    }
}

// ------------------------------------------------------------------ Dynamic CLAP Instance

struct ClapPluginInstance {
    plugin: *const ClapPlugin,
    entry: *const ClapPluginEntry,
    params_ext: *const ClapPluginParams,
    state_ext: *const ClapPluginState,
    gui_ext: *const ClapPluginGui,
    param_cookies: HashMap<u32, *mut c_void>,
    param_ranges: HashMap<u32, (f64, f64)>,
    pending_events: Vec<ClapEventParamValue>,
    in_buf_l: Vec<f32>,
    in_buf_r: Vec<f32>,
    steady_time: i64,
    active: bool,
    processing: bool,
    gui_created: bool,
    name: String,
    // Kept alive strictly longer than `plugin` (dropped after `Drop::drop` runs)
    _host: Box<ClapHost>,
    _library: Library,
}

// SAFETY: Every `ClapPluginInstance` is exclusively owned by a `PluginChain` inside
// `AudioMixerEngine` (behind its mutex) and is never shared across threads concurrently.
unsafe impl Send for ClapPluginInstance {}

impl ClapPluginInstance {
    fn load(
        binary_path: &Path,
        plugin_uid: &str,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Self, String> {
        let lib = unsafe { Library::new(binary_path) }
            .map_err(|e| format!("Falha ao carregar biblioteca CLAP '{}': {e}", binary_path.display()))?;

        let entry_ptr: *const ClapPluginEntry = unsafe {
            let sym: libloading::Symbol<*const ClapPluginEntry> = lib
                .get(b"clap_entry\0")
                .map_err(|e| format!("Símbolo 'clap_entry' ausente em '{}': {e}", binary_path.display()))?;
            *sym
        };
        if entry_ptr.is_null() {
            return Err("clap_entry retornou ponteiro nulo.".to_string());
        }

        let path_c = CString::new(binary_path.to_string_lossy().as_bytes())
            .map_err(|_| "Caminho do plugin contém byte nulo.".to_string())?;

        unsafe {
            let init_fn = (*entry_ptr)
                .init
                .ok_or_else(|| "clap_entry.init ausente.".to_string())?;
            if !init_fn(path_c.as_ptr()) {
                return Err("clap_entry.init retornou false.".to_string());
            }

            let get_factory = (*entry_ptr)
                .get_factory
                .ok_or_else(|| "clap_entry.get_factory ausente.".to_string())?;
            let factory = get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr() as *const c_char)
                as *const ClapPluginFactory;
            if factory.is_null() {
                if let Some(deinit) = (*entry_ptr).deinit {
                    deinit();
                }
                return Err("CLAP plugin factory indisponível.".to_string());
            }

            let count = (*factory)
                .get_plugin_count
                .map(|f| f(factory))
                .unwrap_or(0);
            if count == 0 {
                if let Some(deinit) = (*entry_ptr).deinit {
                    deinit();
                }
                return Err("Arquivo CLAP não exporta nenhum plugin.".to_string());
            }

            let mut target_id: Option<CString> = None;
            let mut target_name = file_stem_name(binary_path);

            for idx in 0..count {
                let desc = (*factory)
                    .get_plugin_descriptor
                    .map(|f| f(factory, idx))
                    .unwrap_or(ptr::null());
                if desc.is_null() {
                    continue;
                }
                let id_str = c_ptr_to_string((*desc).id);
                let name_str = c_ptr_to_string((*desc).name);
                if plugin_uid.is_empty() || id_str == plugin_uid {
                    target_id = CString::new(id_str).ok();
                    if !name_str.is_empty() {
                        target_name = name_str;
                    }
                    break;
                }
                if target_id.is_none() {
                    target_id = CString::new(id_str).ok();
                    if !name_str.is_empty() {
                        target_name = name_str;
                    }
                }
            }

            let Some(plugin_id_c) = target_id else {
                if let Some(deinit) = (*entry_ptr).deinit {
                    deinit();
                }
                return Err("Nenhum descritor CLAP válido encontrado.".to_string());
            };

            let host = make_clap_host();
            let create_fn = (*factory)
                .create_plugin
                .ok_or_else(|| "clap_plugin_factory.create_plugin ausente.".to_string())?;
            let plugin = create_fn(factory, &*host as *const ClapHost, plugin_id_c.as_ptr());
            if plugin.is_null() {
                if let Some(deinit) = (*entry_ptr).deinit {
                    deinit();
                }
                return Err("Falha ao instanciar plugin CLAP.".to_string());
            }

            let ok_init = (*plugin).init.map(|f| f(plugin)).unwrap_or(false);
            if !ok_init {
                if let Some(destroy) = (*plugin).destroy {
                    destroy(plugin);
                }
                if let Some(deinit) = (*entry_ptr).deinit {
                    deinit();
                }
                return Err("clap_plugin.init falhou.".to_string());
            }

            let get_ext = (*plugin).get_extension;
            let params_ext = get_ext
                .map(|f| f(plugin, CLAP_EXT_PARAMS.as_ptr() as *const c_char) as *const ClapPluginParams)
                .unwrap_or(ptr::null());
            let state_ext = get_ext
                .map(|f| f(plugin, CLAP_EXT_STATE.as_ptr() as *const c_char) as *const ClapPluginState)
                .unwrap_or(ptr::null());
            let gui_ext = get_ext
                .map(|f| f(plugin, CLAP_EXT_GUI.as_ptr() as *const c_char) as *const ClapPluginGui)
                .unwrap_or(ptr::null());

            let mut param_cookies = HashMap::new();
            let mut param_ranges = HashMap::new();
            if !params_ext.is_null() {
                if let (Some(cnt_fn), Some(info_fn)) = ((*params_ext).count, (*params_ext).get_info) {
                    let pcount = cnt_fn(plugin);
                    for i in 0..pcount {
                        let mut info: ClapParamInfo = std::mem::zeroed();
                        if info_fn(plugin, i, &mut info) {
                            param_cookies.insert(info.id, info.cookie);
                            param_ranges.insert(info.id, (info.min_value, info.max_value));
                        }
                    }
                }
            }

            let max_f = max_frames.max(64) as u32;
            let active = (*plugin)
                .activate
                .map(|f| f(plugin, sample_rate as f64, 1, max_f))
                .unwrap_or(false);
            if !active {
                if let Some(destroy) = (*plugin).destroy {
                    destroy(plugin);
                }
                if let Some(deinit) = (*entry_ptr).deinit {
                    deinit();
                }
                return Err("clap_plugin.activate falhou.".to_string());
            }

            let processing = (*plugin)
                .start_processing
                .map(|f| f(plugin))
                .unwrap_or(true);

            Ok(Self {
                plugin,
                entry: entry_ptr,
                params_ext,
                state_ext,
                gui_ext,
                param_cookies,
                param_ranges,
                pending_events: Vec::new(),
                in_buf_l: vec![0.0; max_frames.max(64)],
                in_buf_r: vec![0.0; max_frames.max(64)],
                steady_time: 0,
                active,
                processing,
                gui_created: false,
                name: target_name,
                _host: host,
                _library: lib,
            })
        }
    }

    fn native_gui_api() -> Option<&'static [u8]> {
        #[cfg(windows)]
        {
            Some(b"win32\0")
        }
        #[cfg(target_os = "macos")]
        {
            Some(b"cocoa\0")
        }
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("DISPLAY").is_some() {
                Some(b"x11\0")
            } else {
                None
            }
        }
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        {
            None
        }
    }
}

impl Drop for ClapPluginInstance {
    fn drop(&mut self) {
        unsafe {
            if self.gui_created && !self.gui_ext.is_null() {
                if let Some(hide) = (*self.gui_ext).hide {
                    hide(self.plugin);
                }
                if let Some(destroy_gui) = (*self.gui_ext).destroy {
                    destroy_gui(self.plugin);
                }
                self.gui_created = false;
            }
            if !self.plugin.is_null() {
                if self.processing {
                    if let Some(stop) = (*self.plugin).stop_processing {
                        stop(self.plugin);
                    }
                    self.processing = false;
                }
                if self.active {
                    if let Some(deact) = (*self.plugin).deactivate {
                        deact(self.plugin);
                    }
                    self.active = false;
                }
                if let Some(destroy) = (*self.plugin).destroy {
                    destroy(self.plugin);
                }
                self.plugin = ptr::null();
            }
            if !self.entry.is_null() {
                if let Some(deinit) = (*self.entry).deinit {
                    deinit();
                }
                self.entry = ptr::null();
            }
        }
    }
}

impl AudioPluginProcessor for ClapPluginInstance {
    fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        if frames == 0 || self.plugin.is_null() {
            return;
        }
        if self.in_buf_l.len() < frames {
            self.in_buf_l.resize(frames, 0.0);
            self.in_buf_r.resize(frames, 0.0);
        }
        self.in_buf_l[..frames].copy_from_slice(&left[..frames]);
        self.in_buf_r[..frames].copy_from_slice(&right[..frames]);

        let mut in_ptrs = [self.in_buf_l.as_mut_ptr(), self.in_buf_r.as_mut_ptr()];
        let mut out_ptrs = [left.as_mut_ptr(), right.as_mut_ptr()];

        let in_buf = ClapAudioBuffer {
            data32: in_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut out_buf = ClapAudioBuffer {
            data32: out_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };

        let in_events = ClapInputEvents {
            ctx: &self.pending_events as *const Vec<ClapEventParamValue> as *mut c_void,
            size: Some(clap_in_events_size),
            get: Some(clap_in_events_get),
        };
        let out_events = ClapOutputEvents {
            ctx: ptr::null_mut(),
            try_push: Some(clap_out_events_try_push),
        };

        let process = ClapProcess {
            steady_time: self.steady_time,
            frames_count: frames as u32,
            transport: ptr::null(),
            audio_inputs: &in_buf,
            audio_outputs: &mut out_buf,
            audio_inputs_count: 1,
            audio_outputs_count: 1,
            in_events: &in_events,
            out_events: &out_events,
        };

        unsafe {
            if let Some(proc_fn) = (*self.plugin).process {
                let status = proc_fn(self.plugin, &process);
                if status == CLAP_PROCESS_ERROR {
                    left[..frames].copy_from_slice(&self.in_buf_l[..frames]);
                    right[..frames].copy_from_slice(&self.in_buf_r[..frames]);
                }
            }
        }
        self.pending_events.clear();
        self.steady_time = self.steady_time.wrapping_add(frames as i64);
    }

    fn set_param(&mut self, param_id: u32, value: f32) -> Result<(), String> {
        let mut val = value as f64;
        if let Some(&(min, max)) = self.param_ranges.get(&param_id) {
            if min <= max {
                val = val.clamp(min, max);
            }
        }
        let cookie = self
            .param_cookies
            .get(&param_id)
            .copied()
            .unwrap_or(ptr::null_mut());
        self.pending_events.push(ClapEventParamValue {
            header: ClapEventHeader {
                size: std::mem::size_of::<ClapEventParamValue>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                event_type: CLAP_EVENT_PARAM_VALUE,
                flags: 0,
            },
            param_id,
            cookie,
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value: val,
        });
        Ok(())
    }

    fn get_params(&self) -> Vec<AudioPluginParam> {
        let mut out = Vec::new();
        if self.plugin.is_null() || self.params_ext.is_null() {
            return out;
        }
        unsafe {
            let (Some(cnt_fn), Some(info_fn)) = ((*self.params_ext).count, (*self.params_ext).get_info)
            else {
                return out;
            };
            let get_val = (*self.params_ext).get_value;
            let count = cnt_fn(self.plugin);
            for i in 0..count {
                let mut info: ClapParamInfo = std::mem::zeroed();
                if !info_fn(self.plugin, i, &mut info) {
                    continue;
                }
                let mut cur = info.default_value;
                if let Some(gv) = get_val {
                    let _ = gv(self.plugin, info.id, &mut cur);
                }
                out.push(AudioPluginParam {
                    id: info.id,
                    name: c_char_array_to_string(&info.name),
                    value: cur as f32,
                    min: info.min_value as f32,
                    max: info.max_value as f32,
                    default_value: info.default_value as f32,
                });
            }
        }
        out
    }

    fn save_state_b64(&self) -> Option<String> {
        if self.plugin.is_null() || self.state_ext.is_null() {
            return None;
        }
        unsafe {
            let save_fn = (*self.state_ext).save?;
            let mut bytes = Vec::new();
            let stream = ClapOStream {
                ctx: &mut bytes as *mut Vec<u8> as *mut c_void,
                write: Some(clap_ostream_write),
            };
            if save_fn(self.plugin, &stream) {
                Some(BASE64_STANDARD.encode(&bytes))
            } else {
                None
            }
        }
    }

    fn load_state_b64(&mut self, b64: &str) -> Result<(), String> {
        if self.plugin.is_null() || self.state_ext.is_null() {
            return Err("Plugin CLAP não suporta extensão clap.state.".to_string());
        }
        let bytes = BASE64_STANDARD
            .decode(b64)
            .map_err(|e| format!("Estado Base64 inválido: {e}"))?;
        unsafe {
            let load_fn = (*self.state_ext)
                .load
                .ok_or_else(|| "clap_plugin_state.load ausente.".to_string())?;
            let mut cursor = ClapReadCursor {
                data: &bytes,
                pos: 0,
            };
            let stream = ClapIStream {
                ctx: &mut cursor as *mut ClapReadCursor<'_> as *mut c_void,
                read: Some(clap_istream_read),
            };
            if load_fn(self.plugin, &stream) {
                Ok(())
            } else {
                Err("Plugin CLAP rejeitou a restauração de estado.".to_string())
            }
        }
    }

    fn open_gui(&mut self) -> Result<(), String> {
        if self.plugin.is_null() || self.gui_ext.is_null() {
            return Err(format!("Plugin '{}' não possui interface gráfica (headless).", self.name));
        }
        let Some(api) = Self::native_gui_api() else {
            return Err(
                "Ambiente sem display gráfico ativo ($DISPLAY indisponível) para abrir GUI CLAP."
                    .to_string(),
            );
        };
        let api_ptr = api.as_ptr() as *const c_char;
        unsafe {
            let gui = &*self.gui_ext;
            let supports_floating = gui
                .is_api_supported
                .map(|f| f(self.plugin, api_ptr, true))
                .unwrap_or(false);
            if !supports_floating {
                let supports_embedded = gui
                    .is_api_supported
                    .map(|f| f(self.plugin, api_ptr, false))
                    .unwrap_or(false);
                if supports_embedded {
                    return Err(format!(
                        "Plugin '{}' suporta apenas GUI embutida; use os controles de parâmetros no PrismCast.",
                        self.name
                    ));
                }
                return Err(format!(
                    "Plugin '{}' não suporta GUI nativa nesta plataforma.",
                    self.name
                ));
            }

            if !self.gui_created {
                let created = gui
                    .create
                    .map(|f| f(self.plugin, api_ptr, true))
                    .unwrap_or(false);
                if !created {
                    return Err(format!("Falha ao criar janela GUI flutuante para '{}'.", self.name));
                }
                self.gui_created = true;
                if let (Some(suggest), Ok(title)) = (gui.suggest_title, CString::new(self.name.as_str())) {
                    suggest(self.plugin, title.as_ptr());
                }
            }

            let shown = gui.show.map(|f| f(self.plugin)).unwrap_or(false);
            if shown {
                Ok(())
            } else {
                Err(format!("Falha ao exibir janela GUI de '{}'.", self.name))
            }
        }
    }

    fn has_gui(&self) -> bool {
        !self.gui_ext.is_null()
    }
}

// ------------------------------------------------------------------ Dynamic VST3 Instance

#[allow(dead_code)]
enum Vst3ModuleExitKind {
    None,
    LinuxModuleExit,
    WindowsExitDll,
    MacBundleExit,
}

struct Vst3PluginInstance {
    factory: *mut IPluginFactory,
    component: *mut IComponent,
    processor: *mut IAudioProcessor,
    controller: *mut IEditController,
    split_controller: bool,
    active: bool,
    processing: bool,
    exit_kind: Vst3ModuleExitKind,
    pending_params: Vec<(u32, f64)>,
    param_changes: BatchParameterChanges,
    in_buf_l: Vec<f32>,
    in_buf_r: Vec<f32>,
    name: String,
    // Kept alive strictly longer than all COM pointers above
    _host_app: Box<Vst3HostApp>,
    _library: Library,
}

// SAFETY: Exclusively owned by its channel's `PluginChain` under `AudioMixerEngine` mutex.
unsafe impl Send for Vst3PluginInstance {}

impl Vst3PluginInstance {
    unsafe fn init_module(lib: &Library) -> Result<Vst3ModuleExitKind, String> {
        #[cfg(target_os = "linux")]
        {
            if let Ok(entry) =
                lib.get::<unsafe extern "C" fn(*mut c_void) -> bool>(b"ModuleEntry\0")
            {
                if !entry(ptr::null_mut()) {
                    return Err("VST3 ModuleEntry retornou false.".to_string());
                }
                return Ok(Vst3ModuleExitKind::LinuxModuleExit);
            }
        }
        #[cfg(windows)]
        {
            if let Ok(init_dll) = lib.get::<unsafe extern "C" fn() -> bool>(b"InitDll\0") {
                if !init_dll() {
                    return Err("VST3 InitDll retornou false.".to_string());
                }
                return Ok(Vst3ModuleExitKind::WindowsExitDll);
            }
        }
        #[cfg(target_os = "macos")]
        {
            if let Ok(bundle_entry) =
                lib.get::<unsafe extern "C" fn(*mut c_void) -> bool>(b"bundleEntry\0")
            {
                if !bundle_entry(ptr::null_mut()) {
                    return Err("VST3 bundleEntry retornou false.".to_string());
                }
                return Ok(Vst3ModuleExitKind::MacBundleExit);
            }
        }
        Ok(Vst3ModuleExitKind::None)
    }

    unsafe fn exit_module(lib: &Library, kind: &Vst3ModuleExitKind) {
        match kind {
            Vst3ModuleExitKind::None => {}
            Vst3ModuleExitKind::LinuxModuleExit => {
                if let Ok(exit) = lib.get::<unsafe extern "C" fn() -> bool>(b"ModuleExit\0") {
                    let _ = exit();
                }
            }
            Vst3ModuleExitKind::WindowsExitDll => {
                if let Ok(exit) = lib.get::<unsafe extern "C" fn() -> bool>(b"ExitDll\0") {
                    let _ = exit();
                }
            }
            Vst3ModuleExitKind::MacBundleExit => {
                if let Ok(exit) = lib.get::<unsafe extern "C" fn() -> bool>(b"bundleExit\0") {
                    let _ = exit();
                }
            }
        }
    }

    fn load(
        binary_path: &Path,
        plugin_uid: &str,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Self, String> {
        let lib = unsafe { Library::new(binary_path) }.map_err(|e| {
            format!(
                "Falha ao carregar biblioteca VST3 '{}': {e}",
                binary_path.display()
            )
        })?;

        unsafe {
            let exit_kind = Self::init_module(&lib)?;

            let get_factory: libloading::Symbol<unsafe extern "system" fn() -> *mut IPluginFactory> =
                match lib.get(b"GetPluginFactory\0") {
                    Ok(s) => s,
                    Err(e) => {
                        Self::exit_module(&lib, &exit_kind);
                        return Err(format!("Símbolo 'GetPluginFactory' ausente: {e}"));
                    }
                };

            let factory = get_factory();
            if factory.is_null() || (*factory).vtbl.is_null() {
                Self::exit_module(&lib, &exit_kind);
                return Err("GetPluginFactory retornou ponteiro nulo.".to_string());
            }

            let count = ((*(*factory).vtbl).count_classes)(factory);
            let wanted_cid = parse_tuid(plugin_uid);

            let mut chosen_cid: Option<Tuid> = None;
            let mut chosen_name = file_stem_name(binary_path);

            for i in 0..count {
                let mut info: PClassInfo = std::mem::zeroed();
                if ((*(*factory).vtbl).get_class_info)(factory, i, &mut info) != K_RESULT_OK {
                    continue;
                }
                let category = c_char_array_to_string(&info.category);
                if category != "Audio Module Class" {
                    continue;
                }
                let name = c_char_array_to_string(&info.name);
                if wanted_cid.is_none() || wanted_cid == Some(info.cid) {
                    chosen_cid = Some(info.cid);
                    if !name.is_empty() {
                        chosen_name = name;
                    }
                    break;
                }
                if chosen_cid.is_none() {
                    chosen_cid = Some(info.cid);
                    if !name.is_empty() {
                        chosen_name = name;
                    }
                }
            }

            let Some(cid) = chosen_cid else {
                ((*(*factory).vtbl).base.release)(factory as *mut FUnknown);
                Self::exit_module(&lib, &exit_kind);
                return Err("Nenhuma classe 'Audio Module Class' encontrada no VST3.".to_string());
            };

            let mut comp_void: *mut c_void = ptr::null_mut();
            let res = ((*(*factory).vtbl).create_instance)(
                factory,
                cid.as_ptr() as *const c_char,
                IID_ICOMPONENT.as_ptr() as *const c_char,
                &mut comp_void,
            );
            if res != K_RESULT_OK || comp_void.is_null() {
                ((*(*factory).vtbl).base.release)(factory as *mut FUnknown);
                Self::exit_module(&lib, &exit_kind);
                return Err(format!("Falha ao instanciar IComponent VST3 (tresult={res})."));
            }
            let component = comp_void as *mut IComponent;

            let mut host_app = Vst3HostApp::new_boxed();
            let init_res =
                ((*(*component).vtbl).base.initialize)(component as *mut IPluginBase, host_app.as_funknown());
            if init_res != K_RESULT_OK {
                ((*(*component).vtbl).base.base.release)(component as *mut FUnknown);
                ((*(*factory).vtbl).base.release)(factory as *mut FUnknown);
                Self::exit_module(&lib, &exit_kind);
                return Err(format!("IComponent::initialize falhou (tresult={init_res})."));
            }

            // Query IAudioProcessor
            let mut proc_void: *mut c_void = ptr::null_mut();
            let q_proc = ((*(*component).vtbl).base.base.query_interface)(
                component as *mut FUnknown,
                &IID_IAUDIO_PROCESSOR,
                &mut proc_void,
            );
            if q_proc != K_RESULT_OK || proc_void.is_null() {
                let _ = ((*(*component).vtbl).base.terminate)(component as *mut IPluginBase);
                ((*(*component).vtbl).base.base.release)(component as *mut FUnknown);
                ((*(*factory).vtbl).base.release)(factory as *mut FUnknown);
                Self::exit_module(&lib, &exit_kind);
                return Err("Componente VST3 não implementa IAudioProcessor.".to_string());
            }
            let processor = proc_void as *mut IAudioProcessor;

            // Query optional IEditController (single-component or split-component)
            let mut controller: *mut IEditController = ptr::null_mut();
            let mut split_controller = false;
            let mut ctrl_void: *mut c_void = ptr::null_mut();
            if ((*(*component).vtbl).base.base.query_interface)(
                component as *mut FUnknown,
                &IID_IEDIT_CONTROLLER,
                &mut ctrl_void,
            ) == K_RESULT_OK
                && !ctrl_void.is_null()
            {
                controller = ctrl_void as *mut IEditController;
            } else {
                let mut ctrl_cid: Tuid = [0u8; 16];
                if ((*(*component).vtbl).get_controller_class_id)(component, &mut ctrl_cid)
                    == K_RESULT_OK
                {
                    let mut split_void: *mut c_void = ptr::null_mut();
                    if ((*(*factory).vtbl).create_instance)(
                        factory,
                        ctrl_cid.as_ptr() as *const c_char,
                        IID_IEDIT_CONTROLLER.as_ptr() as *const c_char,
                        &mut split_void,
                    ) == K_RESULT_OK
                        && !split_void.is_null()
                    {
                        let ctrl = split_void as *mut IEditController;
                        if ((*(*ctrl).vtbl).base.initialize)(
                            ctrl as *mut IPluginBase,
                            host_app.as_funknown(),
                        ) == K_RESULT_OK
                        {
                            controller = ctrl;
                            split_controller = true;
                        } else {
                            ((*(*ctrl).vtbl).base.base.release)(ctrl as *mut FUnknown);
                        }
                    }
                }
            }

            // Configure stereo f32 processing
            let mut in_arr = [VST3_SPEAKER_STEREO];
            let mut out_arr = [VST3_SPEAKER_STEREO];
            let _ = ((*(*processor).vtbl).set_bus_arrangements)(
                processor,
                in_arr.as_mut_ptr(),
                1,
                out_arr.as_mut_ptr(),
                1,
            );

            let mut setup = ProcessSetup {
                process_mode: VST3_REALTIME,
                symbolic_sample_size: VST3_SAMPLE_32,
                max_samples_per_block: max_frames.max(64) as i32,
                sample_rate: sample_rate as f64,
            };
            let _ = ((*(*processor).vtbl).setup_processing)(processor, &mut setup);

            let _ = ((*(*component).vtbl).activate_bus)(
                component,
                VST3_MEDIA_AUDIO,
                VST3_BUS_INPUT,
                0,
                1,
            );
            let _ = ((*(*component).vtbl).activate_bus)(
                component,
                VST3_MEDIA_AUDIO,
                VST3_BUS_OUTPUT,
                0,
                1,
            );

            let active = ((*(*component).vtbl).set_active)(component, 1) == K_RESULT_OK;
            let processing = ((*(*processor).vtbl).set_processing)(processor, 1) == K_RESULT_OK;

            Ok(Self {
                factory,
                component,
                processor,
                controller,
                split_controller,
                active,
                processing,
                exit_kind,
                pending_params: Vec::new(),
                param_changes: BatchParameterChanges::new(),
                in_buf_l: vec![0.0; max_frames.max(64)],
                in_buf_r: vec![0.0; max_frames.max(64)],
                name: chosen_name,
                _host_app: host_app,
                _library: lib,
            })
        }
    }
}

impl Drop for Vst3PluginInstance {
    fn drop(&mut self) {
        unsafe {
            if !self.processor.is_null() && self.processing {
                let _ = ((*(*self.processor).vtbl).set_processing)(self.processor, 0);
                self.processing = false;
            }
            if !self.component.is_null() && self.active {
                let _ = ((*(*self.component).vtbl).set_active)(self.component, 0);
                self.active = false;
            }
            if !self.processor.is_null() {
                ((*(*self.processor).vtbl).base.release)(self.processor as *mut FUnknown);
                self.processor = ptr::null_mut();
            }
            if !self.controller.is_null() {
                if self.split_controller {
                    let _ = ((*(*self.controller).vtbl).base.terminate)(
                        self.controller as *mut IPluginBase,
                    );
                }
                ((*(*self.controller).vtbl).base.base.release)(self.controller as *mut FUnknown);
                self.controller = ptr::null_mut();
            }
            if !self.component.is_null() {
                let _ =
                    ((*(*self.component).vtbl).base.terminate)(self.component as *mut IPluginBase);
                ((*(*self.component).vtbl).base.base.release)(self.component as *mut FUnknown);
                self.component = ptr::null_mut();
            }
            if !self.factory.is_null() {
                ((*(*self.factory).vtbl).base.release)(self.factory as *mut FUnknown);
                self.factory = ptr::null_mut();
            }
            Self::exit_module(&self._library, &self.exit_kind);
        }
    }
}

impl AudioPluginProcessor for Vst3PluginInstance {
    fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        if frames == 0 || self.processor.is_null() {
            return;
        }
        if self.in_buf_l.len() < frames {
            self.in_buf_l.resize(frames, 0.0);
            self.in_buf_r.resize(frames, 0.0);
        }
        self.in_buf_l[..frames].copy_from_slice(&left[..frames]);
        self.in_buf_r[..frames].copy_from_slice(&right[..frames]);

        let mut in_ptrs = [self.in_buf_l.as_mut_ptr(), self.in_buf_r.as_mut_ptr()];
        let mut out_ptrs = [left.as_mut_ptr(), right.as_mut_ptr()];

        let mut in_bus = AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            channel_buffers_32: in_ptrs.as_mut_ptr(),
        };
        let mut out_bus = AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            channel_buffers_32: out_ptrs.as_mut_ptr(),
        };

        let in_param_ptr = if self.pending_params.is_empty() {
            ptr::null_mut()
        } else {
            self.param_changes.set_changes(&self.pending_params);
            self.param_changes.as_ptr()
        };

        let mut data = ProcessData {
            process_mode: VST3_REALTIME,
            symbolic_sample_size: VST3_SAMPLE_32,
            num_samples: frames as i32,
            num_inputs: 1,
            num_outputs: 1,
            inputs: &mut in_bus,
            outputs: &mut out_bus,
            input_parameter_changes: in_param_ptr,
            output_parameter_changes: ptr::null_mut(),
            input_events: ptr::null_mut(),
            output_events: ptr::null_mut(),
            process_context: ptr::null_mut(),
        };

        unsafe {
            let res = ((*(*self.processor).vtbl).process)(self.processor, &mut data);
            if res != K_RESULT_OK {
                left[..frames].copy_from_slice(&self.in_buf_l[..frames]);
                right[..frames].copy_from_slice(&self.in_buf_r[..frames]);
            }
        }
        self.pending_params.clear();
    }

    fn set_param(&mut self, param_id: u32, value: f32) -> Result<(), String> {
        let norm = (value as f64).clamp(0.0, 1.0);
        if !self.controller.is_null() {
            unsafe {
                let _ = ((*(*self.controller).vtbl).set_param_normalized)(
                    self.controller,
                    param_id,
                    norm,
                );
            }
        }
        self.pending_params.push((param_id, norm));
        Ok(())
    }

    fn get_params(&self) -> Vec<AudioPluginParam> {
        let mut out = Vec::new();
        if self.controller.is_null() {
            return out;
        }
        unsafe {
            let count = ((*(*self.controller).vtbl).get_parameter_count)(self.controller);
            for i in 0..count {
                let mut info: Vst3ParameterInfo = std::mem::zeroed();
                if ((*(*self.controller).vtbl).get_parameter_info)(
                    self.controller,
                    i,
                    &mut info,
                ) != K_RESULT_OK
                {
                    continue;
                }
                let cur =
                    ((*(*self.controller).vtbl).get_param_normalized)(self.controller, info.id);
                out.push(AudioPluginParam {
                    id: info.id,
                    name: utf16_array_to_string(&info.title),
                    value: cur as f32,
                    min: 0.0,
                    max: 1.0,
                    default_value: info.default_normalized_value as f32,
                });
            }
        }
        out
    }

    fn save_state_b64(&self) -> Option<String> {
        if self.component.is_null() {
            return None;
        }
        unsafe {
            let mut stream = MemoryBStream::new(Vec::new());
            if ((*(*self.component).vtbl).get_state)(self.component, stream.as_ibstream())
                == K_RESULT_OK
            {
                Some(BASE64_STANDARD.encode(&stream.data))
            } else {
                None
            }
        }
    }

    fn load_state_b64(&mut self, b64: &str) -> Result<(), String> {
        if self.component.is_null() {
            return Err("IComponent VST3 indisponível.".to_string());
        }
        let bytes = BASE64_STANDARD
            .decode(b64)
            .map_err(|e| format!("Estado Base64 VST3 inválido: {e}"))?;
        unsafe {
            let mut stream = MemoryBStream::new(bytes.clone());
            let res = ((*(*self.component).vtbl).set_state)(self.component, stream.as_ibstream());
            if res != K_RESULT_OK {
                return Err(format!("IComponent::setState falhou (tresult={res})."));
            }
            if !self.controller.is_null() {
                let mut ctrl_stream = MemoryBStream::new(bytes);
                let _ = ((*(*self.controller).vtbl).set_component_state)(
                    self.controller,
                    ctrl_stream.as_ibstream(),
                );
            }
        }
        Ok(())
    }

    fn open_gui(&mut self) -> Result<(), String> {
        if self.controller.is_null() {
            return Err(format!(
                "Plugin VST3 '{}' não expõe IEditController (headless).",
                self.name
            ));
        }
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none()
            {
                return Err(
                    "Ambiente Linux sem display gráfico ativo para abrir view VST3.".to_string(),
                );
            }
        }
        unsafe {
            let view = ((*(*self.controller).vtbl).create_view)(
                self.controller,
                b"editor\0".as_ptr() as *const c_char,
            );
            if view.is_null() {
                return Err(format!(
                    "Plugin VST3 '{}' não possui interface gráfica (createView retornou nulo).",
                    self.name
                ));
            }

            #[cfg(windows)]
            let platform_type = b"HWND\0".as_ptr() as *const c_char;
            #[cfg(target_os = "macos")]
            let platform_type = b"NSView\0".as_ptr() as *const c_char;
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let platform_type = b"X11EmbedWindowID\0".as_ptr() as *const c_char;

            let supported =
                ((*(*view).vtbl).is_platform_type_supported)(view, platform_type) == K_RESULT_OK;
            let mut rect = ViewRect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            let _ = ((*(*view).vtbl).get_size)(view, &mut rect);
            ((*(*view).vtbl).base.release)(view as *mut FUnknown);

            if !supported {
                return Err(format!(
                    "Plugin VST3 '{}' não suporta o tipo de janela desta plataforma.",
                    self.name
                ));
            }
            let w = (rect.right - rect.left).max(0);
            let h = (rect.bottom - rect.top).max(0);
            Err(format!(
                "View VST3 '{}' detectada ({w}x{h}), mas requer janela host embutida; ajuste os parâmetros diretamente no mixer.",
                self.name
            ))
        }
    }

    fn has_gui(&self) -> bool {
        if self.controller.is_null() {
            return false;
        }
        unsafe {
            let view = ((*(*self.controller).vtbl).create_view)(
                self.controller,
                b"editor\0".as_ptr() as *const c_char,
            );
            if view.is_null() {
                false
            } else {
                ((*(*view).vtbl).base.release)(view as *mut FUnknown);
                true
            }
        }
    }
}

// ------------------------------------------------------------------ Plugin Scanner & Probing

/// Standard VST3 search directories for the current OS.
pub fn default_vst3_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        if let Ok(common) = std::env::var("CommonProgramFiles") {
            dirs.push(PathBuf::from(common).join("VST3"));
        } else {
            dirs.push(PathBuf::from(r"C:\Program Files\Common Files\VST3"));
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Programs").join("Common").join("VST3"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        dirs.push(PathBuf::from("/usr/lib/vst3"));
        dirs.push(PathBuf::from("/usr/local/lib/vst3"));
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".vst3"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join("Library/Audio/Plug-Ins/VST3"));
        }
    }
    dirs
}

/// Standard CLAP search directories for the current OS.
pub fn default_clap_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        if let Ok(common) = std::env::var("CommonProgramFiles") {
            dirs.push(PathBuf::from(common).join("CLAP"));
        } else {
            dirs.push(PathBuf::from(r"C:\Program Files\Common Files\CLAP"));
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Programs").join("Common").join("CLAP"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        dirs.push(PathBuf::from("/usr/lib/clap"));
        dirs.push(PathBuf::from("/usr/local/lib/clap"));
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".clap"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join("Library/Audio/Plug-Ins/CLAP"));
        }
    }
    dirs
}

fn vst3_bundle_arch_subdirs() -> &'static [&'static str] {
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        &["x86_64-win", "x64-win"]
    }
    #[cfg(all(windows, target_arch = "aarch64"))]
    {
        &["arm64-win", "aarch64-win", "x86_64-win"]
    }
    #[cfg(all(windows, target_arch = "x86"))]
    {
        &["x86-win"]
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        &["x86_64-linux"]
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        &["aarch64-linux", "arm64-linux"]
    }
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    {
        &["armv7l-linux", "arm-linux"]
    }
    #[cfg(all(target_os = "linux", target_arch = "x86"))]
    {
        &["i386-linux", "x86-linux"]
    }
    #[cfg(target_os = "macos")]
    {
        &["MacOS"]
    }
    #[cfg(not(any(
        all(windows, any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "x86")),
        all(
            target_os = "linux",
            any(
                target_arch = "x86_64",
                target_arch = "aarch64",
                target_arch = "arm",
                target_arch = "x86"
            )
        ),
        target_os = "macos"
    )))]
    {
        &["x86_64-linux", "x86_64-win", "MacOS"]
    }
}

fn is_native_binary_ext(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    #[cfg(windows)]
    {
        matches!(ext.as_str(), "vst3" | "dll" | "clap")
    }
    #[cfg(target_os = "linux")]
    {
        matches!(ext.as_str(), "so" | "vst3" | "clap")
    }
    #[cfg(target_os = "macos")]
    {
        // macOS bundles often have extensionless Mach-O executables inside Contents/MacOS/
        ext.is_empty() || matches!(ext.as_str(), "dylib" | "so" | "vst3" | "clap")
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        matches!(ext.as_str(), "so" | "dll" | "dylib" | "vst3" | "clap")
    }
}

/// Resolves a `.vst3` bundle directory (`Contents/<arch>-<os>/*.so|*.dll|*.vst3`)
/// or flat `.vst3` shared library file to its loadable native binary path.
pub fn resolve_vst3_binary(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if !path.is_dir() {
        return None;
    }
    let contents = path.join("Contents");
    if !contents.is_dir() {
        return None;
    }

    // 1. Check current platform/arch subdirs first
    for subdir in vst3_bundle_arch_subdirs() {
        let dir = contents.join(subdir);
        if let Some(bin) = find_first_native_binary_in(&dir) {
            return Some(bin);
        }
    }

    // 2. Fallback: inspect any `<arch>-<os>` subdir under `Contents/`
    if let Ok(entries) = std::fs::read_dir(&contents) {
        for entry in entries.flatten() {
            let sub = entry.path();
            if sub.is_dir() {
                if let Some(bin) = find_first_native_binary_in(&sub) {
                    return Some(bin);
                }
            }
        }
    }
    None
}

/// Resolves a flat `.clap` shared library or macOS `.clap` bundle directory to its native binary.
pub fn resolve_clap_binary(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if path.is_dir() {
        let macos = path.join("Contents").join("MacOS");
        if let Some(bin) = find_first_native_binary_in(&macos) {
            return Some(bin);
        }
        return resolve_vst3_binary(path);
    }
    None
}

fn find_first_native_binary_in(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut candidates: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_native_binary_ext(p))
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

fn probe_clap_binary(binary_path: &Path, display_path: &Path) -> Result<Vec<AudioPluginInfo>, String> {
    let lib = unsafe { Library::new(binary_path) }.map_err(|e| e.to_string())?;
    let entry_ptr: *const ClapPluginEntry = unsafe {
        let sym: libloading::Symbol<*const ClapPluginEntry> =
            lib.get(b"clap_entry\0").map_err(|e| e.to_string())?;
        *sym
    };
    if entry_ptr.is_null() {
        return Err("clap_entry nulo".to_string());
    }
    let path_c = CString::new(binary_path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    let mut results = Vec::new();

    unsafe {
        let init_fn = (*entry_ptr).init.ok_or_else(|| "init ausente".to_string())?;
        if !init_fn(path_c.as_ptr()) {
            return Err("clap_entry.init retornou false".to_string());
        }

        let factory = (*entry_ptr)
            .get_factory
            .map(|f| f(CLAP_PLUGIN_FACTORY_ID.as_ptr() as *const c_char) as *const ClapPluginFactory)
            .unwrap_or(ptr::null());

        if !factory.is_null() {
            let count = (*factory)
                .get_plugin_count
                .map(|f| f(factory))
                .unwrap_or(0);
            let host = make_clap_host();

            for i in 0..count {
                let desc = (*factory)
                    .get_plugin_descriptor
                    .map(|f| f(factory, i))
                    .unwrap_or(ptr::null());
                if desc.is_null() {
                    continue;
                }
                let uid = c_ptr_to_string((*desc).id);
                let name = c_ptr_to_string((*desc).name);
                let vendor = c_ptr_to_string((*desc).vendor);
                let version = c_ptr_to_string((*desc).version);

                let mut has_gui = false;
                let mut params = Vec::new();

                if let (Some(create), Ok(uid_c)) = ((*factory).create_plugin, CString::new(uid.as_str()))
                {
                    let plugin = create(factory, &*host as *const ClapHost, uid_c.as_ptr());
                    if !plugin.is_null() {
                        if (*plugin).init.map(|f| f(plugin)).unwrap_or(false) {
                            if let Some(get_ext) = (*plugin).get_extension {
                                let gui_ext =
                                    get_ext(plugin, CLAP_EXT_GUI.as_ptr() as *const c_char);
                                has_gui = !gui_ext.is_null();

                                let params_ext = get_ext(
                                    plugin,
                                    CLAP_EXT_PARAMS.as_ptr() as *const c_char,
                                ) as *const ClapPluginParams;
                                if !params_ext.is_null() {
                                    if let (Some(cnt), Some(info_fn)) =
                                        ((*params_ext).count, (*params_ext).get_info)
                                    {
                                        let pcount = cnt(plugin).min(256);
                                        for pi in 0..pcount {
                                            let mut pinfo: ClapParamInfo = std::mem::zeroed();
                                            if info_fn(plugin, pi, &mut pinfo) {
                                                params.push(AudioPluginParam {
                                                    id: pinfo.id,
                                                    name: c_char_array_to_string(&pinfo.name),
                                                    value: pinfo.default_value as f32,
                                                    min: pinfo.min_value as f32,
                                                    max: pinfo.max_value as f32,
                                                    default_value: pinfo.default_value as f32,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(destroy) = (*plugin).destroy {
                            destroy(plugin);
                        }
                    }
                }

                results.push(AudioPluginInfo {
                    name: if name.is_empty() {
                        file_stem_name(display_path)
                    } else {
                        name
                    },
                    vendor,
                    version,
                    format: AudioPluginFormat::Clap,
                    path: display_path.to_string_lossy().to_string(),
                    plugin_uid: if uid.is_empty() {
                        format!("clap:{}", file_stem_name(display_path))
                    } else {
                        uid
                    },
                    has_gui,
                    params,
                });
            }
        }

        if let Some(deinit) = (*entry_ptr).deinit {
            deinit();
        }
    }

    if results.is_empty() {
        return Err("Nenhum descritor CLAP retornado.".to_string());
    }
    Ok(results)
}

fn probe_vst3_binary(binary_path: &Path, display_path: &Path) -> Result<Vec<AudioPluginInfo>, String> {
    let lib = unsafe { Library::new(binary_path) }.map_err(|e| e.to_string())?;
    let mut results = Vec::new();

    unsafe {
        let exit_kind = Vst3PluginInstance::init_module(&lib)?;
        let get_factory: Result<
            libloading::Symbol<unsafe extern "system" fn() -> *mut IPluginFactory>,
            _,
        > = lib.get(b"GetPluginFactory\0");

        let Ok(get_factory) = get_factory else {
            Vst3PluginInstance::exit_module(&lib, &exit_kind);
            return Err("GetPluginFactory ausente.".to_string());
        };

        let factory = get_factory();
        if factory.is_null() || (*factory).vtbl.is_null() {
            Vst3PluginInstance::exit_module(&lib, &exit_kind);
            return Err("GetPluginFactory retornou nulo.".to_string());
        }

        let mut finfo: PFactoryInfo = std::mem::zeroed();
        let vendor = if ((*(*factory).vtbl).get_factory_info)(factory, &mut finfo) == K_RESULT_OK {
            c_char_array_to_string(&finfo.vendor)
        } else {
            String::new()
        };

        let count = ((*(*factory).vtbl).count_classes)(factory);
        for i in 0..count {
            let mut cinfo: PClassInfo = std::mem::zeroed();
            if ((*(*factory).vtbl).get_class_info)(factory, i, &mut cinfo) != K_RESULT_OK {
                continue;
            }
            let category = c_char_array_to_string(&cinfo.category);
            if category != "Audio Module Class" {
                continue;
            }
            let name = c_char_array_to_string(&cinfo.name);
            let uid = format_tuid(&cinfo.cid);
            results.push(AudioPluginInfo {
                name: if name.is_empty() {
                    file_stem_name(display_path)
                } else {
                    name
                },
                vendor: vendor.clone(),
                version: "3.0".to_string(),
                format: AudioPluginFormat::Vst3,
                path: display_path.to_string_lossy().to_string(),
                plugin_uid: uid,
                has_gui: true,
                params: Vec::new(),
            });
        }

        ((*(*factory).vtbl).base.release)(factory as *mut FUnknown);
        Vst3PluginInstance::exit_module(&lib, &exit_kind);
    }

    if results.is_empty() {
        return Err("Nenhuma classe de áudio encontrada no VST3.".to_string());
    }
    Ok(results)
}

fn collect_plugin_entries(dir: &Path, ext: &str, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 6 || !dir.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();

    for path in children {
        let matches_ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.eq_ignore_ascii_case(ext))
            .unwrap_or(false);
        if matches_ext {
            out.push(path);
        } else if path.is_dir() {
            collect_plugin_entries(&path, ext, out, depth + 1);
        }
    }
}

/// Scans standard OS directories for installed VST3 and CLAP plugins.
pub fn scan_plugins() -> Vec<AudioPluginInfo> {
    scan_plugins_in_dirs(&default_vst3_dirs(), &default_clap_dirs())
}

/// Scans explicit VST3 and CLAP directories (recursively up to 6 levels), probing each
/// discovered binary safely via `libloading` and falling back to bundle metadata if a binary
/// cannot be dynamically loaded on the host.
pub fn scan_plugins_in_dirs(vst3_dirs: &[PathBuf], clap_dirs: &[PathBuf]) -> Vec<AudioPluginInfo> {
    let mut out = Vec::new();
    let mut seen_keys = HashSet::<String>::new();

    let mut vst3_paths = Vec::new();
    for dir in vst3_dirs {
        collect_plugin_entries(dir, "vst3", &mut vst3_paths, 0);
    }
    for entry_path in vst3_paths {
        let Some(binary_path) = resolve_vst3_binary(&entry_path) else {
            continue;
        };
        let probed = catch_unwind(AssertUnwindSafe(|| probe_vst3_binary(&binary_path, &entry_path)));
        match probed {
            Ok(Ok(infos)) => {
                for info in infos {
                    let key = format!("vst3:{}:{}", info.path, info.plugin_uid);
                    if seen_keys.insert(key) {
                        out.push(info);
                    }
                }
            }
            _ => {
                let stem = file_stem_name(&entry_path);
                let uid = format!("vst3:{stem}");
                let path_str = entry_path.to_string_lossy().to_string();
                let key = format!("vst3:{path_str}:{uid}");
                if seen_keys.insert(key) {
                    out.push(AudioPluginInfo {
                        name: stem,
                        vendor: String::new(),
                        version: String::new(),
                        format: AudioPluginFormat::Vst3,
                        path: path_str,
                        plugin_uid: uid,
                        has_gui: false,
                        params: Vec::new(),
                    });
                }
            }
        }
    }

    let mut clap_paths = Vec::new();
    for dir in clap_dirs {
        collect_plugin_entries(dir, "clap", &mut clap_paths, 0);
    }
    for entry_path in clap_paths {
        let Some(binary_path) = resolve_clap_binary(&entry_path) else {
            continue;
        };
        let probed = catch_unwind(AssertUnwindSafe(|| probe_clap_binary(&binary_path, &entry_path)));
        match probed {
            Ok(Ok(infos)) => {
                for info in infos {
                    let key = format!("clap:{}:{}", info.path, info.plugin_uid);
                    if seen_keys.insert(key) {
                        out.push(info);
                    }
                }
            }
            _ => {
                let stem = file_stem_name(&entry_path);
                let uid = format!("clap:{stem}");
                let path_str = entry_path.to_string_lossy().to_string();
                let key = format!("clap:{path_str}:{uid}");
                if seen_keys.insert(key) {
                    out.push(AudioPluginInfo {
                        name: stem,
                        vendor: String::new(),
                        version: String::new(),
                        format: AudioPluginFormat::Clap,
                        path: path_str,
                        plugin_uid: uid,
                        has_gui: false,
                        params: Vec::new(),
                    });
                }
            }
        }
    }

    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn load_dynamic_plugin(
    format: AudioPluginFormat,
    path: &str,
    plugin_uid: &str,
    sample_rate: f32,
    max_frames: usize,
) -> Result<Box<dyn AudioPluginProcessor>, String> {
    if path.trim().is_empty() {
        return Err("Caminho do plugin vazio.".to_string());
    }
    let p = Path::new(path);
    match format {
        AudioPluginFormat::Clap => {
            let bin = resolve_clap_binary(p)
                .ok_or_else(|| format!("Binário CLAP não encontrado em '{path}'."))?;
            let inst = ClapPluginInstance::load(&bin, plugin_uid, sample_rate, max_frames)?;
            Ok(Box::new(inst))
        }
        AudioPluginFormat::Vst3 => {
            let bin = resolve_vst3_binary(p)
                .ok_or_else(|| format!("Binário VST3 não encontrado em '{path}'."))?;
            let inst = Vst3PluginInstance::load(&bin, plugin_uid, sample_rate, max_frames)?;
            Ok(Box::new(inst))
        }
    }
}

// ------------------------------------------------------------------ PluginChain

const MAX_CONSECUTIVE_NAN_BLOCKS: u32 = 100;

struct LoadedSlot {
    slot_id: String,
    path: String,
    plugin_uid: String,
    format: AudioPluginFormat,
    enabled: bool,
    wet_dry: f32,
    faulted: bool,
    fault_reason: Option<String>,
    consecutive_nan_blocks: u32,
    cached_state_b64: Option<String>,
    applied_params: HashMap<u32, f32>,
    processor: Option<Box<dyn AudioPluginProcessor>>,
}

/// Fault-tolerant, zero-allocation stereo plugin insert chain per audio mixer channel.
pub struct PluginChain {
    slots: Vec<LoadedSlot>,
    dry_l: Vec<f32>,
    dry_r: Vec<f32>,
}

impl Default for PluginChain {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginChain {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            dry_l: Vec::new(),
            dry_r: Vec::new(),
        }
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    pub fn is_slot_faulted(&self, slot_id: &str) -> bool {
        self.slots
            .iter()
            .find(|s| s.slot_id == slot_id)
            .map(|s| s.faulted)
            .unwrap_or(false)
    }

    pub fn slot_fault_reason(&self, slot_id: &str) -> Option<&str> {
        self.slots
            .iter()
            .find(|s| s.slot_id == slot_id)
            .and_then(|s| s.fault_reason.as_deref())
    }

    /// Inserts a pre-constructed processor slot (useful for deterministic unit tests and
    /// built-in processors that share the same chain pipeline).
    pub fn push_slot_with_processor(
        &mut self,
        slot: AudioPluginSlot,
        processor: Box<dyn AudioPluginProcessor>,
    ) {
        let mut loaded = LoadedSlot {
            slot_id: slot.id,
            path: slot.path,
            plugin_uid: slot.plugin_uid,
            format: slot.format,
            enabled: slot.enabled,
            wet_dry: slot.wet_dry.clamp(0.0, 1.0),
            faulted: false,
            fault_reason: None,
            consecutive_nan_blocks: 0,
            cached_state_b64: slot.state_b64.clone(),
            applied_params: HashMap::new(),
            processor: Some(processor),
        };
        if let Some(ref b64) = slot.state_b64 {
            if let Some(ref mut proc) = loaded.processor {
                let _ = catch_unwind(AssertUnwindSafe(|| proc.load_state_b64(b64)));
            }
        }
        for (k, &v) in &slot.params {
            if let Ok(pid) = k.parse::<u32>() {
                if let Some(ref mut proc) = loaded.processor {
                    let _ = catch_unwind(AssertUnwindSafe(|| proc.set_param(pid, v)));
                }
                loaded.applied_params.insert(pid, v);
            }
        }
        self.slots.push(loaded);
    }

    /// Synchronizes the ordered slot chain with the latest channel configuration.
    /// Existing live plugin instances matching `(id, format, path, plugin_uid)` are preserved
    /// in-place so settings updates do not interrupt DSP state.
    pub fn sync_slots(&mut self, slots: &[AudioPluginSlot], sample_rate: f32, max_frames: usize) {
        if self.dry_l.len() < max_frames {
            self.dry_l.resize(max_frames, 0.0);
            self.dry_r.resize(max_frames, 0.0);
        }

        let mut old_by_id: HashMap<String, LoadedSlot> = self
            .slots
            .drain(..)
            .map(|s| (s.slot_id.clone(), s))
            .collect();

        let mut next = Vec::with_capacity(slots.len());
        for spec in slots {
            let mut loaded = match old_by_id.remove(&spec.id) {
                Some(existing)
                    if existing.format == spec.format
                        && existing.path == spec.path
                        && existing.plugin_uid == spec.plugin_uid =>
                {
                    existing
                }
                _ => {
                    let loaded_res = catch_unwind(AssertUnwindSafe(|| {
                        load_dynamic_plugin(
                            spec.format,
                            &spec.path,
                            &spec.plugin_uid,
                            sample_rate,
                            max_frames,
                        )
                    }));
                    let (processor, faulted, fault_reason) = match loaded_res {
                        Ok(Ok(proc)) => (Some(proc), false, None),
                        Ok(Err(err)) => {
                            log::warn!(
                                "vst_host: falha ao carregar plugin '{}' ({}): {}",
                                spec.name,
                                spec.path,
                                err
                            );
                            (None, true, Some(err))
                        }
                        Err(_) => {
                            let msg = format!(
                                "Plugin '{}' sofreu pânico ao inicializar; ignorado com segurança.",
                                spec.name
                            );
                            log::warn!("vst_host: {msg}");
                            (None, true, Some(msg))
                        }
                    };
                    LoadedSlot {
                        slot_id: spec.id.clone(),
                        path: spec.path.clone(),
                        plugin_uid: spec.plugin_uid.clone(),
                        format: spec.format,
                        enabled: spec.enabled,
                        wet_dry: spec.wet_dry.clamp(0.0, 1.0),
                        faulted,
                        fault_reason,
                        consecutive_nan_blocks: 0,
                        cached_state_b64: None,
                        applied_params: HashMap::new(),
                        processor,
                    }
                }
            };

            loaded.enabled = spec.enabled;
            loaded.wet_dry = spec.wet_dry.clamp(0.0, 1.0);

            // Restore state if newly provided or changed
            if let Some(ref incoming_state) = spec.state_b64 {
                if loaded.cached_state_b64.as_deref() != Some(incoming_state.as_str()) {
                    if let Some(ref mut proc) = loaded.processor {
                        if !loaded.faulted {
                            match catch_unwind(AssertUnwindSafe(|| {
                                proc.load_state_b64(incoming_state)
                            })) {
                                Ok(Ok(())) => {
                                    loaded.cached_state_b64 = Some(incoming_state.clone());
                                }
                                Ok(Err(e)) => {
                                    log::debug!("vst_host: load_state_b64 aviso: {e}");
                                    loaded.cached_state_b64 = Some(incoming_state.clone());
                                }
                                Err(_) => {
                                    loaded.faulted = true;
                                    loaded.fault_reason = Some(
                                        "Plugin sofreu pânico ao restaurar estado; auto-bypassado."
                                            .to_string(),
                                    );
                                }
                            }
                        }
                    } else {
                        loaded.cached_state_b64 = Some(incoming_state.clone());
                    }
                }
            }

            // Apply changed parameter overrides
            if !loaded.faulted {
                for (k, &val) in &spec.params {
                    let Ok(param_id) = k.parse::<u32>() else {
                        continue;
                    };
                    let changed = loaded
                        .applied_params
                        .get(&param_id)
                        .map(|&prev| (prev - val).abs() > 1e-6)
                        .unwrap_or(true);
                    if changed {
                        if let Some(ref mut proc) = loaded.processor {
                            match catch_unwind(AssertUnwindSafe(|| proc.set_param(param_id, val))) {
                                Ok(Ok(())) => {
                                    loaded.applied_params.insert(param_id, val);
                                }
                                Ok(Err(_)) => {}
                                Err(_) => {
                                    loaded.faulted = true;
                                    loaded.fault_reason = Some(
                                        "Plugin sofreu pânico em set_param; auto-bypassado."
                                            .to_string(),
                                    );
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            next.push(loaded);
        }

        self.slots = next;
    }

    /// Processes a stereo audio block through all enabled, non-faulted slots in order.
    ///
    /// Guarantees:
    /// - Never panics across FFI/plugin faults (`catch_unwind` restores dry audio & auto-bypasses).
    /// - Sanitizes all `NaN`/`Inf` output samples to `0.0` (and auto-bypasses persistent NaN generators).
    /// - Applies per-slot `wet_dry` mix cleanly.
    pub fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 || self.slots.is_empty() {
            return;
        }
        if self.dry_l.len() < len {
            self.dry_l.resize(len, 0.0);
            self.dry_r.resize(len, 0.0);
        }

        for slot in &mut self.slots {
            if !slot.enabled || slot.faulted || slot.wet_dry <= 0.0 {
                continue;
            }
            let Some(ref mut proc) = slot.processor else {
                continue;
            };

            self.dry_l[..len].copy_from_slice(&left[..len]);
            self.dry_r[..len].copy_from_slice(&right[..len]);

            let res = catch_unwind(AssertUnwindSafe(|| {
                proc.process_stereo(&mut left[..len], &mut right[..len]);
            }));

            if res.is_err() {
                left[..len].copy_from_slice(&self.dry_l[..len]);
                right[..len].copy_from_slice(&self.dry_r[..len]);
                slot.faulted = true;
                slot.fault_reason = Some(
                    "Plugin falhou durante o processamento de áudio e foi colocado em bypass automático."
                        .to_string(),
                );
                log::error!(
                    "vst_host: slot '{}' sofreu pânico em process_block -> auto-bypass ativado",
                    slot.slot_id
                );
                continue;
            }

            // Sanitize NaN / Inf outputs so corrupted samples never reach the master bus
            let mut saw_non_finite = false;
            for i in 0..len {
                if !left[i].is_finite() {
                    left[i] = 0.0;
                    saw_non_finite = true;
                }
                if !right[i].is_finite() {
                    right[i] = 0.0;
                    saw_non_finite = true;
                }
            }
            if saw_non_finite {
                slot.consecutive_nan_blocks = slot.consecutive_nan_blocks.saturating_add(1);
                if slot.consecutive_nan_blocks >= MAX_CONSECUTIVE_NAN_BLOCKS {
                    left[..len].copy_from_slice(&self.dry_l[..len]);
                    right[..len].copy_from_slice(&self.dry_r[..len]);
                    slot.faulted = true;
                    slot.fault_reason = Some(
                        "Plugin gerou amostras NaN/Inf continuamente e foi colocado em bypass automático."
                            .to_string(),
                    );
                    continue;
                }
            } else {
                slot.consecutive_nan_blocks = 0;
            }

            if slot.wet_dry < 0.9999 {
                let wet = slot.wet_dry;
                let dry = 1.0 - wet;
                for i in 0..len {
                    left[i] = self.dry_l[i] * dry + left[i] * wet;
                    right[i] = self.dry_r[i] * dry + right[i] * wet;
                }
            }
        }
    }

    pub fn set_param(&mut self, slot_id: &str, param_id: u32, value: f32) -> Result<(), String> {
        let Some(slot) = self.slots.iter_mut().find(|s| s.slot_id == slot_id) else {
            return Err(format!("Slot de plugin '{slot_id}' não encontrado."));
        };
        if slot.faulted {
            return Err(slot
                .fault_reason
                .clone()
                .unwrap_or_else(|| "Slot de plugin está em bypass por falha.".to_string()));
        }
        let Some(ref mut proc) = slot.processor else {
            return Err(format!("Plugin no slot '{slot_id}' não está carregado."));
        };
        match catch_unwind(AssertUnwindSafe(|| proc.set_param(param_id, value))) {
            Ok(Ok(())) => {
                slot.applied_params.insert(param_id, value);
                if let Ok(Some(st)) = catch_unwind(AssertUnwindSafe(|| proc.save_state_b64())) {
                    slot.cached_state_b64 = Some(st);
                }
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                slot.faulted = true;
                slot.fault_reason =
                    Some("Plugin sofreu pânico ao alterar parâmetro; auto-bypassado.".to_string());
                Err("Plugin sofreu pânico ao alterar parâmetro.".to_string())
            }
        }
    }

    pub fn open_gui(&mut self, slot_id: &str) -> Result<(), String> {
        let Some(slot) = self.slots.iter_mut().find(|s| s.slot_id == slot_id) else {
            return Err(format!("Slot de plugin '{slot_id}' não encontrado."));
        };
        if slot.faulted {
            return Err(slot
                .fault_reason
                .clone()
                .unwrap_or_else(|| "Slot de plugin está em bypass por falha.".to_string()));
        }
        let Some(ref mut proc) = slot.processor else {
            return Err(format!("Plugin no slot '{slot_id}' não está carregado."));
        };
        match catch_unwind(AssertUnwindSafe(|| proc.open_gui())) {
            Ok(res) => res,
            Err(_) => {
                slot.faulted = true;
                slot.fault_reason =
                    Some("Plugin sofreu pânico ao abrir GUI; auto-bypassado.".to_string());
                Err("Plugin sofreu pânico ao abrir GUI.".to_string())
            }
        }
    }

    pub fn export_states(&self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for slot in &self.slots {
            let live_state = if !slot.faulted {
                slot.processor.as_ref().and_then(|p| {
                    catch_unwind(AssertUnwindSafe(|| p.save_state_b64()))
                        .ok()
                        .flatten()
                })
            } else {
                None
            };
            if let Some(state) = live_state.or_else(|| slot.cached_state_b64.clone()) {
                out.insert(slot.slot_id.clone(), state);
            }
        }
        out
    }
}

// ------------------------------------------------------------------ Unit Tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Arc;

    struct GainProcessor {
        gain: f32,
        offset: f32,
        state_tag: String,
    }

    impl GainProcessor {
        fn new(gain: f32, offset: f32) -> Self {
            Self {
                gain,
                offset,
                state_tag: format!("gain:{gain:.3}"),
            }
        }
    }

    impl AudioPluginProcessor for GainProcessor {
        fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32]) {
            for s in left.iter_mut() {
                *s = (*s * self.gain) + self.offset;
            }
            for s in right.iter_mut() {
                *s = (*s * self.gain) + self.offset;
            }
        }

        fn set_param(&mut self, param_id: u32, value: f32) -> Result<(), String> {
            match param_id {
                0 => {
                    self.gain = value;
                    self.state_tag = format!("gain:{value:.3}");
                    Ok(())
                }
                1 => {
                    self.offset = value;
                    Ok(())
                }
                _ => Err(format!("Parâmetro desconhecido: {param_id}")),
            }
        }

        fn get_params(&self) -> Vec<AudioPluginParam> {
            vec![AudioPluginParam {
                id: 0,
                name: "Gain".to_string(),
                value: self.gain,
                min: 0.0,
                max: 4.0,
                default_value: 1.0,
            }]
        }

        fn save_state_b64(&self) -> Option<String> {
            Some(BASE64_STANDARD.encode(self.state_tag.as_bytes()))
        }

        fn load_state_b64(&mut self, b64: &str) -> Result<(), String> {
            let raw = BASE64_STANDARD.decode(b64).map_err(|e| e.to_string())?;
            let text = String::from_utf8(raw).map_err(|e| e.to_string())?;
            if let Some(v) = text.strip_prefix("gain:").and_then(|s| s.parse::<f32>().ok()) {
                self.gain = v;
                self.state_tag = text;
                Ok(())
            } else {
                Err("invalid state".to_string())
            }
        }

        fn open_gui(&mut self) -> Result<(), String> {
            Err("headless test processor".to_string())
        }
    }

    struct NanProcessor;
    impl AudioPluginProcessor for NanProcessor {
        fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32]) {
            for (i, s) in left.iter_mut().enumerate() {
                *s = if i % 2 == 0 { f32::NAN } else { f32::INFINITY };
            }
            for s in right.iter_mut() {
                *s = f32::NEG_INFINITY;
            }
        }
        fn set_param(&mut self, _id: u32, _v: f32) -> Result<(), String> {
            Ok(())
        }
        fn get_params(&self) -> Vec<AudioPluginParam> {
            Vec::new()
        }
        fn save_state_b64(&self) -> Option<String> {
            None
        }
        fn load_state_b64(&mut self, _b64: &str) -> Result<(), String> {
            Ok(())
        }
        fn open_gui(&mut self) -> Result<(), String> {
            Err("headless".to_string())
        }
    }

    struct PanickingProcessor {
        calls: Arc<AtomicUsize>,
    }
    impl AudioPluginProcessor for PanickingProcessor {
        fn process_stereo(&mut self, left: &mut [f32], _right: &mut [f32]) {
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            // Corrupt buffer before panicking to verify dry buffer rollback!
            if let Some(first) = left.first_mut() {
                *first = 999.0;
            }
            panic!("simulated plugin crash");
        }
        fn set_param(&mut self, _id: u32, _v: f32) -> Result<(), String> {
            Ok(())
        }
        fn get_params(&self) -> Vec<AudioPluginParam> {
            Vec::new()
        }
        fn save_state_b64(&self) -> Option<String> {
            None
        }
        fn load_state_b64(&mut self, _b64: &str) -> Result<(), String> {
            Ok(())
        }
        fn open_gui(&mut self) -> Result<(), String> {
            Err("headless".to_string())
        }
    }

    fn test_slot(id: &str, enabled: bool, wet_dry: f32) -> AudioPluginSlot {
        AudioPluginSlot {
            id: id.to_string(),
            name: id.to_string(),
            vendor: "TestVendor".to_string(),
            format: AudioPluginFormat::Clap,
            path: format!("/tmp/{id}.clap"),
            plugin_uid: format!("test.{id}"),
            enabled,
            wet_dry,
            state_b64: None,
            params: HashMap::new(),
        }
    }

    #[test]
    fn test_scan_plugin_directories_vst3_bundles_and_clap_files() {
        let base = std::env::temp_dir().join(format!(
            "prismcast_vst_scan_test_{}",
            uuid::Uuid::new_v4()
        ));
        let vst3_dir = base.join("vst3");
        let clap_dir = base.join("clap");

        // Build a standard `.vst3` bundle layout matching the current platform
        let arch_subdir = vst3_bundle_arch_subdirs()[0];
        let bundle_dir = vst3_dir
            .join("VendorA")
            .join("ProComp.vst3")
            .join("Contents")
            .join(arch_subdir);
        std::fs::create_dir_all(&bundle_dir).unwrap();
        #[cfg(windows)]
        let vst3_bin = bundle_dir.join("ProComp.vst3");
        #[cfg(target_os = "macos")]
        let vst3_bin = bundle_dir.join("ProComp");
        #[cfg(all(not(windows), not(target_os = "macos")))]
        let vst3_bin = bundle_dir.join("ProComp.so");
        std::fs::write(&vst3_bin, b"not-a-real-shared-lib").unwrap();

        // Build a flat `.clap` plugin inside a vendor subdirectory
        std::fs::create_dir_all(clap_dir.join("VendorB")).unwrap();
        let clap_file = clap_dir.join("VendorB").join("CleanGate.clap");
        std::fs::write(&clap_file, b"not-a-real-clap-lib").unwrap();

        // Verify bundle binary resolver explicitly
        let resolved_vst3 = resolve_vst3_binary(&vst3_dir.join("VendorA").join("ProComp.vst3"));
        assert_eq!(resolved_vst3.as_deref(), Some(vst3_bin.as_path()));
        let resolved_clap = resolve_clap_binary(&clap_file);
        assert_eq!(resolved_clap.as_deref(), Some(clap_file.as_path()));

        // Verify full directory scan picks up both entries cleanly
        let found = scan_plugins_in_dirs(&[vst3_dir], &[clap_dir]);
        assert_eq!(found.len(), 2, "expected 2 discovered plugins, got {found:?}");
        assert!(found
            .iter()
            .any(|p| p.name == "ProComp" && p.format == AudioPluginFormat::Vst3));
        assert!(found
            .iter()
            .any(|p| p.name == "CleanGate" && p.format == AudioPluginFormat::Clap));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_plugin_chain_ordering_bypass_and_wet_dry_mixing() {
        let mut chain = PluginChain::new();
        // Slot 1: multiply by 2.0 (enabled, 100% wet)
        chain.push_slot_with_processor(
            test_slot("slot1", true, 1.0),
            Box::new(GainProcessor::new(2.0, 0.0)),
        );
        // Slot 2: add +0.5 (BYPASSED)
        chain.push_slot_with_processor(
            test_slot("slot2", false, 1.0),
            Box::new(GainProcessor::new(1.0, 0.5)),
        );
        // Slot 3: add +0.4 at 50% wet/dry -> dry*0.5 + (dry + 0.4)*0.5 = dry + 0.2
        chain.push_slot_with_processor(
            test_slot("slot3", true, 0.5),
            Box::new(GainProcessor::new(1.0, 0.4)),
        );

        let mut l = vec![0.25_f32; 16];
        let mut r = vec![-0.25_f32; 16];
        chain.process_block(&mut l, &mut r);

        // Expected L: (0.25 * 2.0) = 0.5 -> bypassed slot2 -> +0.2 = 0.7
        // Expected R: (-0.25 * 2.0) = -0.5 -> bypassed slot2 -> +0.2 = -0.3
        for i in 0..16 {
            assert!((l[i] - 0.7).abs() < 1e-5, "l[{i}] = {}", l[i]);
            assert!((r[i] - (-0.3)).abs() < 1e-5, "r[{i}] = {}", r[i]);
        }
    }

    #[test]
    fn test_plugin_chain_nan_inf_sanitization() {
        let mut chain = PluginChain::new();
        // 50% wet/dry with NaN/Inf generator: sanitized wet is 0.0, mixed with 50% dry
        chain.push_slot_with_processor(test_slot("nan", true, 0.5), Box::new(NanProcessor));

        let mut l = vec![0.4_f32; 32];
        let mut r = vec![-0.6_f32; 32];
        chain.process_block(&mut l, &mut r);

        for i in 0..32 {
            assert!(l[i].is_finite(), "left sample {i} must be finite");
            assert!(r[i].is_finite(), "right sample {i} must be finite");
            assert!((l[i] - 0.2).abs() < 1e-5);
            assert!((r[i] - (-0.3)).abs() < 1e-5);
        }
    }

    #[test]
    fn test_plugin_chain_fault_auto_bypass_and_dry_rollback() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut chain = PluginChain::new();
        chain.push_slot_with_processor(
            test_slot("crashy", true, 1.0),
            Box::new(PanickingProcessor {
                calls: calls.clone(),
            }),
        );
        // Downstream healthy slot still executes after crashy slot rolls back!
        chain.push_slot_with_processor(
            test_slot("healthy", true, 1.0),
            Box::new(GainProcessor::new(0.5, 0.0)),
        );

        let mut l = vec![0.8_f32; 8];
        let mut r = vec![-0.4_f32; 8];
        chain.process_block(&mut l, &mut r);

        assert!(chain.is_slot_faulted("crashy"));
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
        // Dry buffer (0.8 / -0.4) was restored on panic, then scaled by healthy slot (0.5)
        assert!((l[0] - 0.4).abs() < 1e-5);
        assert!((r[0] - (-0.2)).abs() < 1e-5);

        // Subsequent block never invokes the faulted plugin again
        let mut l2 = vec![0.6_f32; 8];
        let mut r2 = vec![0.2_f32; 8];
        chain.process_block(&mut l2, &mut r2);
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
        assert!((l2[0] - 0.3).abs() < 1e-5);
    }

    #[test]
    fn test_plugin_chain_sync_slots_params_and_state_export() {
        let mut chain = PluginChain::new();
        let mut slot = test_slot("eq1", true, 1.0);
        slot.state_b64 = Some(BASE64_STANDARD.encode(b"gain:1.500"));
        chain.push_slot_with_processor(slot.clone(), Box::new(GainProcessor::new(1.0, 0.0)));

        // Verify state_b64 was restored on push
        let mut l = vec![0.2_f32; 4];
        let mut r = vec![0.2_f32; 4];
        chain.process_block(&mut l, &mut r);
        assert!((l[0] - 0.3).abs() < 1e-5);

        // Live set_param updates DSP & exported state
        chain.set_param("eq1", 0, 2.0).unwrap();
        let states = chain.export_states();
        assert_eq!(
            states.get("eq1").map(String::as_str),
            Some(BASE64_STANDARD.encode(b"gain:2.000").as_str())
        );

        // sync_slots preserves existing live slot when identity matches and applies new wet_dry & params
        let mut updated = slot.clone();
        updated.wet_dry = 0.5;
        updated.params.insert("0".to_string(), 3.0);
        updated.state_b64 = None;
        chain.sync_slots(&[updated], 48_000.0, 480);
        assert_eq!(chain.slot_count(), 1);
        assert!(!chain.is_slot_faulted("eq1"));

        let mut l2 = vec![0.2_f32; 4];
        let mut r2 = vec![0.2_f32; 4];
        chain.process_block(&mut l2, &mut r2);
        // 50% dry (0.2) + 50% wet (0.2 * 3.0 = 0.6) = 0.4
        assert!((l2[0] - 0.4).abs() < 1e-5);

        // open_gui on headless slot returns informative Err rather than panicking
        let gui_res = chain.open_gui("eq1");
        assert!(gui_res.is_err());
    }
}
