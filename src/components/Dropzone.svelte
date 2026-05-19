<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { open } from "@tauri-apps/plugin-dialog";
  import { diagnosticInfo } from "../lib/api";
  import { editor } from "../lib/store.svelte";

  let hovering = $state(false);
  let copied = $state(false);
  let lastAttemptedPath = $state<string | null>(null);

  async function openExternal(url: string) {
    try {
      await invoke("plugin:shell|open", { path: url });
    } catch {
      /* fall through silently — link is plain text if the open fails */
    }
  }

  async function copyDetails() {
    if (!editor.loadError) return;
    let diag = "";
    try {
      const d = await diagnosticInfo();
      diag = [
        `app version    ${d.app_version}`,
        `target         ${d.target_os}/${d.target_arch}`,
        `ffmpeg         ${d.ffmpeg_exists ? "found" : "MISSING"} at ${d.ffmpeg_path ?? "(unresolved)"}`,
        `ffprobe        ${d.ffprobe_exists ? "found" : "MISSING"} at ${d.ffprobe_path ?? "(unresolved)"}`,
      ].join("\n");
    } catch (e) {
      diag = `(diagnostic_info command failed: ${String(e)})`;
    }
    const report = [
      "autocut error report",
      "====================",
      "",
      "error:",
      editor.loadError,
      "",
      "video path:",
      lastAttemptedPath ?? "(unknown)",
      "",
      "environment:",
      diag,
    ].join("\n");
    try {
      await navigator.clipboard.writeText(report);
      copied = true;
      setTimeout(() => (copied = false), 2000);
    } catch {
      /* clipboard may be locked; fall back is the visible textarea below */
    }
  }

  onMount(() => {
    let unlisten: (() => void) | null = null;
    (async () => {
      try {
        const webview = getCurrentWebview();
        unlisten = await webview.onDragDropEvent((event) => {
          if (event.payload.type === "over") {
            hovering = true;
          } else if (event.payload.type === "leave") {
            hovering = false;
          } else if (event.payload.type === "drop") {
            hovering = false;
            const p = event.payload.paths[0];
            if (!p) return;
            lastAttemptedPath = p;
            if (/\.(mp4|mov|m4v|mkv|webm|avi)$/i.test(p)) {
              editor.loadVideo(p);
            } else {
              editor.loadError = `unsupported file format: ${p.split(/[/\\]/).pop()}`;
            }
          }
        });
      } catch {
        /* Browser preview lacks Tauri's webview drag/drop API. */
      }
    })();
    return () => unlisten?.();
  });

  async function pickFile() {
    const selected = await open({
      multiple: false,
      directory: false,
      filters: [
        {
          name: "Video",
          extensions: ["mp4", "mov", "m4v", "mkv", "webm", "avi"],
        },
      ],
    });
    if (typeof selected === "string") {
      lastAttemptedPath = selected;
      editor.loadVideo(selected);
    }
  }
</script>

