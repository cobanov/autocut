<script lang="ts">
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { editor } from "../lib/store.svelte";

  let videoEl: HTMLVideoElement | null = $state(null);

  let path = $derived(editor.pendingPath ?? editor.video?.path ?? null);
  let src = $derived(path ? convertFileSrc(path) : null);

  $effect(() => {
    editor.seekToken;
    if (videoEl) videoEl.currentTime = editor.seekTarget;
  });

  // UI-driven play/pause: any caller can bump playToggleToken to toggle.
  $effect(() => {
    editor.playToggleToken;
    if (!videoEl) return;
    if (editor.playToggleToken === 0) return; // skip initial run
    if (videoEl.paused || videoEl.ended) {
      videoEl.play().catch(() => {});
    } else {
      videoEl.pause();
    }
  });

  // Mirror underlying paused state into the store so UI glyphs match reality.
  $effect(() => {
    if (!videoEl) return;
    const v = videoEl;
    const onPlay = () => (editor.isPlaying = true);
    const onPause = () => (editor.isPlaying = false);
    v.addEventListener("play", onPlay);
    v.addEventListener("playing", onPlay);
    v.addEventListener("pause", onPause);
    v.addEventListener("ended", onPause);
    return () => {
      v.removeEventListener("play", onPlay);
      v.removeEventListener("playing", onPlay);
      v.removeEventListener("pause", onPause);
      v.removeEventListener("ended", onPause);
    };
  });

  // Spacebar play/pause anywhere in the app, except while typing in inputs.
  $effect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.code !== "Space" && e.key !== " ") return;
      const ae = document.activeElement as HTMLElement | null;
      const tag = ae?.tagName;
      if (
        tag === "INPUT" ||
        tag === "TEXTAREA" ||
        tag === "SELECT" ||
        ae?.isContentEditable
      ) {
        return;
      }
      if (!videoEl) return;
      e.preventDefault();
      if (videoEl.paused || videoEl.ended) {
        videoEl.play().catch(() => {});
      } else {
        videoEl.pause();
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  });

  // Frame-rate-precision skip. `ontimeupdate` only fires ~4×/sec which is
  // long enough to be visible as a stutter when a remove segment is short.
  // requestVideoFrameCallback fires per decoded frame, so the jump lands
  // within one frame of entering the remove region. Fallback to rAF for
  // browsers without rVFC (Firefox at time of writing).
  //
  // Skip strategy:
  //   1. Pre-roll: when the playhead enters the last ~80ms of a keep that
  //      precedes a remove, jump straight to the next keep's start. The
  //      old "wait until we cross into the remove and then jump" approach
  //      always rendered 1-2 frames of the silent gap, which read as a
  //      stutter on short cuts.
  //   2. Post-detect: if we somehow land inside an omitted interval (e.g.
  //      from a manual seek), jump to its end.
  // The lookahead window matches a 60fps frame budget so we never schedule
  // a skip more than ~5 frames in advance.
  const SKIP_LOOKAHEAD = 0.08;

  $effect(() => {
    if (!videoEl) return;
    const v = videoEl;
    let cancelled = false;
    let scheduled = false;
    let scheduleToken = 0;
    let animationFrameId: number | null = null;
    let videoFrameId: number | null = null;
    let lastSkipTarget = -1;
    const frameVideo = v as HTMLVideoElement & {
      requestVideoFrameCallback?: (cb: () => void) => number;
      cancelVideoFrameCallback?: (handle: number) => void;
    };

    function tick() {
      if (cancelled) return;
      const t = v.currentTime;
      editor.currentTime = t;
      const isPlaying = !v.paused && !v.ended;
      if (editor.skipRemoved && editor.cutlist) {
        const intervals = editor.cutlist.intervals;
        for (let i = 0; i < intervals.length; i++) {
          const c = intervals[i];
          const isOmitted = c.kind === "remove" || (c.kind === "keep" && c.disabled);
          if (!isOmitted) continue;
          // Already inside an omitted interval: jump out immediately.
          if (t >= c.start && t < c.end - 0.005) {
            performSkip(c.end);
            break;
          }
          // Approaching an omitted interval from the kept side: skip a few
          // frames before we'd cross the boundary so the user never sees
          // the dead frame.
          if (isPlaying && t < c.start && c.start - t < SKIP_LOOKAHEAD) {
            performSkip(c.end);
            break;
          }
        }
      }
      if (isPlaying) schedule();
    }

    function performSkip(target: number) {
      // Avoid hammering currentTime= when we already requested this exact
      // skip — the browser is mid-seek and re-assigning the value can
      // cause an audible click as the audio context resets.
      if (Math.abs(lastSkipTarget - target) < 0.005) return;
      lastSkipTarget = target;
      v.currentTime = target;
    }

    function clearScheduled() {
      scheduleToken += 1;
      scheduled = false;
      if (animationFrameId !== null) {
        cancelAnimationFrame(animationFrameId);
        animationFrameId = null;
      }
      if (videoFrameId !== null) {
        frameVideo.cancelVideoFrameCallback?.(videoFrameId);
        videoFrameId = null;
      }
    }

    function runScheduledTick(token: number) {
      if (token !== scheduleToken) return;
      scheduled = false;
      animationFrameId = null;
      videoFrameId = null;
      tick();
    }

    function schedule() {
      if (cancelled || scheduled || v.paused || v.ended) return;
      scheduled = true;
      const token = ++scheduleToken;
      if (frameVideo.requestVideoFrameCallback) {
        videoFrameId = frameVideo.requestVideoFrameCallback(() => {
          runScheduledTick(token);
        });
      } else {
        animationFrameId = requestAnimationFrame(() => {
          runScheduledTick(token);
        });
      }
    }

    function start() {
      cancelled = false;
      schedule();
    }

    function stop() {
      cancelled = true;
      clearScheduled();
    }

    const onPause = () => {
      editor.currentTime = v.currentTime;
      stop();
    };
    const onSeeked = () => {
      editor.currentTime = v.currentTime;
      lastSkipTarget = -1;
      cancelled = false;
      tick();
    };
    const onTimeUpdate = () => {
      editor.currentTime = v.currentTime;
    };

    v.addEventListener("play", start);
    v.addEventListener("playing", start);
    v.addEventListener("pause", onPause);
    v.addEventListener("ended", onPause);
    v.addEventListener("seeked", onSeeked);
    v.addEventListener("timeupdate", onTimeUpdate);
    start();

    return () => {
      stop();
      v.removeEventListener("play", start);
      v.removeEventListener("playing", start);
      v.removeEventListener("pause", onPause);
      v.removeEventListener("ended", onPause);
      v.removeEventListener("seeked", onSeeked);
      v.removeEventListener("timeupdate", onTimeUpdate);
    };
  });
</script>

{#if src}
  <video
    bind:this={videoEl}
    {src}
    controls
    preload="metadata"
  >
    <track kind="captions" />
  </video>
{:else}
  <div class="empty mono muted-2">no video loaded</div>
{/if}

<style>
  video {
    width: 100%;
    height: 100%;
    object-fit: contain;
    background: #000;
    display: block;
  }
  .empty {
    display: grid;
    place-items: center;
    height: 100%;
    font-size: 12px;
  }
</style>
