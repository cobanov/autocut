// Mirrors the Rust types in src-tauri/src. Keep in sync.

export type VideoInfo = {
  path: string;
  duration: number;
  fps: number;
  width: number;
  height: number;
  has_audio: boolean;
  start_timecode: string | null;
};

export type CutKind = "keep" | "remove";

export type Cut = {
  start: number;
  end: number;
  kind: CutKind;
  /// Frontend-only flag. A disabled `keep` still appears in the UI but the
  /// store collapses it into a `remove` before sending to the Rust exporter.
  /// Rust never sees this field.
  disabled?: boolean;
};

export type CutList = {
  source_duration: number;
  intervals: Cut[];
};

export type DetectParams = {
  threshold: number;
  min_silence_ms: number;
  min_speech_ms: number;
  pad: number;
  preview_range: [number, number] | null;
};

export type ExportQuality = "high" | "medium" | "small";
export type ExportResolution = "source" | "1080p" | "720p" | "480p";

export type ExportOptions = {
  quality: ExportQuality;
  resolution: ExportResolution;
};

export type ExportProgressEvent = {
  pct: number;
  message: string;
};

export type DiagnosticInfo = {
  app_version: string;
  target_os: string;
  target_arch: string;
  ffmpeg_path: string | null;
  ffmpeg_exists: boolean;
  ffprobe_path: string | null;
  ffprobe_exists: boolean;
};
