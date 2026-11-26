//! Phase 2 VoIP media: Opus encode/decode + RTP over UDP.
//!
//! v1: captures mic, encodes Opus, sends RTP to `rtp_addr`; receives RTP,
//! decodes Opus, plays to speaker. Simple blocking loop with 20 ms frames.
//! No jitter buffer / resampling / echo cancellation yet.

use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use opus::{Channels as OpusChannels, Decoder, Encoder};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::UdpSocket;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering},
    mpsc, Arc, LazyLock, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const SAMPLE_RATE: u32 = 16000;
const FRAME_SAMPLES: usize = 320; // 20 ms @ 16 kHz
const AUDIO_RING_CAPACITY: usize = 4096; // 256 ms @ 16 kHz (power of two)
const AUDIO_MAX_OUTPUT_RING_SAMPLES: usize = FRAME_SAMPLES * 6; // 120 ms max playback queue

struct CachedDevices {
    mics: Vec<String>,
    cams: Vec<String>,
    updated_at: Instant,
}

static SELECTED_MIC_DEVICE: Mutex<String> = Mutex::new(String::new());
static MIC_SELECTION_EPOCH: AtomicU64 = AtomicU64::new(1);
static SELECTED_CAMERA_DEVICE: Mutex<String> = Mutex::new(String::new());
static CACHED_DEVICES: Mutex<Option<CachedDevices>> = Mutex::new(None);
static DEVICES_SCANNING: AtomicBool = AtomicBool::new(false);
static SHARED_CAM_H264: Mutex<(u64, Vec<u8>, Vec<u8>)> = Mutex::new((0, Vec::new(), Vec::new()));
static MIC_GAIN_PERCENT: AtomicU32 = AtomicU32::new(100);
static MIC_LEVEL_PERMILLE: AtomicU32 = AtomicU32::new(0);
static REMOTE_AUDIO_ACTIVE: AtomicBool = AtomicBool::new(false);
static CALL_VIDEO_FRAMES: LazyLock<Mutex<HashMap<String, Option<eframe::egui::ColorImage>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static CALL_VIDEO_DIRTY: AtomicBool = AtomicBool::new(false);
static FFMPEG_PROBE_STATE: AtomicU8 = AtomicU8::new(0); // 0 = unknown, 1 = missing, 2 = available
static FFMPEG_PROBING: AtomicBool = AtomicBool::new(false);

/// Publish the latest decoded video frame for `key`, coalescing any unconsumed older frame
/// so video queues never grow unbounded when the UI renders slower than the decoder.
pub fn publish_call_video_frame(key: &str, image: eframe::egui::ColorImage) {
    let mut guard = CALL_VIDEO_FRAMES
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.insert(key.to_owned(), Some(image));
    CALL_VIDEO_DIRTY.store(true, Ordering::Release);
}

/// Mark a call video texture slot as cleared (e.g. remote camera paused/stopped).
pub fn clear_call_video_frame(key: &str) {
    let mut guard = CALL_VIDEO_FRAMES
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.insert(key.to_owned(), None);
    CALL_VIDEO_DIRTY.store(true, Ordering::Release);
}

/// Drop all pending call video frames (called on call teardown / app exit).
pub fn clear_all_call_video_frames() {
    let mut guard = CALL_VIDEO_FRAMES
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.clear();
    CALL_VIDEO_DIRTY.store(false, Ordering::Release);
}

/// Drain the latest frame (or clear tombstone) per call video stream without blocking.
pub fn drain_call_video_frames() -> Vec<(String, Option<eframe::egui::ColorImage>)> {
    if !CALL_VIDEO_DIRTY.swap(false, Ordering::AcqRel) {
        return Vec::new();
    }
    let mut guard = CALL_VIDEO_FRAMES
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.drain().collect()
}

/// RAII guard for external `ffmpeg` child processes: closes stdio pipes, kills the child,
/// and performs a bounded reap loop so cameras are released and no zombie processes remain.
struct ChildGuard(Option<std::process::Child>);

impl ChildGuard {
    fn new(child: std::process::Child) -> Self {
        Self(Some(child))
    }

    fn take_stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.0.as_mut().and_then(|c| c.stdin.take())
    }

    fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.0.as_mut().and_then(|c| c.stdout.take())
    }

    fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.0.as_mut().and_then(|c| c.stderr.take())
    }

    fn kill_and_reap(&mut self) {
        if let Some(mut child) = self.0.take() {
            drop(child.stdin.take());
            drop(child.stdout.take());
            drop(child.stderr.take());
            let _ = child.kill();
            for _ in 0..50 {
                match child.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) => thread::sleep(Duration::from_millis(10)),
                }
            }
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.kill_and_reap();
    }
}

/// Set real-time microphone input gain (`0..=200` percent, `100` = unity gain).
pub fn set_mic_gain_percent(pct: u32) {
    MIC_GAIN_PERCENT.store(pct.min(200), Ordering::Relaxed);
}

/// Get current microphone input gain (`0..=200` percent).
pub fn mic_gain_percent() -> u32 {
    MIC_GAIN_PERCENT.load(Ordering::Relaxed)
}

/// Get instantaneous microphone peak level (`0..=1000` permille) for the live Call UI meter.
pub fn mic_level_permille() -> u32 {
    MIC_LEVEL_PERMILLE.load(Ordering::Relaxed)
}
/// Đặt mức mic tức thời từ peak tuyến tính (`0.0..=1.0`) — dùng cho meter khi thu voice.
pub fn set_mic_level_from_peak(peak: f32) {
    MIC_LEVEL_PERMILLE.store((peak.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
}


/// Whether remote peer audio is currently above the speaking threshold.
pub fn is_remote_speaking() -> bool {
    REMOTE_AUDIO_ACTIVE.load(Ordering::Relaxed)
}

/// Filter out infrared/Windows Hello depth cameras and virtual audio monitor loopbacks.
pub fn is_ignored_virtual_or_ir_device(name: &str) -> bool {
    let low = name.trim().to_lowercase();
    low.contains("ir camera")
        || low.contains("infrared")
        || low.contains("windows hello")
        || low.contains("depth camera")
        || low.starts_with("monitor of ")
}
pub fn set_selected_mic_device(name: &str) {
    let normalized = name.trim().to_string();
    let changed = if let Ok(mut g) = SELECTED_MIC_DEVICE.lock() {
        if *g != normalized {
            *g = normalized.clone();
            true
        } else {
            false
        }
    } else {
        false
    };
    if changed {
        MIC_SELECTION_EPOCH.fetch_add(1, Ordering::AcqRel);
        let label = if normalized.is_empty() {
            "<default>"
        } else {
            &normalized
        };
        crate::logger::log_voip_event("audio::mic_select", &format!("selected={label}"), false);
    }
}

pub fn set_selected_camera_device(name: &str) {
    if let Ok(mut g) = SELECTED_CAMERA_DEVICE.lock() {
        *g = name.trim().to_string();
    }
}

pub fn selected_mic_device() -> String {
    SELECTED_MIC_DEVICE
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

pub fn selected_camera_device() -> String {
    SELECTED_CAMERA_DEVICE
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

enum AudioEnumReq {
    EnumerateInputs {
        reply: mpsc::Sender<Result<(Vec<String>, bool), String>>,
    },
    ResolveInputDevice {
        preferred: String,
        reply: mpsc::Sender<Result<(cpal::Device, String), String>>,
    },
    ResolveOutputDevice {
        reply: mpsc::Sender<Result<(cpal::Device, String), String>>,
    },
}

fn safe_cpal_device_name(d: &cpal::Device) -> Option<String> {
    crate::logger::with_suppressed_panic_log(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| d.name().ok()))
            .ok()
            .flatten()
    })
}

fn collect_input_names_on_host(host: &cpal::Host) -> Result<(Vec<String>, bool), String> {
    let devs = host
        .input_devices()
        .map_err(|e| format!("input_devices: {e}"))?;
    let mut mics = Vec::new();
    for d in devs {
        if let Some(name) = safe_cpal_device_name(&d) {
            let trimmed = name.trim().to_string();
            if !trimmed.is_empty()
                && !is_ignored_virtual_or_ir_device(&trimmed)
                && !mics.contains(&trimmed)
            {
                mics.push(trimmed);
            }
        }
    }
    let has_default = host.default_input_device().is_some();
    Ok((mics, has_default))
}

fn find_input_device_on_host(
    host: &cpal::Host,
    preferred: &str,
) -> Result<(cpal::Device, String), String> {
    let pref = preferred.trim();
    if !pref.is_empty() {
        if let Ok(devs) = host.input_devices() {
            for d in devs {
                if let Some(name) = safe_cpal_device_name(&d) {
                    let trimmed = name.trim().to_string();
                    if trimmed == pref {
                        return Ok((d, trimmed));
                    }
                }
            }
        }
    }
    let dev = host
        .default_input_device()
        .ok_or_else(|| "no default input device".to_string())?;
    let actual = safe_cpal_device_name(&dev)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".to_string());
    Ok((dev, actual))
}

fn find_output_device_on_host(host: &cpal::Host) -> Result<(cpal::Device, String), String> {
    let dev = host
        .default_output_device()
        .ok_or_else(|| "no default output device".to_string())?;
    let actual = safe_cpal_device_name(&dev)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".to_string());
    Ok((dev, actual))
}

/// Process-lifetime dedicated COM/WASAPI audio enumerator thread.
/// Keeping `cpal::default_host()` on a single non-exiting thread prevents `cpal 0.15.3`'s
/// `OnceLock<Enumerator>` STA `CoUninitialize()` poisoning across short-lived scan threads.
static AUDIO_ENUM_TX: LazyLock<mpsc::Sender<AudioEnumReq>> = LazyLock::new(|| {
    let (tx, rx) = mpsc::channel::<AudioEnumReq>();
    let _ = thread::Builder::new()
        .name("hz-audio-enumerator".into())
        .spawn(move || {
            let host = cpal::default_host();
            while let Ok(req) = rx.recv() {
                match req {
                    AudioEnumReq::EnumerateInputs { reply } => {
                        let res = collect_input_names_on_host(&host);
                        let _ = reply.send(res);
                    }
                    AudioEnumReq::ResolveInputDevice { preferred, reply } => {
                        let res = find_input_device_on_host(&host, &preferred);
                        let _ = reply.send(res);
                    }
                    AudioEnumReq::ResolveOutputDevice { reply } => {
                        let res = find_output_device_on_host(&host);
                        let _ = reply.send(res);
                    }
                }
            }
        });
    tx
});

fn enumerate_input_devices_via_worker() -> Result<(Vec<String>, bool), String> {
    let (reply_tx, reply_rx) = mpsc::channel();
    AUDIO_ENUM_TX
        .send(AudioEnumReq::EnumerateInputs { reply: reply_tx })
        .map_err(|_| "hz-audio-enumerator channel closed".to_string())?;
    reply_rx
        .recv_timeout(Duration::from_secs(3))
        .map_err(|e| format!("hz-audio-enumerator timeout: {e}"))?
}

/// Resolve a `cpal` input device matching `preferred` (or system default) via the persistent
/// `"hz-audio-enumerator"` thread so Windows COM/WASAPI remains healthy.
pub fn resolve_input_device(preferred: &str) -> Result<(cpal::Device, String), String> {
    let (reply_tx, reply_rx) = mpsc::channel();
    AUDIO_ENUM_TX
        .send(AudioEnumReq::ResolveInputDevice {
            preferred: preferred.trim().to_string(),
            reply: reply_tx,
        })
        .map_err(|_| "hz-audio-enumerator channel closed".to_string())?;
    reply_rx
        .recv_timeout(Duration::from_secs(3))
        .map_err(|e| format!("hz-audio-enumerator timeout: {e}"))?
}

/// Resolve the system default `cpal` output device via the persistent `"hz-audio-enumerator"` thread.
pub fn resolve_output_device() -> Result<(cpal::Device, String), String> {
    let (reply_tx, reply_rx) = mpsc::channel();
    AUDIO_ENUM_TX
        .send(AudioEnumReq::ResolveOutputDevice { reply: reply_tx })
        .map_err(|_| "hz-audio-enumerator channel closed".to_string())?;
    reply_rx
        .recv_timeout(Duration::from_secs(3))
        .map_err(|e| format!("hz-audio-enumerator timeout: {e}"))?
}

/// Check whether `ffmpeg` is installed and reachable in `PATH` (cached after first probe).
pub fn is_ffmpeg_available() -> bool {
    static FFMPEG_OK: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.arg("-version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let ok = cmd.status().map(|s| s.success()).unwrap_or(false);
        FFMPEG_PROBE_STATE.store(if ok { 2 } else { 1 }, Ordering::Release);
        ok
    });
    let ok = *FFMPEG_OK;
    FFMPEG_PROBE_STATE.store(if ok { 2 } else { 1 }, Ordering::Release);
    ok
}

/// Non-blocking check for `ffmpeg` availability on the UI thread; returns `None` while the
/// background probe is still running so the GUI never stalls spawning a subprocess.
pub fn is_ffmpeg_available_nonblocking() -> Option<bool> {
    match FFMPEG_PROBE_STATE.load(Ordering::Acquire) {
        1 => return Some(false),
        2 => return Some(true),
        _ => {}
    }
    if FFMPEG_PROBING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        let spawned = std::thread::Builder::new()
            .name("hz-ffmpeg-probe".into())
            .spawn(|| {
                let _ = is_ffmpeg_available();
                FFMPEG_PROBING.store(false, Ordering::Release);
            });
        if spawned.is_err() {
            FFMPEG_PROBING.store(false, Ordering::Release);
        }
    }
    None
}
/// Merge a fresh microphone enumeration result against the previously cached list so a transient
/// COM/WASAPI error or empty glitch never wipes previously valid microphones from the UI dropdown.
/// Returns `(merged_mics, preserved_on_fault)`.
fn merge_mic_scan_result(
    prev: &[String],
    scan: Result<(Vec<String>, bool), &str>,
) -> (Vec<String>, bool) {
    match scan {
        Ok((new_mics, _has_default)) if !new_mics.is_empty() => (new_mics, false),
        Ok((_empty, true)) if !prev.is_empty() => (prev.to_vec(), true),
        Ok((empty, _)) => (empty, false),
        Err(_) => (prev.to_vec(), true),
    }
}

fn probe_media_devices_sync(prev_mics: &[String]) -> (Vec<String>, Vec<String>) {
    let scan = enumerate_input_devices_via_worker();
    let (mics, preserved_on_fault) = match &scan {
        Ok((list, has_default)) => {
            merge_mic_scan_result(prev_mics, Ok((list.clone(), *has_default)))
        }
        Err(err) => merge_mic_scan_result(prev_mics, Err(err.as_str())),
    };
    match &scan {
        Ok((found, has_default)) => {
            crate::logger::log_voip_event(
                "audio::device_scan",
                &format!(
                    "mics_found={} cached_mics={} has_default={has_default} preserved={preserved_on_fault}",
                    found.len(),
                    mics.len()
                ),
                preserved_on_fault,
            );
        }
        Err(err) => {
            crate::logger::log_voip_event(
                "audio::device_scan",
                &format!("scan_err={err} preserved_mics={}", mics.len()),
                true,
            );
        }
    }
    let cams = enumerate_ffmpeg_cameras();
    (mics, cams)
}

/// Return cached `(mic_devices, camera_devices)` or enumerate once (with `refresh=true` to rescan).
pub fn list_media_devices(refresh: bool) -> (Vec<String>, Vec<String>) {
    let prev_mics = CACHED_DEVICES
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|c| c.mics.clone()))
        .unwrap_or_default();
    if !refresh {
        if let Ok(g) = CACHED_DEVICES.lock() {
            if let Some(ref cached) = *g {
                return (cached.mics.clone(), cached.cams.clone());
            }
        }
    }
    let (mics, cams) = probe_media_devices_sync(&prev_mics);
    if let Ok(mut g) = CACHED_DEVICES.lock() {
        *g = Some(CachedDevices {
            mics: mics.clone(),
            cams: cams.clone(),
            updated_at: Instant::now(),
        });
    }
    (mics, cams)
}

/// Return cached `(mics, cams, is_scanning)` immediately without blocking the UI thread.
/// Spawns a background scan on cold cache, when `refresh` is true, or when `auto_refresh_ttl` has elapsed.
pub fn list_media_devices_with_ttl(
    refresh: bool,
    auto_refresh_ttl: Option<Duration>,
) -> (Vec<String>, Vec<String>, bool) {
    let snapshot = CACHED_DEVICES
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|c| (c.mics.clone(), c.cams.clone(), c.updated_at)));
    let needs_scan = match (&snapshot, auto_refresh_ttl) {
        _ if refresh => true,
        (None, _) => true,
        (Some((_, _, updated_at)), Some(ttl)) => updated_at.elapsed() >= ttl,
        (Some(_), None) => false,
    };
    let (mics, cams) = snapshot
        .map(|(m, c, _)| (m, c))
        .unwrap_or_default();
    if !needs_scan {
        return (mics, cams, DEVICES_SCANNING.load(Ordering::Acquire));
    }
    if DEVICES_SCANNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        let prev_mics = mics.clone();
        let spawned = std::thread::Builder::new()
            .name("hz-media-scan".into())
            .spawn(move || {
                struct ScanResetGuard;
                impl Drop for ScanResetGuard {
                    fn drop(&mut self) {
                        DEVICES_SCANNING.store(false, Ordering::SeqCst);
                    }
                }
                let _guard = ScanResetGuard;
                let (new_mics, new_cams) = probe_media_devices_sync(&prev_mics);
                if let Ok(mut g) = CACHED_DEVICES.lock() {
                    *g = Some(CachedDevices {
                        mics: new_mics,
                        cams: new_cams,
                        updated_at: Instant::now(),
                    });
                }
            });
        if spawned.is_err() {
            DEVICES_SCANNING.store(false, Ordering::SeqCst);
        }
    }
    (mics, cams, DEVICES_SCANNING.load(Ordering::Acquire))
}

/// Return cached `(mics, cams, is_scanning)` immediately without blocking the UI thread.
/// Spawns a background scan on cold cache or when `refresh` is true.
pub fn list_media_devices_nonblocking(refresh: bool) -> (Vec<String>, Vec<String>, bool) {
    list_media_devices_with_ttl(refresh, None)
}

fn enumerate_ffmpeg_cameras() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.args([
            "-hide_banner",
            "-list_devices",
            "true",
            "-f",
            "dshow",
            "-i",
            "dummy",
        ])
        .creation_flags(0x08000000);
        if let Ok(out) = cmd.output() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut cams = Vec::new();
            for line in stderr.lines() {
                if line.contains("Alternative name") {
                    continue;
                }
                if line.contains("(video)") && line.contains('"') {
                    let parts: Vec<&str> = line.split('"').collect();
                    if parts.len() >= 2 {
                        let name = parts[1].trim().to_string();
                        if !name.is_empty()
                            && !is_ignored_virtual_or_ir_device(&name)
                            && !cams.contains(&name)
                        {
                            cams.push(name);
                        }
                    }
                }
            }
            if cams.is_empty() {
                for line in stderr.lines() {
                    if line.contains("Alternative name") {
                        continue;
                    }
                    if line.contains("(none)") && line.contains('"') {
                        let parts: Vec<&str> = line.split('"').collect();
                        if parts.len() >= 2 {
                            let name = parts[1].trim().to_string();
                            if !name.is_empty()
                                && !is_ignored_virtual_or_ir_device(&name)
                                && !cams.contains(&name)
                            {
                                cams.push(name);
                            }
                        }
                    }
                }
            }
            return cams;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.args([
            "-hide_banner",
            "-f",
            "avfoundation",
            "-list_devices",
            "true",
            "-i",
            "",
        ]);
        if let Ok(out) = cmd.output() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut cams = Vec::new();
            let mut in_video_section = false;
            for line in stderr.lines() {
                if line.contains("AVFoundation video devices:") {
                    in_video_section = true;
                    continue;
                }
                if line.contains("AVFoundation audio devices:") {
                    break;
                }
                if in_video_section {
                    if let Some((_, right)) = line.rsplit_once(']') {
                        let name = right.trim().to_string();
                        if !name.is_empty() && !cams.contains(&name) {
                            cams.push(name);
                        }
                    }
                }
            }
            if !cams.is_empty() {
                return cams;
            }
        }
        return vec!["0".to_string()];
    }
    #[cfg(target_os = "linux")]
    {
        let mut cams = Vec::new();
        for idx in 0..8 {
            let dev = format!("/dev/video{idx}");
            if std::path::Path::new(&dev).exists() {
                cams.push(dev);
            }
        }
        return cams;
    }
    #[allow(unreachable_code)]
    Vec::new()
}

pub struct MediaHandle {
    stop: Arc<AtomicBool>,
    mic_muted: Arc<AtomicBool>,
    cam_enabled: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}
