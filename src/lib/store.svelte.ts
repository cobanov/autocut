import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import {
  cancelExport as apiCancelExport,
  computeWaveform,
  detectSilence,
  exportFcpxml as apiExportFcpxml,
  exportMp4 as apiExportMp4,
  onExportProgress,
  openVideo,
} from "./api";
import type { Cut, CutList, DetectParams, VideoInfo } from "./types";

type JobStatus = "idle" | "detecting" | "exporting";

/// Fire-and-forget OS notification after a successful export. Falls back
/// silently if the user denied notification permission; the in-app
/// "Show in Finder" affordance still works.
async function announceExport(path: string, format: "mp4" | "fcpxml") {
  try {
    let granted = await isPermissionGranted();
    if (!granted) {
      granted = (await requestPermission()) === "granted";
    }
    if (!granted) return;
    const filename = path.split(/[/\\]/).pop() ?? path;
    sendNotification({
      title: "autocut · export ready",
      body: `${filename} (${format})`,
    });
  } catch {
    /* notifications are nice-to-have; never fail the export over them */
  }
}

const DEFAULTS: DetectParams = {
  threshold: 0.5,
  min_silence_ms: 100,
  min_speech_ms: 150,
  pad: 0.3,
  preview_range: null,
};

class EditorStore {
  video = $state<VideoInfo | null>(null);
  /// Raw filesystem path of the just-dropped file. Set BEFORE ffprobe returns
  /// so the <video> element can start loading bytes in parallel with metadata.
  pendingPath = $state<string | null>(null);
  cutlist = $state<CutList | null>(null);
  /// Index of the keep interval currently hovered (in either Timeline or
  /// ManualCutPanel). Used to sync the highlight between the two views.
  hoveredKeepIndex = $state<number | null>(null);
  /// Index of the keep interval the user explicitly focused by clicking it
  /// in the timeline. ManualCutPanel scrolls the matching row into view in
  /// response to this; hover changes are deliberately not enough to trigger
  /// a scroll, because timeline scrubbing constantly shifts segments under
  /// the cursor and any follow-the-hover scroll became a per-frame animation
  /// that pinned the UI thread on Windows.
  focusedKeepIndex = $state<number | null>(null);
  /// Downsampled amplitude envelope of the source audio, one peak per bin,
  /// uniformly spaced across the full duration. Populated asynchronously
  /// after a video is loaded.
  waveform = $state<number[] | null>(null);
  /// Surface for the "export succeeded" affordance — ExportPanel reads this
  /// to render the success row with a Show-in-Finder button. Cleared when
  /// the user starts a new export or closes the video.
  lastExport = $state<{ path: string; format: "mp4" | "fcpxml" } | null>(null);
  params = $state<DetectParams>({ ...DEFAULTS });
  usePreviewRange = $state(false);
  previewRange = $state<[number, number]>([0, 0]);
  currentTime = $state(0);
  skipRemoved = $state(true);
  jobStatus = $state<JobStatus>("idle");
  exportProgress = $state<{ pct: number; message: string } | null>(null);
  error = $state<string | null>(null);

  /// Increments on every requestSeek() so VideoPlayer can drive the
  /// underlying <video> imperatively without binding currentTime two-way.
  seekToken = $state(0);
  seekTarget = $state(0);
  /// Same token pattern for play/pause. UI code increments this; VideoPlayer
  /// reads the latest value and toggles the underlying element.
  playToggleToken = $state(0);
  /// Mirrors the underlying <video>.paused state so UI components can show
  /// the right glyph without holding a ref to the element.
  isPlaying = $state(false);

  private detectTimer: ReturnType<typeof setTimeout> | null = null;
  private progressUnlisten: (() => void) | null = null;

  async loadVideo(path: string) {
    this.error = null;
    this.cutlist = null;
    this.waveform = null;
    // Surface the path immediately so the <video> tag starts streaming bytes
    // while ffprobe is running. ffprobe is usually fast but on multi-GB
    // sources it can take a second or two, and we don't want the UI to look
    // frozen during that window.
    this.pendingPath = path;
    try {
      const info = await openVideo(path);
      this.video = info;
      this.currentTime = 0;
      this.previewRange = [0, Math.min(info.duration, 60)];
      this.usePreviewRange = false;
      // Detection is manual on first run so the preview opens immediately.
      // After the first detect, slider changes auto-rerun (see scheduleDetect).
      // Waveform extraction is fire-and-forget — the player works without it.
      this.fetchWaveform(path);
    } catch (err) {
      // Reset BOTH video and pendingPath so the dropzone screen comes back,
      // but keep `error` set — Dropzone reads it and surfaces it. Otherwise
      // a failure in ffprobe (e.g. macOS Gatekeeper blocking the sidecar
      // binary, or an unreadable file) shows nothing and looks like the app
      // ate the drop.
      this.error = String(err);
      this.video = null;
      this.pendingPath = null;
    }
  }

