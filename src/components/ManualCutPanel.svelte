<script lang="ts">
  import { editor } from "../lib/store.svelte";

  let keeps = $derived(editor.keepIntervals());
  let duration = $derived(editor.video?.duration ?? 0);
  let disabledCount = $derived(keeps.filter((k) => k.disabled).length);

  // Width-based responsive collapse driven by ResizeObserver. Container
  // queries should do this for free, but Chromium webviews evaluate them
  // against an unset width on first paint, so the layout starts broken
  // and only corrects after a window resize. ResizeObserver fires after
  // initial layout reliably.
  let listEl: HTMLDivElement | null = $state(null);
  let isNarrow = $state(false);

  $effect(() => {
    if (!listEl) return;
    const ro = new ResizeObserver((entries) => {
      for (const entry of entries) {
        isNarrow = entry.contentRect.width <= 240;
      }
    });
    ro.observe(listEl);
    return () => ro.disconnect();
  });

  // Scroll the matching row into view only when the user explicitly clicks a
  // segment in the timeline. Earlier this auto-scrolled on hover, but during
  // a timeline scrub the cursor "hovers" dozens of segments per second, which
  // stacked smooth-scroll animations and pinned the UI thread on Windows.
  // `block: "nearest"` makes it a no-op when the row is already on screen.
  $effect(() => {
    const idx = editor.focusedKeepIndex;
    if (idx === null || !listEl) return;
    const row = listEl.querySelector(
      `[data-keep-index="${idx}"]`,
    ) as HTMLElement | null;
    row?.scrollIntoView({ block: "nearest", behavior: "instant" });
  });

  function fmt(s: number): string {
    return s.toFixed(2);
  }

  function commitStart(i: number, value: number) {
    if (!Number.isFinite(value)) return;
    const k = keeps[i];
    editor.updateKeep(i, value, k.end);
  }
  function commitEnd(i: number, value: number) {
    if (!Number.isFinite(value)) return;
    const k = keeps[i];
    editor.updateKeep(i, k.start, value);
  }
  function addAtPlayhead() {
    const t = editor.currentTime;
    const len = Math.min(2, duration - t);
    if (len <= 0) return;
    editor.addKeepAt(t, t + len);
  }
</script>

