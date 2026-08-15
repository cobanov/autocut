// Mirrors the Rust types in src-tauri/src. Keep in sync.

export type MediaInfo = {
  path: string;
  duration: number;
  fps: number;
  /// Zero for audio-only media.
  width: number;
  height: number;
  has_video: boolean;
  has_audio: boolean;
  start_timecode: string | null;
};

/// A piece of media that rides along with the reference clip and gets cut at
/// the same points. Video tracks land above the reference in the exported
/// timeline, audio-only tracks below it.
export type LinkedTrack = {
  id: string;
  info: MediaInfo;
  /// Project-time second at which this track's media begins. Project time is
  /// the reference clip's own timeline, so the reference is always at 0.
  offset: number;
  /// True when `offset` was derived by comparing embedded timecodes rather
  /// than assumed to be zero. Surfaced in the UI so a user with no timecode
  /// knows the number is a guess they may need to correct.
  offsetFromTimecode: boolean;
};

/// What `open_track` hands back: the probe plus where the track appears to
/// belong. SMPTE parsing (including drop-frame) lives in Rust; duplicating it
/// in TypeScript would be two implementations to keep in agreement.
export type TrackProbe = {
  info: MediaInfo;
  offset: number;
  offset_from_timecode: boolean;
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
  /// Project-time second the analysed audio begins at. Non-zero only when
  /// detection is running on a linked track that starts later than the
  /// reference — the resulting cutlist is always in project time.
  source_offset: number;
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
