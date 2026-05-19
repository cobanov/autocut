//! Tauri command surface. Frontend talks to these via `invoke()`.
//!
//! Long-running operations (`detect_silence`, `export_mp4`) run on a worker
//! thread and emit progress events via the Tauri Emitter API. Frontend
//! subscribes via `listen('export-progress', ...)` etc.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::audio::extract_pcm_with_cancel;
use crate::binaries::{ffmpeg_path, ffprobe_path};
use crate::cutlist::CutList;
use crate::export_fcpxml::{render as render_fcpxml, FcpxmlParams};
use crate::export_mp4;
use crate::probe::{probe, VideoInfo};
use crate::vad::{detect_with_cancel as detect_vad_with_cancel, VadParams};
use crate::waveform::extract_waveform_with_cancel;

/// Render an anyhow::Error including the full chain of `with_context`
/// messages. Plain `.to_string()` would only show the outermost message,
/// hiding the actual root cause (e.g. "spawning ffprobe at ...: No such
/// file or directory" instead of just "spawning ffprobe at ...").
fn fmt_err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

#[derive(Default)]
pub struct AppState {
    /// Single-job cancellation flag. Set by `cancel_export`, polled by the
    /// export worker. We only support one concurrent export.
    pub export_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub detect_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub waveform_cancel: Mutex<Option<Arc<AtomicBool>>>,
}

fn install_cancel_slot(slot: &Mutex<Option<Arc<AtomicBool>>>) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut guard = slot.lock().unwrap();
    if let Some(previous) = guard.replace(cancel.clone()) {
        previous.store(true, Ordering::SeqCst);
    }
    cancel
}

fn clear_cancel_slot(slot: &Mutex<Option<Arc<AtomicBool>>>, cancel: &Arc<AtomicBool>) {
    let mut guard = slot.lock().unwrap();
    if guard
        .as_ref()
        .map(|current| Arc::ptr_eq(current, cancel))
        .unwrap_or(false)
    {
        *guard = None;
    }
}

fn cancel_slot(slot: &Mutex<Option<Arc<AtomicBool>>>) {
    if let Some(flag) = slot.lock().unwrap().take() {
        flag.store(true, Ordering::SeqCst);
    }
}

#[derive(Debug, Deserialize)]
pub struct DetectParams {
    pub threshold: f32,
    pub min_silence_ms: u32,
    pub min_speech_ms: u32,
    pub pad: f64,
    pub preview_range: Option<(f64, f64)>,
}

impl From<&DetectParams> for VadParams {
    fn from(p: &DetectParams) -> Self {
        VadParams {
            threshold: p.threshold,
            min_silence_ms: p.min_silence_ms,
            min_speech_ms: p.min_speech_ms,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DetectResult {
    pub cutlist: CutList,
}

fn resource_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().resource_dir().ok()
}

#[tauri::command]
pub async fn compute_waveform(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    target_bins: usize,
) -> Result<Vec<f32>, String> {
    let ffmpeg = ffmpeg_path(resource_dir(&app).as_deref()).map_err(fmt_err)?;
    let video = PathBuf::from(&path);
    let cancel = install_cancel_slot(&state.waveform_cancel);
    let cancel_for_worker = cancel.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        extract_waveform_with_cancel(&ffmpeg, &video, target_bins, Some(cancel_for_worker))
    })
    .await;
    clear_cancel_slot(&state.waveform_cancel, &cancel);
    let result = joined.map_err(fmt_err)?;
    result.map_err(fmt_err)
}

#[derive(Debug, Serialize)]
pub struct DiagnosticInfo {
    pub app_version: &'static str,
    pub target_os: &'static str,
    pub target_arch: &'static str,
    pub ffmpeg_path: Option<String>,
    pub ffmpeg_exists: bool,
    pub ffprobe_path: Option<String>,
    pub ffprobe_exists: bool,
}

/// Snapshot of build/runtime info we ask the user to paste into bug reports
/// when something fails. No PII; just versions and paths.
#[tauri::command]
pub fn diagnostic_info(app: AppHandle) -> DiagnosticInfo {
    let rd = resource_dir(&app);
    let ffmpeg = ffmpeg_path(rd.as_deref()).ok();
    let ffprobe = ffprobe_path(rd.as_deref()).ok();
    DiagnosticInfo {
        app_version: env!("CARGO_PKG_VERSION"),
        target_os: std::env::consts::OS,
        target_arch: std::env::consts::ARCH,
        ffmpeg_exists: ffmpeg.as_ref().map(|p| p.exists()).unwrap_or(false),
        ffmpeg_path: ffmpeg.map(|p| p.display().to_string()),
        ffprobe_exists: ffprobe.as_ref().map(|p| p.exists()).unwrap_or(false),
        ffprobe_path: ffprobe.map(|p| p.display().to_string()),
    }
}

#[tauri::command]
pub async fn open_video(app: AppHandle, path: String) -> Result<VideoInfo, String> {
    let ffprobe = ffprobe_path(resource_dir(&app).as_deref()).map_err(fmt_err)?;
    let video = PathBuf::from(&path);
    tauri::async_runtime::spawn_blocking(move || probe(&ffprobe, &video))
        .await
        .map_err(fmt_err)?
        .map_err(fmt_err)
}

