pub mod audio;
pub mod binaries;
pub mod commands;
pub mod cutlist;
pub mod export_fcpxml;
pub mod export_mp4;
pub mod probe;
pub mod timecode;
pub mod vad;
pub mod waveform;

use commands::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::open_video,
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
