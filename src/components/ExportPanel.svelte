<script lang="ts">
  import { save } from "@tauri-apps/plugin-dialog";
  import { diagnosticInfo, revealInFinder } from "../lib/api";
  import { editor } from "../lib/store.svelte";
  import type { ExportQuality, ExportResolution } from "../lib/types";

  function basename(p: string): string {
    return p.split(/[/\\]/).pop() ?? p;
  }

  function suggestedName(ext: string): string {
    if (!editor.video) return `untitled_autocut.${ext}`;
    const path = editor.video.path;
    const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
    const dot = path.lastIndexOf(".");
    const stem = path.slice(slash + 1, dot > slash ? dot : undefined);
    return `${stem}_autocut.${ext}`;
  }

  async function exportMp4() {
    const out = await save({
      defaultPath: suggestedName("mp4"),
      filters: [{ name: "MP4", extensions: ["mp4"] }],
    });
    if (out) editor.exportMp4(out);
  }

  async function exportFcpxml() {
    const out = await save({
      defaultPath: suggestedName("fcpxml"),
      filters: [{ name: "FCPXML", extensions: ["fcpxml"] }],
    });
    if (!out || !editor.video) return;
    const title = suggestedName("").replace(/\.$/, "");
    await editor.exportFcpxml(out, title);
  }

  let copyState = $state<"idle" | "copied" | "failed">("idle");

  /// Bundle the error message together with diagnostic info (app version,
  /// OS, sidecar paths) and the current export settings. Gives the
  /// developer enough to reproduce the failure without a back-and-forth.
  async function copyErrorReport() {
    if (!editor.exportError) return;
    try {
      const diag = await diagnosticInfo();
      const lines = [
        "autocut error report",
        "====================",
        `app version : ${diag.app_version}`,
        `platform    : ${diag.target_os}/${diag.target_arch}`,
        `ffmpeg      : ${diag.ffmpeg_exists ? "ok" : "MISSING"} ${diag.ffmpeg_path ?? ""}`,
        `ffprobe     : ${diag.ffprobe_exists ? "ok" : "MISSING"} ${diag.ffprobe_path ?? ""}`,
        editor.video
          ? `source      : ${editor.video.width}x${editor.video.height} @ ${editor.video.fps.toFixed(3)}fps, ${editor.video.duration.toFixed(2)}s`
          : "source      : (no video loaded)",
        `quality     : ${editor.exportOptions.quality}`,
        `resolution  : ${editor.exportOptions.resolution}`,
        "",
        "error",
        "-----",
        editor.exportError,
      ];
      await navigator.clipboard.writeText(lines.join("\n"));
      copyState = "copied";
    } catch {
      copyState = "failed";
    }
    setTimeout(() => (copyState = "idle"), 2200);
  }

  let canExport = $derived(!!editor.cutlist && editor.jobStatus === "idle");
  // Disabled keeps are excluded from the export, so don't count them.
  let keptDuration = $derived(
    editor.cutlist
      ? editor.cutlist.intervals
          .filter((c) => c.kind === "keep" && !c.disabled)
          .reduce((a, c) => a + (c.end - c.start), 0)
      : 0,
  );
  let totalDuration = $derived(editor.video?.duration ?? 0);
  let keptPct = $derived(totalDuration > 0 ? (keptDuration / totalDuration) * 100 : 0);

  const qualityOptions: { value: ExportQuality; label: string; hint: string }[] = [
    { value: "high", label: "high", hint: "crf 18 · ~visually lossless" },
    { value: "medium", label: "medium", hint: "crf 22 · good for share" },
    { value: "small", label: "small", hint: "crf 26 · smallest file" },
  ];

  const resolutionOptions: { value: ExportResolution; label: string }[] = [
    { value: "source", label: "source" },
    { value: "1080p", label: "1080p" },
    { value: "720p", label: "720p" },
    { value: "480p", label: "480p" },
  ];

  // FCPXML doesn't re-encode so quality/resolution don't apply — surface
  // that visually by dimming the picker when the user is about to export
  // an FCPXML. Pickers stay clickable so people can flip presets while
  // deciding which format to use.
  let activeQuality = $derived(editor.exportOptions.quality);
  let activeResolution = $derived(editor.exportOptions.resolution);
</script>