#[tauri::command]
pub async fn detect_silence(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    duration: f64,
    params: DetectParams,
) -> Result<DetectResult, String> {
    let ffmpeg = ffmpeg_path(resource_dir(&app).as_deref()).map_err(fmt_err)?;
    let video = PathBuf::from(&path);
    let range = params.preview_range;
    let pad = params.pad;
    let vad_params: VadParams = (&params).into();
    let offset = range.map(|(s, _)| s).unwrap_or(0.0);
    let cancel = install_cancel_slot(&state.detect_cancel);
    let cancel_for_worker = cancel.clone();

    let joined = tauri::async_runtime::spawn_blocking(move || -> Result<DetectResult, String> {
        let samples =
            extract_pcm_with_cancel(&ffmpeg, &video, range, Some(cancel_for_worker.clone()))
                .map_err(fmt_err)?;
        let segments = detect_vad_with_cancel(
            &samples,
            vad_params,
            offset,
            Some(cancel_for_worker.as_ref()),
        )
        .map_err(fmt_err)?;
        let cutlist = CutList::from_speech_segments(&segments, duration, pad);
        Ok(DetectResult { cutlist })
    })
    .await;
    clear_cancel_slot(&state.detect_cancel, &cancel);
    let result = joined.map_err(fmt_err)?;
    result
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportQuality {
    High,
    Medium,
    Small,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportResolution {
    Source,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "480p")]
    P480,
}

#[derive(Debug, Deserialize)]
pub struct ExportMp4Args {
    pub source: String,
    pub output: String,
    pub cutlist: CutList,
    #[serde(default = "default_quality")]
    pub quality: ExportQuality,
    #[serde(default = "default_resolution")]
    pub resolution: ExportResolution,
}

fn default_quality() -> ExportQuality {
    ExportQuality::Medium
}
fn default_resolution() -> ExportResolution {
    ExportResolution::Source
}

impl From<ExportQuality> for export_mp4::Quality {
    fn from(q: ExportQuality) -> Self {
        match q {
            ExportQuality::High => export_mp4::Quality::High,
            ExportQuality::Medium => export_mp4::Quality::Medium,
            ExportQuality::Small => export_mp4::Quality::Small,
        }
    }
}

impl From<ExportResolution> for export_mp4::Resolution {
    fn from(r: ExportResolution) -> Self {
        match r {
            ExportResolution::Source => export_mp4::Resolution::Source,
            ExportResolution::P1080 => export_mp4::Resolution::P1080,
            ExportResolution::P720 => export_mp4::Resolution::P720,
            ExportResolution::P480 => export_mp4::Resolution::P480,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ExportProgressEvent {
    pct: f32,
    message: String,
}

#[tauri::command]
pub async fn export_mp4(
    app: AppHandle,
    state: State<'_, AppState>,
    args: ExportMp4Args,
) -> Result<(), String> {
    let ffmpeg = ffmpeg_path(resource_dir(&app).as_deref()).map_err(fmt_err)?;
    let source = PathBuf::from(&args.source);
    let output = PathBuf::from(&args.output);

    let cancel = install_cancel_slot(&state.export_cancel);

    let app_for_progress = app.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let cancel_for_worker = cancel.clone();
    let options = export_mp4::ExportOptions {
        quality: args.quality.into(),
        resolution: args.resolution.into(),
    };
    thread::spawn(move || {
        let on_progress = move |p: export_mp4::ExportProgress| {
            let _ = app_for_progress.emit(
                "export-progress",
                ExportProgressEvent {
                    pct: p.pct,
                    message: p.message,
                },
            );
        };
        let res = export_mp4::export(
            &ffmpeg,
            &source,
            &output,
            &args.cutlist,
            options,
            cancel_for_worker,
            on_progress,
        );
        let _ = tx.send(res.map_err(fmt_err));
    });

    let joined = tauri::async_runtime::spawn_blocking(move || {
        rx.recv().unwrap_or_else(|e| Err(e.to_string()))
    })
    .await;
    clear_cancel_slot(&state.export_cancel, &cancel);
    let result = joined.map_err(fmt_err)?;
    result
}

#[tauri::command]
pub fn cancel_export(state: State<'_, AppState>) -> Result<(), String> {
    cancel_slot(&state.export_cancel);
    Ok(())
}

#[tauri::command]
pub fn cancel_detect(state: State<'_, AppState>) -> Result<(), String> {
    cancel_slot(&state.detect_cancel);
    Ok(())
}

#[tauri::command]
pub fn cancel_waveform(state: State<'_, AppState>) -> Result<(), String> {
    cancel_slot(&state.waveform_cancel);
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct ExportFcpxmlArgs {
    pub source: String,
    pub output: String,
    pub cutlist: CutList,
    pub fps: f64,
    pub start_timecode: Option<String>,
    pub title: String,
}

/// Reveal a file in the OS file manager. macOS opens Finder with the file
/// selected; Linux opens the parent folder; Windows opens Explorer with
/// the file selected.
#[tauri::command]
pub fn reveal_in_finder(path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("file does not exist: {path}"));
    }
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.args(["-R", &path]);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{path}"));
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let parent = p
            .parent()
            .map(|x| x.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let mut c = std::process::Command::new("xdg-open");
        c.arg(parent);
        c
    };
    cmd.spawn().map_err(fmt_err)?;
    Ok(())
}

#[tauri::command]
pub fn export_fcpxml(args: ExportFcpxmlArgs) -> Result<(), String> {
    let asset_path = Path::new(&args.source);
    let xml = render_fcpxml(
        &args.cutlist,
        FcpxmlParams {
            fps: args.fps,
            asset_path: Some(asset_path),
            start_timecode: args.start_timecode.as_deref(),
            title: &args.title,
        },
    );
    std::fs::write(&args.output, xml).map_err(fmt_err)?;
    Ok(())
}
