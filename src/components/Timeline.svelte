<script lang="ts">
  import { untrack } from "svelte";
  import { editor } from "../lib/store.svelte";

  const MIN_VIEW_SPAN = 1.5;
  const HANDLE_FRACTION = 0.15;
  const MIN_KEEP_SECONDS = 0.05;

  let barEl: HTMLDivElement | null = $state(null);
  let navEl: HTMLDivElement | null = $state(null);

  let viewStart = $state(0);
  let viewEnd = $state(0);

  let duration = $derived(editor.video?.duration ?? 0);

  // Pick a sensible default visible window on first load so long videos
  // don't open at maximum zoom-out (where individual cuts are 1px wide and
  // people who haven't found the navigator never zoom in). Short clips
  // (<= 90s) still open fully; longer clips show ~duration/5 capped at 4
  // minutes so the first 60-240 seconds are immediately legible.
  function defaultViewSpan(d: number): number {
    if (d <= 90) return d;
    return Math.min(d, Math.max(60, Math.min(240, d / 5)));
  }

  $effect(() => {
    if (editor.video) {
      viewStart = 0;
      viewEnd = defaultViewSpan(editor.video.duration);
    }
  });

  // Auto-pan rules:
  //   - While playing: keep the playhead inside the visible window. When it
  //     reaches the right edge, scroll the window forward 80% of its span so
  //     the next chunk fits without disorienting the eye.
  //   - While paused: do nothing. The user owns the view and can pan/zoom
  //     freely. Auto-panning during pause locked the window to the
  //     playhead and made scroll-to-pan unusable.
  //   - On a far jump (e.g. a click in the cuts panel that lands fully
  //     outside the window): recenter regardless of play state, since the
  //     user explicitly asked to go there. Detected via the seekToken bump.
  $effect(() => {
    if (!editor.isPlaying) return;
    const t = editor.currentTime;
    if (!duration) return;
    const span = viewEnd - viewStart;
    if (span >= duration - 0.001) return;
    const margin = span * 0.08;
    const tooLate = t > viewEnd - margin;
    const tooEarly = t < viewStart + margin;
    if (!tooLate && !tooEarly) return;
    let newStart: number;
    if (t < viewStart || t > viewEnd) {
      newStart = t - span / 2;
    } else if (tooLate) {
      newStart = viewStart + span * 0.8;
    } else {
      newStart = viewStart - span * 0.8;
    }
    const [s, en] = clampWindow(newStart, newStart + span);
    viewStart = s;
    viewEnd = en;
  });

  // Explicit seeks (cuts panel click, prev/next-keep transport) recenter the
  // window even when paused, because the user just asked to go somewhere.
  // We MUST only act when `seekToken` itself bumps - if the effect also
  // depended on `viewStart`/`viewEnd`, a manual pan would re-fire it and
  // snap the window back to the playhead, defeating the whole pause-pan
  // workflow. So: track only the token, and read viewStart/viewEnd inside
  // an untracked branch.
  let lastSeekToken = 0;
  $effect(() => {
    const token = editor.seekToken;
    if (token === lastSeekToken) return;
    lastSeekToken = token;
    if (!duration) return;
    untrack(() => {
      const t = editor.seekTarget;
      const span = viewEnd - viewStart;
      if (span >= duration - 0.001) return;
      if (t >= viewStart && t <= viewEnd) return;
      const newStart = t - span / 2;
      const [s, en] = clampWindow(newStart, newStart + span);
      viewStart = s;
      viewEnd = en;
    });
  });

  let viewSpan = $derived(Math.max(0.001, viewEnd - viewStart));
  let isZoomed = $derived(duration > 0 && viewSpan < duration - 0.001);

  function clampWindow(start: number, end: number): [number, number] {
    const span = Math.max(MIN_VIEW_SPAN, Math.min(duration, end - start));
    let s = Math.max(0, Math.min(duration - span, start));
    const e = Math.min(duration, s + span);
    if (e > duration) s = duration - span;
    return [Math.max(0, s), Math.min(duration, s + span)];
  }

  // Use $effect so the listener re-attaches whenever barEl swaps in. The
  // {#if editor.video} block above means the bar div is created LATER than
  // this component's first mount (it only renders once metadata arrives),
  // and a plain onMount would run before that with barEl still null.
  $effect(() => {
    const el = barEl;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!duration) return;
      // Use whichever axis has more motion. Trackpad sideways swipes report
      // on deltaX; mouse wheels report on deltaY. Either should move things.
      const delta = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
      if (delta === 0) return;

      const canPan = viewSpan < duration - 0.001;
      if (canPan) {
        // Zoomed in: pan the visible window.
        const panDelta = delta * (viewSpan / 600);
        const [s, en] = clampWindow(viewStart + panDelta, viewEnd + panDelta);
        viewStart = s;
        viewEnd = en;
      } else {
        // Fully zoomed out: there's nothing to pan past, so scroll seeks the
        // playhead instead. Keeps the gesture from feeling broken at default
        // zoom.
        const seekDelta = delta * (duration / 600);
        const next = Math.max(
          0,
          Math.min(duration, editor.currentTime + seekDelta),
        );
        editor.requestSeek(next);
      }
      e.preventDefault();
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  });

  type NavDragMode = "pan" | "resize-l" | "resize-r";
  function startNavDrag(e: PointerEvent, mode: NavDragMode) {
    if (!duration || !navEl) return;
    e.preventDefault();
    e.stopPropagation();
    const track = navEl.getBoundingClientRect();
    const initialStart = viewStart;
    const initialEnd = viewEnd;
    const initialX = e.clientX;
    const onMove = (ev: PointerEvent) => {
      const ds = ((ev.clientX - initialX) / track.width) * duration;
      if (mode === "pan") {
        const [s, en] = clampWindow(initialStart + ds, initialEnd + ds);
        viewStart = s;
        viewEnd = en;
      } else if (mode === "resize-l") {
        viewStart = Math.min(
          initialEnd - MIN_VIEW_SPAN,
          Math.max(0, initialStart + ds),
        );
        viewEnd = initialEnd;
      } else {
        viewStart = initialStart;
        viewEnd = Math.max(
          initialStart + MIN_VIEW_SPAN,
          Math.min(duration, initialEnd + ds),
        );
      }
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
  }

  function onNavTrackPointer(e: PointerEvent) {
    if (!duration || !navEl) return;
    if (e.target !== navEl) return;
    const rect = navEl.getBoundingClientRect();
    const t = ((e.clientX - rect.left) / rect.width) * duration;
    const halfSpan = viewSpan / 2;
    const [s, en] = clampWindow(t - halfSpan, t + halfSpan);
    viewStart = s;
    viewEnd = en;
  }

  function onWindowPointerDown(e: PointerEvent) {
    const rect = (e.currentTarget as HTMLDivElement).getBoundingClientRect();
    const x = e.clientX - rect.left;
    const handle = Math.min(14, rect.width * HANDLE_FRACTION);
    if (x < handle) startNavDrag(e, "resize-l");
    else if (x > rect.width - handle) startNavDrag(e, "resize-r");
    else startNavDrag(e, "pan");
  }

  // Button-driven zoom that keeps the current center of the visible window
  // anchored, so the user's eye doesn't jump around when they press +/-.
  function zoomBy(factor: number) {
    if (!duration) return;
    const center = (viewStart + viewEnd) / 2;
    const newSpan = Math.max(MIN_VIEW_SPAN, Math.min(duration, viewSpan * factor));
    const [s, en] = clampWindow(center - newSpan / 2, center + newSpan / 2);
    viewStart = s;
    viewEnd = en;
  }
  function zoomIn() { zoomBy(1 / 1.6); }
  function zoomOut() { zoomBy(1.6); }
  function fitAll() {
    if (!duration) return;
    viewStart = 0;
    viewEnd = duration;
  }

  function fmt(seconds: number): string {
    const m = Math.floor(seconds / 60);
    const s = (seconds % 60).toFixed(1);
    return `${m}:${s.padStart(4, "0")}`;
  }

  function onBarClick(e: MouseEvent) {
    // If a keep-edge drag just finished we get a click event afterward; the
    // drag handler stops propagation so we never reach this. Click on empty
    // bar area still seeks.
    if (!barEl || !duration) return;
    const rect = barEl.getBoundingClientRect();
    const x = (e.clientX - rect.left) / rect.width;
    const t = viewStart + Math.max(0, Math.min(1, x)) * viewSpan;
    editor.requestSeek(t);
  }

  // ---- keep-edge dragging ----

  type EdgeDragKind = "in" | "out";
  let dragHint = $state<{ x: number; t: number; label: string } | null>(null);

  function pxToTime(clientX: number): number {
    if (!barEl) return 0;
    const rect = barEl.getBoundingClientRect();
    const x = Math.max(0, Math.min(rect.width, clientX - rect.left));
    return viewStart + (x / rect.width) * viewSpan;
  }

  function startEdgeDrag(e: PointerEvent, keepIndex: number, kind: EdgeDragKind) {
    e.preventDefault();
    e.stopPropagation();
    const target = e.currentTarget as HTMLElement;
    target.setPointerCapture(e.pointerId);
    const keeps = editor.keepIntervals();
    const prevEnd = keepIndex > 0 ? keeps[keepIndex - 1].end : 0;
    const nextStart =
      keepIndex < keeps.length - 1 ? keeps[keepIndex + 1].start : duration;
    const own = keeps[keepIndex];

    const onMove = (ev: PointerEvent) => {
      const t = pxToTime(ev.clientX);
      if (kind === "in") {
        const next = Math.max(prevEnd, Math.min(own.end - MIN_KEEP_SECONDS, t));
        editor.updateKeep(keepIndex, next, own.end);
        dragHint = { x: ev.clientX, t: next, label: "in" };
      } else {
        const next = Math.min(nextStart, Math.max(own.start + MIN_KEEP_SECONDS, t));
        editor.updateKeep(keepIndex, own.start, next);
        dragHint = { x: ev.clientX, t: next, label: "out" };
      }
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      dragHint = null;
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
  }

  // ---- derived display data ----

  let intervals = $derived(editor.cutlist?.intervals ?? []);
  let segWithKeepIndex = $derived.by(() => {
    let k = 0;
    return intervals.map((c) => ({
      cut: c,
      keepIndex: c.kind === "keep" ? k++ : -1,
    }));
  });
  let visibleSegments = $derived(
    segWithKeepIndex.filter(
      (s) => s.cut.end > viewStart && s.cut.start < viewEnd,
    ),
  );
  // Disabled keeps count toward "removed" since they won't appear in the
  // exported output. Only active keeps contribute to "kept".
  let keptDuration = $derived(
    intervals
      .filter((c) => c.kind === "keep" && !c.disabled)
      .reduce((a, c) => a + (c.end - c.start), 0),
  );
  let removedDuration = $derived(
    intervals
      .filter((c) => c.kind === "remove" || (c.kind === "keep" && c.disabled))
      .reduce((a, c) => a + (c.end - c.start), 0),
  );
  let playheadInView = $derived(
    editor.currentTime >= viewStart && editor.currentTime <= viewEnd,
  );
  let playheadPct = $derived(((editor.currentTime - viewStart) / viewSpan) * 100);

  // ---- waveform path (visible slice) ----

  // Cap SVG path complexity. The bar/nav are at most a couple thousand pixels
  // wide on any reasonable display, so anything past ~2k points renders the
  // same pixels but costs Chromium/WebView2 a real amount of string parsing per
  // repaint. On Windows this was the main reason scrolling a long video's
  // timeline froze the rest of the UI (video, panels) — every viewStart/viewEnd
  // tick rebuilt a 50k-point path. macOS/WKWebView absorbed it; WebView2 does
  // not. Downsampling here is a no-op visually.
  const MAX_RENDER_POINTS = 2000;

  function downsamplePeaks(peaks: number[], target: number): number[] {
    if (peaks.length <= target) return peaks;
    const out = new Array<number>(target);
    const ratio = peaks.length / target;
    for (let i = 0; i < target; i++) {
      const s = Math.floor(i * ratio);
      const e = Math.max(s + 1, Math.floor((i + 1) * ratio));
      let peak = 0;
      for (let j = s; j < e && j < peaks.length; j++) {
        if (peaks[j] > peak) peak = peaks[j];
      }
      out[i] = peak;
    }
    return out;
  }

  function buildWavePath(peaks: number[]): string {
    if (peaks.length < 2) return "";
    // Compress softer parts so the timeline never looks flat. sqrt scaling
    // matches what most NLEs do visually.
    const maxAmp = Math.max(0.01, ...peaks);
    const normalize = (p: number) => Math.sqrt(Math.max(0, p) / maxAmp);
    let path = "";
    for (let i = 0; i < peaks.length; i++) {
      const x = (i / (peaks.length - 1)) * 100;
      const h = Math.max(0.4, normalize(peaks[i]) * 48);
      path += `${i === 0 ? "M" : " L"}${x.toFixed(3)},${(50 - h).toFixed(3)}`;
    }
    for (let i = peaks.length - 1; i >= 0; i--) {
      const x = (i / (peaks.length - 1)) * 100;
      const h = Math.max(0.4, normalize(peaks[i]) * 48);
      path += ` L${x.toFixed(3)},${(50 + h).toFixed(3)}`;
    }
    path += " Z";
    return path;
  }

  let wavePath = $derived.by(() => {
    if (!editor.waveform || !editor.waveform.length || !duration) return "";
    const total = editor.waveform.length;
    const start = Math.max(
      0,
      Math.min(total - 1, Math.floor((viewStart / duration) * total)),
    );
    const end = Math.max(
      start + 1,
      Math.min(total, Math.ceil((viewEnd / duration) * total)),
    );
    return buildWavePath(
      downsamplePeaks(editor.waveform.slice(start, end), MAX_RENDER_POINTS),
    );
  });

  let navWavePath = $derived.by(() => {
    if (!editor.waveform || !editor.waveform.length) return "";
    return buildWavePath(downsamplePeaks(editor.waveform, MAX_RENDER_POINTS));
  });
</script>

{#if editor.video}
  <section class="card timeline-card">
    <header class="card-head">
      <div class="head-left">
        <h2>timeline</h2>
        <p class="card-sub">
          {#if editor.cutlist}
            <span class="mono">kept {fmt(keptDuration)}</span>
            <span class="muted-2">·</span>
            <span class="mono">removed {fmt(removedDuration)}</span>
          {:else}
            scroll to pan · drag window below to zoom · detect to populate
          {/if}
        </p>
      </div>

      <div class="head-center">
        <div class="transport">
          <button
            class="tbtn"
            onclick={zoomOut}
            disabled={!isZoomed && viewSpan >= duration - 0.001}
            title="Zoom out"
            aria-label="Zoom out"
          >−</button>
          <button
            class="tbtn"
            onclick={zoomIn}
            disabled={viewSpan <= MIN_VIEW_SPAN + 0.001}
            title="Zoom in"
            aria-label="Zoom in"
          >+</button>
          <button
            class="tbtn fit"
            onclick={fitAll}
            disabled={!isZoomed}
            title="Fit timeline to full duration"
          >{isZoomed ? `${(duration / viewSpan).toFixed(1)}×` : "1×"}</button>
        </div>
        <div class="transport">
          <button
            class="tbtn"
            onclick={() => editor.prevKeep()}
            disabled={!editor.cutlist}
            title="Previous cut"
            aria-label="Previous cut"
          >⏮</button>
          <button
            class="tbtn play"
            onclick={() => editor.togglePlay()}
            disabled={!editor.video}
            title={editor.isPlaying ? "Pause (space)" : "Play (space)"}
            aria-label={editor.isPlaying ? "Pause" : "Play"}
          >{editor.isPlaying ? "⏸" : "▶"}</button>
          <button
            class="tbtn"
            onclick={() => editor.nextKeep()}
            disabled={!editor.cutlist}
            title="Next cut"
            aria-label="Next cut"
          >⏭</button>
        </div>
      </div>

      <div class="head-right">
        <label class="skip-switch" title="Toggle skipping removed segments during playback">
          <input type="checkbox" bind:checked={editor.skipRemoved} />
          <span class="mono">{editor.skipRemoved ? "skipping cuts" : "playing all"}</span>
        </label>
        <span class="vsep" aria-hidden="true"></span>
        <span class="mono muted-2 time">
          {fmt(editor.currentTime)} / {fmt(duration)}
        </span>
      </div>
    </header>

    <div class="tl-body">
      <div bind:this={barEl} class="bar" onclick={onBarClick} role="presentation">
        {#if wavePath}
          <svg
            class="waveform"
            viewBox="0 0 100 100"
            preserveAspectRatio="none"
            aria-hidden="true"
          >
            <path d={wavePath} />
          </svg>
        {/if}
        {#each visibleSegments as s (s.cut.start + "-" + s.cut.end + "-" + s.cut.kind)}
          {@const c = s.cut}
          {@const isKeep = c.kind === "keep"}
          {@const isDisabled = isKeep && c.disabled === true}
          {@const isHovered = isKeep && editor.hoveredKeepIndex === s.keepIndex}
          <div
            class="seg {c.kind}"
            class:disabled={isDisabled}
            class:hovered={isHovered}
            style:left="{((c.start - viewStart) / viewSpan) * 100}%"
            style:width="{((c.end - c.start) / viewSpan) * 100}%"
            title="{isDisabled ? 'disabled' : c.kind} {fmt(c.start)} → {fmt(c.end)}"
            onmouseenter={isKeep ? () => editor.setHoveredKeep(s.keepIndex) : undefined}
            onmouseleave={isKeep ? () => editor.setHoveredKeep(null) : undefined}
            onclick={isKeep ? () => editor.focusKeep(s.keepIndex) : undefined}
            role="presentation"
          >
            {#if isKeep}
              <!-- svelte-ignore a11y_click_events_have_key_events -->
              <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
              <div
                class="edge edge-in"
                onpointerdown={(e) => startEdgeDrag(e, s.keepIndex, "in")}
                onclick={(e) => e.stopPropagation()}
                title="Drag to adjust in-point"
                role="separator"
                aria-label="In point handle"
              ></div>
              <!-- svelte-ignore a11y_click_events_have_key_events -->
              <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
              <div
                class="edge edge-out"
                onpointerdown={(e) => startEdgeDrag(e, s.keepIndex, "out")}
                onclick={(e) => e.stopPropagation()}
                title="Drag to adjust out-point"
                role="separator"
                aria-label="Out point handle"
              ></div>
            {/if}
          </div>
        {/each}

        {#if playheadInView}
          <div class="playhead" style:left="{playheadPct}%"></div>
        {/if}
        {#if dragHint}
          <div
            class="hint"
            style:left="{((dragHint.t - viewStart) / viewSpan) * 100}%"
          >
            <span class="mono">{dragHint.label} {fmt(dragHint.t)}</span>
          </div>
        {/if}
      </div>

      <div bind:this={navEl} class="nav" onpointerdown={onNavTrackPointer} role="presentation">
        {#if navWavePath}
          <svg
            class="nav-waveform"
            viewBox="0 0 100 100"
            preserveAspectRatio="none"
            aria-hidden="true"
          >
            <path d={navWavePath} />
          </svg>
        {/if}
        {#each intervals as c (c.start + "n-" + c.end)}
          <div
            class="nav-seg {c.kind}"
            class:disabled={c.kind === "keep" && c.disabled === true}
            style:left="{(c.start / duration) * 100}%"
            style:width="{((c.end - c.start) / duration) * 100}%"
          ></div>
        {/each}
        <div
          class="nav-playhead"
          style:left="{(editor.currentTime / duration) * 100}%"
        ></div>
        <div
          class="nav-window"
          style:left="{(viewStart / duration) * 100}%"
          style:width="{(viewSpan / duration) * 100}%"
          onpointerdown={onWindowPointerDown}
          role="presentation"
        >
          <div class="nav-handle left"></div>
          <div class="nav-handle right"></div>
        </div>
      </div>
    </div>
  </section>
{/if}

<style>
  /* Fill the entire bottom pane so the timeline never leaves dead space
     below itself. The bar grows to absorb extra height so the waveform
     visually scales with the pane (taller pane = bigger waveform). */
  .timeline-card {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  /* Three-zone header: title-left, transport-center, controls-right.
     Overrides the global card-head's flex with a grid so the transport
     stays optically centered regardless of left/right content widths. */
  .card-head {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    gap: 12px;
  }
  .head-center {
    justify-self: center;
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .head-right {
    display: flex;
    align-items: center;
    gap: 14px;
    justify-self: end;
    flex-wrap: wrap;
  }
  .vsep {
    width: 1px;
    height: 18px;
    background: var(--border);
    display: inline-block;
  }

  .time { font-size: 11px; }

  .transport {
    display: inline-flex;
    align-items: stretch;
    gap: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--surface-2);
  }
  .tbtn {
    width: 36px;
    height: 30px;
    background: transparent;
    border: 0;
    color: var(--muted);
    cursor: pointer;
    font-size: 13px;
    line-height: 1;
    padding: 0;
    transition: background 120ms, color 120ms;
    font-family: ui-monospace, monospace;
  }
  .tbtn + .tbtn { border-left: 1px solid var(--border); }
  .tbtn:hover:not(:disabled) {
    background: var(--elevated);
    color: var(--foreground);
  }
  .tbtn:disabled { opacity: 0.35; cursor: not-allowed; }
  .tbtn.play {
    width: 44px;
    font-size: 14px;
    color: var(--foreground);
    background: var(--elevated);
  }
  .tbtn.play:hover:not(:disabled) {
    background: var(--border-strong);
  }
  .tbtn.fit {
    width: auto;
    min-width: 44px;
    padding: 0 10px;
    font-size: 11px;
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }

  .skip-switch {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 11px;
    color: var(--muted);
    cursor: pointer;
    user-select: none;
  }
  .skip-switch input[type="checkbox"] {
    appearance: none;
    -webkit-appearance: none;
    position: relative;
    width: 30px;
    height: 17px;
    border-radius: 999px;
    background: var(--input);
    border: 1px solid var(--border);
    cursor: pointer;
    transition: background 120ms, border-color 120ms;
    flex-shrink: 0;
  }
  .skip-switch input[type="checkbox"]::after {
    content: "";
    position: absolute;
    top: 1px;
    left: 1px;
    width: 13px;
    height: 13px;
    border-radius: 50%;
    background: var(--muted);
    transition: transform 140ms ease, background 140ms;
  }
  .skip-switch input[type="checkbox"]:checked {
    background: var(--pos);
    border-color: var(--pos);
  }
  .skip-switch input[type="checkbox"]:checked::after {
    transform: translateX(13px);
    background: var(--primary-fg);
  }
  .skip-switch input[type="checkbox"]:checked + .mono {
    color: var(--pos);
  }

  .waveform, .nav-waveform {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
  }
  .waveform path {
    fill: hsl(0 0% 90% / 0.18);
  }
  .nav-waveform path {
    fill: hsl(0 0% 90% / 0.22);
  }
  .tl-body {
    padding: 12px 14px 14px;
    display: grid;
    grid-template-rows: minmax(56px, 1fr) auto;
    gap: 8px;
    flex: 1;
    min-height: 0;
  }
  .bar {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 56px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    overflow: hidden;
    cursor: crosshair;
  }
  .seg {
    position: absolute;
    top: 0;
    bottom: 0;
    transition: filter 120ms, box-shadow 120ms;
  }
  .seg.keep {
    background: hsl(142 71% 55% / 0.32);
    border-left: 1px solid hsl(142 71% 55% / 0.6);
    border-right: 1px solid hsl(142 71% 55% / 0.6);
  }
  .seg.keep.hovered {
    background: hsl(142 71% 55% / 0.55);
    box-shadow: inset 0 0 0 1px hsl(142 71% 55% / 0.9);
    filter: brightness(1.1);
    z-index: 2;
  }
  .seg.keep.disabled {
    background: hsl(280 70% 68% / 0.22);
    background-image: repeating-linear-gradient(
      45deg,
      transparent 0px,
      transparent 6px,
      hsl(280 70% 68% / 0.18) 6px,
      hsl(280 70% 68% / 0.18) 7px
    );
    border-left-color: hsl(280 70% 68% / 0.6);
    border-right-color: hsl(280 70% 68% / 0.6);
  }
  .seg.keep.disabled.hovered {
    background: hsl(280 70% 68% / 0.42);
    box-shadow: inset 0 0 0 1px hsl(280 70% 68% / 0.9);
    filter: brightness(1.1);
  }
  .seg.keep.disabled .edge::after {
    background: hsl(280 70% 68% / 0.9);
  }
  .seg.keep.disabled .edge:hover {
    background: hsl(280 70% 68% / 0.2);
  }
  .seg.remove {
    background: hsl(0 84% 65% / 0.18);
    background-image: repeating-linear-gradient(
      135deg,
      transparent 0px,
      transparent 6px,
      hsl(0 84% 65% / 0.18) 6px,
      hsl(0 84% 65% / 0.18) 7px
    );
  }

  .edge {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 8px;
    cursor: ew-resize;
    z-index: 3;
    background: transparent;
    transition: background 120ms;
  }
  .edge::after {
    content: "";
    position: absolute;
    top: 4px;
    bottom: 4px;
    left: 50%;
    width: 2px;
    transform: translateX(-1px);
    background: hsl(142 71% 55% / 0.9);
    border-radius: 1px;
    opacity: 0;
    transition: opacity 120ms;
  }
  .edge-in { left: -4px; }
  .edge-out { right: -4px; }
  .seg.keep:hover .edge::after,
  .seg.keep.hovered .edge::after,
  .edge:hover::after {
    opacity: 1;
  }
  .edge:hover {
    background: hsl(142 71% 55% / 0.2);
  }

  .playhead {
    position: absolute;
    top: -2px;
    bottom: -2px;
    width: 2px;
    background: var(--foreground);
    box-shadow: 0 0 6px hsl(0 0% 100% / 0.6);
    pointer-events: none;
    z-index: 4;
  }
  .hint {
    position: absolute;
    top: -22px;
    transform: translateX(-50%);
    padding: 2px 8px;
    background: var(--elevated);
    border: 1px solid var(--border-strong);
    border-radius: 4px;
    font-size: 10px;
    font-family: var(--font-mono);
    color: var(--foreground);
    white-space: nowrap;
    pointer-events: none;
    z-index: 5;
    box-shadow: 0 4px 12px hsl(0 0% 0% / 0.4);
  }
  .hint::after {
    content: "";
    position: absolute;
    top: 100%;
    left: 50%;
    transform: translateX(-50%);
    border: 4px solid transparent;
    border-top-color: var(--border-strong);
  }

  .nav {
    position: relative;
    width: 100%;
    height: 20px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    overflow: hidden;
    cursor: pointer;
  }
  .nav-seg {
    position: absolute;
    top: 0;
    bottom: 0;
  }
  .nav-seg.keep { background: hsl(142 71% 55% / 0.25); }
  .nav-seg.keep.disabled { background: hsl(280 70% 68% / 0.3); }
  .nav-seg.remove { background: hsl(0 84% 65% / 0.3); }
  .nav-playhead {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1px;
    background: var(--foreground);
    pointer-events: none;
  }
  .nav-window {
    position: absolute;
    top: -1px;
    bottom: -1px;
    border: 1px solid var(--accent);
    background: hsl(213 94% 68% / 0.08);
    border-radius: 3px;
    cursor: grab;
  }
  .nav-handle {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 4px;
    background: var(--accent);
    opacity: 0.7;
  }
  .nav-handle.left { left: 0; cursor: ew-resize; border-radius: 2px 0 0 2px; }
  .nav-handle.right { right: 0; cursor: ew-resize; border-radius: 0 2px 2px 0; }

</style>