  private async fetchWaveform(path: string) {
    try {
      // Scale bin count with duration so very long clips still have enough
      // detail when zoomed in 50x-100x via the navigator. A flat 2000-bin
      // overview looks fine for a 30-second clip but goes blocky on hour-
      // long footage. ~10 bins/sec, capped so memory doesn't run away.
      const duration = this.video?.duration ?? 60;
      const bins = Math.min(50000, Math.max(2000, Math.ceil(duration * 10)));
      const w = await computeWaveform(path, bins);
      // Guard against races: if the user already loaded a different video
      // while waveform extraction was running, drop the result.
      if (this.video?.path === path) {
        this.waveform = w;
      }
    } catch {
      /* non-fatal: timeline just renders without the waveform overlay */
    }
  }

  requestSeek(time: number) {
    this.seekTarget = time;
    this.seekToken += 1;
  }

  togglePlay() {
    this.playToggleToken += 1;
  }

  /// Jump to the previous keep interval's start. If we're more than ~1s into
  /// the current keep, restart it instead (standard media-player behavior).
  prevKeep() {
    const keeps = this.keepIntervals();
    if (!keeps.length) return;
    const t = this.currentTime;
    let curr = -1;
    for (let i = 0; i < keeps.length; i++) {
      if (keeps[i].start <= t) curr = i;
      else break;
    }
    if (curr === -1) {
      this.requestSeek(keeps[0].start);
      return;
    }
    if (t - keeps[curr].start > 1.0) {
      this.requestSeek(keeps[curr].start);
    } else if (curr > 0) {
      this.requestSeek(keeps[curr - 1].start);
    } else {
      this.requestSeek(keeps[curr].start);
    }
  }

  nextKeep() {
    const keeps = this.keepIntervals();
    if (!keeps.length) return;
    const t = this.currentTime;
    for (const k of keeps) {
      if (k.start > t + 0.05) {
        this.requestSeek(k.start);
        return;
      }
    }
    this.requestSeek(keeps[keeps.length - 1].start);
  }

  // Debounced rerun. Parameter sliders call scheduleDetect() on every change.
  // Only fires after the first manual detect — before that, sliders are inert
  // so the preview opens immediately without spending CPU on every drop.
  scheduleDetect(delayMs = 300) {
    if (!this.cutlist) return;
    if (this.detectTimer) clearTimeout(this.detectTimer);
    this.detectTimer = setTimeout(() => this.runDetectNow(), delayMs);
  }

  async runDetectNow() {
    if (!this.video || this.jobStatus === "exporting") return;
    if (this.detectTimer) {
      clearTimeout(this.detectTimer);
      this.detectTimer = null;
    }
    this.jobStatus = "detecting";
    this.error = null;
    try {
      const params: DetectParams = {
        ...this.params,
        preview_range:
          this.usePreviewRange &&
          this.previewRange[1] > this.previewRange[0]
            ? this.previewRange
            : null,
      };
      const res = await detectSilence(
        this.video.path,
        this.video.duration,
        params,
      );
      this.cutlist = res.cutlist;
    } catch (err) {
      this.error = String(err);
    } finally {
      this.jobStatus = "idle";
    }
  }

  async exportMp4(outputPath: string) {
    if (!this.video || !this.cutlist) return;
    if (this.usePreviewRange) {
      // Export always runs on the full video. Before exporting we re-detect on
      // the whole timeline so the cutlist reflects every segment, not just
      // the preview window.
      const prevUse = this.usePreviewRange;
      this.usePreviewRange = false;
      await this.runDetectNow();
      this.usePreviewRange = prevUse;
    }
    const normalized = this.normalizedCutlist();
    if (!normalized) return;
    this.lastExport = null;
    this.jobStatus = "exporting";
    this.exportProgress = { pct: 0, message: "starting" };
    this.error = null;
    const unlisten = await onExportProgress((e) => {
      this.exportProgress = e;
    });
    this.progressUnlisten = unlisten;
    try {
      await apiExportMp4(this.video.path, outputPath, normalized);
      this.exportProgress = { pct: 100, message: "done" };
      this.lastExport = { path: outputPath, format: "mp4" };
      announceExport(outputPath, "mp4");
    } catch (err) {
      this.error = String(err);
    } finally {
      unlisten();
      this.progressUnlisten = null;
      this.jobStatus = "idle";
    }
  }

  async exportFcpxml(outputPath: string, title: string) {
    if (!this.video || !this.cutlist) return;
    if (this.usePreviewRange) {
      const prevUse = this.usePreviewRange;
      this.usePreviewRange = false;
      await this.runDetectNow();
      this.usePreviewRange = prevUse;
    }
    const normalized = this.normalizedCutlist();
    if (!normalized) return;
    this.lastExport = null;
    try {
      await apiExportFcpxml(
        this.video.path,
        outputPath,
        normalized,
        this.video.fps,
        this.video.start_timecode,
        title,
      );
      this.lastExport = { path: outputPath, format: "fcpxml" };
      announceExport(outputPath, "fcpxml");
    } catch (err) {
      this.error = String(err);
    }
  }