<div class="empty">
  <section class="hero card" class:hovering>
    <div class="hero-glyph" aria-hidden="true">
      <svg viewBox="0 0 64 64" width="44" height="44">
        <rect x="6" y="14" width="52" height="36" rx="4"
              fill="none" stroke="currentColor" stroke-width="1.2" />
        <line x1="6" y1="22" x2="58" y2="22"
              stroke="currentColor" stroke-width="1.2" opacity="0.5" />
        <line x1="6" y1="42" x2="58" y2="42"
              stroke="currentColor" stroke-width="1.2" opacity="0.5" />
        <line x1="32" y1="14" x2="32" y2="50"
              stroke="currentColor" stroke-width="1.4" stroke-dasharray="2 3" />
        <path d="M32 4 L32 12 M28 8 L32 12 L36 8"
              fill="none" stroke="currentColor" stroke-width="1.4"
              stroke-linecap="round" stroke-linejoin="round" />
      </svg>
    </div>

    <h1 class="hero-title">drop a video</h1>

    <button class="btn btn-primary btn-lg" onclick={pickFile}>
      browse files
    </button>

    <div class="formats">
      <span class="mono">mp4</span>
      <span class="sep">·</span>
      <span class="mono">mov</span>
      <span class="sep">·</span>
      <span class="mono">m4v</span>
      <span class="sep">·</span>
      <span class="mono">mkv</span>
      <span class="sep">·</span>
      <span class="mono">webm</span>
      <span class="sep">·</span>
      <span class="mono">avi</span>
    </div>

    {#if editor.loadError}
      <div class="error">
        <div class="error-row">
          <span class="error-head mono">couldn't open that video</span>
          <div class="error-actions">
            <button class="error-btn" onclick={copyDetails} title="Copy a full error report (error + diagnostic info) to the clipboard">
              {copied ? "copied" : "copy details"}
            </button>
            <button class="error-btn" onclick={() => (editor.loadError = null)} title="Dismiss this error">
              dismiss
            </button>
          </div>
        </div>
        <pre class="error-body mono">{editor.loadError}</pre>
        {#if /gatekeeper|denied|not permitted|operation not permitted|killed|cannot be opened/i.test(editor.loadError)}
          <div class="error-hint">
            macOS is blocking the bundled ffmpeg helper. open Terminal and
            run once:
            <code>xattr -cr /Applications/autocut.app</code>
            then relaunch the app.
          </div>
        {/if}
      </div>
    {/if}
  </section>

  <footer class="meta mono muted-2">
    <span><span class="brand-dot"></span> autocut</span>
    <span class="sep">·</span>
    <span>v{__APP_VERSION__}</span>
    <span class="sep">·</span>
    <button
      class="link"
      onclick={() => openExternal("https://github.com/cobanov/autocut")}
    >github.com/cobanov/autocut</button>
    <span class="sep">·</span>
    <span>built by mert cobanov</span>
  </footer>
</div>

<style>
  .empty {
    width: min(720px, 100%);
    display: grid;
    gap: 24px;
    padding: 12px;
  }

  /* ===== hero ===== */
  /* The hero is a drop target, so it advertises that via a dashed border
     and a tightly-spaced "dotted" inset rule. Stays refined: subtle until
     the user drags a file over the window, at which point both the outer
     dashed stroke and the inner radial glow lift to the accent color. */
  .hero {
    padding: 48px 40px 36px;
    text-align: center;
    display: grid;
    gap: 14px;
    place-items: center;
    background: linear-gradient(
        180deg,
        hsl(240 6% 8%) 0%,
        hsl(240 6% 6%) 100%
      );
    border-style: dashed;
    border-color: var(--border-strong);
    border-width: 1.5px;
    transition: border-color 200ms, background 200ms, transform 200ms;
  }
  .hero.hovering {
    border-color: var(--accent);
    background:
      radial-gradient(
        circle at 50% 0%,
        hsl(213 94% 68% / 0.14),
        transparent 60%
      ),
      hsl(240 6% 8%);
    transform: scale(1.005);
  }

  .hero-glyph {
    color: var(--muted);
    width: 64px;
    height: 64px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: var(--surface-2);
    border: 1px solid var(--border);
    margin-bottom: 6px;
    transition: color 200ms, border-color 200ms;
  }
  .hero.hovering .hero-glyph {
    color: var(--accent);
    border-color: hsl(213 94% 68% / 0.4);
  }

  .hero-title {
    margin: 0;
    font-family: var(--font-mono);
    font-size: 28px;
    font-weight: 500;
    letter-spacing: -0.02em;
    line-height: 1.05;
  }
  .hero .btn-primary {
    margin-top: 6px;
    padding: 0 20px;
    min-width: 160px;
  }

  .formats {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    margin-top: 4px;
    padding: 4px 14px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 999px;
    font-size: 11px;
    color: var(--muted-2);
    letter-spacing: 0.04em;
  }
  .formats .sep { color: var(--border-strong); }

  .error {
    margin-top: 16px;
    max-width: 560px;
    padding: 12px 14px;
    background: hsl(0 84% 65% / 0.07);
    border: 1px solid hsl(0 84% 65% / 0.35);
    border-radius: var(--radius);
    text-align: left;
    display: grid;
    gap: 8px;
  }
  .error-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .error-head {
    color: var(--neg);
    font-size: 12px;
    font-weight: 500;
    letter-spacing: 0.02em;
  }
  .error-actions {
    display: flex;
    gap: 6px;
  }
  .error-btn {
    background: transparent;
    border: 1px solid hsl(0 84% 65% / 0.35);
    color: var(--neg);
    border-radius: var(--radius-sm);
    padding: 3px 10px;
    font: inherit;
    font-size: 10px;
    letter-spacing: 0.04em;
    cursor: pointer;
    text-transform: lowercase;
    transition: background 120ms, color 120ms, border-color 120ms;
  }
  .error-btn:hover {
    background: hsl(0 84% 65% / 0.12);
    color: hsl(0 84% 85%);
  }
  .error-body {
    margin: 0;
    color: var(--muted);
    font-size: 11px;
    line-height: 1.55;
    white-space: pre-wrap;
    word-break: break-word;
    user-select: text;
    background: hsl(0 0% 0% / 0.25);
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    border: 1px solid hsl(0 84% 65% / 0.2);
    max-height: 200px;
    overflow-y: auto;
  }
  .error-hint {
    margin-top: 2px;
    padding-top: 8px;
    border-top: 1px solid hsl(0 84% 65% / 0.2);
    font-size: 11px;
    color: var(--muted);
    line-height: 1.6;
  }
  .error-hint code {
    display: block;
    margin-top: 6px;
    padding: 6px 8px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 4px;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--foreground);
    user-select: all;
  }

  /* ===== meta footer ===== */
  .meta {
    display: flex;
    justify-content: center;
    align-items: center;
    gap: 10px;
    font-size: 11px;
    letter-spacing: 0.04em;
  }
  .meta .sep { color: var(--border-strong); }
  .meta .brand-dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
    box-shadow: 0 0 6px hsl(213 94% 68% / 0.6);
    margin-right: 6px;
    vertical-align: middle;
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    font: inherit;
    color: inherit;
    cursor: pointer;
    text-decoration: none;
    letter-spacing: 0.04em;
    transition: color 120ms;
  }
  .link:hover {
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  @media (max-width: 640px) {
    .hero { padding: 36px 24px 28px; }
    .hero-title { font-size: 24px; }
  }
</style>
