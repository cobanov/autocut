<script lang="ts">
  import Slider from "./Slider.svelte";
  import { editor } from "../lib/store.svelte";

  function bump() {
    editor.scheduleDetect();
  }

  let hasCutlist = $derived(!!editor.cutlist);
  let busy = $derived(editor.jobStatus !== "idle");
</script>

<section class="card">
  <header class="card-head">
    <div>
      <h2>detection</h2>
      <p class="card-sub">silero v5 · 16khz</p>
    </div>
    <div class="card-head-actions">
      <button
        class="btn btn-ghost btn-sm"
        onclick={() => editor.resetParams()}
        disabled={busy}
        title="Restore threshold, pad, and min silence/speech to defaults"
      >defaults</button>
    </div>
  </header>

  <div class="card-body">
    <button
      class="btn btn-primary btn-block btn-lg"
      onclick={() => editor.runDetectNow()}
      disabled={busy}
    >
      {#if editor.jobStatus === "detecting"}
        analyzing…
      {:else if hasCutlist}
        re-detect
      {:else}
        detect silences
      {/if}
    </button>

    {#if editor.detectError}
      <div class="error">
        <span class="dot dot-neg"></span>
        <span class="mono">{editor.detectError}</span>
      </div>
    {/if}

    <div class="fields">
      <Slider
        label="threshold"
        min={0.1}
        max={0.95}
        step={0.05}
        bind:value={editor.params.threshold}
        format={(v) => v.toFixed(2)}
        oninput={bump}
      />
      <Slider
        label="pad"
        min={0}
        max={2}
        step={0.05}
        bind:value={editor.params.pad}
        format={(v) => `${v.toFixed(2)}s`}
        oninput={bump}
      />
      <Slider
        label="min silence"
        min={100}
        max={2000}
        step={50}
        bind:value={editor.params.min_silence_ms}
        format={(v) => `${v}ms`}
        oninput={bump}
      />
      <Slider
        label="min speech"
        min={100}
        max={2000}
        step={50}
        bind:value={editor.params.min_speech_ms}
        format={(v) => `${v}ms`}
        oninput={bump}
      />

      <p class="tip mono muted-2">
        hold <span class="kbd">shift</span> for fine adjustment
      </p>
    </div>
  </div>
</section>

<style>
  /* Fill the available pane height so the left column doesn't leave dead
     space below the parameter card when the detection card's natural
     content is shorter than the column. The body scrolls when content
     exceeds the available height. Svelte scopes these selectors so they
     don't bleed into ExportPanel's .card / .card-body. */
  .card {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .card-body {
    flex: 1;
    overflow-y: auto;
    min-height: 0;
  }

  .fields {
    display: grid;
    gap: 18px;
    margin-top: 14px;
  }
  .tip {
    font-size: 10px;
    line-height: 1.6;
    margin: 2px 0 0;
    color: var(--muted-2);
    letter-spacing: 0.02em;
  }
  .kbd {
    display: inline-block;
    padding: 0 5px;
    border: 1px solid var(--border);
    border-radius: 3px;
    background: var(--surface-2);
    color: var(--muted);
    font-size: 9px;
    line-height: 14px;
    margin: 0 1px;
  }
  .error {
    margin-top: 12px;
    display: flex;
    gap: 8px;
    align-items: flex-start;
    padding: 8px 10px;
    background: hsl(0 84% 65% / 0.06);
    border: 1px solid hsl(0 84% 65% / 0.25);
    border-radius: var(--radius-sm);
    font-size: 11px;
    color: var(--neg);
    word-break: break-word;
  }
  .error .dot {
    margin-top: 5px;
    flex-shrink: 0;
  }
</style>
