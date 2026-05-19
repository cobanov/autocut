import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  CutList,
  DetectParams,
  DiagnosticInfo,
  ExportOptions,
  ExportProgressEvent,
  VideoInfo,
} from "./types";

export async function openVideo(path: string): Promise<VideoInfo> {
  return invoke<VideoInfo>("open_video", { path });
}

export async function diagnosticInfo(): Promise<DiagnosticInfo> {
  return invoke<DiagnosticInfo>("diagnostic_info");
}

export async function computeWaveform(
  path: string,
  targetBins: number,
): Promise<number[]> {
  return invoke<number[]>("compute_waveform", { path, targetBins });
}

export async function detectSilence(
  path: string,
  duration: number,
  params: DetectParams,
): Promise<{ cutlist: CutList }> {
  return invoke<{ cutlist: CutList }>("detect_silence", {
    path,
    duration,
    params,
  });
}

export async function cancelDetect(): Promise<void> {
  await invoke("cancel_detect");
}

export async function exportMp4(
  source: string,
  output: string,
  cutlist: CutList,
  options: ExportOptions,
  hasAudio: boolean,
): Promise<void> {
  await invoke("export_mp4", {
    args: {
      source,
      output,
      cutlist,
      quality: options.quality,
      resolution: options.resolution,
      has_audio: hasAudio,
    },
  });
}

export async function cancelExport(): Promise<void> {
  await invoke("cancel_export");
}

export async function cancelWaveform(): Promise<void> {
  await invoke("cancel_waveform");
}

export async function revealInFinder(path: string): Promise<void> {
  await invoke("reveal_in_finder", { path });
}

export async function exportFcpxml(
  source: string,
  output: string,
  cutlist: CutList,
  fps: number,
  startTimecode: string | null,
  title: string,
  hasAudio: boolean,
): Promise<void> {
  await invoke("export_fcpxml", {
    args: {
      source,
      output,
      cutlist,
      fps,
      start_timecode: startTimecode,
      title,
      has_audio: hasAudio,
    },
  });
}

export function onExportProgress(
  handler: (e: ExportProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<ExportProgressEvent>("export-progress", (event) => {
    handler(event.payload);
  });
}