impl MediaHandle {
    pub fn start(rtp_addr: String, local_port: u16) -> Result<Self> {
        Self::start_with_sockets(
            rtp_addr,
            String::new(),
            0,
            0,
            true,
            0,
            None,
            None,
            local_port,
            false,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_with_sockets(
        rtp_addr: String,
        rtcp_addr: String,
        from_uid_u32: u32,
        partner_uid_u32: u32,
        is_caller: bool,
        relay_token: u32,
        udp_rtp_sock: Option<Arc<tokio::net::UdpSocket>>,
        udp_rtcp_sock: Option<Arc<tokio::net::UdpSocket>>,
        local_port: u16,
        is_video: bool,
        gui_tx: Option<crate::gui::GuiTx>,
        egui_ctx: Option<eframe::egui::Context>,
    ) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let mic_muted = Arc::new(AtomicBool::new(false));
        let cam_enabled = Arc::new(AtomicBool::new(is_video));
        let mut threads = Vec::new();
        let slot_idx = if udp_rtp_sock.is_some() {
            local_port
        } else {
            0
        };
        let fallback_sock = if udp_rtp_sock.is_none() {
            let s = UdpSocket::bind(format!("0.0.0.0:{local_port}"))?;
            let _ = s.set_read_timeout(Some(Duration::from_millis(10)));
            Some(Arc::new(s))
        } else {
            None
        };

        let stop_t = stop.clone();
        let mute_t = mic_muted.clone();
        let cam_t = cam_enabled.clone();
        let t = thread::spawn(move || {
            run_media_loop(
                stop_t,
                mute_t,
                cam_t,
                udp_rtp_sock,
                udp_rtcp_sock,
                fallback_sock,
                rtp_addr,
                rtcp_addr,
                from_uid_u32,
                partner_uid_u32,
                is_caller,
                relay_token,
                slot_idx,
                is_video,
                gui_tx,
                egui_ctx,
            );
        });
        threads.push(t);

        Ok(Self {
            stop,
            mic_muted,
            cam_enabled,
            threads,
        })
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn set_mic_muted(&self, muted: bool) {
        self.mic_muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_cam_enabled(&self, enabled: bool) {
        self.cam_enabled.store(enabled, Ordering::Relaxed);
    }
}

impl Drop for MediaHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Unwrap a Zalo UDP relay packet (`0x00893524..0x008936D1` in `ZaloCall.exe`) into its inner RTP slice.
pub fn unwrap_zalo_relay_rtp(pkt: &[u8]) -> Option<&[u8]> {
    if pkt.len() < 12 {
        return None;
    }
    let inner = match pkt[0] {
        // 1-byte relay media header (`0x04` = incoming audio RTP, `0x07` = P2P RTP, `0x0E` = incoming video RTP)
        0x04 | 0x07 | 0x0E => pkt.get(1..)?,
        // 5-byte relay media header (`0x03` / `0x05` / `0x0D` / `0x0F` + `token: u32 LE`)
        0x03 | 0x05 | 0x0D | 0x0F => pkt.get(5..)?,
        // 9-byte `0x7F` relay media header (`0x7F, 0x00, role, call_id: u32 LE, cmd: u16 LE`)
        0x7F if pkt.len() > 9 => {
            let cmd = u16::from_le_bytes([pkt[7], pkt[8]]);
            if (6..=10).contains(&cmd) {
                pkt.get(9..)?
            } else {
                return None;
            }
        }
        // 25-byte `0x02` relay media header (`cmd == 0x0032` at `0x008937C8`)
        0x02 if pkt.len() > 25 && u16::from_le_bytes([pkt[18], pkt[19]]) == 0x0032 => {
            pkt.get(25..)?
        }
        // Standard raw RTP v2 (`V=2` in top 2 bits)
        b if (b & 0xC0) == 0x80 => pkt,
        _ => return None,
    };
    if inner.len() >= 12 && (inner[0] & 0xC0) == 0x80 {
        Some(inner)
    } else {
        None
    }
}

/// Inbox for datagrams received on the RTCP socket. The ZRTP keepalive task in `call.rs` is the
/// permanent reader of that socket; it forwards everything that is not a ZRTP init reply here so
/// `run_media_loop` never competes with it for datagrams. Single active call assumed.
static RTCP_INBOX: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());
const RTCP_INBOX_MAX: usize = 512;

/// Queue a raw (still relay-framed) datagram read from the RTCP socket. Bounded; oldest is dropped.
pub fn push_rtcp_inbox(pkt: &[u8]) {
    if let Ok(mut q) = RTCP_INBOX.lock() {
        if q.len() >= RTCP_INBOX_MAX {
            q.pop_front();
        }
        q.push_back(pkt.to_vec());
    }
}

fn drain_rtcp_inbox() -> Vec<Vec<u8>> {
    RTCP_INBOX
        .lock()
        .map(|mut q| q.drain(..).collect())
        .unwrap_or_default()
}

fn clear_rtcp_inbox() {
    if let Ok(mut q) = RTCP_INBOX.lock() {
        q.clear();
    }
}

/// Cross-leg audio conference bridge for multi-member group calls (up to 4 slots).
/// Decoded PCM from slot `s` is pushed into every other slot's queue and mixed into
/// each slot's outbound Opus stream so all group call participants hear one another.
static PEER_AUDIO_BRIDGE: [Mutex<VecDeque<f32>>; 4] = [
    Mutex::new(VecDeque::new()),
    Mutex::new(VecDeque::new()),
    Mutex::new(VecDeque::new()),
    Mutex::new(VecDeque::new()),
];
const PEER_BRIDGE_MAX_SAMPLES: usize = FRAME_SAMPLES * 8;

fn push_peer_bridge_audio(from_slot: u16, pcm: &[f32]) {
    let idx = from_slot as usize;
    if idx >= PEER_AUDIO_BRIDGE.len() {
        return;
    }
    for (other, q_mtx) in PEER_AUDIO_BRIDGE.iter().enumerate() {
        if other == idx {
            continue;
        }
        if let Ok(mut q) = q_mtx.lock() {
            let q_len = q.len();
            if q_len + pcm.len() > PEER_BRIDGE_MAX_SAMPLES {
                let drop_n = ((q_len + pcm.len()) - PEER_BRIDGE_MAX_SAMPLES).min(q_len);
                q.drain(..drop_n);
            }
            q.extend(pcm.iter().copied());
        }
    }
}

fn mix_peer_bridge_audio(for_slot: u16, frame: &mut [f32]) {
    let idx = for_slot as usize;
    if idx >= PEER_AUDIO_BRIDGE.len() {
        return;
    }
    if let Ok(mut q) = PEER_AUDIO_BRIDGE[idx].lock() {
        if q.len() >= frame.len() {
            for s in frame.iter_mut() {
                if let Some(p) = q.pop_front() {
                    *s = (*s + p).clamp(-1.0, 1.0);
                }
            }
        }
    }
}

/// True if a raw relay-framed datagram is RTCP (PT 200..=207) containing a PLI or FIR.
#[cfg(test)]
fn wire_requests_keyframe(wire: &[u8]) -> bool {
    match unwrap_zalo_relay_rtp(wire) {
        Some(inner) => {
            matches!(inner.get(1), Some(b) if (200..=207).contains(b))
                && rtcp_requests_keyframe(inner)
        }
        None => false,
    }
}

const VIDEO_PT: u8 = 97;

/// Zalo video wire format, observed in two live calls (`voip.log` 2026-10-30, peer -> us), NOT RFC 6184:
/// * Whole access unit as Annex-B (`00 00 00 01 <NAL>...`) in one RTP packet, marker = 1.
/// * Larger access units: `0x1C, flags, <chunk of the Annex-B AU>` with flags `0x80` = first, `0x40` = last,
///   `0x00` = middle (low 5 bits 0); marker only on the last fragment. The AU is split **evenly** into
///   `ceil(len / ~1200)` parts (captured: 2x1083, 3x1062 in one call, 853/853/851 in another; the largest
///   unfragmented payload was 1212 bytes).
/// * Payload type: `PT 98` was seen only on the fragments of the first keyframe (SPS/PPS/SEI/IDR); fragmented
///   P frames used `PT 97`. So "PT 98 = keyframe" is an inference from two samples; receive ignores the PT.
const VIDEO_PT_FRAG: u8 = 98;
const ZALO_FRAG_INDICATOR: u8 = 0x1C;
/// Max RTP payload per packet (2-byte fragment header included). Peer packets reached 1212; 1200 is the
/// threshold that reproduces its split sizes. Whether the relay/peer accepts it from us is unverified.
const RTP_MAX_PAYLOAD: usize = 1200;
const ZALO_FRAG_BUF_MAX: usize = 1 << 20;
const VIDEO_IDR_INTERVAL_FRAMES: u32 = 15;

// Valid libx264-encoded H.264 Constrained Baseline (320x240, 15 fps) SPS/PPS/IDR/P-slice NAL units
const H264_SPS: &[u8] = &[
    0x67, 0x42, 0xC0, 0x1E, 0xDA, 0x05, 0x07, 0xEC, 0x04, 0x40, 0x00, 0x00, 0x03, 0x00, 0x40, 0x00,
    0x00, 0x07, 0x83, 0xC5, 0x8B, 0xA8,
];
const H264_PPS: &[u8] = &[0x68, 0xCE, 0x0F, 0x2C, 0x80];
const H264_IDR: &[u8] = &[
    0x65, 0x88, 0x84, 0x5C, 0x46, 0x28, 0x00, 0x08, 0x6C, 0xC7, 0x00, 0x02, 0x51, 0xC0, 0x19, 0x39,
    0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39, 0x39,
    0x39, 0x3A, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBC,
];
const H264_IDR_ALT: &[u8] = &[
    0x65, 0x88, 0x82, 0x1F, 0x11, 0x8A, 0x00, 0x02, 0x22, 0xF1, 0xC0, 0x00, 0xB4, 0x70, 0x01, 0x93,
    0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93, 0x93,
    0x93, 0x93, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB,
    0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE,
    0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA, 0xEB, 0xAE, 0xBA,
    0xEB, 0xAE, 0xBA, 0xEB, 0xC0,
];
#[cfg(test)]
const H264_PSLICE: &[u8] = &[0x41, 0x9A, 0x21, 0x70, 0x09, 0x6C];
const H264_PSLICES: [&[u8]; 14] = [
    &[0x41, 0x9A, 0x21, 0x70, 0x09, 0x6C],
    &[0x41, 0x9A, 0x41, 0x70, 0x09, 0x6C],
    &[0x41, 0x9A, 0x61, 0x70, 0x09, 0x6C],
    &[0x41, 0x9A, 0x81, 0x70, 0x09, 0x6C],
    &[0x41, 0x9A, 0xA1, 0x70, 0x09, 0x6C],
    &[0x41, 0x9A, 0xC1, 0x70, 0x09, 0x6C],
    &[0x41, 0x9A, 0xE1, 0x70, 0x09, 0x6C],
    &[0x41, 0x9B, 0x01, 0x70, 0x09, 0x6C],
    &[0x41, 0x9B, 0x21, 0x70, 0x09, 0x6C],
    &[0x41, 0x9B, 0x41, 0x70, 0x09, 0x6C],
    &[0x41, 0x9B, 0x61, 0x70, 0x09, 0x6C],
    &[0x41, 0x9B, 0x81, 0x70, 0x09, 0x6C],
    &[0x41, 0x9B, 0xA1, 0x70, 0x09, 0x6C],
    &[0x41, 0x9B, 0xC1, 0x70, 0x09, 0x6C],
];

/// Build a 12-byte-header RTP v2 packet.
fn build_rtp_packet(pt: u8, marker: bool, seq: u16, ts: u32, ssrc: u32, payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(12 + payload.len());
    p.push(0x80);
    p.push(if marker { 0x80 | pt } else { pt });
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&ts.to_be_bytes());
    p.extend_from_slice(&ssrc.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

/// Build an RTP v2 audio packet with RFC 5285 one-byte header extensions (`0xBEDE`, 3 words = 12 bytes)
/// matching `ZaloCall.exe` group audio packets (`0x20` audio level + `0xD6` 64-bit NTP timestamp).
fn build_group_audio_rtp_packet(
    pt: u8,
    seq: u16,
    ts: u32,
    ssrc: u32,
    ntp: (u32, u32),
    payload: &[u8],
) -> Vec<u8> {
    let mut p = Vec::with_capacity(28 + payload.len());
    p.push(0x90); // V=2, P=0, X=1, CC=0
    p.push(pt);
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&ts.to_be_bytes());
    p.extend_from_slice(&ssrc.to_be_bytes());
    p.extend_from_slice(&[0xBE, 0xDE, 0x00, 0x03]);
    p.extend_from_slice(&[0x20, 0x4A]); // ID=2 (audio level), len=1
    p.push(0xD6); // ID=13 (Zalo NTP timestamp), len=7 (24-bitntp_sec + 32-bitntp_frac)
    let (ntp_sec, ntp_frac) = ntp;
    let sec_be = ntp_sec.to_be_bytes();
    p.extend_from_slice(&[sec_be[1], sec_be[2], sec_be[3]]);
    p.extend_from_slice(&ntp_frac.to_be_bytes());
    p.extend_from_slice(&[0x00, 0x00]); // 2 padding bytes -> 12 bytes (3 words)
    p.extend_from_slice(payload);
    p
}

/// Build an RTP v2 video packet with RFC 5285 one-byte header extensions (`0xBEDE`, 5 words = 20 bytes)
/// required by Zalo's Group Call SFU (`RtpHeaderExtensionMap` at `0x00A77943..0x00A77ED1` in `ZaloCall.exe`):
/// - Extension ID 3 (`abs-send-time`, len=3): `0x32, t2, t1, t0`
/// - Extension ID 5 (`transport-wide-cc-02`, len=2): `0x51, twcc_hi, twcc_lo`
/// - Extension ID 12 (`Zalo simulcast layer info`, len=3): `0xC2, layer_byte, pic_hi, pic_lo`
/// - Extension ID 13 (`Zalo 56-bit NTP timestamp`, len=7): `0xD6, sec24_be[0..3], frac32_be[0..4], 0x00`
fn build_group_video_rtp_packet(
    pt: u8,
    marker: bool,
    seq: u16,
    ts: u32,
    ssrc: u32,
    pic_id: u16,
    layer_byte: u8,
    ntp: (u32, u32),
    payload: &[u8],
) -> Vec<u8> {
    let mut p = Vec::with_capacity(36 + payload.len());
    p.push(0x90); // V=2, P=0, X=1 (header extension present), CC=0
    p.push(if marker { 0x80 | pt } else { pt });
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&ts.to_be_bytes());
    p.extend_from_slice(&ssrc.to_be_bytes());
    // RFC 5285 1-byte header extension profile 0xBEDE, length = 5 (20 bytes)
    p.extend_from_slice(&[0xBE, 0xDE, 0x00, 0x05]);
    let abs_send_24 = ((ts / 3) & 0x00FF_FFFF).to_be_bytes();
    p.extend_from_slice(&[0x32, abs_send_24[1], abs_send_24[2], abs_send_24[3]]);
    let twcc = seq.wrapping_add(1).to_be_bytes();
    p.extend_from_slice(&[0x51, twcc[0], twcc[1]]);
    let pic = pic_id.to_be_bytes();
    p.extend_from_slice(&[0xC2, layer_byte, pic[0], pic[1]]);
    p.push(0xD6);
    let (ntp_sec, ntp_frac) = ntp;
    let sec_be = ntp_sec.to_be_bytes();
    p.extend_from_slice(&[sec_be[1], sec_be[2], sec_be[3]]);
    p.extend_from_slice(&ntp_frac.to_be_bytes());
    p.push(0x00); // 1 padding byte -> 20 bytes (5 words)
    p.extend_from_slice(payload);
    p
}

/// Concatenate NAL units (no start code) into one Annex-B access unit.
fn annexb_au(nals: &[&[u8]]) -> Vec<u8> {
    let mut v = Vec::new();
    for n in nals {
        v.extend_from_slice(&[0, 0, 0, 1]);
        v.extend_from_slice(n);
    }
    v
}

fn is_annexb(p: &[u8]) -> bool {
    p.starts_with(&[0, 0, 0, 1]) || p.starts_with(&[0, 0, 1])
}

fn is_zalo_frag(p: &[u8]) -> bool {
    p.len() >= 2 && (p[0] & 0x1F) == 28 && (p[1] & 0x1F) == 0
}

/// True if the Annex-B access unit contains a NAL unit whose type is in `types`.
fn au_has_nal_type(au: &[u8], types: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 3 < au.len() {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            if types.contains(&(au[i + 3] & 0x1F)) {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

/// True if the Annex-B access unit contains an IDR slice (NAL type 5).
fn au_has_idr(au: &[u8]) -> bool {
    au_has_nal_type(au, &[5])
}

/// True if the access unit can start decoding: it carries an IDR slice (5) or an SPS (7).
#[cfg(test)]
fn au_is_sync_point(au: &[u8]) -> bool {
    au_has_nal_type(au, &[5, 7])
}

/// Split one Annex-B access unit into Zalo video payloads: `(pt, payload)` per RTP packet.
/// Fits in one packet -> `(pt, au)`. Otherwise it is split evenly into `ceil(len / (max_payload - 2))` fragments;
/// keyframes (`au_has_idr`) always use `PT 98` (`0x62`) and delta frames use `PT 97` (`0x61`), matching
/// `RTPSender::SendOutgoingData` (`0x727948`) and `RTPReceiverVideo::ParseRtpPacket` (`0x7185c0`) in `libzrtc.so`.
fn packetize_zalo_au(au: &[u8], max_payload: usize) -> Vec<(u8, Vec<u8>)> {
    if au.is_empty() {
        return Vec::new();
    }
    let pt = if au_has_idr(au) {
        VIDEO_PT_FRAG
    } else {
        VIDEO_PT
    };
    if au.len() <= max_payload || max_payload < 3 {
        return vec![(pt, au.to_vec())];
    }
    let parts = au.len().div_ceil(max_payload - 2);
    let chunk = au.len().div_ceil(parts);
    let n = au.len().div_ceil(chunk); // actual number of fragments produced by `chunks(chunk)`
    au.chunks(chunk)
        .enumerate()
        .map(|(i, c)| {
            let mut flags = 0u8;
            if i == 0 {
                flags |= 0x80;
            }
            if i + 1 == n {
                flags |= 0x40;
            }
            let mut v = Vec::with_capacity(2 + c.len());
            v.push(ZALO_FRAG_INDICATOR);
            v.push(flags);
            v.extend_from_slice(c);
            (pt, v)
        })
        .collect()
}

/// Resolve which remote SSRC should receive RTCP video feedback (`PLI` / `REMB` / `RR`).
/// Never allows audio SSRC to clobber a known video SSRC.
fn resolve_video_feedback_ssrc(peer_video_ssrc: u32, peer_audio_ssrc: u32) -> Option<u32> {
    if peer_video_ssrc != 0 {
        Some(peer_video_ssrc)
    } else if peer_audio_ssrc != 0 {
        Some(peer_audio_ssrc)
    } else {
        None
    }
}

/// True when an incoming RTP packet carries video media rather than Opus audio, PCMU, RTCP, or FEC.
fn is_incoming_video_rtp(pt: u8, outer_byte: u8, payload: &[u8]) -> bool {
    if pt == 112 || pt == 0 || (200..=207).contains(&pt) {
        return false;
    }
    if matches!(pt, 96 | 97 | VIDEO_PT_FRAG | 121 | 122) {
        return true;
    }
    outer_byte == 0x0E && (is_annexb(payload) || is_zalo_frag(payload))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FragMode {
    None,
    Zalo,
    FuA,
}

#[derive(Debug, PartialEq, Eq)]
enum DepacketizeResult {
    Ready(Vec<u8>),
    Buffering,
    DroppedCorrupt,
    Ignored,
}

/// Stateful RTP H.264 depacketizer supporting Zalo Annex-B / `0x1C` fragments and RFC 6184
/// Single NAL / STAP-A / FU-A with strict RTP `seq`/`ts` continuity validation.
struct VideoDepacketizer {
    buf: Vec<u8>,
    mode: FragMode,
    last_seq: Option<u16>,
    frag_ts: Option<u32>,
    corrupt_frags_dropped: u64,
}

impl Default for VideoDepacketizer {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoDepacketizer {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(65536),
            mode: FragMode::None,
            last_seq: None,
            frag_ts: None,
            corrupt_frags_dropped: 0,
        }
    }

    fn reset_frag(&mut self) {
        self.buf.clear();
        self.mode = FragMode::None;
        self.last_seq = None;
        self.frag_ts = None;
    }

    fn push(&mut self, seq: u16, ts: u32, payload: &[u8]) -> DepacketizeResult {
        if payload.is_empty() {
            return DepacketizeResult::Ignored;
        }
        if is_annexb(payload) {
            self.reset_frag();
            return DepacketizeResult::Ready(payload.to_vec());
        }
        if is_zalo_frag(payload) {
            let flags = payload[1];
            let start_bit = (flags & 0x80) != 0;
            let end_bit = (flags & 0x40) != 0;
            let body = &payload[2..];
            if start_bit {
                self.reset_frag();
                self.buf.extend_from_slice(body);
                if self.buf.len() > ZALO_FRAG_BUF_MAX {
                    self.reset_frag();
                    self.corrupt_frags_dropped += 1;
                    return DepacketizeResult::DroppedCorrupt;
                }
                if end_bit {
                    let done = std::mem::take(&mut self.buf);
                    return if done.is_empty() {
                        DepacketizeResult::Ignored
                    } else {
                        DepacketizeResult::Ready(done)
                    };
                }
                self.mode = FragMode::Zalo;
                self.last_seq = Some(seq);
                self.frag_ts = Some(ts);
                return DepacketizeResult::Buffering;
            }
            let seq_ok = self.last_seq == Some(seq.wrapping_sub(1));
            let ts_ok = self.frag_ts == Some(ts);
            if self.mode != FragMode::Zalo || !seq_ok || !ts_ok || self.buf.is_empty() {
                self.reset_frag();
                self.corrupt_frags_dropped += 1;
                return DepacketizeResult::DroppedCorrupt;
            }
            self.buf.extend_from_slice(body);
            if self.buf.len() > ZALO_FRAG_BUF_MAX {
                self.reset_frag();
                self.corrupt_frags_dropped += 1;
                return DepacketizeResult::DroppedCorrupt;
            }
            self.last_seq = Some(seq);
            if end_bit {
                let done = std::mem::take(&mut self.buf);
                self.reset_frag();
                return if done.is_empty() {
                    DepacketizeResult::Ignored
                } else {
                    DepacketizeResult::Ready(done)
                };
            }
            return DepacketizeResult::Buffering;
        }
        self.push_rfc6184(seq, ts, payload)
    }

    fn push_rfc6184(&mut self, seq: u16, ts: u32, payload: &[u8]) -> DepacketizeResult {
        let nal_type = payload[0] & 0x1F;
        match nal_type {
            1..=23 => {
                self.reset_frag();
                let mut annexb = Vec::with_capacity(4 + payload.len());
                annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                annexb.extend_from_slice(payload);
                DepacketizeResult::Ready(annexb)
            }
            24 => {
                self.reset_frag();
                let mut idx = 1usize;
                let mut annexb = Vec::new();
                while idx + 2 <= payload.len() {
                    let sz = u16::from_be_bytes([payload[idx], payload[idx + 1]]) as usize;
                    idx += 2;
                    if sz > 0 && idx + sz <= payload.len() {
                        annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                        annexb.extend_from_slice(&payload[idx..idx + sz]);
                        idx += sz;
                    } else {
                        break;
                    }
                }
                if annexb.is_empty() {
                    DepacketizeResult::Ignored
                } else {
                    DepacketizeResult::Ready(annexb)
                }
            }
            28 if payload.len() >= 2 => {
                let fu_ind = payload[0];
                let fu_hdr = payload[1];
                let start_bit = (fu_hdr & 0x80) != 0;
                let end_bit = (fu_hdr & 0x40) != 0;
                let orig_nal_hdr = (fu_ind & 0xE0) | (fu_hdr & 0x1F);
                if start_bit {
                    self.reset_frag();
                    self.buf
                        .extend_from_slice(&[0x00, 0x00, 0x00, 0x01, orig_nal_hdr]);
                    self.buf.extend_from_slice(&payload[2..]);
                    if self.buf.len() > ZALO_FRAG_BUF_MAX {
                        self.reset_frag();
                        self.corrupt_frags_dropped += 1;
                        return DepacketizeResult::DroppedCorrupt;
                    }
                    if end_bit {
                        let done = std::mem::take(&mut self.buf);
                        return DepacketizeResult::Ready(done);
                    }
                    self.mode = FragMode::FuA;
                    self.last_seq = Some(seq);
                    self.frag_ts = Some(ts);
                    return DepacketizeResult::Buffering;
                }
                let seq_ok = self.last_seq == Some(seq.wrapping_sub(1));
                let ts_ok = self.frag_ts == Some(ts);
                if self.mode != FragMode::FuA || !seq_ok || !ts_ok || self.buf.is_empty() {
                    self.reset_frag();
                    self.corrupt_frags_dropped += 1;
                    return DepacketizeResult::DroppedCorrupt;
                }
                self.buf.extend_from_slice(&payload[2..]);
                if self.buf.len() > ZALO_FRAG_BUF_MAX {
                    self.reset_frag();
                    self.corrupt_frags_dropped += 1;
                    return DepacketizeResult::DroppedCorrupt;
                }
                self.last_seq = Some(seq);
                if end_bit {
                    let done = std::mem::take(&mut self.buf);
                    self.reset_frag();
                    return DepacketizeResult::Ready(done);
                }
                DepacketizeResult::Buffering
            }
            _ => DepacketizeResult::Ignored,
        }
    }
}

#[cfg(test)]
fn depacketize_h264(
    payload: &[u8],
    frag_buf: &mut Vec<u8>,
    h264_tx: &std::sync::mpsc::SyncSender<Vec<u8>>,
) {
    if payload.is_empty() {
        return;
    }
    if is_annexb(payload) {
        frag_buf.clear();
        let _ = h264_tx.try_send(payload.to_vec());
        return;
    }
    if is_zalo_frag(payload) {
        let flags = payload[1];
        let body = &payload[2..];
        if flags & 0x80 != 0 {
            frag_buf.clear();
            frag_buf.extend_from_slice(body);
        } else if !frag_buf.is_empty() {
            frag_buf.extend_from_slice(body);
        } else {
            return;
        }
        if frag_buf.len() > ZALO_FRAG_BUF_MAX {
            frag_buf.clear();
            return;
        }
        if flags & 0x40 != 0 {
            let done = std::mem::take(frag_buf);
            if !done.is_empty() {
                let _ = h264_tx.try_send(done);
            }
        }
        return;
    }
    depacketize_rfc6184_h264(payload, frag_buf, h264_tx);
}

/// RFC 6184 packetization of one NAL unit (header byte included, no start code).
/// No longer used on the wire (the peer sends/expects the Zalo format above); kept to test the RFC 6184 receive path.
/// Returns one single-NAL payload when it fits in `max_payload`, otherwise FU-A fragments.
#[cfg(test)]
fn packetize_h264_nal(nal: &[u8], max_payload: usize) -> Vec<Vec<u8>> {
    if nal.is_empty() {
        return Vec::new();
    }
    if nal.len() <= max_payload || max_payload < 3 {
        return vec![nal.to_vec()];
    }
    let indicator = (nal[0] & 0xE0) | 28;
    let nal_type = nal[0] & 0x1F;
    let body = &nal[1..];
    let chunk = max_payload - 2;
    let n = body.len().div_ceil(chunk);
    body.chunks(chunk)
        .enumerate()
        .map(|(i, c)| {
            let mut fu_hdr = nal_type;
            if i == 0 {
                fu_hdr |= 0x80;
            }
            if i + 1 == n {
                fu_hdr |= 0x40;
            }
            let mut v = Vec::with_capacity(2 + c.len());
            v.push(indicator);
            v.push(fu_hdr);
            v.extend_from_slice(c);
            v
        })
        .collect()
}

/// Middle 32 bits of the NTP timestamp of the first Sender Report in a (compound) RTCP packet (the `LSR`
/// value a receiver echoes back in its report block), or `None` if there is no SR.
fn sr_middle_ntp(rtcp: &[u8]) -> Option<u32> {
    let mut i = 0usize;
    while i + 4 <= rtcp.len() {
        if (rtcp[i] & 0xC0) != 0x80 {
            return None;
        }
        let words = u16::from_be_bytes([rtcp[i + 2], rtcp[i + 3]]) as usize;
        if rtcp[i + 1] == 200 {
            let b = rtcp.get(i + 10..i + 14)?;
            return Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        }
        i += (words + 1) * 4;
    }
    None
}

/// Reception statistics of one incoming RTP stream (RFC 3550 A.1, jitter not computed).
#[derive(Default)]
struct RxStats {
    started: bool,
    base_seq: u16,
    max_seq: u16,
    cycles: u32,
    received: u32,
    expected_prior: u32,
    received_prior: u32,
}

impl RxStats {
    /// Count one received packet; returns true if it skipped ahead (at least one packet missing before it).
    fn update(&mut self, seq: u16) -> bool {
        let mut gap = false;
        if !self.started {
            self.started = true;
            self.base_seq = seq;
            self.max_seq = seq;
        } else {
            let d = seq.wrapping_sub(self.max_seq);
            if d != 0 && d < 0x8000 {
                gap = d > 1;
                if seq < self.max_seq {
                    self.cycles = self.cycles.wrapping_add(1 << 16);
                }
                self.max_seq = seq;
            }
        }
        self.received = self.received.wrapping_add(1);
        gap
    }

    /// 24-byte RTCP report block about `ssrc` (None until the first packet arrived). Advances the interval.
    fn block(&mut self, ssrc: u32, lsr: u32, dlsr: u32) -> Option<[u8; 24]> {
        if !self.started {
            return None;
        }
        let ext_max = self.cycles | self.max_seq as u32;
        let expected = ext_max.wrapping_sub(self.base_seq as u32).wrapping_add(1);
        let lost_total = expected.saturating_sub(self.received).min(0x7F_FFFF);
        let exp_int = expected.wrapping_sub(self.expected_prior);
        let rcv_int = self.received.wrapping_sub(self.received_prior);
        self.expected_prior = expected;
        self.received_prior = self.received;
        let lost_int = exp_int.saturating_sub(rcv_int);
        let fraction = if exp_int == 0 {
            0
        } else {
            ((lost_int as u64 * 256) / exp_int as u64).min(255) as u8
        };
        let mut b = [0u8; 24];
        b[0..4].copy_from_slice(&ssrc.to_be_bytes());
        b[4] = fraction;
        b[5..8].copy_from_slice(&lost_total.to_be_bytes()[1..]);
        b[8..12].copy_from_slice(&ext_max.to_be_bytes());
        // b[12..16] interarrival jitter: not computed (0)
        b[16..20].copy_from_slice(&lsr.to_be_bytes());
        b[20..24].copy_from_slice(&dlsr.to_be_bytes());
        Some(b)
    }
}

/// NTP timestamp (seconds since 1900, 2^-32 s fraction) for "now".
fn ntp_now() -> (u32, u32) {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs().wrapping_add(2_208_988_800) as u32;
    let frac = ((d.subsec_nanos() as u64) << 32) / 1_000_000_000;
    (secs, frac as u32)
}

/// RTCP Sender Report (PT 200) with an optional report block. Layout as captured from the peer
/// (`len=52` = 28 + one 24-byte block).
fn build_sender_report(
    ssrc: u32,
    ntp: (u32, u32),
    rtp_ts: u32,
    packets: u32,
    octets: u32,
    block: Option<[u8; 24]>,
) -> Vec<u8> {
    let rc = block.is_some() as u8;
    let words = 6 + 6 * rc as u16; // length field = total 32-bit words - 1
    let mut p = Vec::with_capacity(28 + 24);
    p.push(0x80 | rc);
    p.push(200);
    p.extend_from_slice(&words.to_be_bytes());
    p.extend_from_slice(&ssrc.to_be_bytes());
    p.extend_from_slice(&ntp.0.to_be_bytes());
    p.extend_from_slice(&ntp.1.to_be_bytes());
    p.extend_from_slice(&rtp_ts.to_be_bytes());
    p.extend_from_slice(&packets.to_be_bytes());
    p.extend_from_slice(&octets.to_be_bytes());
    if let Some(b) = block {
        p.extend_from_slice(&b);
    }
    p
}

/// Empty SDES chunk (12 bytes, as sent by the peer): one source, no items.
fn build_sdes_empty(ssrc: u32) -> Vec<u8> {
    let mut p = vec![0x81, 202, 0, 2];
    p.extend_from_slice(&ssrc.to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0]);
    p
}

/// RTCP Picture Loss Indication (PSFB, fmt 1, 12 bytes): ask `media_ssrc` for a keyframe.
fn build_pli(sender_ssrc: u32, media_ssrc: u32) -> Vec<u8> {
    let mut p = vec![0x81, 206, 0, 2];
    p.extend_from_slice(&sender_ssrc.to_be_bytes());
    p.extend_from_slice(&media_ssrc.to_be_bytes());
    p
}
/// RTCP REMB (Payload-Specific Feedback fmt=15, PT=206, 24 bytes): advertise receiver max bitrate (`bitrate_bps`)
/// for `media_ssrc` (`RTCPSender::BuildREMB @ 0x706fb4` in `libzrtc.so`).
fn build_remb(sender_ssrc: u32, media_ssrc: u32, bitrate_bps: u32) -> Vec<u8> {
    let mut exp: u8 = 0;
    let mut mantissa: u32 = bitrate_bps;
    while mantissa > 0x3FFFF && exp < 63 {
        mantissa >>= 1;
        exp += 1;
    }
    let mut p = Vec::with_capacity(24);
    p.extend_from_slice(&[0x8F, 206, 0, 5]);
    p.extend_from_slice(&sender_ssrc.to_be_bytes());
    p.extend_from_slice(&0u32.to_be_bytes());
    p.extend_from_slice(b"REMB");
    p.push(1); // num_ssrc = 1
    p.push((exp << 2) | ((mantissa >> 16) as u8 & 0x03));
    p.extend_from_slice(&(mantissa as u16).to_be_bytes());
    p.extend_from_slice(&media_ssrc.to_be_bytes());
    p
}

/// Build `ZRTPPacket::initZRTPPacketIncallNegotiateInfo` (`0x4ad5a0` / `0x48c9e0` in `libzrtc.so`):
/// `cmd = 0x0020`, `sub_cmd = 0x14`, payload = `[seq: u32 BE, count=2: u8, H264DecCapacityInfo(720p,30fps), NegoNetworkInfo(1)]`.
fn build_zrtp_incall_negotiate_packet(call_id: u32, relay_token: u32, nego_seq: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(11);
    body.extend_from_slice(&nego_seq.to_be_bytes());
    body.push(2);
    body.extend_from_slice(&[0x0B, 0x02, 0xD0, 0x1E]);
    body.extend_from_slice(&[0x29, 0x01]);

    let mut pkt = Vec::with_capacity(25 + body.len());
    pkt.push(0x01); // magic
    pkt.push(0x01); // version
    pkt.extend_from_slice(&nego_seq.to_le_bytes()); // seq
    pkt.extend_from_slice(&0u32.to_le_bytes()); // sub_seq
    pkt.extend_from_slice(&call_id.to_le_bytes()); // [0x10] callId
    pkt.extend_from_slice(&relay_token.to_le_bytes()); // [0x14] token
    pkt.extend_from_slice(&0x0020u16.to_le_bytes()); // [0x18] cmd = 0x0020
    pkt.push(0x14); // [0x1A] sub_cmd = 0x14 (IncallNegotiateInfo)
    pkt.extend_from_slice(&(body.len() as u32).to_le_bytes());
    pkt.extend_from_slice(&body);
    pkt
}

/// Spawn a background webcam capture thread via `ffmpeg -f dshow` (on Windows) that encodes live 320x240 15fps
/// Constrained Baseline H.264 access units for RTP transmission AND decodes a local PIP preview for the call window.
fn spawn_camera_capture_thread(
    stop: Arc<AtomicBool>,
    cam_enabled: Arc<AtomicBool>,
    want_keyframe: Arc<AtomicBool>,
    cam_au_tx: std::sync::mpsc::SyncSender<Vec<u8>>,
    gui_tx: Option<crate::gui::GuiTx>,
    egui_ctx: Option<eframe::egui::Context>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        use std::io::Read;
        use std::process::{Command, Stdio};

        let mut current_dev = String::new();
        let mut child_opt: Option<(ChildGuard, std::process::ChildStdout)> = None;
        let mut failed_devs: Vec<String> = Vec::new();
        let mut read_buf = [0u8; 8192];
        let mut stream_acc: Vec<u8> = Vec::with_capacity(65536);
        let mut warned_missing = false;

        // Optional local preview decoder (`ffmpeg` H.264 -> 240x180 RGBA)
        let (prev_tx, prev_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(15);
        let local_preview_handle = egui_ctx.clone().map(|ctx| {
            let stop_p = stop.clone();
            thread::spawn(move || {
                run_local_preview_decoder(stop_p, prev_rx, ctx);
            })
        });

        while !stop.load(Ordering::Relaxed) {
            if !cam_enabled.load(Ordering::Relaxed) {
                if let Some((mut guard, stdout)) = child_opt.take() {
                    drop(stdout);
                    guard.kill_and_reap();
                }
                thread::sleep(Duration::from_millis(80));
                continue;
            }

            if !is_ffmpeg_available() {
                if !warned_missing {
                    warned_missing = true;
                    crate::voip_log::line(
                        "VCAM",
                        "ffmpeg not found in PATH; camera capture unavailable",
                    );
                    if let Some(ref gtx) = gui_tx {
                        let _ = gtx.send(crate::gui::GuiEvent::StatusUpdate(
                            "⚠️ Không tìm thấy ffmpeg trong PATH để bật Camera (cài bằng: winget install Gyan.FFmpeg)".into(),
                        ));
                    }
                    if let Some(ref ctx) = egui_ctx {
                        ctx.request_repaint();
                    }
                }
                thread::sleep(Duration::from_millis(500));
                continue;
            }

            let desired = {
                let sel = selected_camera_device();
                if !sel.is_empty() && !failed_devs.contains(&sel) {
                    sel
                } else {
                    let (_, cams) = list_media_devices(false);
                    cams.into_iter()
                        .find(|c| !failed_devs.contains(c))
                        .unwrap_or_default()
                }
            };
            if desired.is_empty() {
                if !warned_missing {
                    warned_missing = true;
                    if let Some(ref gtx) = gui_tx {
                        let _ = gtx.send(crate::gui::GuiEvent::StatusUpdate(
                            "⚠️ Không tìm thấy thiết bị Camera khả dụng trên máy".into(),
                        ));
                    }
                    if let Some(ref ctx) = egui_ctx {
                        ctx.request_repaint();
                    }
                }
                thread::sleep(Duration::from_millis(250));
                continue;
            }

            if child_opt.is_none() || current_dev != desired {
                if let Some((mut guard, stdout)) = child_opt.take() {
                    drop(stdout);
                    guard.kill_and_reap();
                }
                current_dev = desired.clone();
                stream_acc.clear();

                let mut cmd = Command::new("ffmpeg");
                cmd.args(["-hide_banner", "-loglevel", "error"]);
                #[cfg(target_os = "windows")]
                cmd.args([
                    "-f",
                    "dshow",
                    "-rtbufsize",
                    "32M",
                    "-i",
                    &format!("video={current_dev}"),
                ]);
                #[cfg(target_os = "macos")]
                cmd.args([
                    "-f",
                    "avfoundation",
                    "-framerate",
                    "30",
                    "-i",
                    &format!("{current_dev}:none"),
                ]);
                #[cfg(target_os = "linux")]
                cmd.args(["-f", "v4l2", "-i", &current_dev]);
                cmd.args([
                    "-vf",
                    "fps=15,scale=320:240:force_original_aspect_ratio=decrease,pad=320:240:(ow-iw)/2:(oh-ih)/2,format=yuv420p",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-tune",
                    "zerolatency",
                    "-profile:v",
                    "baseline",
                    "-level",
                    "3.0",
                    "-b:v",
                    "260k",
                    "-maxrate",
                    "320k",
                    "-bufsize",
                    "640k",
                    "-x264-params",
                    "keyint=10:min-keyint=1:bframes=0:ref=1:cabac=0:sliced-threads=0:scenecut=0:aud=1:repeat-headers=1",
                    "-f",
                    "h264",
                    "pipe:1",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
                #[cfg(target_os = "windows")]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x08000000);
                }

                match cmd.spawn() {
                    Ok(c) => {
                        let mut guard = ChildGuard::new(c);
                        let Some(stdout) = guard.take_stdout() else {
                            guard.kill_and_reap();
                            thread::sleep(Duration::from_millis(250));
                            continue;
                        };
                        crate::voip_log::line(
                            "VCAM",
                            format!("started webcam capture: {current_dev}"),
                        );
                        let _ = want_keyframe.swap(false, Ordering::Relaxed);
                        child_opt = Some((guard, stdout));
                    }
                    Err(e) => {
                        crate::voip_log::line(
                            "VCAM",
                            format!("webcam spawn failed ({current_dev}): {e}"),
                        );
                        thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                }
            }

            let read_res = match &mut child_opt {
                Some((_, stdout)) => stdout.read(&mut read_buf),
                None => continue,
            };
            match read_res {
                Ok(0) | Err(_) => {
                    if let Some((mut guard, stdout)) = child_opt.take() {
                        drop(stdout);
                        guard.kill_and_reap();
                    }
                    if stream_acc.is_empty()
                        && !current_dev.is_empty()
                        && !failed_devs.contains(&current_dev)
                    {
                        crate::voip_log::line(
                            "VCAM",
                            format!(
                                "webcam device produced 0 bytes, marking failed: {current_dev}"
                            ),
                        );
                        failed_devs.push(current_dev.clone());
                    }
                    thread::sleep(Duration::from_millis(250));
                }
                Ok(n) => {
                    stream_acc.extend_from_slice(&read_buf[..n]);
                    while let Some(pos) = find_aud_delimiter(&stream_acc, 6) {
                        let chunk = stream_acc[..pos].to_vec();
                        stream_acc.drain(..pos);
                        let clean = strip_leading_aud(&chunk);
                        if !clean.is_empty() {
                            if let Ok(mut sh) = SHARED_CAM_H264.lock() {
                                sh.0 = sh.0.wrapping_add(1);
                                if au_has_idr(&clean) {
                                    sh.2 = clean.clone();
                                }
                                sh.1 = clean.clone();
                            }
                            let _ = prev_tx.try_send(clean.clone());
                            let _ = cam_au_tx.try_send(clean);
                        }
                    }
                    if stream_acc.len() > 262_144 {
                        stream_acc.clear();
                    }
                }
            }
        }
        if let Some((mut guard, stdout)) = child_opt.take() {
            drop(stdout);
            guard.kill_and_reap();
        }
        drop(prev_tx);
        if let Some(h) = local_preview_handle {
            let _ = h.join();
        }
    })
}

fn find_aud_delimiter(buf: &[u8], start_from: usize) -> Option<usize> {
    if buf.len() < start_from + 6 {
        return None;
    }
    for i in start_from..=buf.len() - 6 {
        if buf[i] == 0
            && buf[i + 1] == 0
            && buf[i + 2] == 0
            && buf[i + 3] == 1
            && (buf[i + 4] & 0x1F) == 9
        {
            return Some(i);
        }
    }
    None
}

fn strip_leading_aud(au: &[u8]) -> Vec<u8> {
    if au.len() >= 6 && au[0..4] == [0, 0, 0, 1] && (au[4] & 0x1F) == 9 {
        normalize_annexb_au_4byte(&au[6..])
    } else {
        normalize_annexb_au_4byte(au)
    }
}

/// Extract raw NAL slices from an Annex-B buffer (3- or 4-byte start codes), filtering SEI (6) & AUD (9).
fn extract_annexb_nals(au: &[u8]) -> Vec<&[u8]> {
    let mut nal_ranges: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    let mut cur_start: Option<usize> = None;
    while i + 3 <= au.len() {
        let sc_len = if i + 4 <= au.len() && au[i..i + 4] == [0, 0, 0, 1] {
            4
        } else if au[i..i + 3] == [0, 0, 1] {
            3
        } else {
            0
        };
        if sc_len > 0 {
            if let Some(s) = cur_start {
                if i > s {
                    nal_ranges.push((s, i));
                }
            }
            cur_start = Some(i + sc_len);
            i += sc_len;
        } else {
            i += 1;
        }
    }
    if let Some(s) = cur_start {
        if au.len() > s {
            nal_ranges.push((s, au.len()));
        }
    }
    nal_ranges
        .into_iter()
        .filter_map(|(s, e)| {
            let nal = &au[s..e];
            if nal.is_empty() {
                return None;
            }
            let nal_type = nal[0] & 0x1F;
            if nal_type == 6 || nal_type == 9 {
                None
            } else {
                Some(nal)
            }
        })
        .collect()
}

/// Normalize an H.264 Annex-B Access Unit so that:
/// 1) Both 3-byte (`00 00 01`) and 4-byte (`00 00 00 01`) start codes are parsed.
/// 2) AUD (`nal_type == 9`) and SEI (`nal_type == 6`) NAL units are stripped.
/// 3) Every remaining NAL unit (`SPS=7`, `PPS=8`, `IDR=5`, `Slice=1`) is prefixed with a 4-byte `00 00 00 01` start code.
fn normalize_annexb_au_4byte(au: &[u8]) -> Vec<u8> {
    let nals = extract_annexb_nals(au);
    if nals.is_empty() {
        return au.to_vec();
    }
    let mut out = Vec::with_capacity(au.len() + nals.len() * 2);
    for nal in nals {
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        out.extend_from_slice(nal);
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
enum PreparedAu {
    ParamSetOnly,
    MissingParamsForIdr,
    DroppedPreSync,
    Keyframe(Vec<u8>),
    DeltaFrame(Vec<u8>),
}

/// Caches out-of-band SPS/PPS packets from mobile peers so standalone SPS/PPS never get separated
/// from the following IDR by a premature `H264_AUD`, and bare IDRs automatically receive cached SPS/PPS.
#[derive(Default)]
struct H264AccessUnitPreparer {
    cached_sps: Option<Vec<u8>>,
    cached_pps: Option<Vec<u8>>,
    synced: bool,
}

impl H264AccessUnitPreparer {
    fn new() -> Self {
        Self::default()
    }

    fn reset_sync(&mut self) {
        self.synced = false;
    }

    fn has_params(&self) -> bool {
        self.cached_sps.is_some() && self.cached_pps.is_some()
    }

    fn prepare(&mut self, raw_au: &[u8]) -> PreparedAu {
        let nals = extract_annexb_nals(raw_au);
        if nals.is_empty() {
            return PreparedAu::ParamSetOnly;
        }
        let mut has_sps = false;
        let mut has_pps = false;
        let mut has_idr = false;
        let mut has_vcl = false;
        for nal in &nals {
            match nal[0] & 0x1F {
                7 => {
                    has_sps = true;
                    self.cached_sps = Some(nal.to_vec());
                }
                8 => {
                    has_pps = true;
                    self.cached_pps = Some(nal.to_vec());
                }
                5 => {
                    has_idr = true;
                    has_vcl = true;
                }
                1..=4 => {
                    has_vcl = true;
                }
                _ => {}
            }
        }
        if !has_vcl {
            return PreparedAu::ParamSetOnly;
        }
        if has_idr {
            let sps_needed = (!has_sps).then_some(self.cached_sps.as_deref()).flatten();
            let pps_needed = (!has_pps).then_some(self.cached_pps.as_deref()).flatten();
            if (!has_sps && sps_needed.is_none()) || (!has_pps && pps_needed.is_none()) {
                return PreparedAu::MissingParamsForIdr;
            }
            let mut out = Vec::with_capacity(raw_au.len() + 64);
            if let Some(sps) = sps_needed {
                out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                out.extend_from_slice(sps);
            }
            if let Some(pps) = pps_needed {
                out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                out.extend_from_slice(pps);
            }
            for nal in nals {
                out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                out.extend_from_slice(nal);
            }
            out.extend_from_slice(H264_AUD);
            self.synced = true;
            return PreparedAu::Keyframe(out);
        }
        if !self.synced {
            return PreparedAu::DroppedPreSync;
        }
        let mut out = Vec::with_capacity(raw_au.len() + 16);
        for nal in nals {
            out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            out.extend_from_slice(nal);
        }
        out.extend_from_slice(H264_AUD);
        PreparedAu::DeltaFrame(out)
    }
}

static GROUP_SRTP_KEY: Mutex<Option<(String, bool)>> = Mutex::new(None);

/// Set or clear the active Zavi SFU group call SRTP key (`AES_CM_128_NULL_AUTH` + `srtpNoSeq`).
pub fn set_group_srtp_key(key: Option<String>, no_seq: bool) {
    if let Ok(mut g) = GROUP_SRTP_KEY.lock() {
        *g = key.filter(|s| s.len() >= 30).map(|s| (s, no_seq));
    }
}

/// RFC 3711 AES-128-ICM SRTP context (`AES_CM_128_NULL_AUTH`) with Zalo's `srtpNoSeq` mode
/// (reverse-engineered from `ZaloCall.exe` `0x9474af` & `0x94764a`).
#[derive(Clone, Debug)]
pub struct SrtpContext {
    session_key: [u8; 16],
    session_salt: [u8; 14],
    no_seq: bool,
}

impl SrtpContext {
    /// Derive `session_key` (label `0x00`) and `session_salt` (label `0x02`) from a 30-byte ASCII `srtpKey`
    /// (`master_key = raw[0..16]`, `master_salt = raw[16..30]`).
    pub fn from_ascii_key(key_str: &str, no_seq: bool) -> Option<Self> {
        use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
        let raw = key_str.as_bytes();
        if raw.len() < 30 {
            return None;
        }
        let master_key = GenericArray::from_slice(&raw[0..16]);
        let master_salt = &raw[16..30];
        let cipher = aes::Aes128::new(master_key);

        let mut iv0 = [0u8; 16];
        iv0[0..14].copy_from_slice(master_salt);
        iv0[7] ^= 0x00;
        let mut blk0 = GenericArray::clone_from_slice(&iv0);
        cipher.encrypt_block(&mut blk0);
        let mut session_key = [0u8; 16];
        session_key.copy_from_slice(blk0.as_slice());

        let mut iv2 = [0u8; 16];
        iv2[0..14].copy_from_slice(master_salt);
        iv2[7] ^= 0x02;
        let mut blk2 = GenericArray::clone_from_slice(&iv2);
        cipher.encrypt_block(&mut blk2);
        let mut session_salt = [0u8; 14];
        session_salt.copy_from_slice(&blk2.as_slice()[0..14]);

        Some(Self {
            session_key,
            session_salt,
            no_seq,
        })
    }

    /// Return the byte offset where the RTP payload begins (after 12-byte header, CSRCs, and RFC 5285 extension).
    pub fn rtp_payload_offset(rtp: &[u8]) -> Option<usize> {
        if rtp.len() < 12 || (rtp[0] & 0xC0) != 0x80 {
            return None;
        }
        let cc = (rtp[0] & 0x0F) as usize;
        let has_ext = (rtp[0] & 0x10) != 0;
        let mut offset = 12 + cc * 4;
        if has_ext {
            if rtp.len() < offset + 4 {
                return None;
            }
            let ext_words = u16::from_be_bytes([rtp[offset + 2], rtp[offset + 3]]) as usize;
            offset += 4 + ext_words * 4;
        }
        if offset <= rtp.len() {
            Some(offset)
        } else {
            None
        }
    }

    /// XOR the RTP payload in-place with the RFC 3711 AES-128-ICM keystream.
    pub fn crypt_rtp_packet(&self, rtp: &mut [u8]) {
        use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
        let Some(offset) = Self::rtp_payload_offset(rtp) else {
            return;
        };
        if offset >= rtp.len() {
            return;
        }
        let mut base_iv = [0u8; 16];
        base_iv[0..14].copy_from_slice(&self.session_salt);
        base_iv[4] ^= rtp[8];
        base_iv[5] ^= rtp[9];
        base_iv[6] ^= rtp[10];
        base_iv[7] ^= rtp[11];
        if !self.no_seq {
            base_iv[12] ^= rtp[2];
            base_iv[13] ^= rtp[3];
        }
        let cipher = aes::Aes128::new(GenericArray::from_slice(&self.session_key));
        let payload = &mut rtp[offset..];
        for (blk_idx, chunk) in payload.chunks_mut(16).enumerate() {
            let mut iv = base_iv;
            let ctr = (blk_idx as u16).to_be_bytes();
            iv[14] = ctr[0];
            iv[15] = ctr[1];
            let mut ks = GenericArray::clone_from_slice(&iv);
            cipher.encrypt_block(&mut ks);
            for (b, k) in chunk.iter_mut().zip(ks.as_slice().iter()) {
                *b ^= *k;
            }
        }
    }

    /// Decrypt the payload of an incoming RTP packet (leaving already-plaintext Annex-B packets untouched).
    pub fn decrypt_rtp_payload(&self, rtp: &[u8]) -> Option<Vec<u8>> {
        let raw_payload = extract_rtp_opus_payload(rtp)?;
        // If a peer sent unencrypted Annex-B video (`00 00 00 01` or `1c 80 00 00 00 01`), keep as-is
        if (raw_payload.len() >= 4 && raw_payload[0..4] == [0x00, 0x00, 0x00, 0x01])
            || (raw_payload.len() >= 6 && raw_payload[0..6] == [0x1C, 0x80, 0x00, 0x00, 0x00, 0x01])
        {
            return Some(raw_payload.to_vec());
        }
        let offset = Self::rtp_payload_offset(rtp)?;
        let mut copy = rtp[..offset + raw_payload.len()].to_vec();
        self.crypt_rtp_packet(&mut copy);
        Some(copy[offset..].to_vec())
    }
}

fn run_local_preview_decoder(
    stop: Arc<AtomicBool>,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    egui_ctx: eframe::egui::Context,
) {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    const PREV_W: usize = 240;
    const PREV_H: usize = 180;
    const PREV_BYTES: usize = PREV_W * PREV_H * 4;

    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-fflags",
        "nobuffer",
        "-flags",
        "low_delay",
        "-threads",
        "1",
        "-probesize",
        "32768",
        "-analyzeduration",
        "0",
        "-f",
        "h264",
        "-i",
        "pipe:0",
        "-vf",
        "scale=240:180:force_original_aspect_ratio=decrease,pad=240:180:(ow-iw)/2:(oh-ih)/2",
        "-fps_mode",
        "passthrough",
        "-pix_fmt",
        "rgba",
        "-f",
        "rawvideo",
        "pipe:1",
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }

    let Ok(child) = cmd.spawn() else {
        return;
    };
    let mut guard = ChildGuard::new(child);
    let Some(mut stdin) = guard.take_stdin() else {
        return;
    };
    let Some(mut stdout) = guard.take_stdout() else {
        return;
    };
    let stop_w = stop.clone();
    let writer_handle = thread::spawn(move || {
        while !stop_w.load(Ordering::Relaxed) {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(au) => {
                    if stdin.write_all(H264_AUD).is_err() || stdin.write_all(&au).is_err() {
                        break;
                    }
                    let _ = stdin.flush();
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    let mut frame = vec![0u8; PREV_BYTES];
    while !stop.load(Ordering::Relaxed) {
        if stdout.read_exact(&mut frame).is_ok() {
            let (cw, ch, cropped) = trim_black_borders(&frame, PREV_W, PREV_H);
            let img = eframe::egui::ColorImage::from_rgba_unmultiplied([cw, ch], &cropped);
            publish_call_video_frame("call_local_video", img);
            egui_ctx.request_repaint();
        } else {
            break;
        }
    }
    drop(stdout);
    guard.kill_and_reap();
    let _ = writer_handle.join();
}

/// Result counters of every UDP send. They used to be dropped silently (`let _ = try_send_to`) while the
/// RTP sequence number still advanced. The peer's RTCP reported ~78% loss of our audio and video, in
/// bursts that hit both streams at once; whether any of that is a failed local send is **unknown** until
/// the `TXST` log line of the next call shows these counters.
#[derive(Default)]
struct TxStats {
    ok: u32,
    /// tokio answered `WouldBlock` (not necessarily a full OS buffer): the datagram was NOT sent.
    would_block: u32,
    short: u32,
    err: u32,
    last_err: Option<String>,
}

/// Send one datagram and record the outcome.
fn send_dgram(
    sock: &tokio::net::UdpSocket,
    data: &[u8],
    target: std::net::SocketAddr,
    st: &mut TxStats,
) {
    match sock.try_send_to(data, target) {
        Ok(n) if n == data.len() => st.ok += 1,
        Ok(_) => st.short += 1,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => st.would_block += 1,
        Err(e) => {
            st.err += 1;
            st.last_err = Some(e.to_string());
        }
    }
}

/// True if a (compound) RTCP packet contains a PLI (PSFB fmt 1) or FIR (PSFB fmt 4).
#[cfg(test)]
pub(crate) fn rtcp_requests_keyframe(rtcp: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 4 <= rtcp.len() {
        if (rtcp[i] & 0xC0) != 0x80 {
            return false;
        }
        let fmt = rtcp[i] & 0x1F;
        let pt = rtcp[i + 1];
        let words = u16::from_be_bytes([rtcp[i + 2], rtcp[i + 3]]) as usize;
        if pt == 206 && (fmt == 1 || fmt == 4) {
            return true;
        }
        i += (words + 1) * 4;
    }
    false
}

/// Extract the Opus payload slice from a standard RFC 3550 RTP v2 packet (handling CSRC & header extensions).
fn extract_rtp_opus_payload(rtp: &[u8]) -> Option<&[u8]> {
    if rtp.len() < 12 || (rtp[0] & 0xC0) != 0x80 {
        return None;
    }
    let cc = (rtp[0] & 0x0F) as usize;
    let has_ext = (rtp[0] & 0x10) != 0;
    let has_pad = (rtp[0] & 0x20) != 0;
    let mut offset = 12 + cc * 4;
    if has_ext {
        if rtp.len() < offset + 4 {
            return None;
        }
        let ext_words = u16::from_be_bytes([rtp[offset + 2], rtp[offset + 3]]) as usize;
        offset += 4 + ext_words * 4;
    }
    let mut end = rtp.len();
    if has_pad && end > offset {
        let pad_len = rtp[end - 1] as usize;
        if pad_len > 0 && pad_len <= end - offset {
            end -= pad_len;
        }
    }
    if offset < end {
        Some(&rtp[offset..end])
    } else {
        None
    }
}

#[inline]
fn i16_to_f32(v: i16) -> f32 {
    v as f32 / 32768.0
}

#[inline]
fn u16_to_f32(v: u16) -> f32 {
    (v as f32 - 32768.0) / 32768.0
}

#[inline]
fn f32_to_i16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

#[inline]
fn f32_to_u16(v: f32) -> u16 {
    ((v.clamp(-1.0, 1.0) * 32767.0).round() as i32 + 32768).clamp(0, 65535) as u16
}

/// Wait-free, zero-`unsafe` SPSC lock-free ring buffer for `f32` PCM samples.
/// Producer updates `head` only; consumer updates `tail` only.
struct AudioRingBuffer {
    buf: Box<[AtomicU32]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl AudioRingBuffer {
    fn new(capacity_pow2: usize) -> Self {
        let cap = capacity_pow2.max(64).next_power_of_two();
        let buf: Vec<AtomicU32> = (0..cap).map(|_| AtomicU32::new(0)).collect();
        Self {
            buf: buf.into_boxed_slice(),
            mask: cap - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    #[cfg(test)]
    fn capacity(&self) -> usize {
        self.buf.len()
    }

    fn len(&self) -> usize {
        let h = self.head.load(Ordering::Acquire);
        let t = self.tail.load(Ordering::Acquire);
        h.wrapping_sub(t)
    }

    fn push(&self, sample: f32) -> bool {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        if h.wrapping_sub(t) >= self.buf.len() {
            return false;
        }
        self.buf[h & self.mask].store(sample.to_bits(), Ordering::Relaxed);
        self.head.store(h.wrapping_add(1), Ordering::Release);
        true
    }

    fn pop(&self) -> Option<f32> {
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        if t == h {
            return None;
        }
        let bits = self.buf[t & self.mask].load(Ordering::Relaxed);
        self.tail.store(t.wrapping_add(1), Ordering::Release);
        Some(f32::from_bits(bits))
    }

    /// Consumer-only: discard up to `n` oldest buffered samples to bound latency.
    fn discard(&self, n: usize) {
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        let avail = h.wrapping_sub(t);
        let drop_n = n.min(avail);
        if drop_n > 0 {
            self.tail.store(t.wrapping_add(drop_n), Ordering::Release);
        }
    }
}

/// 2nd-order Butterworth low-pass biquad (Direct Form II Transposed) for anti-aliasing / anti-imaging.
struct BiquadLpf {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl BiquadLpf {
    fn new(cutoff_hz: f64, sample_rate: f64) -> Self {
        let fs = sample_rate.max(1000.0);
        let fc = cutoff_hz.clamp(100.0, fs * 0.48);
        let omega = 2.0 * std::f64::consts::PI * fc / fs;
        let sin_w = omega.sin();
        let cos_w = omega.cos();
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let alpha = sin_w / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b0 = ((1.0 - cos_w) * 0.5) / a0;
        let b1 = (1.0 - cos_w) / a0;
        let b2 = b0;
        let a1 = (-2.0 * cos_w) / a0;
        let a2 = (1.0 - alpha) / a0;
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let xd = x as f64;
        let y = self.b0 * xd + self.z1;
        self.z1 = self.b1 * xd - self.a1 * y + self.z2;
        self.z2 = self.b2 * xd - self.a2 * y;
        y as f32
    }
}

#[inline]
fn hermite4(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c0 = y1;
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + c0
}

/// Phase-continuous, anti-aliased mono audio resampler supporting arbitrary sample rates
/// (including 8 kHz Bluetooth HFP, 16 kHz, 44.1 kHz, and 48 kHz) without over-drain or drift.
struct AudioResampler {
    in_rate: u32,
    out_rate: u32,
    step: f64,
    pos: f64,
    history: VecDeque<f32>,
    lpf: Option<BiquadLpf>,
    underrun_last: f32,
}

impl AudioResampler {
    fn new(in_rate: u32, out_rate: u32) -> Self {
        let ir = in_rate.max(8000);
        let or = out_rate.max(8000);
        let lpf = if ir == or {
            None
        } else {
            let max_rate = ir.max(or) as f64;
            let min_rate = ir.min(or) as f64;
            Some(BiquadLpf::new(0.45 * min_rate, max_rate))
        };
        let mut history = VecDeque::with_capacity(512);
        history.push_back(0.0); // lookbehind y0 for index 1
        Self {
            in_rate: ir,
            out_rate: or,
            step: ir as f64 / or as f64,
            pos: 1.0,
            history,
            lpf,
            underrun_last: 0.0,
        }
    }

    fn process_capture(&mut self, mono_in: &[f32], mut emit: impl FnMut(f32)) {
        if self.in_rate == self.out_rate {
            for &s in mono_in {
                emit(s);
            }
            return;
        }
        if self.in_rate > self.out_rate {
            if let Some(ref mut lpf) = self.lpf {
                for &s in mono_in {
                    self.history.push_back(lpf.process(s));
                }
            }
        } else {
            self.history.extend(mono_in.iter().copied());
        }
        while self.pos + 2.0 < self.history.len() as f64 {
            let i = self.pos.floor() as usize;
            let t = (self.pos - i as f64) as f32;
            let mut s = hermite4(
                self.history[i - 1],
                self.history[i],
                self.history[i + 1],
                self.history[i + 2],
                t,
            );
            if self.in_rate < self.out_rate {
                if let Some(ref mut lpf) = self.lpf {
                    s = lpf.process(s);
                }
            }
            emit(s);
            self.pos += self.step;
        }
        let drop_n = (self.pos.floor() as usize)
            .saturating_sub(1)
            .min(self.history.len().saturating_sub(1));
        if drop_n > 0 {
            self.history.drain(..drop_n);
            self.pos -= drop_n as f64;
        }
    }

    fn fill_playback(&mut self, src: &AudioRingBuffer, out_mono: &mut [f32]) {
        if src.len() > AUDIO_MAX_OUTPUT_RING_SAMPLES {
            src.discard(src.len() - AUDIO_MAX_OUTPUT_RING_SAMPLES);
        }
        if self.in_rate == self.out_rate {
            for dst in out_mono.iter_mut() {
                if let Some(s) = src.pop() {
                    self.underrun_last = s;
                    *dst = s;
                } else {
                    self.underrun_last *= 0.92;
                    if self.underrun_last.abs() < 1e-5 {
                        self.underrun_last = 0.0;
                    }
                    *dst = self.underrun_last;
                }
            }
            return;
        }
        let needed_len = (self.pos + out_mono.len() as f64 * self.step).ceil() as usize + 3;
        while self.history.len() < needed_len {
            let Some(raw) = src.pop() else {
                break;
            };
            let pushed = if self.in_rate > self.out_rate {
                self.lpf.as_mut().map_or(raw, |f| f.process(raw))
            } else {
                raw
            };
            self.history.push_back(pushed);
        }
        for dst in out_mono.iter_mut() {
            if self.pos + 2.0 < self.history.len() as f64 {
                let i = self.pos.floor() as usize;
                let t = (self.pos - i as f64) as f32;
                let mut s = hermite4(
                    self.history[i - 1],
                    self.history[i],
                    self.history[i + 1],
                    self.history[i + 2],
                    t,
                );
                if self.in_rate < self.out_rate {
                    if let Some(ref mut lpf) = self.lpf {
                        s = lpf.process(s);
                    }
                }
                self.underrun_last = s;
                *dst = s;
                self.pos += self.step;
            } else {
                self.underrun_last *= 0.92;
                if self.underrun_last.abs() < 1e-5 {
                    self.underrun_last = 0.0;
                }
                *dst = self.underrun_last;
            }
        }
        let drop_n = (self.pos.floor() as usize)
            .saturating_sub(1)
            .min(self.history.len().saturating_sub(1));
        if drop_n > 0 {
            self.history.drain(..drop_n);
            self.pos -= drop_n as f64;
        }
    }
}

/// 1st-order 80 Hz high-pass filter removing DC offset, wind rumble, and desk vibration.
struct HighPassFilter80Hz {
    alpha: f32,
    prev_x: f32,
    prev_y: f32,
}

impl HighPassFilter80Hz {
    fn new(sample_rate: u32) -> Self {
        let dt = 1.0 / sample_rate.max(1000) as f32;
        let rc = 1.0 / (2.0 * std::f32::consts::PI * 80.0);
        Self {
            alpha: rc / (rc + dt),
            prev_x: 0.0,
            prev_y: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.alpha * (self.prev_y + x - self.prev_x);
        self.prev_x = x;
        self.prev_y = y;
        y
    }
}

/// Smooth soft-knee limiter avoiding harsh digital clipping past `±0.85`.
#[inline]
fn soft_limit(x: f32) -> f32 {
    let abs = x.abs();
    if abs <= 0.85 {
        x
    } else {
        x.signum() * (0.85 + 0.13 * ((abs - 0.85) / 0.13).tanh())
    }
}

/// Configure Opus VoIP encoder defaults (32 kbps VBR, Voice signal, complexity 8, in-band FEC + 10% loss).
fn configure_voip_opus_encoder(encoder: &mut Encoder) {
    let _ = encoder.set_bitrate(opus::Bitrate::Bits(32_000));
    let _ = encoder.set_vbr(true);
    let _ = encoder.set_vbr_constraint(true);
    let _ = encoder.set_complexity(8);
    let _ = encoder.set_signal(opus::Signal::Voice);
    let _ = encoder.set_inband_fec(true);
    let _ = encoder.set_packet_loss_perc(10);
    let _ = encoder.set_dtx(false);
}

#[derive(Debug, PartialEq, Eq)]
enum JitterPop {
    Buffering,
    Packet(Vec<u8>),
    FecFromNext(Vec<u8>),
    Plc,
    Silence,
}

/// Adaptive ordered Opus jitter buffer (40–80 ms adaptive target, 200 ms max cap) with FEC & PLC.
struct AudioJitterBuffer {
    packets: BTreeMap<i64, Vec<u8>>,
    highest_unwrapped: i64,
    next_play_seq: i64,
    has_seen_packet: bool,
    primed: bool,
    target_packets: usize,
    max_packets: usize,
    consecutive_plc: usize,
    clean_streak: u32,
    rx_pushed: u64,
    played_normal: u64,
    fec_recovered: u64,
    plc_concealed: u64,
    late_dropped: u64,
    overflow_dropped: u64,
    reordered: u64,
}

impl Default for AudioJitterBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioJitterBuffer {
    fn new() -> Self {
        Self {
            packets: BTreeMap::new(),
            highest_unwrapped: 0,
            next_play_seq: 0,
            has_seen_packet: false,
            primed: false,
            target_packets: 3, // 60 ms initial target (adapts between 40..=80 ms)
            max_packets: 10,   // 200 ms hard latency cap
            consecutive_plc: 0,
            clean_streak: 0,
            rx_pushed: 0,
            played_normal: 0,
            fec_recovered: 0,
            plc_concealed: 0,
            late_dropped: 0,
            overflow_dropped: 0,
            reordered: 0,
        }
    }

    fn push(&mut self, seq: u16, payload: Vec<u8>) {
        self.rx_pushed += 1;
        if !self.has_seen_packet {
            self.has_seen_packet = true;
            let s = seq as i64;
            self.highest_unwrapped = s;
            self.next_play_seq = s;
            self.packets.insert(s, payload);
            return;
        }
        let ref_seq = (self.highest_unwrapped & 0xFFFF) as u16;
        let delta = seq.wrapping_sub(ref_seq) as i16 as i64;
        let unwrapped = self.highest_unwrapped + delta;
        if unwrapped - self.highest_unwrapped > 250 || self.highest_unwrapped - unwrapped > 250 {
            // Stream reset / SSRC jump
            self.packets.clear();
            self.highest_unwrapped = unwrapped;
            self.next_play_seq = unwrapped;
            self.primed = false;
            self.consecutive_plc = 0;
            self.packets.insert(unwrapped, payload);
            return;
        }
        if self.primed && unwrapped < self.next_play_seq {
            self.late_dropped += 1;
            self.clean_streak = 0;
            if self.target_packets < 4 {
                self.target_packets += 1;
            }
            return;
        }
        if unwrapped < self.highest_unwrapped {
            self.reordered += 1;
        } else {
            self.highest_unwrapped = unwrapped;
        }
        self.packets.insert(unwrapped, payload);
        while self.packets.len() > self.max_packets {
            if let Some((dropped_seq, _)) = self.packets.pop_first() {
                self.overflow_dropped += 1;
                if self.next_play_seq <= dropped_seq {
                    if let Some(&first_rem) = self.packets.keys().next() {
                        self.next_play_seq = first_rem;
                    }
                }
            }
        }
    }

    fn pop_next(&mut self) -> JitterPop {
        if !self.primed {
            if self.packets.len() < self.target_packets {
                return JitterPop::Buffering;
            }
            self.primed = true;
            if let Some(&first_seq) = self.packets.keys().next() {
                self.next_play_seq = first_seq;
            }
        }
        if let Some(pkt) = self.packets.remove(&self.next_play_seq) {
            self.next_play_seq += 1;
            self.consecutive_plc = 0;
            self.played_normal += 1;
            self.clean_streak += 1;
            if self.clean_streak >= 150 && self.target_packets > 2 {
                self.target_packets -= 1;
                self.clean_streak = 0;
            }
            return JitterPop::Packet(pkt);
        }
        if self.packets.is_empty() && self.consecutive_plc >= 5 {
            self.primed = false;
            return JitterPop::Silence;
        }
        if let Some(next_pkt) = self.packets.get(&(self.next_play_seq + 1)).cloned() {
            self.next_play_seq += 1;
            self.consecutive_plc += 1;
            self.fec_recovered += 1;
            self.clean_streak = 0;
            if self.target_packets < 4 {
                self.target_packets += 1;
            }
            JitterPop::FecFromNext(next_pkt)
        } else {
            self.next_play_seq += 1;
            self.consecutive_plc += 1;
            self.plc_concealed += 1;
            self.clean_streak = 0;
            if self.target_packets < 4 {
                self.target_packets += 1;
            }
            JitterPop::Plc
        }
    }
}

fn build_typed_call_input_stream<T: cpal::SizedSample + Send + 'static>(
    input_dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    in_rate: u32,
    in_channels: usize,
    ring: Arc<AudioRingBuffer>,
    err_flag: Arc<AtomicBool>,
    to_f32: fn(T) -> f32,
) -> Result<cpal::Stream, String> {
    let mut resampler = AudioResampler::new(in_rate, SAMPLE_RATE);
    let mut mono_scratch = Vec::<f32>::with_capacity(2048);
    let stream = input_dev
        .build_input_stream(
            cfg,
            move |data: &[T], _| {
                let total_frames = data.len() / in_channels;
                if total_frames == 0 {
                    return;
                }
                mono_scratch.clear();
                mono_scratch.reserve(total_frames);
                for f in 0..total_frames {
                    let mut sum = 0.0_f32;
                    for ch in 0..in_channels {
                        sum += to_f32(data[f * in_channels + ch]);
                    }
                    mono_scratch.push(sum / in_channels as f32);
                }
                resampler.process_capture(&mono_scratch, |s| {
                    let _ = ring.push(s);
                });
            },
            move |e| {
                err_flag.store(true, Ordering::Relaxed);
                eprintln!("input stream err {e}");
            },
            None,
        )
        .map_err(|e| format!("build input stream: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("play input stream: {e}"))?;
    Ok(stream)
}

fn build_call_input_stream(
    input_dev: &cpal::Device,
    ring: Arc<AudioRingBuffer>,
    err_flag: Arc<AtomicBool>,
) -> Result<(cpal::Stream, u32, usize, cpal::SampleFormat), String> {
    let in_cfg = input_dev
        .default_input_config()
        .map_err(|e| format!("default_input_config: {e}"))?;
    let in_rate = in_cfg.sample_rate().0.max(8000);
    let in_channels = (in_cfg.channels() as usize).max(1);
    let fmt = in_cfg.sample_format();
    let cfg: cpal::StreamConfig = in_cfg.into();
    let stream = match fmt {
        cpal::SampleFormat::F32 => {
            build_typed_call_input_stream::<f32>(input_dev, &cfg, in_rate, in_channels, ring, err_flag, |v| v)?
        }
        cpal::SampleFormat::I16 => {
            build_typed_call_input_stream::<i16>(input_dev, &cfg, in_rate, in_channels, ring, err_flag, i16_to_f32)?
        }
        cpal::SampleFormat::U16 => {
            build_typed_call_input_stream::<u16>(input_dev, &cfg, in_rate, in_channels, ring, err_flag, u16_to_f32)?
        }
        other => return Err(format!("unsupported input sample format: {other:?}")),
    };
    Ok((stream, in_rate, in_channels, fmt))
}

fn build_typed_call_output_stream<T: cpal::SizedSample + Send + 'static>(
    output_dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    out_rate: u32,
    out_channels: usize,
    ring: Arc<AudioRingBuffer>,
    err_flag: Arc<AtomicBool>,
    from_f32: fn(f32) -> T,
) -> Result<cpal::Stream, String> {
    let mut resampler = AudioResampler::new(SAMPLE_RATE, out_rate);
    let mut mono_scratch = Vec::<f32>::with_capacity(2048);
    let stream = output_dev
        .build_output_stream(
            cfg,
            move |data: &mut [T], _| {
                let out_frames = data.len() / out_channels;
                if out_frames == 0 {
                    return;
                }
                mono_scratch.resize(out_frames, 0.0);
                resampler.fill_playback(&ring, &mut mono_scratch);
                for (i, &s) in mono_scratch.iter().enumerate() {
                    let val = from_f32(soft_limit(s));
                    for ch in 0..out_channels {
                        data[i * out_channels + ch] = val;
                    }
                }
            },
            move |e| {
                err_flag.store(true, Ordering::Relaxed);
                eprintln!("output stream err {e}");
            },
            None,
        )
        .map_err(|e| format!("build output stream: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("play output stream: {e}"))?;
    Ok(stream)
}

fn build_call_output_stream(
    output_dev: &cpal::Device,
    ring: Arc<AudioRingBuffer>,
    err_flag: Arc<AtomicBool>,
) -> Result<(cpal::Stream, u32, usize, cpal::SampleFormat), String> {
    let out_cfg = output_dev
        .default_output_config()
        .map_err(|e| format!("default_output_config: {e}"))?;
    let out_rate = out_cfg.sample_rate().0.max(8000);
    let out_channels = (out_cfg.channels() as usize).max(1);
    let fmt = out_cfg.sample_format();
    let cfg: cpal::StreamConfig = out_cfg.into();
    let stream = match fmt {
        cpal::SampleFormat::F32 => {
            build_typed_call_output_stream::<f32>(output_dev, &cfg, out_rate, out_channels, ring, err_flag, |v| v)?
        }
        cpal::SampleFormat::I16 => {
            build_typed_call_output_stream::<i16>(output_dev, &cfg, out_rate, out_channels, ring, err_flag, f32_to_i16)?
        }
        cpal::SampleFormat::U16 => {
            build_typed_call_output_stream::<u16>(output_dev, &cfg, out_rate, out_channels, ring, err_flag, f32_to_u16)?
        }
        other => return Err(format!("unsupported output sample format: {other:?}")),
    };
    Ok((stream, out_rate, out_channels, fmt))
}

struct ExtraVideoStream {
    ssrc: u32,
    tex_key: String,
    depacketizer: VideoDepacketizer,
    tx: std::sync::mpsc::SyncSender<Vec<u8>>,
    last_rx: Option<std::time::Instant>,
    _handle: JoinHandle<()>,
}

#[allow(clippy::too_many_arguments)]
fn run_media_loop(
    stop: Arc<AtomicBool>,
    mic_muted: Arc<AtomicBool>,
    cam_enabled: Arc<AtomicBool>,
    udp_rtp_sock: Option<Arc<tokio::net::UdpSocket>>,
    udp_rtcp_sock: Option<Arc<tokio::net::UdpSocket>>,
    fallback_sock: Option<Arc<UdpSocket>>,
    rtp_addr: String,
    rtcp_addr: String,
    from_uid_u32: u32,
    partner_uid_u32: u32,
    is_caller: bool,
    relay_token: u32,
    slot_idx: u16,
    is_video: bool,
    gui_tx: Option<crate::gui::GuiTx>,
    egui_ctx: Option<eframe::egui::Context>,
) {
    let (h264_tx, h264_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(120);
    let (cam_au_tx, cam_au_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(4);
    let video_synced = Arc::new(AtomicBool::new(false));
    let want_keyframe = Arc::new(AtomicBool::new(false));
    let mut decoder_active = false;
    let tex_key = if slot_idx == 0 {
        "call_remote_video".to_string()
    } else {
        format!("call_remote_video_{slot_idx}")
    };
    let primary_dec_handle = if let Some(ctx) = egui_ctx.clone() {
        let stop_v = stop.clone();
        let synced_v = video_synced.clone();
        decoder_active = true;
        Some(thread::spawn(move || {
            run_h264_ffmpeg_decoder(stop_v, h264_rx, ctx, synced_v, tex_key);
        }))
    } else {
        drop(h264_rx);
        None
    };
    let egui_ctx_extra = egui_ctx.clone();
    let cam_handle = if slot_idx == 0 {
        Some(spawn_camera_capture_thread(
            stop.clone(),
            cam_enabled.clone(),
            want_keyframe.clone(),
            cam_au_tx.clone(),
            gui_tx,
            egui_ctx,
        ))
    } else {
        None
    };
    let mut primary_depacketizer = VideoDepacketizer::new();
    let (_output_name, out_dev) = match resolve_output_device() {
        Ok((d, name)) => (name, d),
        Err(e) => {
            eprintln!("media: no output device ({e})");
            return;
        }
    };
    let output_ring = Arc::new(AudioRingBuffer::new(AUDIO_RING_CAPACITY));
    let out_err_flag = Arc::new(AtomicBool::new(false));
    let (output_stream, out_rate, out_channels, out_fmt) =
        match build_call_output_stream(&out_dev, output_ring.clone(), out_err_flag.clone()) {
            Ok(res) => res,
            Err(e) => {
                eprintln!("media: output stream err {e}");
                return;
            }
        };

    let mut last_mic_epoch = MIC_SELECTION_EPOCH.load(Ordering::Acquire);
    let mut input_ring = Arc::new(AudioRingBuffer::new(AUDIO_RING_CAPACITY));
    let mic_err_flag = Arc::new(AtomicBool::new(false));
    let mut active_mic_name = String::new();
    let mut active_mic_stream: Option<cpal::Stream> = None;
    let open_mic_stream = |desired: &str,
                           ring: Arc<AudioRingBuffer>,
                           err_flag: Arc<AtomicBool>|
     -> Result<(cpal::Stream, String, u32, usize, cpal::SampleFormat), String> {
        let primary = resolve_input_device(desired);
        let (dev, actual_name) = match primary {
            Ok(pair) => pair,
            Err(first_err) if !desired.trim().is_empty() => {
                resolve_input_device("").map_err(|e| format!("{first_err}; fallback: {e}"))?
            }
            Err(e) => return Err(e),
        };
        match build_call_input_stream(&dev, ring.clone(), err_flag.clone()) {
            Ok((stream, rate, ch, fmt)) => Ok((stream, actual_name, rate, ch, fmt)),
            Err(first_err) if !desired.trim().is_empty() => {
                let (fb_dev, fb_name) = resolve_input_device("")
                    .map_err(|e| format!("{first_err}; fallback_resolve: {e}"))?;
                let (stream, rate, ch, fmt) = build_call_input_stream(&fb_dev, ring, err_flag)?;
                Ok((stream, fb_name, rate, ch, fmt))
            }
            Err(e) => Err(e),
        }
    };

    let initial_pref_mic = selected_mic_device();
    match open_mic_stream(&initial_pref_mic, input_ring.clone(), mic_err_flag.clone()) {
        Ok((stream, actual_name, rate, ch, fmt)) => {
            active_mic_name = actual_name.clone();
            active_mic_stream = Some(stream);
            crate::logger::log_voip_event(
                "audio::mic_switch",
                &format!("reason=init from=<none> to={actual_name} rate={rate} ch={ch} fmt={fmt:?}"),
                false,
            );
            crate::voip_log::line(
                "AUDIO",
                format!(
                    "init mic={actual_name} ({rate}Hz/{ch}ch/{fmt:?}) out={_output_name} ({out_rate}Hz/{out_channels}ch/{out_fmt:?})"
                ),
            );
        }
        Err(e) => {
            crate::logger::log_voip_event(
                "audio::mic_switch",
                &format!("reason=init_failed pref={} err={e}", if initial_pref_mic.is_empty() { "<default>" } else { &initial_pref_mic }),
                true,
            );
        }
    }
    let mut last_mic_attempt = Instant::now();

    // Opus codecs
    let mut encoder = match Encoder::new(SAMPLE_RATE, OpusChannels::Mono, opus::Application::Voip) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("media: encoder err {e}");
            return;
        }
    };
    configure_voip_opus_encoder(&mut encoder);
    let mut decoder = match Decoder::new(SAMPLE_RATE, OpusChannels::Mono) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("media: decoder err {e}");
            return;
        }
    };
    let mut tx_hpf = HighPassFilter80Hz::new(SAMPLE_RATE);
    let mut jitter_buf = AudioJitterBuffer::new();
    let mut next_rx_playout = Instant::now();
    let mut last_audio_stats_log = Instant::now();

    let mut seq: u16 = 0;
    let mut ts: u32 = 0;
    let ssrc = if from_uid_u32 != 0 {
        from_uid_u32
    } else {
        0x5A41_4C4F
    };
    let rtp_sock_addr = rtp_addr.parse::<std::net::SocketAddr>().ok();
    let rtcp_sock_addr = rtcp_addr.parse::<std::net::SocketAddr>().ok();
    let mut last_rtcp = Instant::now() - Duration::from_secs(2);
    let mut last_rtp_tx = Instant::now();
    let mut last_video_tx = Instant::now();
    let mut vid_seq: u16 = 0;
    let mut vid_ts: u32 = 0;
    let mut vid_frame_no: u32 = 0;
    let mut vid_gop_pos: usize = 0;
    let mut vid_idr_parity: u8 = 0;
    clear_rtcp_inbox();
    // Reception stats for the peer's audio/video streams (tracked separately so audio packets never clobber video SSRC).
    let mut rx_audio = RxStats::default();
    let mut rx_video = RxStats::default();
    let mut peer_audio_ssrc: u32 = 0;
    let mut peer_video_ssrc: u32 = 0;
    let mut last_peer_sr: Option<(u32, Instant)> = None;
    let mut vid_pkts_tx: u32 = 0;
    let mut vid_octets_tx: u32 = 0;
    let mut wl = crate::voip_log::WireLog::new();
    // Send outcome counters + longest main-loop stall, reported as `TXST` every 5 s.
    let mut txs = TxStats::default();
    let mut last_txst = Instant::now();
    let mut last_iter = Instant::now();
    let mut max_loop_gap = Duration::ZERO;
    // Peer video that cannot be decoded yet (no SPS/IDR seen) or has a hole -> we ask for a keyframe (PLI).
    // `video_synced` is set by the decoder feeder when the first SPS/IDR access unit arrives.
    let mut pli_gap = false;
    let mut pli_sent: u32 = 0;
    let mut last_pli = Instant::now() - Duration::from_secs(10);
    crate::voip_log::line(
        "CALL",
        format!(
            "media start rtp={rtp_addr} rtcp={rtcp_addr} video={is_video} caller={is_caller} relay_token={} tx_video_pt={VIDEO_PT} tx_frag_pt={VIDEO_PT_FRAG} video_format=annexb-au tx_audio_pt=112 max_payload={RTP_MAX_PAYLOAD}",
            relay_token != 0
        ),
    );
    let vid_ssrc = ssrc;
    let srtp_ctx: Option<SrtpContext> = GROUP_SRTP_KEY
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .and_then(|(k, no_seq)| SrtpContext::from_ascii_key(&k, no_seq));
    let mut recent_vid_seqs: VecDeque<(u32, u16)> = VecDeque::with_capacity(64);
    let mut nego_seq: u32 = 1;
    let mut last_cam_idr: Option<Vec<u8>> = None;
    let mut last_shared_cam_seq: u64 = 0;
    let mut primary_video_ssrc: u32 = 0;
    let mut last_primary_vid_rx: Option<Instant> = None;
    let mut extra_vid_streams: Vec<ExtraVideoStream> = Vec::new();
    let mut room_partners: Vec<u32> = Vec::new();
    let mut tx_layer_byte: u8 = 0x00;
    if partner_uid_u32 != 0 && partner_uid_u32 != from_uid_u32 {
        room_partners.push(partner_uid_u32);
    }

    while !stop.load(Ordering::Relaxed) {
        // --- Live Mic Switching & Hot-Plug Recovery ---
        let cur_mic_epoch = MIC_SELECTION_EPOCH.load(Ordering::Acquire);
        let mic_switch_reason = if cur_mic_epoch != last_mic_epoch {
            last_mic_epoch = cur_mic_epoch;
            Some("user_switch")
        } else if mic_err_flag.swap(false, Ordering::AcqRel)
            && last_mic_attempt.elapsed() >= Duration::from_millis(500)
        {
            Some("stream_error_recover")
        } else if active_mic_stream.is_none()
            && last_mic_attempt.elapsed() >= Duration::from_secs(2)
        {
            Some("retry")
        } else {
            None
        };
        if let Some(reason) = mic_switch_reason {
            last_mic_attempt = Instant::now();
            let prev_name = if active_mic_name.is_empty() {
                "<none>".to_string()
            } else {
                active_mic_name.clone()
            };
            if let Some(old_stream) = active_mic_stream.take() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                    drop(old_stream)
                }));
            }
            input_ring = Arc::new(AudioRingBuffer::new(AUDIO_RING_CAPACITY));
            tx_hpf = HighPassFilter80Hz::new(SAMPLE_RATE);
            let desired = selected_mic_device();
            match open_mic_stream(&desired, input_ring.clone(), mic_err_flag.clone()) {
                Ok((stream, actual_name, rate, ch, fmt)) => {
                    active_mic_name = actual_name.clone();
                    active_mic_stream = Some(stream);
                    crate::logger::log_voip_event(
                        "audio::mic_switch",
                        &format!(
                            "reason={reason} from={prev_name} to={actual_name} rate={rate} ch={ch} fmt={fmt:?}"
                        ),
                        false,
                    );
                    crate::voip_log::line(
                        "AUDIO",
                        format!(
                            "mic switch ({reason}) from={prev_name} to={actual_name} ({rate}Hz/{ch}ch/{fmt:?})"
                        ),
                    );
                }
                Err(e) => {
                    active_mic_name.clear();
                    crate::logger::log_voip_event(
                        "audio::mic_switch",
                        &format!("reason={reason} from={prev_name} err={e}"),
                        true,
                    );
                }
            }
        }

        // --- Capture: drain full 320-sample frames from lock-free ring (never inject silence when 319 samples wait) ---
        if input_ring.len() > FRAME_SAMPLES * 12 {
            input_ring.discard(input_ring.len() - FRAME_SAMPLES * 4);
        }
        let mut tx_burst = 0usize;
        while tx_burst < 2 {
            let mut frame: Vec<f32> = if input_ring.len() >= FRAME_SAMPLES {
                let mut f = Vec::with_capacity(FRAME_SAMPLES);
                for _ in 0..FRAME_SAMPLES {
                    if let Some(s) = input_ring.pop() {
                        f.push(s);
                    }
                }
                f
            } else if tx_burst == 0
                && ((active_mic_stream.is_none()
                    && last_rtp_tx.elapsed() >= Duration::from_millis(20))
                    || last_rtp_tx.elapsed() >= Duration::from_millis(60))
            {
                vec![0.0_f32; FRAME_SAMPLES]
            } else {
                break;
            };
            if frame.len() != FRAME_SAMPLES {
                break;
            }
            tx_burst += 1;
            last_rtp_tx = Instant::now();
            let gain = (MIC_GAIN_PERCENT.load(Ordering::Relaxed) as f32 / 100.0).clamp(0.0, 2.0);
            let mut peak = 0.0_f32;
            if mic_muted.load(Ordering::Relaxed) || gain <= 0.001 {
                frame.fill(0.0);
            } else {
                for s in &mut frame {
                    let filtered = tx_hpf.process(*s);
                    let scaled = soft_limit(filtered * gain);
                    *s = scaled;
                    peak = peak.max(scaled.abs());
                }
            }
            MIC_LEVEL_PERMILLE.store((peak * 1000.0).clamp(0.0, 1000.0) as u32, Ordering::Relaxed);
            mix_peer_bridge_audio(slot_idx, &mut frame);
            for s in &mut frame {
                *s = soft_limit(*s);
            }
            let mut out = vec![0u8; 4000];
            match encoder.encode_float(&frame, &mut out) {
                Ok(n) => {
                    let is_group_sfu = rtp_addr.ends_with(":3000") || rtcp_addr.ends_with(":3500");
                    let mut rtp_pkt = if is_group_sfu {
                        build_group_audio_rtp_packet(112, seq, ts, ssrc, ntp_now(), &out[..n])
                    } else {
                        let mut p = Vec::with_capacity(12 + n);
                        p.push(0x80); // V=2, P=0, X=0, CC=0
                        p.push(112); // payload type 112 (opus/16000/1)
                        p.extend_from_slice(&seq.to_be_bytes());
                        p.extend_from_slice(&ts.to_be_bytes());
                        p.extend_from_slice(&ssrc.to_be_bytes());
                        p.extend_from_slice(&out[..n]);
                        p
                    };
                    if is_group_sfu {
                        if let Some(ref srtp) = srtp_ctx {
                            srtp.crypt_rtp_packet(&mut rtp_pkt);
                        }
                    }

                    if let (Some(tsock), Some(target)) = (&udp_rtp_sock, rtp_sock_addr) {
                        if relay_token != 0 {
                            let type_byte: u8 = 0x03; // `ZRTPPacket::initZRTPPacketAudio` (`isRtcp=false`)
                            let mut wire = Vec::with_capacity(5 + rtp_pkt.len());
                            wire.push(type_byte);
                            wire.extend_from_slice(&relay_token.to_le_bytes());
                            wire.extend_from_slice(&rtp_pkt);
                            send_dgram(tsock, &wire, target, &mut txs);
                            wl.rtp(">", type_byte, wire.len(), &rtp_addr, &rtp_pkt);
                        } else {
                            send_dgram(tsock, &rtp_pkt, target, &mut txs);
                            wl.rtp(">", rtp_pkt[0], rtp_pkt.len(), &rtp_addr, &rtp_pkt);
                        }
                    } else if let Some(fsock) = &fallback_sock {
                        let _ = fsock.send_to(&rtp_pkt, &rtp_addr);
                    }
                    seq = seq.wrapping_add(1);
                    ts = ts.wrapping_add(FRAME_SAMPLES as u32);
                }
                Err(e) => eprintln!("media: encode err {e}"),
            }
        }

        // --- Video: synthetic H.264 stream (90 kHz clock, 15 fps) in the Zalo wire format ---
        // One Annex-B access unit per frame: `PT 97` when it fits `RTP_MAX_PAYLOAD`, else `PT 98` fragments
        // (see `packetize_zalo_au`); marker bit only on the last packet of the frame.
        // A PLI/FIR from the peer (RTCP inbox, or RTP socket if the relay mixes them) forces an immediate SPS+PPS+IDR.
        if (is_video || cam_enabled.load(Ordering::Relaxed))
            && last_video_tx.elapsed() >= Duration::from_millis(66)
        {
            last_video_tx = Instant::now();
            let force_kf = vid_frame_no == 0
                || vid_gop_pos >= H264_PSLICES.len()
                || vid_frame_no % VIDEO_IDR_INTERVAL_FRAMES == 0
                || want_keyframe.swap(false, Ordering::Relaxed);

            // Drain latest live webcam access unit if available (or read shared webcam stream for extra legs)
            let mut latest_cam_au: Option<Vec<u8>> = None;
            while let Ok(au) = cam_au_rx.try_recv() {
                if au_has_idr(&au) {
                    last_cam_idr = Some(au.clone());
                }
                latest_cam_au = Some(au);
            }
            if latest_cam_au.is_none() && slot_idx > 0 {
                if let Ok(sh) = SHARED_CAM_H264.lock() {
                    if !sh.2.is_empty() {
                        last_cam_idr = Some(sh.2.clone());
                    }
                    if sh.0 != last_shared_cam_seq && !sh.1.is_empty() {
                        last_shared_cam_seq = sh.0;
                        latest_cam_au = Some(sh.1.clone());
                    }
                }
            }
            let au_opt: Option<Vec<u8>> = if cam_enabled.load(Ordering::Relaxed) {
                if let Some(au) = latest_cam_au {
                    Some(au)
                } else if last_cam_idr.is_some() {
                    // Live webcam already started: never inject synthetic P-slices or stale IDR
                    // into the active libx264 reference chain when a 66ms tick happens between webcam frames.
                    None
                } else {
                    let idr_nal = if (vid_idr_parity & 1) == 0 {
                        H264_IDR
                    } else {
                        H264_IDR_ALT
                    };
                    let p_nal = H264_PSLICES[vid_gop_pos.min(H264_PSLICES.len() - 1)];
                    if force_kf {
                        vid_idr_parity = vid_idr_parity.wrapping_add(1);
                        vid_gop_pos = 0;
                        Some(annexb_au(&[H264_SPS, H264_PPS, idr_nal]))
                    } else {
                        vid_gop_pos += 1;
                        Some(annexb_au(&[p_nal]))
                    }
                }
            } else {
                let idr_nal = if (vid_idr_parity & 1) == 0 {
                    H264_IDR
                } else {
                    H264_IDR_ALT
                };
                let p_nal = H264_PSLICES[vid_gop_pos.min(H264_PSLICES.len() - 1)];
                if force_kf {
                    vid_idr_parity = vid_idr_parity.wrapping_add(1);
                    vid_gop_pos = 0;
                    Some(annexb_au(&[H264_SPS, H264_PPS, idr_nal]))
                } else {
                    vid_gop_pos += 1;
                    Some(annexb_au(&[p_nal]))
                }
            };

            if let Some(au_bytes) = au_opt {
                let is_group_sfu = rtp_addr.ends_with(":3000") || rtcp_addr.ends_with(":3500");
                let payloads = packetize_zalo_au(&au_bytes, RTP_MAX_PAYLOAD);
                let last = payloads.len().saturating_sub(1);
                let pic_id = (vid_frame_no as u16).wrapping_add(1);
                let cur_ntp = ntp_now();
                for (i, (vpt, payload)) in payloads.iter().enumerate() {
                    let mut vpkt = if is_group_sfu {
                        build_group_video_rtp_packet(
                            *vpt,
                            i == last,
                            vid_seq,
                            vid_ts,
                            vid_ssrc,
                            pic_id,
                            tx_layer_byte,
                            cur_ntp,
                            payload,
                        )
                    } else {
                        build_rtp_packet(*vpt, i == last, vid_seq, vid_ts, vid_ssrc, payload)
                    };
                    if is_group_sfu {
                        if let Some(ref srtp) = srtp_ctx {
                            srtp.crypt_rtp_packet(&mut vpkt);
                        }
                    }
                    vid_seq = vid_seq.wrapping_add(1);
                    vid_pkts_tx = vid_pkts_tx.wrapping_add(1);
                    vid_octets_tx = vid_octets_tx.wrapping_add(payload.len() as u32);
                    if let (Some(tsock), Some(target)) = (&udp_rtp_sock, rtp_sock_addr) {
                        if relay_token != 0 {
                            let type_byte: u8 = 0x0D; // `ZRTPPacket::initZRTPPacketVideo` (`isRtcp=false`)
                            let mut wire = Vec::with_capacity(5 + vpkt.len());
                            wire.push(type_byte);
                            wire.extend_from_slice(&relay_token.to_le_bytes());
                            wire.extend_from_slice(&vpkt);
                            send_dgram(tsock, &wire, target, &mut txs);
                            wl.rtp(">", type_byte, wire.len(), &rtp_addr, &vpkt);
                        } else {
                            send_dgram(tsock, &vpkt, target, &mut txs);
                            wl.rtp(">", vpkt[0], vpkt.len(), &rtp_addr, &vpkt);
                        }
                    }
                }
                vid_frame_no = vid_frame_no.wrapping_add(1);
                vid_ts = vid_ts.wrapping_add(6000); // 90000 Hz / 15 fps
            }
        }
        if last_rtcp.elapsed() >= Duration::from_secs(2) {
            last_rtcp = Instant::now();
            if let (Some(ref rtcp_s), Some(target)) = (&udp_rtcp_sock, rtcp_sock_addr) {
                // LSR/DLSR of the peer's last SR (RFC 3550 6.4.1); DLSR is in 1/65536 s.
                let (lsr, dlsr) = match last_peer_sr {
                    Some((m, at)) => (m, (at.elapsed().as_secs_f64() * 65536.0) as u32),
                    None => (0, 0),
                };
                let mut pkts: Vec<(u8, Vec<u8>)> = Vec::new();
                // Audio: RR (`0x05` = `initZRTPPacketAudio` with `isRtcp=true`) from our audio SSRC.
                let mut rr = vec![0x80u8, 201, 0, 1];
                rr.extend_from_slice(&ssrc.to_be_bytes());
                if let Some(b) = rx_audio.block(peer_audio_ssrc, lsr, dlsr) {
                    rr[0] = 0x81;
                    rr[2..4].copy_from_slice(&7u16.to_be_bytes());
                    rr.extend_from_slice(&b);
                }
                pkts.push((0x05, rr));
                // Video: SR + SDES + REMB (`0x0F` = `initZRTPPacketVideo` with `isRtcp=true`) from our video SSRC.
                if is_video || cam_enabled.load(Ordering::Relaxed) {
                    let fb_ssrc = resolve_video_feedback_ssrc(peer_video_ssrc, peer_audio_ssrc);
                    let sr = build_sender_report(
                        vid_ssrc,
                        ntp_now(),
                        vid_ts,
                        vid_pkts_tx,
                        vid_octets_tx,
                        fb_ssrc.and_then(|s| rx_video.block(s, lsr, dlsr)),
                    );
                    let mut c = sr;
                    c.extend_from_slice(&build_sdes_empty(vid_ssrc));
                    if let Some(v_ssrc) = fb_ssrc {
                        c.extend_from_slice(&build_remb(vid_ssrc, v_ssrc, 1_500_000));
                    }
                    pkts.push((0x0F, c));
                }
                // Also send ZRTP IncallNegotiateInfo (`H264DecCapacityInfo(720, 30)`) on RTP socket
                if (is_video || cam_enabled.load(Ordering::Relaxed)) && relay_token != 0 {
                    if let (Some(tsock), Some(rtarget)) = (&udp_rtp_sock, rtp_sock_addr) {
                        let nego_pkt = build_zrtp_incall_negotiate_packet(
                            partner_uid_u32,
                            relay_token,
                            nego_seq,
                        );
                        nego_seq = nego_seq.wrapping_add(1);
                        send_dgram(tsock, &nego_pkt, rtarget, &mut txs);
                        if rtp_addr.ends_with(":3000") || rtcp_addr.ends_with(":3500") {
                            let lay_pkt = crate::call::build_zrtp_group_video_layers_packet(
                                from_uid_u32,
                                relay_token,
                                nego_seq,
                            );
                            nego_seq = nego_seq.wrapping_add(1);
                            send_dgram(tsock, &lay_pkt, rtarget, &mut txs);
                            if !room_partners.is_empty() {
                                let sub_pkt = crate::call::build_zrtp_group_subscribe_video_packet(
                                    from_uid_u32,
                                    relay_token,
                                    nego_seq,
                                    &room_partners,
                                );
                                nego_seq = nego_seq.wrapping_add(1);
                                send_dgram(tsock, &sub_pkt, rtarget, &mut txs);
                            }
                        }
                    }
                }
                let rtcp_sender = udp_rtp_sock.as_ref().unwrap_or(rtcp_s);
                for (rtcp_type, rtcp_pkt) in pkts {
                    if relay_token != 0 {
                        let mut wire = Vec::with_capacity(5 + rtcp_pkt.len());
                        wire.push(rtcp_type);
                        wire.extend_from_slice(&relay_token.to_le_bytes());
                        wire.extend_from_slice(&rtcp_pkt);
                        send_dgram(rtcp_sender, &wire, target, &mut txs);
                        if let Some(rtarget) = rtp_sock_addr {
                            if rtarget != target {
                                send_dgram(rtcp_sender, &wire, rtarget, &mut txs);
                            }
                        }
                        wl.rtcp(
                            ">",
                            crate::voip_log::Via::RtcpSock,
                            rtcp_type,
                            wire.len(),
                            &rtcp_addr,
                            &rtcp_pkt,
                        );
                    } else {
                        send_dgram(rtcp_sender, &rtcp_pkt, target, &mut txs);
                        wl.rtcp(
                            ">",
                            crate::voip_log::Via::RtcpSock,
                            rtcp_pkt[0],
                            rtcp_pkt.len(),
                            &rtcp_addr,
                            &rtcp_pkt,
                        );
                    }
                }
            }
        }

        // --- RTCP from the keepalive task's socket reader: PLI / FIR -> force a keyframe ---
        // --- Drain datagrams from the RTCP socket inbox + up to 64 datagrams from the RTP socket ---
        let mut incoming_dgrams: Vec<(crate::voip_log::Via, String, Vec<u8>)> = drain_rtcp_inbox()
            .into_iter()
            .map(|w| (crate::voip_log::Via::RtcpSock, rtcp_addr.clone(), w))
            .collect();
        let mut buf = [0u8; 4000];
        for _ in 0..64 {
            let recv_info = if let Some(tsock) = &udp_rtp_sock {
                tsock.try_recv_from(&mut buf).ok()
            } else if let Some(fsock) = &fallback_sock {
                fsock.recv_from(&mut buf).ok()
            } else {
                None
            };
            let Some((n, peer)) = recv_info else {
                break;
            };
            incoming_dgrams.push((
                crate::voip_log::Via::RtpSock,
                peer.to_string(),
                buf[..n].to_vec(),
            ));
        }
        for (via, peer_s, raw_pkt) in incoming_dgrams {
            let n = raw_pkt.len();
            // Parse `CMD_ZAVI_ROOM_UPDATED` (`0x02 0x7E ... cmd=0x0032` at `0x0092B922` in `ZaloCall.exe`)
            if n >= 29
                && matches!(raw_pkt[0], 0x01 | 0x02)
                && matches!(raw_pkt[1], 0x00 | 0x7E)
                && u16::from_le_bytes([raw_pkt[18], raw_pkt[19]]) == 0x0032
            {
                let res_code =
                    i32::from_le_bytes([raw_pkt[21], raw_pkt[22], raw_pkt[23], raw_pkt[24]]);
                if res_code == 0 {
                    let mut idx = 25usize;
                    let mut added = false;
                    while idx + 4 <= n {
                        let p_uid = u32::from_le_bytes([
                            raw_pkt[idx],
                            raw_pkt[idx + 1],
                            raw_pkt[idx + 2],
                            raw_pkt[idx + 3],
                        ]);
                        idx += 4;
                        if p_uid != 0 && p_uid != from_uid_u32 && !room_partners.contains(&p_uid) {
                            room_partners.push(p_uid);
                            added = true;
                        }
                    }
                    crate::call::trace_line(
                        "ZRTP",
                        format!("CMD_ZAVI_ROOM_UPDATED partners={room_partners:?}"),
                    );
                    if (added || !room_partners.is_empty()) && relay_token != 0 {
                        if let (Some(tsock), Some(rtarget)) = (&udp_rtp_sock, rtp_sock_addr) {
                            let sub_pkt = crate::call::build_zrtp_group_subscribe_video_packet(
                                from_uid_u32,
                                relay_token,
                                nego_seq,
                                &room_partners,
                            );
                            nego_seq = nego_seq.wrapping_add(1);
                            send_dgram(tsock, &sub_pkt, rtarget, &mut txs);
                        }
                        want_keyframe.store(true, Ordering::Relaxed);
                    }
                }
                continue;
            }
            // Parse group SFU `cmd = 0x0020` control packets (`0x00924325` in `ZaloCall.exe`):
            // - `sub_cmd = 0x0C` (spatial layers announcement from a partner or our own echo)
            // - `sub_cmd = 0x0D` (video subscription request from a partner or our own echo)
            if n >= 21
                && matches!(raw_pkt[0], 0x01 | 0x02)
                && raw_pkt[1] == 0x01
                && u16::from_le_bytes([raw_pkt[18], raw_pkt[19]]) == 0x0020
            {
                let sub_cmd_20 = raw_pkt[20];
                let sender_uid =
                    u32::from_le_bytes([raw_pkt[10], raw_pkt[11], raw_pkt[12], raw_pkt[13]]);
                if sender_uid != 0
                    && sender_uid != from_uid_u32
                    && !room_partners.contains(&sender_uid)
                {
                    room_partners.push(sender_uid);
                }
                if sub_cmd_20 == 0x0C
                    && sender_uid != 0
                    && sender_uid != from_uid_u32
                    && relay_token != 0
                {
                    // Partner just announced video spatial layers: immediately subscribe to their video!
                    if let (Some(tsock), Some(rtarget)) = (&udp_rtp_sock, rtp_sock_addr) {
                        let sub_pkt = crate::call::build_zrtp_group_subscribe_video_packet(
                            from_uid_u32,
                            relay_token,
                            nego_seq,
                            &room_partners,
                        );
                        nego_seq = nego_seq.wrapping_add(1);
                        send_dgram(tsock, &sub_pkt, rtarget, &mut txs);
                    }
                } else if sub_cmd_20 == 0x0D && sender_uid != from_uid_u32 {
                    // Parse requested quality_id for our UID (`0x009246DD` in `ZaloCall.exe`):
                    // payload @ 21: `[u16_len][count: u8]([uid: u32 LE][quality_id: u8])*`
                    if n >= 24 {
                        let cnt = raw_pkt[23] as usize;
                        let mut off = 24usize;
                        for _ in 0..cnt {
                            if off + 5 <= n {
                                let req_uid = u32::from_le_bytes([
                                    raw_pkt[off],
                                    raw_pkt[off + 1],
                                    raw_pkt[off + 2],
                                    raw_pkt[off + 3],
                                ]);
                                let req_q = raw_pkt[off + 4];
                                off += 5;
                                if (req_uid == from_uid_u32 || cnt == 1) && req_q != 0xFF {
                                    tx_layer_byte = (req_q << 4) | (req_q & 0x0F);
                                }
                            }
                        }
                    }
                    // A partner subscribed to our video stream: force an immediate IDR keyframe!
                    want_keyframe.store(true, Ordering::Relaxed);
                }
                continue;
            }
            if unwrap_zalo_relay_rtp(&raw_pkt).is_none() {
                wl.unknown(via, n, &peer_s, &raw_pkt);
            }
            if let Some(inner_rtp) = unwrap_zalo_relay_rtp(&raw_pkt) {
                // RTCP (RFC 5761 range 200..=207): react to PLI / FIR by forcing a keyframe.
                if matches!(inner_rtp.get(1), Some(b) if (200..=207).contains(b)) {
                    if let Some(m) = sr_middle_ntp(inner_rtp) {
                        last_peer_sr = Some((m, Instant::now()));
                    }
                    if wl.rtcp("<", via, raw_pkt[0], n, &peer_s, inner_rtp) {
                        want_keyframe.store(true, Ordering::Relaxed);
                        crate::call::trace_line(
                            "RTCP<",
                            format!("from={peer_s} PLI/FIR -> force IDR"),
                        );
                    }
                    continue;
                }
                let pt = inner_rtp.get(1).map(|b| b & 0x7F).unwrap_or(0);
                wl.rtp("<", raw_pkt[0], n, &peer_s, inner_rtp);
                let parsed_h = crate::voip_log::parse_rtp(inner_rtp);
                let is_group_sfu = rtp_addr.ends_with(":3000") || rtcp_addr.ends_with(":3500");
                let raw_payload_opt: Option<Vec<u8>> = if is_group_sfu {
                    if let Some(ref srtp) = srtp_ctx {
                        srtp.decrypt_rtp_payload(inner_rtp)
                    } else {
                        extract_rtp_opus_payload(inner_rtp).map(|s| s.to_vec())
                    }
                } else {
                    extract_rtp_opus_payload(inner_rtp).map(|s| s.to_vec())
                };
                let is_video_pkt = raw_payload_opt
                    .as_deref()
                    .is_some_and(|p| is_incoming_video_rtp(pt, raw_pkt[0], p));

                if is_video_pkt {
                    if let Some(ref h) = parsed_h {
                        let key = (h.ssrc, h.seq);
                        if recent_vid_seqs.contains(&key) {
                            continue;
                        }
                        if recent_vid_seqs.len() >= 64 {
                            recent_vid_seqs.pop_front();
                        }
                        recent_vid_seqs.push_back(key);
                    }
                }
                if let Some(ref h) = parsed_h {
                    if pt == 112 {
                        rx_audio.update(h.seq);
                        if h.ssrc != 0 {
                            peer_audio_ssrc = h.ssrc;
                        }
                    } else if is_video_pkt {
                        if rx_video.update(h.seq) {
                            pli_gap = true; // hole in the peer's video: ask for a keyframe
                        }
                        if h.ssrc != 0 {
                            peer_video_ssrc = h.ssrc;
                        }
                    }
                }
                static LOGGED_RX: AtomicUsize = AtomicUsize::new(0);
                let cnt = LOGGED_RX.fetch_add(1, Ordering::Relaxed);
                if cnt < 16 || cnt % 100 == 0 {
                    crate::call::trace_line(
                        "RTP<",
                        format!("from={peer_s} len={n} outer=0x{:02x} pt={pt}", raw_pkt[0]),
                    );
                }
                if pt == 112 {
                    if let (Some(payload), Some(ref h)) = (raw_payload_opt, parsed_h) {
                        jitter_buf.push(h.seq, payload);
                    }
                } else if is_video_pkt {
                    let pkt_ssrc = parsed_h.as_ref().map(|h| h.ssrc).unwrap_or(0);
                    let pkt_seq = parsed_h.as_ref().map(|h| h.seq).unwrap_or(0);
                    let pkt_ts = parsed_h.as_ref().map(|h| h.ts).unwrap_or(0);
                    if primary_video_ssrc == 0 || pkt_ssrc == 0 || pkt_ssrc == primary_video_ssrc {
                        if pkt_ssrc != 0 {
                            primary_video_ssrc = pkt_ssrc;
                        }
                        last_primary_vid_rx = Some(Instant::now());
                        if let Some(payload) = raw_payload_opt.as_deref() {
                            match primary_depacketizer.push(pkt_seq, pkt_ts, payload) {
                                DepacketizeResult::Ready(au) => {
                                    let _ = h264_tx.try_send(au);
                                }
                                DepacketizeResult::DroppedCorrupt => {
                                    pli_gap = true;
                                }
                                DepacketizeResult::Buffering | DepacketizeResult::Ignored => {}
                            }
                        }
                    } else if slot_idx == 0 {
                        let stream_pos = extra_vid_streams
                            .iter()
                            .position(|s| s.ssrc == pkt_ssrc);
                        let idx = if let Some(pos) = stream_pos {
                            Some(pos)
                        } else if extra_vid_streams.len() < 5 {
                            if let Some(ctx) = egui_ctx_extra.clone() {
                                let stream_num = extra_vid_streams.len() + 1;
                                let tex_key = format!("call_remote_video_{stream_num}");
                                let tex_key_cl = tex_key.clone();
                                let (tx_n, rx_n) = mpsc::sync_channel::<Vec<u8>>(64);
                                let stop_vn = stop.clone();
                                let synced_vn = Arc::new(AtomicBool::new(false));
                                let handle = thread::spawn(move || {
                                    run_h264_ffmpeg_decoder(
                                        stop_vn, rx_n, ctx, synced_vn, tex_key_cl,
                                    );
                                });
                                extra_vid_streams.push(ExtraVideoStream {
                                    ssrc: pkt_ssrc,
                                    tex_key,
                                    depacketizer: VideoDepacketizer::new(),
                                    tx: tx_n,
                                    last_rx: None,
                                    _handle: handle,
                                });
                                Some(extra_vid_streams.len() - 1)
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        if let Some(i) = idx {
                            if let Some(payload) = raw_payload_opt.as_deref() {
                                let stream = &mut extra_vid_streams[i];
                                stream.last_rx = Some(Instant::now());
                                match stream.depacketizer.push(pkt_seq, pkt_ts, payload) {
                                    DepacketizeResult::Ready(au) => {
                                        let _ = stream.tx.try_send(au);
                                    }
                                    DepacketizeResult::DroppedCorrupt => {
                                        pli_gap = true;
                                    }
                                    DepacketizeResult::Buffering | DepacketizeResult::Ignored => {}
                                }
                            }
                        }
                    }
                }
            }
        }

        // --- RX Audio Jitter Buffer Playout (20 ms cadence with Opus FEC & PLC) ---
        let mut rx_play_burst = 0usize;
        let now_rx = Instant::now();
        if now_rx.duration_since(next_rx_playout) > Duration::from_millis(120) {
            next_rx_playout = now_rx;
        }
        while next_rx_playout <= now_rx && rx_play_burst < 3 {
            next_rx_playout += Duration::from_millis(20);
            rx_play_burst += 1;
            let mut pcm = [0.0_f32; FRAME_SAMPLES];
            let dec_res = match jitter_buf.pop_next() {
                JitterPop::Packet(p) => decoder.decode_float(&p, &mut pcm, false),
                JitterPop::FecFromNext(p) => decoder.decode_float(&p, &mut pcm, true),
                JitterPop::Plc => decoder.decode_float(&[], &mut pcm, false),
                JitterPop::Buffering | JitterPop::Silence => {
                    REMOTE_AUDIO_ACTIVE.store(false, Ordering::Relaxed);
                    continue;
                }
            };
            if let Ok(decoded_n) = dec_res {
                let slice = &mut pcm[..decoded_n.min(FRAME_SAMPLES)];
                let mut r_peak = 0.0_f32;
                for s in slice.iter_mut() {
                    *s = soft_limit(*s);
                    r_peak = r_peak.max(s.abs());
                }
                REMOTE_AUDIO_ACTIVE.store(r_peak > 0.025, Ordering::Relaxed);
                push_peer_bridge_audio(slot_idx, slice);
                for &s in slice.iter() {
                    let _ = output_ring.push(s);
                }
            }
        }

        // Clear frozen remote video texture if a peer stops sending video RTP for > 1.8s (camera turned off)
        if let Some(t) = last_primary_vid_rx {
            if t.elapsed() >= Duration::from_millis(1800) {
                last_primary_vid_rx = None;
                let pri_key = if slot_idx == 0 {
                    "call_remote_video".to_string()
                } else {
                    format!("call_remote_video_{slot_idx}")
                };
                clear_call_video_frame(&pri_key);
                if let Some(ref ctx) = egui_ctx_extra {
                    ctx.request_repaint();
                }
            }
        }
        for stream in &mut extra_vid_streams {
            if let Some(t) = stream.last_rx {
                if t.elapsed() >= Duration::from_millis(1800) {
                    stream.last_rx = None;
                    clear_call_video_frame(&stream.tex_key);
                    if let Some(ref ctx) = egui_ctx_extra {
                        ctx.request_repaint();
                    }
                }
            }
        }
        wl.tick();
        // --- PLI towards the peer: its video is undecodable until a keyframe (SPS/IDR) reaches the decoder ---
        if let Some(target_video_ssrc) =
            resolve_video_feedback_ssrc(peer_video_ssrc, peer_audio_ssrc)
        {
            if (is_video || cam_enabled.load(Ordering::Relaxed))
                && decoder_active
                && (rx_video.started || rx_audio.started)
                && (!video_synced.load(Ordering::Relaxed) || pli_gap)
                && last_pli.elapsed() >= Duration::from_millis(500)
            {
                last_pli = Instant::now();
                let was_gap = pli_gap;
                pli_gap = false;
                if let (Some(ref rtcp_s), Some(target)) = (&udp_rtcp_sock, rtcp_sock_addr) {
                    let rtcp_sender = udp_rtp_sock.as_ref().unwrap_or(rtcp_s);
                    // Compound packet: empty RR first (RFC 3550 6.1), then the PLI.
                    let mut pkt = vec![0x80u8, 201, 0, 1];
                    pkt.extend_from_slice(&ssrc.to_be_bytes());
                    pkt.extend_from_slice(&build_pli(ssrc, target_video_ssrc));
                    if relay_token != 0 {
                        let rtcp_type: u8 = 0x0F; // `ZRTPPacket::initZRTPPacketVideo` (`isRtcp=true`)
                        let mut wire = Vec::with_capacity(5 + pkt.len());
                        wire.push(rtcp_type);
                        wire.extend_from_slice(&relay_token.to_le_bytes());
                        wire.extend_from_slice(&pkt);
                        send_dgram(rtcp_sender, &wire, target, &mut txs);
                        if let Some(rtarget) = rtp_sock_addr {
                            if rtarget != target {
                                send_dgram(rtcp_sender, &wire, rtarget, &mut txs);
                            }
                        }
                        wl.rtcp(
                            ">",
                            crate::voip_log::Via::RtcpSock,
                            rtcp_type,
                            wire.len(),
                            &rtcp_addr,
                            &pkt,
                        );
                    } else {
                        send_dgram(rtcp_sender, &pkt, target, &mut txs);
                        wl.rtcp(
                            ">",
                            crate::voip_log::Via::RtcpSock,
                            pkt[0],
                            pkt.len(),
                            &rtcp_addr,
                            &pkt,
                        );
                    }
                    pli_sent += 1;
                    crate::logger::log_voip_event(
                        "video::pli_sent",
                        &format!(
                            "target_ssrc={target_video_ssrc:#010x} video_ssrc={peer_video_ssrc:#010x} audio_ssrc={peer_audio_ssrc:#010x} count={pli_sent} synced={} gap={was_gap}",
                            video_synced.load(Ordering::Relaxed)
                        ),
                        false,
                    );
                }
            }
        }
        if last_audio_stats_log.elapsed() >= Duration::from_secs(5) {
            last_audio_stats_log = Instant::now();
            crate::logger::log_voip_event(
                "audio::quality_stats",
                &format!(
                    "jb_target_ms={} jb_depth={} rx_pkts={} played={} fec={} plc={} late={} overflow={} reordered={} in_ring={} out_ring={}",
                    jitter_buf.target_packets * 20,
                    jitter_buf.packets.len(),
                    jitter_buf.rx_pushed,
                    jitter_buf.played_normal,
                    jitter_buf.fec_recovered,
                    jitter_buf.plc_concealed,
                    jitter_buf.late_dropped,
                    jitter_buf.overflow_dropped,
                    jitter_buf.reordered,
                    input_ring.len(),
                    output_ring.len(),
                ),
                false,
            );
        }
        // --- send-side health: result counters of every UDP send + longest stall of this loop ---
        let gap = last_iter.elapsed();
        last_iter = Instant::now();
        if gap > max_loop_gap {
            max_loop_gap = gap;
        }
        if last_txst.elapsed() >= Duration::from_secs(5) {
            last_txst = Instant::now();
            crate::voip_log::line(
                "TXST",
                format!(
                    "sent_ok={} would_block={} short={} err={} last_err={} max_loop_gap_ms={} pli_sent={} video_synced={}",
                    txs.ok,
                    txs.would_block,
                    txs.short,
                    txs.err,
                    txs.last_err.as_deref().unwrap_or("-"),
                    max_loop_gap.as_millis(),
                    pli_sent,
                    video_synced.load(Ordering::Relaxed)
                ),
            );
            max_loop_gap = Duration::ZERO;
        }
        thread::sleep(Duration::from_millis(2));
    }
    crate::voip_log::line("CALL", "media stop");

    drop(h264_tx);
    drop(cam_au_tx);
    let extra_handles: Vec<JoinHandle<()>> = extra_vid_streams
        .drain(..)
        .map(|s| s._handle)
        .collect();
    if let Some(h) = primary_dec_handle {
        let _ = h.join();
    }
    if let Some(h) = cam_handle {
        let _ = h.join();
    }
    for h in extra_handles {
        let _ = h.join();
    }
    if let Some(mic_stream) = active_mic_stream.take() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(mic_stream)));
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(output_stream)));
}

/// Reassemble RFC 6184 H.264 RTP payloads (Single NAL 1..23, STAP-A 24, FU-A 28) into Annex-B (`00 00 00 01`) byte stream.
#[cfg(test)]
fn depacketize_rfc6184_h264(
    payload: &[u8],
    fu_a_buf: &mut Vec<u8>,
    h264_tx: &std::sync::mpsc::SyncSender<Vec<u8>>,
) {
    if payload.is_empty() {
        return;
    }
    let nal_type = payload[0] & 0x1F;
    match nal_type {
        1..=23 => {
            let mut annexb = Vec::with_capacity(4 + payload.len());
            annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            annexb.extend_from_slice(payload);
            let _ = h264_tx.try_send(annexb);
        }
        24 => {
            // STAP-A: multiple NAL units prefixed by u16 BE length
            let mut idx = 1usize;
            let mut annexb = Vec::new();
            while idx + 2 <= payload.len() {
                let sz = u16::from_be_bytes([payload[idx], payload[idx + 1]]) as usize;
                idx += 2;
                if sz > 0 && idx + sz <= payload.len() {
                    annexb.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                    annexb.extend_from_slice(&payload[idx..idx + sz]);
                    idx += sz;
                } else {
                    break;
                }
            }
            if !annexb.is_empty() {
                let _ = h264_tx.try_send(annexb);
            }
        }
        28 if payload.len() >= 2 => {
            // FU-A fragmented NAL unit
            let fu_ind = payload[0];
            let fu_hdr = payload[1];
            let start_bit = (fu_hdr & 0x80) != 0;
            let end_bit = (fu_hdr & 0x40) != 0;
            let orig_nal_hdr = (fu_ind & 0xE0) | (fu_hdr & 0x1F);
            if start_bit {
                fu_a_buf.clear();
                fu_a_buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, orig_nal_hdr]);
                fu_a_buf.extend_from_slice(&payload[2..]);
            } else if !fu_a_buf.is_empty() {
                fu_a_buf.extend_from_slice(&payload[2..]);
                if end_bit {
                    let completed = std::mem::take(fu_a_buf);
                    let _ = h264_tx.try_send(completed);
                }
            }
        }
        _ => {}
    }
}

/// Access Unit Delimiter NAL (`primary_pic_type=7`), written before every access unit sent to ffmpeg.
const H264_AUD: &[u8] = &[0, 0, 0, 1, 0x09, 0xf0];

/// Spawn a headless `ffmpeg` pipe to decode incoming H.264 Annex-B NAL units into `480×360` RGBA frames.
/// Trim black letterbox/pillarbox borders added by `ffmpeg pad` so the returned RGBA buffer
/// has the exact aspect ratio of the active video content with zero black bars.
fn trim_black_borders(rgba: &[u8], w: usize, h: usize) -> (usize, usize, Vec<u8>) {
    if rgba.len() < w * h * 4 || w < 16 || h < 16 {
        return (w, h, rgba.to_vec());
    }
    let is_non_black = |x: usize, y: usize| -> bool {
        let idx = (y * w + x) * 4;
        rgba[idx] > 4 || rgba[idx + 1] > 4 || rgba[idx + 2] > 4
    };
    let mid_y = h / 2;
    let y1 = h / 4;
    let y2 = (h * 3) / 4;
    let mut left = 0usize;
    while left < w / 2
        && !is_non_black(left, mid_y)
        && !is_non_black(left, y1)
        && !is_non_black(left, y2)
    {
        left += 1;
    }
    let mut right = w;
    while right > w / 2
        && !is_non_black(right - 1, mid_y)
        && !is_non_black(right - 1, y1)
        && !is_non_black(right - 1, y2)
    {
        right -= 1;
    }
    let mid_x = w / 2;
    let x1 = w / 4;
    let x2 = (w * 3) / 4;
    let mut top = 0usize;
    while top < h / 2
        && !is_non_black(mid_x, top)
        && !is_non_black(x1, top)
        && !is_non_black(x2, top)
    {
        top += 1;
    }
    let mut bottom = h;
    while bottom > h / 2
        && !is_non_black(mid_x, bottom - 1)
        && !is_non_black(x1, bottom - 1)
        && !is_non_black(x2, bottom - 1)
    {
        bottom -= 1;
    }
    let sym_x = left.min(w.saturating_sub(right));
    let sym_y = top.min(h.saturating_sub(bottom));
    if sym_x < 4 && sym_y < 4 {
        return (w, h, rgba.to_vec());
    }
    let cw = w.saturating_sub(sym_x * 2).max(16);
    let ch = h.saturating_sub(sym_y * 2).max(16);
    let mut out = Vec::with_capacity(cw * ch * 4);
    for y in sym_y..(sym_y + ch) {
        let row_start = (y * w + sym_x) * 4;
        let row_end = row_start + cw * 4;
        out.extend_from_slice(&rgba[row_start..row_end]);
    }
    (cw, ch, out)
}

fn run_h264_ffmpeg_decoder(
    stop: Arc<AtomicBool>,
    h264_rx: std::sync::mpsc::Receiver<Vec<u8>>,
    egui_ctx: eframe::egui::Context,
    synced: Arc<AtomicBool>,
    tex_key: String,
) {
    use std::io::{Read, Write};
    use std::process::{Child, Command, Stdio};

    const VID_W: usize = 640;
    const VID_H: usize = 480;
    const FRAME_BYTES: usize = VID_W * VID_H * 4;

    fn spawn_ffmpeg_h264_child(tex_key: &str) -> Option<Child> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-hide_banner",
            "-loglevel",
            "warning",
            "-fflags",
            "nobuffer",
            "-flags",
            "low_delay",
            "-threads",
            "1",
            "-probesize",
            "32768",
            "-analyzeduration",
            "0",
            "-f",
            "h264",
            "-i",
            "pipe:0",
            "-vf",
            "scale=640:480:force_original_aspect_ratio=decrease:flags=lanczos,pad=640:480:(ow-iw)/2:(oh-ih)/2:black",
            "-fps_mode",
            "passthrough",
            "-pix_fmt",
            "rgba",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        match cmd.spawn() {
            Ok(c) => Some(c),
            Err(e) => {
                crate::voip_log::line("VDEC", format!("ffmpeg spawn failed: {e}"));
                crate::logger::log_voip_event(
                    "video::decoder_error",
                    &format!("ffmpeg spawn failed for {tex_key}: {e}"),
                    true,
                );
                None
            }
        }
    }

    let decoded_counter = Arc::new(AtomicU64::new(0));
    let mut preparer = H264AccessUnitPreparer::new();
    let mut restarts = 0u64;
    let mut keyframes_seen = 0u64;
    let mut dropped_pre_sync = 0u64;
    let mut first_frame_logged = false;

    while !stop.load(Ordering::Relaxed) {
        synced.store(false, Ordering::Relaxed);
        preparer.reset_sync();
        let Some(child) = spawn_ffmpeg_h264_child(&tex_key) else {
            return;
        };
        let mut child_guard = ChildGuard::new(child);
        crate::voip_log::line("VDEC", "ffmpeg decoder started");
        let Some(mut stdin) = child_guard.take_stdin() else {
            return;
        };
        let Some(mut stdout) = child_guard.take_stdout() else {
            return;
        };
        let stderr_handle = child_guard.take_stderr().map(|mut stderr| {
            thread::spawn(move || {
                let mut buf = [0u8; 512];
                let mut lines = 0u32;
                while let Ok(n) = stderr.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    lines += 1;
                    if lines <= 20 {
                        crate::voip_log::line(
                            "VDEC",
                            format!("ffmpeg: {}", String::from_utf8_lossy(&buf[..n]).trim()),
                        );
                    }
                }
            })
        });

        let stop_reader = stop.clone();
        let egui_ctx_r = egui_ctx.clone();
        let tex_key_r = tex_key.clone();
        let dec_cnt_r = decoded_counter.clone();
        let reader_handle = thread::spawn(move || {
            let mut raw_frame = vec![0u8; FRAME_BYTES];
            while !stop_reader.load(Ordering::Relaxed) {
                if stdout.read_exact(&mut raw_frame).is_ok() {
                    let total_dec = dec_cnt_r.fetch_add(1, Ordering::Relaxed) + 1;
                    if total_dec == 1 || total_dec.is_multiple_of(100) {
                        crate::voip_log::line("VDEC", format!("decoded frame #{total_dec}"));
                    }
                    let (cw, ch, cropped) = trim_black_borders(&raw_frame, VID_W, VID_H);
                    if total_dec == 1 {
                        crate::logger::log_voip_event(
                            "video::first_frame",
                            &format!("tex={tex_key_r} size={cw}x{ch}"),
                            false,
                        );
                    }
                    let color_img =
                        eframe::egui::ColorImage::from_rgba_unmultiplied([cw, ch], &cropped);
                    publish_call_video_frame(&tex_key_r, color_img);
                    egui_ctx_r.request_repaint();
                } else {
                    break;
                }
            }
        });

        let mut fed = 0u64;
        let mut last_seen_dec = decoded_counter.load(Ordering::Relaxed);
        let mut last_decode_progress_at = Instant::now();
        let mut fed_since_progress = 0u64;
        let mut sync_at: Option<Instant> = None;
        let mut soft_pli_requested_for_stall = false;
        let mut last_stat_log = Instant::now();
        let mut restart_decoder = false;

        while !stop.load(Ordering::Relaxed) {
            let cur_dec_check = decoded_counter.load(Ordering::Relaxed);
            if cur_dec_check > last_seen_dec {
                if !first_frame_logged {
                    first_frame_logged = true;
                    if let Some(t_sync) = sync_at {
                        crate::logger::log_voip_event(
                            "video::first_frame",
                            &format!(
                                "tex={tex_key} first_frame_latency_ms={} fed={fed}",
                                t_sync.elapsed().as_millis()
                            ),
                            false,
                        );
                    }
                }
                last_seen_dec = cur_dec_check;
                last_decode_progress_at = Instant::now();
                fed_since_progress = 0;
                soft_pli_requested_for_stall = false;
            }

            if last_stat_log.elapsed() >= Duration::from_secs(5) {
                last_stat_log = Instant::now();
                crate::logger::log_voip_event(
                    "video::decode_stats",
                    &format!(
                        "tex={tex_key} fed={fed} decoded={cur_dec_check} keyframes={keyframes_seen} dropped_pre_sync={dropped_pre_sync} restarts={restarts} params_cached={} synced={}",
                        preparer.has_params(),
                        synced.load(Ordering::Relaxed)
                    ),
                    false,
                );
            }

            match h264_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(raw_au) => {
                    let bytes_to_feed = match preparer.prepare(&raw_au) {
                        PreparedAu::ParamSetOnly => {
                            crate::logger::log_voip_event(
                                "video::keyframe",
                                &format!("tex={tex_key} cached_param_set bytes={}", raw_au.len()),
                                false,
                            );
                            continue;
                        }
                        PreparedAu::MissingParamsForIdr => {
                            dropped_pre_sync += 1;
                            synced.store(false, Ordering::Relaxed);
                            if dropped_pre_sync == 1 || dropped_pre_sync.is_multiple_of(30) {
                                crate::logger::log_voip_event(
                                    "video::keyframe",
                                    &format!("tex={tex_key} idr_missing_sps_pps dropped={dropped_pre_sync}"),
                                    true,
                                );
                            }
                            continue;
                        }
                        PreparedAu::DroppedPreSync => {
                            dropped_pre_sync += 1;
                            if dropped_pre_sync == 1 || dropped_pre_sync.is_multiple_of(100) {
                                crate::voip_log::line(
                                    "VDEC",
                                    format!(
                                        "waiting for SPS/IDR, dropped {dropped_pre_sync} P access units"
                                    ),
                                );
                            }
                            continue;
                        }
                        PreparedAu::Keyframe(bytes) => {
                            keyframes_seen += 1;
                            synced.store(true, Ordering::Relaxed);
                            if sync_at.is_none() {
                                sync_at = Some(Instant::now());
                            }
                            if fed_since_progress == 0 {
                                last_decode_progress_at = Instant::now();
                            }
                            crate::voip_log::line(
                                "VDEC",
                                format!(
                                    "keyframe #{keyframes_seen} after {dropped_pre_sync} pre-sync AUs ({} bytes)",
                                    bytes.len()
                                ),
                            );
                            crate::logger::log_voip_event(
                                "video::keyframe",
                                &format!(
                                    "tex={tex_key} keyframe=#{keyframes_seen} bytes={} pre_sync_dropped={dropped_pre_sync}",
                                    bytes.len()
                                ),
                                false,
                            );
                            bytes
                        }
                        PreparedAu::DeltaFrame(bytes) => bytes,
                    };

                    if fed_since_progress >= 10
                        && last_decode_progress_at.elapsed() >= Duration::from_millis(1500)
                        && !soft_pli_requested_for_stall
                    {
                        soft_pli_requested_for_stall = true;
                        synced.store(false, Ordering::Relaxed);
                        crate::logger::log_voip_event(
                            "video::stall",
                            &format!(
                                "tex={tex_key} soft_pli_request fed_since_progress={fed_since_progress} elapsed_ms={}",
                                last_decode_progress_at.elapsed().as_millis()
                            ),
                            false,
                        );
                    }
                    if fed_since_progress >= 30
                        && last_decode_progress_at.elapsed() >= Duration::from_secs(4)
                    {
                        restarts += 1;
                        crate::voip_log::line(
                            "VDEC",
                            format!(
                                "decoder stalled ({fed_since_progress} AUs over {}ms with 0 output frames), requesting PLI & restarting ffmpeg",
                                last_decode_progress_at.elapsed().as_millis()
                            ),
                        );
                        crate::logger::log_voip_event(
                            "video::stall",
                            &format!(
                                "tex={tex_key} restarting_ffmpeg fed_since_progress={fed_since_progress} restarts={restarts}"
                            ),
                            true,
                        );
                        synced.store(false, Ordering::Relaxed);
                        restart_decoder = true;
                        break;
                    }

                    if stdin.write_all(&bytes_to_feed).is_err() {
                        crate::voip_log::line(
                            "VDEC",
                            format!("ffmpeg stdin closed after {fed} access units"),
                        );
                        restarts += 1;
                        restart_decoder = true;
                        break;
                    }
                    let _ = stdin.flush();
                    fed += 1;
                    fed_since_progress += 1;
                    if fed == 1 || fed.is_multiple_of(100) {
                        crate::voip_log::line("VDEC", format!("fed {fed} access units to ffmpeg"));
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    restart_decoder = false;
                    break;
                }
            }
        }

        drop(stdin);
        child_guard.kill_and_reap();
        let _ = reader_handle.join();
        if let Some(h) = stderr_handle {
            let _ = h.join();
        }
        if !restart_decoder || stop.load(Ordering::Relaxed) {
            break;
        }
    }
    crate::voip_log::line(
        "VDEC",
        format!(
            "decoder stop, {} frames decoded",
            decoded_counter.load(Ordering::Relaxed)
        ),
    );
}

/// Play a pleasant, zero-latency Zalo-style two-tone notification chime (`E5` -> `B5`) via a warm background thread.
pub fn play_notification_chime() {
    static CHIME_TX: std::sync::OnceLock<std::sync::mpsc::SyncSender<()>> =
        std::sync::OnceLock::new();
    let tx = CHIME_TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<()>(1);
        thread::spawn(move || {
            while rx.recv().is_ok() {
                let Ok((out_dev, _)) = resolve_output_device() else {
                    continue;
                };
                let Ok(cfg) = out_dev.default_output_config() else {
                    continue;
                };
                let sr = cfg.sample_rate().0 as f32;
                let ch = cfg.channels() as usize;
                let stream_cfg: cpal::StreamConfig = cfg.into();
                let sample_idx = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let idx_cl = sample_idx.clone();
                let total_samples = (sr * 0.21) as usize;
                let split_sample = (sr * 0.075) as usize;

                if let Ok(stream) = out_dev.build_output_stream(
                    &stream_cfg,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        for frame in data.chunks_mut(ch) {
                            let i = idx_cl.fetch_add(1, Ordering::Relaxed);
                            let val = if i < total_samples {
                                let t = i as f32 / sr;
                                let (freq, local_t, dur) = if i < split_sample {
                                    (659.25_f32, t, 0.075_f32)
                                } else {
                                    (987.77_f32, (i - split_sample) as f32 / sr, 0.135_f32)
                                };
                                let env = ((local_t / 0.005).min(1.0))
                                    * (1.0 - (local_t / dur).clamp(0.0, 1.0)).powi(2);
                                let fundamental = (2.0 * std::f32::consts::PI * freq * t).sin();
                                let harmonic =
                                    (2.0 * std::f32::consts::PI * (freq * 2.0) * t).sin() * 0.25;
                                (fundamental + harmonic) * 0.20 * env
                            } else {
                                0.0
                            };
                            for s in frame.iter_mut() {
                                *s = val;
                            }
                        }
                    },
                    |_| {},
                    None,
                ) {
                    if stream.play().is_ok() {
                        thread::sleep(Duration::from_millis(220));
                    }
                }
            }
        });
        tx
    });
    let _ = tx.try_send(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unwrap_zalo_relay_rtp_all_formats() {
        let mut sample_rtp = vec![
            0x80, 112, 0x00, 0x01, 0x00, 0x00, 0x01, 0x40, 0x12, 0x34, 0x56, 0x78,
        ];
        sample_rtp.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);

        // 1. Raw RTP (0x80)
        let r1 = unwrap_zalo_relay_rtp(&sample_rtp).expect("raw rtp");
        assert_eq!(
            extract_rtp_opus_payload(r1),
            Some(&[0xAA, 0xBB, 0xCC, 0xDD][..])
        );

        // 2. 1-byte relay header (0x04 incoming RTP)
        let mut p4 = vec![0x04];
        p4.extend_from_slice(&sample_rtp);
        let r2 = unwrap_zalo_relay_rtp(&p4).expect("0x04 relay rtp");
        assert_eq!(
            extract_rtp_opus_payload(r2),
            Some(&[0xAA, 0xBB, 0xCC, 0xDD][..])
        );

        // 3. 5-byte relay header (0x03 caller / 0x05 callee + 4-byte token LE)
        let mut p5 = vec![0x03, 0x78, 0x56, 0x34, 0x12];
        p5.extend_from_slice(&sample_rtp);
        let r3 = unwrap_zalo_relay_rtp(&p5).expect("0x03 relay rtp");
        assert_eq!(
            extract_rtp_opus_payload(r3),
            Some(&[0xAA, 0xBB, 0xCC, 0xDD][..])
        );

        // 4. 9-byte 0x7F relay header (cmd = 6)
        let mut p9 = vec![0x7F, 0x00, 0x00, 0x11, 0x22, 0x33, 0x44, 0x06, 0x00];
        p9.extend_from_slice(&sample_rtp);
        let r4 = unwrap_zalo_relay_rtp(&p9).expect("0x7F relay rtp");
        assert_eq!(
            extract_rtp_opus_payload(r4),
            Some(&[0xAA, 0xBB, 0xCC, 0xDD][..])
        );
    }

    #[test]
    fn fu_a_roundtrip_and_marker_layout() {
        let mut nal = vec![0x65u8];
        nal.extend((0..3000u32).map(|i| (i % 251) as u8));
        let parts = packetize_h264_nal(&nal, 1200);
        assert!(parts.len() >= 3);
        assert!(parts.iter().all(|p| p.len() <= 1200));
        assert_eq!(parts[0][0], (0x65 & 0xE0) | 28);
        assert_eq!(parts[0][1], 0x80 | 5);
        assert_eq!(parts.last().unwrap()[1], 0x40 | 5);
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
        let mut buf = Vec::new();
        for p in &parts {
            depacketize_rfc6184_h264(p, &mut buf, &tx);
        }
        let out = rx.try_recv().expect("reassembled NAL");
        let mut expect = vec![0, 0, 0, 1];
        expect.extend_from_slice(&nal);
        assert_eq!(out, expect);
    }

    #[test]
    fn small_nal_is_single_packet() {
        let parts = packetize_h264_nal(H264_PPS, 1200);
        assert_eq!(parts, vec![H264_PPS.to_vec()]);
    }

    #[test]
    fn detects_pli_and_fir_in_compound_rtcp() {
        // RR (len=1) followed by PLI (fmt 1, len=2)
        let mut c = vec![0x80, 201, 0, 1, 0, 0, 0, 1];
        c.extend_from_slice(&[0x81, 206, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2]);
        assert!(rtcp_requests_keyframe(&c));
        let fir = [
            0x84, 206, 0, 4, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 1, 0, 0, 0,
        ];
        assert!(rtcp_requests_keyframe(&fir));
        let rr = [0x80, 201, 0, 1, 0, 0, 0, 1];
        assert!(!rtcp_requests_keyframe(&rr));
        // NACK (RTPFB 205) must not trigger a keyframe
        let nack = [0x81, 205, 0, 3, 0, 0, 0, 1, 0, 0, 0, 2, 0, 5, 0, 0];
        assert!(!rtcp_requests_keyframe(&nack));
    }

    #[test]
    fn rtp_marker_bit_only_when_requested() {
        let a = build_rtp_packet(97, true, 1, 2, 3, &[9]);
        assert_eq!(a[1], 0x80 | 97);
        let b = build_rtp_packet(97, false, 1, 2, 3, &[9]);
        assert_eq!(b[1], 97);
        assert_eq!(b.len(), 13);
    }

    #[test]
    fn wire_pli_through_relay_header_forces_keyframe() {
        let pli = [0x81u8, 206, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2];
        let mut w = vec![0x0D, 0x78, 0x56, 0x34, 0x12];
        w.extend_from_slice(&pli);
        assert!(wire_requests_keyframe(&w));
        let mut raw = Vec::new();
        raw.extend_from_slice(&pli);
        assert!(wire_requests_keyframe(&raw));
        // RTP video packet and ZRTP-like junk must not trigger
        let rtp = build_rtp_packet(97, true, 1, 2, 3, &[0x41, 0x9A]);
        assert!(!wire_requests_keyframe(&rtp));
        assert!(!wire_requests_keyframe(&[0x00, 0x0B, 1, 2, 3]));
    }

    #[test]
    fn rtcp_inbox_is_bounded_and_drains() {
        clear_rtcp_inbox();
        for i in 0..(RTCP_INBOX_MAX + 10) {
            push_rtcp_inbox(&[i as u8]);
        }
        let v = drain_rtcp_inbox();
        assert_eq!(v.len(), RTCP_INBOX_MAX);
        assert_eq!(v[0], vec![10u8]);
        assert!(drain_rtcp_inbox().is_empty());
    }

    fn au_of(len: usize) -> Vec<u8> {
        let mut au = annexb_au(&[H264_SPS, H264_PPS]);
        au.extend_from_slice(&[0, 0, 0, 1, 0x65]);
        au.extend((0..len as u32).map(|i| (i % 249) as u8 | 1));
        au
    }

    #[test]
    fn small_au_uses_pt97_for_p_and_pt98_for_idr() {
        let p_au = annexb_au(&[H264_PSLICE]);
        assert_eq!(
            packetize_zalo_au(&p_au, RTP_MAX_PAYLOAD),
            vec![(VIDEO_PT, p_au)]
        );
        let idr_au = annexb_au(&[H264_SPS, H264_PPS, H264_IDR]);
        assert_eq!(
            packetize_zalo_au(&idr_au, RTP_MAX_PAYLOAD),
            vec![(VIDEO_PT_FRAG, idr_au)]
        );
    }

    #[test]
    fn large_au_fragments_like_the_peer_and_roundtrips() {
        let au = au_of(2800);
        let parts = packetize_zalo_au(&au, RTP_MAX_PAYLOAD);
        assert!(parts.len() >= 3);
        // keyframe (has an IDR slice) -> PT 98 fragments, each within the payload limit
        assert!(parts
            .iter()
            .all(|(pt, p)| *pt == VIDEO_PT_FRAG && p.len() <= RTP_MAX_PAYLOAD));
        // header bytes exactly as captured: 1c 80 / 1c 00 / 1c 40
        assert_eq!(&parts[0].1[..2], &[0x1C, 0x80]);
        assert_eq!(&parts[1].1[..2], &[0x1C, 0x00]);
        assert_eq!(&parts.last().unwrap().1[..2], &[0x1C, 0x40]);
        // even split: fragment sizes differ by at most 1 except possibly the last (peer: 1083/1083, 1062x3)
        let sizes: Vec<usize> = parts.iter().map(|(_, p)| p.len()).collect();
        let (mx, mn) = (sizes.iter().max().unwrap(), sizes.iter().min().unwrap());
        assert!(mx - mn <= 2, "uneven split {sizes:?}");
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
        let mut buf = Vec::new();
        for (_, p) in &parts {
            depacketize_h264(p, &mut buf, &tx);
        }
        assert_eq!(rx.try_recv().expect("reassembled AU"), au);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn peer_capture_shapes_are_decoded() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
        let mut buf = Vec::new();
        // PT 97 payload as captured: whole AU, Annex-B, NAL 0x21
        let whole = [0, 0, 0, 1, 0x21, 0xE1, 0x04, 0x06, 0x38];
        depacketize_h264(&whole, &mut buf, &tx);
        assert_eq!(rx.try_recv().unwrap(), whole.to_vec());
        // PT 98 fragments as captured: 1c 80 00 00 00 01 27 42 ..., 1c 00 ..., 1c 40 ...
        depacketize_h264(&[0x1C, 0x80, 0, 0, 0, 1, 0x27, 0x42, 0x00], &mut buf, &tx);
        depacketize_h264(&[0x1C, 0x00, 0x06, 0xF1], &mut buf, &tx);
        assert!(rx.try_recv().is_err());
        depacketize_h264(&[0x1C, 0x40, 0xDE, 0x3B], &mut buf, &tx);
        assert_eq!(
            rx.try_recv().unwrap(),
            vec![0, 0, 0, 1, 0x27, 0x42, 0x00, 0x06, 0xF1, 0xDE, 0x3B]
        );
    }

    #[test]
    fn lost_first_fragment_drops_the_access_unit() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
        let mut buf = Vec::new();
        depacketize_h264(&[0x1C, 0x00, 1, 2], &mut buf, &tx);
        depacketize_h264(&[0x1C, 0x40, 3, 4], &mut buf, &tx);
        assert!(rx.try_recv().is_err());
        // a new access unit still works afterwards
        depacketize_h264(&[0x1C, 0xC0, 0, 0, 0, 1, 0x41], &mut buf, &tx);
        assert_eq!(rx.try_recv().unwrap(), vec![0, 0, 0, 1, 0x41]);
    }