<section class="card">
  <header class="card-head">
    <div>
      <h2>export</h2>
      <p class="card-sub">
        {#if editor.cutlist}
          <span class="mono">{keptDuration.toFixed(1)}s</span> kept
          <span class="muted-2">·</span>
          <span class="mono">{keptPct.toFixed(0)}%</span>
        {:else}
          run detection first
        {/if}
      </p>
    </div>
  </header>

  <div class="card-body">
    <div class="opt">
      <div class="opt-label">quality</div>
      <div class="seg" role="radiogroup" aria-label="Quality">
        {#each qualityOptions as q (q.value)}
          <button
            type="button"
            class="seg-btn"
            class:active={activeQuality === q.value}
            onclick={() => (editor.exportOptions.quality = q.value)}
            title={q.hint}
            aria-pressed={activeQuality === q.value}
          >{q.label}</button>
        {/each}
      </div>
    </div>

    <div class="opt">
      <div class="opt-label">resolution</div>
      <div class="seg" role="radiogroup" aria-label="Resolution">
        {#each resolutionOptions as r (r.value)}
          <button
            type="button"
            class="seg-btn"
            class:active={activeResolution === r.value}
            onclick={() => (editor.exportOptions.resolution = r.value)}
            aria-pressed={activeResolution === r.value}
          >{r.label}</button>
        {/each}
      </div>
    </div>

    <div class="export-row">
      <button class="btn btn-primary" onclick={exportMp4} disabled={!canExport}>
        <span class="mono">▸</span> mp4
      </button>
      <button class="btn" onclick={exportFcpxml} disabled={!canExport}>
        <span class="mono">▸</span> fcpxml
      </button>
    </div>

    <p class="hint muted-2">
      mp4 re-encodes via ffmpeg · fcpxml preserves source timecode for davinci & premiere
    </p>

    {#if editor.jobStatus === "exporting" && editor.exportProgress}
      <div class="progress">
        <div class="progress-head">
          <span class="mono muted">{editor.exportProgress.message}</span>
          <span class="mono">{editor.exportProgress.pct.toFixed(0)}%</span>
        </div>
        <div class="bar">
          <div class="fill" style:width="{editor.exportProgress.pct}%"></div>
        </div>
        <button class="btn btn-danger btn-sm" onclick={() => editor.cancelExport()}>
          cancel
        </button>
      </div>
    {/if}

    {#if editor.lastExport && editor.jobStatus === "idle"}
      <div class="exported">
        <div class="exported-row">
          <span class="exported-tag mono">exported</span>
          <span class="exported-name mono" title={editor.lastExport.path}>
            {basename(editor.lastExport.path)}
          </span>
        </div>
        <div class="exported-actions">
          <button
            class="btn btn-sm"
            onclick={() => revealInFinder(editor.lastExport!.path)}
            title="Reveal file in Finder"
          >show in finder</button>
          <button
            class="btn btn-ghost btn-sm"
            onclick={() => (editor.lastExport = null)}
            title="Dismiss"
            aria-label="Dismiss"
          >×</button>
        </div>
      </div>
    {/if}

    {#if editor.exportError}
      <div class="error">
        <div class="error-head">
          <span class="dot dot-neg"></span>
          <span class="mono error-title">export failed</span>
          <button
            class="copy-btn"
            onclick={copyErrorReport}
            title="Copy a full error report to the clipboard so you can paste it in a bug report"
          >
            {#if copyState === "copied"}copied ✓
            {:else if copyState === "failed"}copy failed
            {:else}copy details{/if}
          </button>
        </div>
        <pre class="error-body mono">{editor.exportError}</pre>
      </div>
    {/if}
  </div>
</section>

<style>
  .card {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .card-body {
    display: grid;
    gap: 12px;
  }

  .opt {
    display: grid;
    gap: 6px;
  }
  .opt-label {
    font-size: 10px;
    font-weight: 500;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--muted-2);
  }
  .seg {
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: 1fr;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--surface-2);
  }
  .seg-btn {
    height: 28px;
    background: transparent;
    border: 0;
    color: var(--muted);
    cursor: pointer;
    font: inherit;
    font-size: 11px;
    font-family: var(--font-mono);
    padding: 0 4px;
    min-width: 0;
    transition: background 120ms, color 120ms;
  }
  .seg-btn + .seg-btn {
    border-left: 1px solid var(--border);
  }
  .seg-btn:hover:not(.active) {
    background: var(--elevated);
    color: var(--foreground);
  }
  .seg-btn.active {
    background: var(--elevated);
    color: var(--foreground);
    box-shadow: inset 0 -2px 0 var(--accent);
  }

  .export-row {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
    margin-top: 4px;
  }
  .hint {
    margin: 0;
    font-size: 11px;
    line-height: 1.5;
  }
  .progress {
    display: grid;
    gap: 6px;
  }
  .progress-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    font-size: 11px;
  }
  .progress-head .mono.muted {
    color: var(--muted-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 70%;
  }
  .progress-head .mono:last-child {
    color: var(--foreground);
  }
  .bar {
    height: 4px;
    background: var(--input);
    border-radius: 999px;
    overflow: hidden;
    border: 1px solid var(--border);
  }
  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 240ms ease-out;
  }
  .progress .btn-danger {
    margin-top: 2px;
    justify-self: start;
  }

  .exported {
    padding: 10px 12px;
    background: hsl(142 71% 55% / 0.06);
    border: 1px solid hsl(142 71% 55% / 0.3);
    border-radius: var(--radius);
    display: grid;
    gap: 8px;
  }
  .exported-row {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
  }
  .exported-tag {
    font-size: 10px;
    color: var(--pos);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    flex-shrink: 0;
  }
  .exported-name {
    font-size: 12px;
    color: var(--foreground);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .exported-actions {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .exported-actions .btn {
    flex: 1;
  }
  .exported-actions .btn-ghost {
    flex: 0 0 auto;
    width: 26px;
    padding: 0;
  }

  .error {
    display: grid;
    gap: 6px;
    padding: 10px 10px 8px;
    background: hsl(0 84% 65% / 0.06);
    border: 1px solid hsl(0 84% 65% / 0.25);
    border-radius: var(--radius);
  }
  .error-head {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .error-head .dot { flex-shrink: 0; }
  .error-title {
    font-size: 11px;
    color: var(--neg);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    flex: 1;
  }
  .copy-btn {
    background: transparent;
    border: 1px solid hsl(0 84% 65% / 0.35);
    color: var(--neg);
    border-radius: var(--radius-sm);
    cursor: pointer;
    height: 22px;
    padding: 0 8px;
    font-size: 10px;
    font-family: var(--font-mono);
    text-transform: lowercase;
    letter-spacing: 0.04em;
    transition: background 120ms, border-color 120ms;
  }
  .copy-btn:hover {
    background: hsl(0 84% 65% / 0.12);
    border-color: var(--neg);
  }
  .error-body {
    margin: 0;
    padding: 8px 10px;
    background: hsl(0 0% 0% / 0.35);
    border: 1px solid hsl(0 84% 65% / 0.2);
    border-radius: var(--radius-sm);
    font-size: 11px;
    line-height: 1.45;
    color: var(--foreground);
    max-height: 160px;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
