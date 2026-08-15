pub mod audio;
pub mod binaries;
pub mod commands;
pub mod cutlist;
pub mod export_fcpxml;
pub mod export_mp4;
pub mod probe;
pub mod sync;
pub mod timecode;
pub mod vad;
pub mod waveform;

/// Excluded from the lib's own test build on purpose. `generate_context!` is a
/// proc macro that validates tauri.conf.json at compile time, including that
/// `frontendDist` exists — so leaving it in would make `cargo test` refuse to
/// compile until someone had run a frontend build. None of the unit tests
/// touch the Tauri runtime. The binary target still compiles the lib without
/// cfg(test), so the real app is unaffected.
#[cfg(not(test))]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use commands::AppState;

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::open_video,
            commands::open_track,
            commands::compute_waveform,
            commands::detect_silence,
            commands::export_mp4,
            commands::cancel_export,
            commands::cancel_detect,
            commands::cancel_waveform,
            commands::export_fcpxml,
            commands::diagnostic_info,
            commands::reveal_in_finder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running autocut");
}