<section class="card panel">
  <header class="card-head">
    <div>
      <h2>cuts</h2>
      <p class="card-sub">
        {#if keeps.length}
          <span class="mono">{keeps.length}</span> keep
          {keeps.length === 1 ? "interval" : "intervals"}
        {:else}
          run detection or add manually
        {/if}
      </p>
    </div>
    <div class="card-head-actions">
      {#if disabledCount > 0}
        <span class="disabled-pill mono" title="Disabled keeps won't be in the export">
          <span class="dot dot-disabled"></span>
          {disabledCount} off
        </span>
      {/if}
      <button
        class="btn btn-sm"
        onclick={addAtPlayhead}
        disabled={!editor.video}
        title="Add 2s keep starting at playhead"
      >
        +
      </button>
    </div>
  </header>

  <div bind:this={listEl} class="list" class:narrow={isNarrow}>
    {#if keeps.length === 0}
      <div class="empty muted-2">
        <p class="empty-title">no cuts yet</p>
        <p class="hint">
          press <span class="kbd">detect</span> on the left, or
          <span class="kbd">+</span> above to add one at the playhead.
          drag the green edge handles in the timeline below to fine-tune.
        </p>
      </div>
    {:else}
      {#each keeps as k, i (i)}
        {@const isHovered = editor.hoveredKeepIndex === i}
        <div
          class="row"
          class:hovered={isHovered}
          class:disabled={k.disabled}
          data-keep-index={i}
          onmouseenter={() => editor.setHoveredKeep(i)}
          onmouseleave={() => editor.setHoveredKeep(null)}
          role="presentation"
        >
          <button
            class="idx mono"
            onclick={() => editor.requestSeek(k.start)}
            title="Seek to in-point"
          >{String(i + 1).padStart(2, "0")}</button>

          <div class="range mono">
            <input
              type="number"
              step="0.05"
              min="0"
              max={duration}
              value={fmt(k.start)}
              aria-label="In point seconds"
              onchange={(e) =>
                commitStart(i, (e.currentTarget as HTMLInputElement).valueAsNumber)}
            />
            <span class="arrow">→</span>
            <input
              type="number"
              step="0.05"
              min="0"
              max={duration}
              value={fmt(k.end)}
              aria-label="Out point seconds"
              onchange={(e) =>
                commitEnd(i, (e.currentTarget as HTMLInputElement).valueAsNumber)}
            />
          </div>

          <span class="dur mono muted-2">{(k.end - k.start).toFixed(2)}s</span>

          <button
            class="toggle-btn"
            class:on={k.disabled}
            onclick={() => editor.toggleKeepDisabled(i)}
            title={k.disabled
              ? "Re-enable this cut"
              : "Disable (excluded from export until re-enabled)"}
            aria-label={k.disabled ? "Re-enable cut" : "Disable cut"}
          >{k.disabled ? "↺" : "×"}</button>
        </div>
      {/each}
    {/if}
  </div>
</section>

<style>
  .panel {
    display: grid;
    grid-template-rows: auto 1fr;
    min-height: 0;
    height: 100%;
  }
  .list {
    overflow-y: auto;
    padding: 6px;
    min-height: 0;
  }
  .empty {
    padding: 20px 14px;
    font-size: 12px;
    text-align: center;
  }
  .empty-title { margin: 0 0 6px; font-weight: 500; color: var(--muted); }
  .empty .hint {
    margin: 0;
    line-height: 1.6;
    color: var(--muted-2);
  }
  .kbd {
    display: inline-block;
    padding: 0 6px;
    border: 1px solid var(--border);
    border-radius: 3px;
    background: var(--surface-2);
    color: var(--muted);
    font-family: var(--font-mono);
    font-size: 11px;
  }

  .row {
    display: grid;
    grid-template-columns: 28px 1fr 44px 22px;
    gap: 6px;
    align-items: center;
    padding: 4px 6px;
    border-radius: var(--radius-sm);
    transition: background 100ms, box-shadow 100ms;
    position: relative;
  }
  .row + .row { margin-top: 2px; }
  .row:hover, .row.hovered {
    background: var(--surface-2);
  }
  .row.hovered {
    box-shadow: inset 2px 0 0 var(--pos);
  }
  .row.disabled.hovered {
    box-shadow: inset 2px 0 0 var(--disabled);
  }
  .row.disabled .range {
    border-color: hsl(280 70% 68% / 0.4);
    background: hsl(280 70% 68% / 0.05);
  }
  .row.disabled .range input {
    color: var(--disabled);
    text-decoration: line-through;
    text-decoration-color: hsl(280 70% 68% / 0.6);
  }
  .row.disabled .range .arrow { color: hsl(280 70% 68% / 0.6); }
  .row.disabled .dur { color: hsl(280 70% 68% / 0.7); text-decoration: line-through; }
  .row.disabled .idx { border-color: hsl(280 70% 68% / 0.5); color: var(--disabled); }

  .idx {
    background: var(--surface-2);
    border: 1px solid var(--border);
    color: var(--muted);
    border-radius: var(--radius-sm);
    height: 26px;
    font-size: 11px;
    font-weight: 500;
    cursor: pointer;
    padding: 0;
    transition: background 120ms, color 120ms, border-color 120ms;
  }
  .idx:hover {
    background: var(--accent);
    color: var(--primary-fg);
    border-color: var(--accent);
  }
  .row.hovered .idx {
    border-color: var(--pos);
    color: var(--pos);
  }

  .range {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    gap: 4px;
    background: var(--input);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    height: 26px;
    padding: 0 6px;
    transition: border-color 120ms;
    min-width: 0;
  }
  .range:focus-within { border-color: var(--border-strong); }
  .range input {
    background: transparent;
    border: 0;
    color: var(--foreground);
    font: inherit;
    font-family: var(--font-mono);
    font-size: 11px;
    text-align: center;
    width: 100%;
    min-width: 0;
    appearance: textfield;
    -moz-appearance: textfield;
    padding: 0;
  }
  .range input::-webkit-outer-spin-button,
  .range input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }
  .range .arrow {
    color: var(--muted-2);
    font-size: 10px;
    line-height: 1;
  }

  .dur {
    font-size: 11px;
    text-align: right;
    font-variant-numeric: tabular-nums;
    color: var(--muted-2);
  }

  .toggle-btn {
    width: 22px;
    height: 22px;
    background: transparent;
    border: 0;
    color: var(--muted-2);
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-size: 14px;
    padding: 0;
    line-height: 1;
    transition: color 120ms, background 120ms;
  }
  .toggle-btn:hover {
    color: var(--disabled);
    background: hsl(280 70% 68% / 0.1);
  }
  .toggle-btn.on {
    color: var(--disabled);
    background: hsl(280 70% 68% / 0.12);
  }
  .toggle-btn.on:hover {
    color: var(--pos);
    background: hsl(142 71% 55% / 0.1);
  }

  .disabled-pill {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 2px 8px;
    border: 1px solid hsl(280 70% 68% / 0.4);
    background: hsl(280 70% 68% / 0.08);
    color: var(--disabled);
    border-radius: 999px;
    font-size: 10px;
    text-transform: lowercase;
    letter-spacing: 0.02em;
  }

  /* Hide the duration when the panel is narrow so the time fields stay
     readable. Width is tracked via ResizeObserver in the script so the
     layout is correct on first paint, not just after a resize. */
  .list.narrow .row {
    grid-template-columns: 24px 1fr 22px;
  }
  .list.narrow .dur {
    display: none;
  }
</style>