  async cancelExport() {
    await apiCancelExport();
  }

  closeVideo() {
    this.video = null;
    this.pendingPath = null;
    this.cutlist = null;
    this.waveform = null;
    this.error = null;
    this.exportProgress = null;
    this.lastExport = null;
    this.hoveredKeepIndex = null;
    this.focusedKeepIndex = null;
    this.currentTime = 0;
    this.usePreviewRange = false;
    if (this.detectTimer) {
      clearTimeout(this.detectTimer);
      this.detectTimer = null;
    }
  }

  setHoveredKeep(i: number | null) {
    this.hoveredKeepIndex = i;
  }

  focusKeep(i: number | null) {
    this.focusedKeepIndex = i;
  }

  resetParams() {
    this.params = { ...DEFAULTS };
    this.runDetectNow();
  }

  // ---- manual cutlist editing ----

  /// Rebuild the cutlist from a (possibly unsorted/overlapping) list of keep
  /// intervals. Gaps become "remove" intervals; the result tiles [0,duration].
  /// Each keep can carry a `disabled` flag that persists through edits and
  /// only resolves to a real `remove` during export normalization.
  setKeepIntervals(
    keeps: { start: number; end: number; disabled?: boolean }[],
  ) {
    if (!this.video) return;
    const dur = this.video.duration;
    const clipped = keeps
      .map((k) => ({
        start: Math.max(0, Math.min(dur, k.start)),
        end: Math.max(0, Math.min(dur, k.end)),
        disabled: !!k.disabled,
      }))
      .filter((k) => k.end - k.start > 0.01)
      .sort((a, b) => a.start - b.start);

    const merged: { start: number; end: number; disabled: boolean }[] = [];
    for (const k of clipped) {
      const last = merged[merged.length - 1];
      if (last && k.start <= last.end) {
        last.end = Math.max(last.end, k.end);
        // Merging keeps: if either side is disabled, treat the merged as
        // disabled (user has to explicitly re-enable).
        last.disabled = last.disabled || k.disabled;
      } else {
        merged.push({ ...k });
      }
    }

    const intervals: Cut[] = [];
    let cursor = 0;
    for (const k of merged) {
      if (k.start > cursor) {
        intervals.push({ start: cursor, end: k.start, kind: "remove" });
      }
      intervals.push({
        start: k.start,
        end: k.end,
        kind: "keep",
        ...(k.disabled ? { disabled: true } : {}),
      });
      cursor = k.end;
    }
    if (cursor < dur) {
      intervals.push({ start: cursor, end: dur, kind: "remove" });
    }
    this.cutlist = { source_duration: dur, intervals };
  }

  keepIntervals(): { start: number; end: number; disabled: boolean }[] {
    return (
      this.cutlist?.intervals
        .filter((c) => c.kind === "keep")
        .map((c) => ({
          start: c.start,
          end: c.end,
          disabled: c.disabled === true,
        })) ?? []
    );
  }

  updateKeep(index: number, start: number, end: number) {
    const keeps = this.keepIntervals();
    if (index < 0 || index >= keeps.length) return;
    const disabled = keeps[index].disabled;
    keeps[index] = { start, end, disabled };
    this.setKeepIntervals(keeps);
  }

  toggleKeepDisabled(index: number) {
    const keeps = this.keepIntervals();
    if (index < 0 || index >= keeps.length) return;
    keeps[index] = { ...keeps[index], disabled: !keeps[index].disabled };
    this.setKeepIntervals(keeps);
  }

  addKeepAt(start: number, end: number) {
    const keeps = this.keepIntervals();
    keeps.push({ start, end, disabled: false });
    this.setKeepIntervals(keeps);
  }

  /// Collapse disabled keeps into the surrounding remove gaps so the Rust
  /// exporter sees a clean cutlist where every interval is either truly kept
  /// or truly removed. Called by exportMp4 / exportFcpxml.
  private normalizedCutlist(): CutList | null {
    if (!this.cutlist) return null;
    const out: Cut[] = [];
    for (const c of this.cutlist.intervals) {
      const next: Cut =
        c.disabled && c.kind === "keep"
          ? { start: c.start, end: c.end, kind: "remove" }
          : { start: c.start, end: c.end, kind: c.kind };
      const last = out[out.length - 1];
      if (
        last &&
        last.kind === "remove" &&
        next.kind === "remove" &&
        Math.abs(last.end - next.start) < 1e-6
      ) {
        last.end = next.end;
      } else {
        out.push(next);
      }
    }
    return { source_duration: this.cutlist.source_duration, intervals: out };
  }
}

export const editor = new EditorStore();