    #[test]
    fn rfc6184_still_accepted_on_receive() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
        let mut buf = Vec::new();
        depacketize_h264(&[0x41, 0x9A, 0x24], &mut buf, &tx);
        assert_eq!(rx.try_recv().unwrap(), vec![0, 0, 0, 1, 0x41, 0x9A, 0x24]);
    }

    #[test]
    fn p_frame_fragments_use_pt97_and_split_evenly() {
        // non-IDR access unit larger than one packet: fragments, but PT 97 (seen live)
        let mut au = vec![0, 0, 0, 1, 0x21];
        au.extend((0..3000u32).map(|i| (i % 249) as u8 | 1));
        let parts = packetize_zalo_au(&au, RTP_MAX_PAYLOAD);
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|(pt, _)| *pt == VIDEO_PT));
        assert_eq!(parts[0].1[..2], [0x1C, 0x80]);
        assert_eq!(parts[2].1[..2], [0x1C, 0x40]);
        let sizes: Vec<usize> = parts.iter().map(|(_, p)| p.len()).collect();
        assert!(
            sizes.iter().max().unwrap() - sizes.iter().min().unwrap() <= 2,
            "{sizes:?}"
        );
        // AU exactly at the limit stays whole
        let whole = vec![0u8; RTP_MAX_PAYLOAD];
        assert_eq!(packetize_zalo_au(&whole, RTP_MAX_PAYLOAD).len(), 1);
    }

    #[test]
    fn detects_idr_slice_in_annexb() {
        assert!(au_has_idr(&annexb_au(&[H264_SPS, H264_PPS, H264_IDR])));
        assert!(!au_has_idr(&annexb_au(&[H264_PSLICE])));
        assert!(!au_has_idr(&[]));
        // a short trailing NAL (4 bytes after the start code) must still be seen
        assert!(au_has_idr(&[0, 0, 0, 1, 0x65, 1, 2]));
    }

    #[test]
    fn sync_point_needs_sps_or_idr() {
        assert!(au_is_sync_point(&annexb_au(&[
            H264_SPS, H264_PPS, H264_IDR
        ])));
        assert!(au_is_sync_point(&annexb_au(&[H264_IDR])));
        assert!(!au_is_sync_point(&annexb_au(&[H264_PSLICE])));
    }

    #[test]
    fn pli_has_rtcp_shape_and_is_detected() {
        let pli = build_pli(0x0102_0304, 0x0A0B_0C0D);
        assert_eq!(pli.len(), 12);
        assert_eq!(&pli[..4], &[0x81, 206, 0, 2]);
        assert_eq!(&pli[8..], &0x0A0B_0C0Du32.to_be_bytes());
        assert!(rtcp_requests_keyframe(&pli));
        let mut compound = vec![0x80u8, 201, 0, 1, 1, 2, 3, 4];
        compound.extend_from_slice(&pli);
        assert!(rtcp_requests_keyframe(&compound));
    }

    #[test]
    fn rx_stats_update_reports_gaps() {
        let mut s = RxStats::default();
        assert!(!s.update(100)); // first packet
        assert!(!s.update(101));
        assert!(s.update(104)); // 102, 103 missing
        assert!(!s.update(103)); // late/reordered packet is not a new gap
        assert!(!s.update(105));
    }

    #[test]
    fn rx_stats_counts_loss_and_wraparound() {
        let mut s = RxStats::default();
        assert!(s.block(1, 0, 0).is_none());
        for seq in [10u16, 11, 12, 15, 16] {
            s.update(seq); // 13, 14 missing
        }
        let b = s.block(0xAABBCCDD, 0x1234, 7).unwrap();
        assert_eq!(&b[0..4], &0xAABBCCDDu32.to_be_bytes());
        assert_eq!(&b[5..8], &[0, 0, 2]); // cumulative lost = 2
        assert_eq!(u32::from_be_bytes([b[8], b[9], b[10], b[11]]), 16); // highest seq
        assert_eq!(b[4], 73); // fraction: 2 lost of 7 expected = 73/256
        assert_eq!(&b[16..20], &0x1234u32.to_be_bytes());
        assert_eq!(&b[20..24], &7u32.to_be_bytes());
        // sequence wrap
        let mut w = RxStats::default();
        for seq in [65534u16, 65535, 0, 1] {
            w.update(seq);
        }
        let b = w.block(1, 0, 0).unwrap();
        assert_eq!(u32::from_be_bytes([b[8], b[9], b[10], b[11]]), 65537);
        assert_eq!(&b[5..8], &[0, 0, 0]);
    }

    #[test]
    fn sender_report_has_peer_shape_and_parses() {
        let blk = RxStats {
            started: true,
            base_seq: 1,
            max_seq: 1,
            received: 1,
            ..Default::default()
        }
        .block(9, 0, 0);
        let sr = build_sender_report(0x1111_2222, (0xAABB_CCDD, 0x1122_3344), 90000, 5, 600, blk);
        assert_eq!(sr.len(), 52); // same length as the peer's SR (len=52)
        assert_eq!(&sr[..4], &[0x81, 200, 0, 12]);
        let sdes = build_sdes_empty(0x1111_2222);
        assert_eq!(sdes.len(), 12); // same as the peer's SDES (len=12)
        let mut compound = sr.clone();
        compound.extend_from_slice(&sdes);
        assert_eq!(sr_middle_ntp(&compound), Some(0xCCDD_1122));
        assert!(!rtcp_requests_keyframe(&compound));
        let (d, kf) = crate::voip_log::describe_rtcp(&compound);
        assert!(!kf && d.starts_with("SR(") && d.contains("+SDES("), "{d}");
        // no report block -> RC 0, 28 bytes
        let bare = build_sender_report(1, (0, 0), 0, 0, 0, None);
        assert_eq!((bare.len(), bare[0], bare[3]), (28, 0x80, 6));
    }

    #[test]
    fn remb_and_incall_negotiate_packets_have_valid_wire_format() {
        let remb = build_remb(0x19f5f926, 0x1bba1962, 1_500_000);
        assert_eq!(remb.len(), 24);
        assert_eq!(&remb[0..4], &[0x8F, 206, 0, 5]);
        assert_eq!(&remb[12..16], b"REMB");
        assert_eq!(remb[16], 1);
        let exp = remb[17] >> 2;
        let mant =
            (((remb[17] & 0x03) as u32) << 16) | (u16::from_be_bytes([remb[18], remb[19]]) as u32);
        let decoded_bps = mant << exp;
        assert!(
            (1_490_000..=1_500_000).contains(&decoded_bps),
            "decoded={decoded_bps}"
        );
        assert_eq!(&remb[20..24], &0x1bba1962u32.to_be_bytes());

        let nego = build_zrtp_incall_negotiate_packet(0x1bba1962, 0x3955e3d2, 7);
        assert_eq!(nego.len(), 36);
        assert_eq!(&nego[0..2], &[0x01, 0x01]);
        assert_eq!(u16::from_le_bytes([nego[18], nego[19]]), 0x0020);
        assert_eq!(nego[20], 0x14);
        assert_eq!(
            u32::from_le_bytes([nego[21], nego[22], nego[23], nego[24]]),
            11
        );
        assert_eq!(
            &nego[25..36],
            &[0, 0, 0, 7, 2, 0x0B, 0x02, 0xD0, 0x1E, 0x29, 0x01]
        );

        let gvid = build_group_video_rtp_packet(
            97,
            true,
            42,
            9000,
            0x19F5F926,
            0x0102,
            0x33,
            (0xEBBC_BFBF, 0x8F85_C177),
            &[0x65, 0x88],
        );
        assert_eq!(gvid[0], 0x90);
        assert_eq!(gvid[1], 0x80 | 97);
        assert_eq!(&gvid[12..16], &[0xBE, 0xDE, 0x00, 0x05]);
        assert_eq!(&gvid[23..27], &[0xC2, 0x33, 0x01, 0x02]);
        assert_eq!(
            &gvid[27..36],
            &[0xD6, 0xBC, 0xBF, 0xBF, 0x8F, 0x85, 0xC1, 0x77, 0x00]
        );

        let gaud = build_group_audio_rtp_packet(
            112,
            10,
            1600,
            0x19F5F926,
            (0xEBBC_BFBF, 0x6151_770A),
            &[0xAA, 0xBB],
        );
        assert_eq!(gaud[0], 0x90);
        assert_eq!(gaud[1], 112);
        assert_eq!(&gaud[12..16], &[0xBE, 0xDE, 0x00, 0x03]);
        assert_eq!(&gaud[16..18], &[0x20, 0x4A]);
        assert_eq!(
            &gaud[18..28],
            &[0xD6, 0xBC, 0xBF, 0xBF, 0x61, 0x51, 0x77, 0x0A, 0x00, 0x00]
        );

        let mut fake_stream = H264_AUD.to_vec();
        fake_stream.extend_from_slice(&[0, 0, 0, 1, 0x67, 1, 2, 3]);
        let second_offset = fake_stream.len();
        fake_stream.extend_from_slice(H264_AUD);
        fake_stream.extend_from_slice(&[0, 0, 0, 1, 0x41, 9, 9]);
        assert_eq!(find_aud_delimiter(&fake_stream, 6), Some(second_offset));
        assert_eq!(
            strip_leading_aud(&fake_stream[..second_offset]),
            vec![0, 0, 0, 1, 0x67, 1, 2, 3]
        );
        // Pillarboxed 64x48 with 16px black bars on left and right -> crops to 32x48
        let mut padded = vec![0u8; 64 * 48 * 4];
        for y in 0..48 {
            for x in 16..48 {
                let i = (y * 64 + x) * 4;
                padded[i] = 120;
                padded[i + 1] = 140;
                padded[i + 2] = 160;
                padded[i + 3] = 255;
            }
        }
        let (cw, ch, cropped) = trim_black_borders(&padded, 64, 48);
        assert_eq!((cw, ch, cropped.len()), (32, 48, 32 * 48 * 4));
    }

    #[test]
    fn srtp_no_seq_decrypts_live_zavi_group_video_packet_and_roundtrips() {
        let srtp = SrtpContext::from_ascii_key("L7N9j-vxBHbJGzoBr7yZJb4kX8NfMp", true)
            .expect("30-byte key");
        // Live encrypted payload prefix from voip.log line 1081 (ssrc = 0x19f5f925)
        let cipher_prefix = [
            0x83, 0xfc, 0xbf, 0x11, 0xc8, 0xa1, 0x5a, 0x34, 0x00, 0x1f, 0x9a, 0x09, 0x06, 0x56,
            0x33, 0x65,
        ];
        let mut rtp = build_group_video_rtp_packet(
            98,
            false,
            0,
            6068952,
            0x19f5f925,
            1,
            0,
            (0, 0),
            &cipher_prefix,
        );
        let dec = srtp.decrypt_rtp_payload(&rtp).expect("decrypt");
        assert_eq!(
            &dec[..16],
            &[
                0x1c, 0x80, 0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xc0, 0x1f, 0xda, 0x0c, 0x0a, 0x79,
                0xf8, 0x40
            ]
        );
        // Roundtrip encrypt + decrypt
        srtp.crypt_rtp_packet(&mut rtp);
        assert_eq!(
            &rtp[36..52],
            &[
                0x1c, 0x80, 0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xc0, 0x1f, 0xda, 0x0c, 0x0a, 0x79,
                0xf8, 0x40
            ]
        );
        // Also verify normalize_annexb_au_4byte strips SEI (6) and converts 3-byte start codes to 4-byte
        let raw_au = [0, 0, 0, 1, 0x06, 0x05, 0xAA, 0, 0, 1, 0x41, 0x9A, 0x24];
        assert_eq!(
            normalize_annexb_au_4byte(&raw_au),
            vec![0, 0, 0, 1, 0x41, 0x9A, 0x24]
        );
    }

    #[test]
    fn list_media_devices_nonblocking_returns_without_blocking() {
        let t0 = std::time::Instant::now();
        let _ = list_media_devices_nonblocking(false);
        assert!(
            t0.elapsed() < Duration::from_millis(100),
            "list_media_devices_nonblocking blocked UI thread for {:?}",
            t0.elapsed()
        );
    }

    #[test]
    fn call_video_mailbox_coalesces_to_latest_frame_and_clears() {
        clear_all_call_video_frames();
        let first = eframe::egui::ColorImage::new([4, 4], eframe::egui::Color32::RED);
        let second = eframe::egui::ColorImage::new([8, 6], eframe::egui::Color32::BLUE);
        publish_call_video_frame("call_remote_video", first);
        publish_call_video_frame("call_remote_video", second);

        let drained = drain_call_video_frames();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].0, "call_remote_video");
        assert_eq!(drained[0].1.as_ref().unwrap().size, [8, 6]);
        assert!(drain_call_video_frames().is_empty());

        clear_call_video_frame("call_remote_video");
        let cleared = drain_call_video_frames();
        assert_eq!(cleared.len(), 1);
        assert!(cleared[0].1.is_none());
    }

    #[test]
    fn child_guard_kills_and_reaps_without_zombie() {
        #[cfg(target_os = "windows")]
        let child = {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("cmd")
                .args(["/C", "timeout", "/T", "5", "/NOBREAK"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .creation_flags(0x08000000)
                .spawn()
        };
        #[cfg(not(target_os = "windows"))]
        let child = std::process::Command::new("sleep")
            .arg("5")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        if let Ok(c) = child {
            let mut g = ChildGuard::new(c);
            g.kill_and_reap();
            assert!(g.0.is_none());
        }
    }

    #[test]
    fn merge_mic_scan_result_preserves_cache_on_error_and_merges_hotplug() {
        let prev = vec!["Microphone (Realtek)".to_string()];
        // Error must preserve previous cache
        let (merged, preserved) = merge_mic_scan_result(&prev, Err("COMSTA error"));
        assert!(preserved);
        assert_eq!(merged, prev);

        // Empty list while default exists must preserve previous cache
        let (merged2, preserved2) = merge_mic_scan_result(&prev, Ok((Vec::new(), true)));
        assert!(preserved2);
        assert_eq!(merged2, prev);

        // Hot-plugged headset appears alongside existing mic
        let hotplug = vec![
            "Microphone (Realtek)".to_string(),
            "Headset (UGREEN HiTune)".to_string(),
        ];
        let (merged3, preserved3) = merge_mic_scan_result(&prev, Ok((hotplug.clone(), true)));
        assert!(!preserved3);
        assert_eq!(merged3, hotplug);
    }

    #[test]
    fn pcm_integer_sample_conversions_roundtrip_accurately() {
        assert!((i16_to_f32(0)).abs() < 1e-6);
        assert!((i16_to_f32(i16::MAX) - 1.0).abs() < 1e-4);
        assert!((i16_to_f32(i16::MIN) + 1.0).abs() < 1e-4);
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.5), i16::MAX);
        assert_eq!(f32_to_i16(-1.5), -i16::MAX);

        assert!((u16_to_f32(32768)).abs() < 1e-6);
        assert!((u16_to_f32(u16::MAX) - 1.0).abs() < 1e-4);
        assert!((u16_to_f32(0) + 1.0).abs() < 1e-4);
        assert_eq!(f32_to_u16(0.0), 32768);
        assert_eq!(f32_to_u16(1.5), 65535);
        assert_eq!(f32_to_u16(-1.5), 1);
    }

    #[test]
    fn video_feedback_ssrc_is_not_clobbered_by_audio_ssrc() {
        assert_eq!(resolve_video_feedback_ssrc(0x2222_3333, 0xAAAA_BBBB), Some(0x2222_3333));
        assert_eq!(resolve_video_feedback_ssrc(0, 0xAAAA_BBBB), Some(0xAAAA_BBBB));
        assert_eq!(resolve_video_feedback_ssrc(0, 0), None);
    }

    #[test]
    fn is_incoming_video_rtp_rejects_fec_on_0x0e_and_accepts_h264() {
        assert!(!is_incoming_video_rtp(112, 0x0E, &[0, 0, 0, 1, 0x65]));
        assert!(!is_incoming_video_rtp(118, 0x0E, &[0xDE, 0xAD, 0xBE, 0xEF]));
        assert!(is_incoming_video_rtp(97, 0x0E, &[0x41, 0x9A]));
        assert!(is_incoming_video_rtp(98, 0x0E, &[0x1C, 0x80, 0, 0, 0, 1, 0x65]));
        assert!(is_incoming_video_rtp(99, 0x0E, &[0, 0, 0, 1, 0x67, 0x42]));
    }

    #[test]
    fn video_depacketizer_handles_fu_a_single_se_and_drops_seq_or_ts_breaks() {
        let mut dep = VideoDepacketizer::new();

        // Single-packet FU-A with both S and E set must emit immediately
        let single_fua = [0x7C, 0xC5, 0xAA, 0xBB];
        assert_eq!(
            dep.push(10, 9000, &single_fua),
            DepacketizeResult::Ready(vec![0, 0, 0, 1, 0x65, 0xAA, 0xBB])
        );

        // Multi-packet FU-A with sequence gap must drop corrupt fragment
        assert_eq!(dep.push(20, 12000, &[0x7C, 0x85, 0x11]), DepacketizeResult::Buffering);
        assert_eq!(
            dep.push(22, 12000, &[0x7C, 0x45, 0x22]),
            DepacketizeResult::DroppedCorrupt
        );
        assert_eq!(dep.corrupt_frags_dropped, 1);

        // Zalo 0x1C fragment with timestamp jump must drop corrupt fragment
        assert_eq!(
            dep.push(30, 15000, &[0x1C, 0x80, 0, 0, 0, 1, 0x41]),
            DepacketizeResult::Buffering
        );
        assert_eq!(
            dep.push(31, 18000, &[0x1C, 0x40, 0x9A]),
            DepacketizeResult::DroppedCorrupt
        );
        assert_eq!(dep.corrupt_frags_dropped, 2);

        // Valid contiguous Zalo 0x1C reassembly
        assert_eq!(
            dep.push(40, 21000, &[0x1C, 0x80, 0, 0, 0, 1, 0x65]),
            DepacketizeResult::Buffering
        );
        assert_eq!(
            dep.push(41, 21000, &[0x1C, 0x40, 0x88, 0x84]),
            DepacketizeResult::Ready(vec![0, 0, 0, 1, 0x65, 0x88, 0x84])
        );
    }

    #[test]
    fn h264_au_preparer_caches_sps_pps_without_splitting_aud_and_prepends_to_idr() {
        let mut prep = H264AccessUnitPreparer::new();

        // P-frame before keyframe must be dropped
        assert_eq!(
            prep.prepare(&annexb_au(&[H264_PSLICE])),
            PreparedAu::DroppedPreSync
        );

        // Standalone SPS + PPS packet is cached without emitting a premature AUD
        assert_eq!(
            prep.prepare(&annexb_au(&[H264_SPS, H264_PPS])),
            PreparedAu::ParamSetOnly
        );
        assert!(prep.has_params());

        // Subsequent bare IDR receives cached SPS + PPS and trailing AUD
        let PreparedAu::Keyframe(kf) = prep.prepare(&annexb_au(&[H264_IDR])) else {
            panic!("expected Keyframe with cached SPS/PPS");
        };
        assert!(au_has_nal_type(&kf, &[7]));
        assert!(au_has_nal_type(&kf, &[8]));
        assert!(au_has_nal_type(&kf, &[5]));
        assert!(kf.ends_with(H264_AUD));

        // Delta frame passes through after sync
        let PreparedAu::DeltaFrame(df) = prep.prepare(&annexb_au(&[H264_PSLICE])) else {
            panic!("expected DeltaFrame");
        };
        assert!(df.ends_with(H264_AUD));

        // After reset_sync (e.g. decoder restart), cached SPS/PPS remain available for bare IDR
        prep.reset_sync();
        assert_eq!(
            prep.prepare(&annexb_au(&[H264_PSLICE])),
            PreparedAu::DroppedPreSync
        );
        assert!(matches!(
            prep.prepare(&annexb_au(&[H264_IDR_ALT])),
            PreparedAu::Keyframe(_)
        ));
    }

    #[test]
    fn voip_opus_encoder_config_enables_fec_bitrate_complexity_and_voice() {
        let mut enc = Encoder::new(SAMPLE_RATE, OpusChannels::Mono, opus::Application::Voip)
            .expect("opus encoder");
        configure_voip_opus_encoder(&mut enc);
        assert_eq!(enc.get_bitrate().unwrap(), opus::Bitrate::Bits(32_000));
        assert!(enc.get_inband_fec().unwrap());
        assert_eq!(enc.get_packet_loss_perc().unwrap(), 10);
        assert_eq!(enc.get_complexity().unwrap(), 8);
        assert_eq!(enc.get_signal().unwrap(), opus::Signal::Voice);
        assert!(!enc.get_dtx().unwrap());
    }

    #[test]
    fn audio_resampler_preserves_rate_and_sine_clarity_across_8k_44k1_48k() {
        // 1. Capture downsampling 48 kHz -> 16 kHz (100 ms of 440 Hz sine)
        let mut cap_48 = AudioResampler::new(48_000, 16_000);
        let in_48: Vec<f32> = (0..4800)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin() * 0.5)
            .collect();
        let mut out_16 = Vec::new();
        for chunk in in_48.chunks(480) {
            cap_48.process_capture(chunk, |s| out_16.push(s));
        }
        assert!(
            (1595..=1602).contains(&out_16.len()),
            "48k->16k len={}",
            out_16.len()
        );
        let peak_16 = out_16[100..].iter().fold(0.0_f32, |m, v| m.max(v.abs()));
        assert!(peak_16 > 0.42 && peak_16 < 0.56, "peak_16={peak_16}");

        // 2. Capture upsampling 8 kHz Bluetooth HFP -> 16 kHz (no 2x speed bug!)
        let mut cap_8 = AudioResampler::new(8_000, 16_000);
        let in_8: Vec<f32> = (0..800)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 8_000.0).sin() * 0.5)
            .collect();
        let mut out_from_8 = Vec::new();
        for chunk in in_8.chunks(80) {
            cap_8.process_capture(chunk, |s| out_from_8.push(s));
        }
        assert!(
            (1590..=1602).contains(&out_from_8.len()),
            "8k->16k len={}",
            out_from_8.len()
        );

        // 3. Playback upsampling 16 kHz -> 44.1 kHz via ring buffer (zero over-drain)
        let ring = AudioRingBuffer::new(4096);
        for i in 0..1600 {
            let s = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin() * 0.5;
            assert!(ring.push(s));
        }
        let mut play_441 = AudioResampler::new(16_000, 44_100);
        let mut rendered = vec![0.0_f32; 4410];
        for chunk in rendered.chunks_mut(441) {
            play_441.fill_playback(&ring, chunk);
        }
        let peak_441 = rendered[300..].iter().fold(0.0_f32, |m, v| m.max(v.abs()));
        assert!(peak_441 > 0.42 && peak_441 < 0.58, "peak_441={peak_441}");
    }

    #[test]
    fn audio_ring_buffer_is_fifo_bounded_and_wraparound_safe() {
        let rb = AudioRingBuffer::new(64);
        assert_eq!(rb.capacity(), 64);
        for round in 0..5 {
            for i in 0..50 {
                assert!(rb.push((round * 100 + i) as f32));
            }
            assert_eq!(rb.len(), 50);
            for i in 0..50 {
                assert_eq!(rb.pop(), Some((round * 100 + i) as f32));
            }
            assert_eq!(rb.pop(), None);
        }
        // Capacity bound
        for i in 0..64 {
            assert!(rb.push(i as f32));
        }
        assert!(!rb.push(999.0));
        rb.discard(10);
        assert_eq!(rb.len(), 54);
        assert_eq!(rb.pop(), Some(10.0));
    }

    #[test]
    fn audio_jitter_buffer_reorders_invokes_fec_and_plc_and_caps_latency() {
        let mut jb = AudioJitterBuffer::new();
        // Push out-of-order packets 100, 102, 101 -> should prime at 3 packets and play 100, 101, 102 in order
        jb.push(100, vec![100]);
        assert_eq!(jb.pop_next(), JitterPop::Buffering);
        jb.push(102, vec![102]);
        assert_eq!(jb.pop_next(), JitterPop::Buffering);
        jb.push(101, vec![101]);
        assert_eq!(jb.reordered, 1);

        assert_eq!(jb.pop_next(), JitterPop::Packet(vec![100]));
        assert_eq!(jb.pop_next(), JitterPop::Packet(vec![101]));
        assert_eq!(jb.pop_next(), JitterPop::Packet(vec![102]));

        // Packet 103 lost, 104 present -> FEC recovery from packet 104, then normal 104
        jb.push(104, vec![104]);
        assert_eq!(jb.pop_next(), JitterPop::FecFromNext(vec![104]));
        assert_eq!(jb.pop_next(), JitterPop::Packet(vec![104]));
        assert_eq!(jb.fec_recovered, 1);

        // Missing 105 with no 106 -> PLC
        assert_eq!(jb.pop_next(), JitterPop::Plc);
        assert_eq!(jb.plc_concealed, 1);

        // Latency cap: pushing 20 packets caps queue at max_packets (10)
        for s in 200..220u16 {
            jb.push(s, vec![s as u8]);
        }
        assert!(jb.packets.len() <= jb.max_packets);
        assert!(jb.overflow_dropped > 0);
    }

    #[test]
    fn hpf_80hz_blocks_dc_and_soft_limiter_bounds_peaks() {
        let mut hpf = HighPassFilter80Hz::new(16_000);
        let mut last = 1.0_f32;
        for _ in 0..400 {
            last = hpf.process(0.75);
        }
        assert!(last.abs() < 0.01, "DC offset leaked through HPF: {last}");

        assert_eq!(soft_limit(0.5), 0.5);
        assert_eq!(soft_limit(-0.5), -0.5);
        let pos = soft_limit(3.0);
        let neg = soft_limit(-3.0);
        assert!(pos > 0.85 && pos < 1.0, "pos={pos}");
        assert!(neg < -0.85 && neg > -1.0, "neg={neg}");
    }
}
