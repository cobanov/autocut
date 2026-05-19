<script lang="ts">
  import type { Snippet } from "svelte";

  type Direction = "horizontal" | "vertical";
  type Props = {
    a: Snippet;
    b: Snippet;
    direction?: Direction;
    initial?: number;
    min?: number;
    max?: number;
    storageKey?: string;
  };

  let {
    a,
    b,
    direction = "horizontal",
    initial = 0.5,
    min = 0.1,
    max = 0.9,
    storageKey,
  }: Props = $props();

  function loadInitial(): number {
    if (storageKey && typeof localStorage !== "undefined") {
      const v = parseFloat(localStorage.getItem(storageKey) ?? "");
      if (Number.isFinite(v) && v > min && v < max) return v;
    }
    return initial;
  }

  let ratio = $state(loadInitial());
  let containerEl: HTMLDivElement | null = $state(null);
  let dragging = $state(false);

  function startDrag(e: PointerEvent) {
    if (!containerEl) return;
    e.preventDefault();
    const handle = e.currentTarget as HTMLElement;
    handle.setPointerCapture(e.pointerId);
    dragging = true;
    const rect = containerEl.getBoundingClientRect();

    const onMove = (ev: PointerEvent) => {
      const total = direction === "horizontal" ? rect.width : rect.height;
      const offset =
        direction === "horizontal" ? ev.clientX - rect.left : ev.clientY - rect.top;
      ratio = Math.max(min, Math.min(max, offset / total));
    };
    const onUp = (ev: PointerEvent) => {
      dragging = false;
      if (handle.hasPointerCapture(ev.pointerId)) {
        handle.releasePointerCapture(ev.pointerId);
      }
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      if (storageKey) {
        try {
          localStorage.setItem(storageKey, String(ratio));
        } catch {
          /* storage may be disabled */
        }
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
  }
</script>

<div
  class="split {direction}"
  class:dragging
  bind:this={containerEl}
  style:--ratio={ratio}
>
  <div class="pane">{@render a()}</div>
  <div
    class="divider"
    onpointerdown={startDrag}
    role="separator"
    aria-orientation={direction === "horizontal" ? "vertical" : "horizontal"}
  ></div>
  <div class="pane">{@render b()}</div>
</div>

<style>
  .split {
    display: grid;
    width: 100%;
    height: 100%;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }
  .split.horizontal {
    grid-template-columns: calc(var(--ratio) * 100%) 6px 1fr;
    grid-template-rows: 100%;
  }
  .split.vertical {
    grid-template-rows: calc(var(--ratio) * 100%) 6px 1fr;
    grid-template-columns: 100%;
  }
  .pane {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    display: grid;
  }
  .divider {
    background: transparent;
    position: relative;
    transition: background 140ms;
  }
  .divider::before {
    content: "";
    position: absolute;
    background: var(--border);
    transition: background 140ms;
  }
  .split.horizontal .divider {
    cursor: col-resize;
  }
  .split.horizontal .divider::before {
    left: 50%;
    top: 0;
    bottom: 0;
    width: 1px;
    transform: translateX(-0.5px);
  }
  .split.vertical .divider {
    cursor: row-resize;
  }
  .split.vertical .divider::before {
    top: 50%;
    left: 0;
    right: 0;
    height: 1px;
    transform: translateY(-0.5px);
  }
  .divider:hover::before,
  .split.dragging .divider::before {
    background: var(--accent);
    box-shadow: 0 0 6px hsl(213 94% 68% / 0.6);
  }
  .split.dragging {
    user-select: none;
  }
</style>
