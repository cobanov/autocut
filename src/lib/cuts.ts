// Pure interval algebra behind the cut editor. Deliberately rune-free so it
// can be unit tested without a Svelte runtime — this is the logic that decides
// what actually ends up in the export, so it earns direct coverage.

import type { Cut, CutList } from "./types";

export type KeepInput = { start: number; end: number; disabled?: boolean };
export type Keep = { start: number; end: number; disabled: boolean };

/// Shorter than this and the interval is a drag artifact, not a cut, so
/// normalization drops it outright.
const DEGENERATE_SECONDS = 0.01;

/// The shortest a keep may be squeezed to by editing one of its edges. Larger
/// than DEGENERATE_SECONDS on purpose: an edit should bottom out at something
/// still visible on the timeline rather than silently deleting the keep.
export const MIN_KEEP_SECONDS = 0.05;

/// Clamp a proposed new in/out point for `keeps[index]` so it stays between
/// its neighbours, inside [0, duration], and leaves the keep at least
/// MIN_KEEP_SECONDS long.
///
/// Both edit surfaces route through this: the timeline edge drag and the
/// numeric fields in the cuts panel. Without it, typing an overlapping value
/// fuses two keeps and renumbers everything after them.
export function clampKeepEdge(
  keeps: Keep[],
  index: number,
  edge: "in" | "out",
  proposed: number,
  duration: number,
): number {
  const own = keeps[index];
  if (!own) return proposed;
  if (edge === "in") {
    const floor = index > 0 ? keeps[index - 1].end : 0;
    const ceil = own.end - MIN_KEEP_SECONDS;
    return Math.min(Math.max(proposed, floor), ceil);
  }
  const floor = own.start + MIN_KEEP_SECONDS;
  const ceil = index < keeps.length - 1 ? keeps[index + 1].start : duration;
  return Math.max(Math.min(proposed, ceil), floor);
}

/// Clip keeps to [0, duration], drop degenerate ones, sort by start, and fuse
/// the ones that genuinely overlap.
///
/// Touching is not overlapping: two keeps that share a boundary stay two
/// keeps. They render identically in the export either way, but fusing them
/// renumbers every later keep, and callers (timeline edge drag) hold an index
/// across the whole gesture.
export function normalizeKeeps(keeps: KeepInput[], duration: number): Keep[] {
  const clipped: Keep[] = keeps
    .map((k) => ({
      start: Math.max(0, Math.min(duration, k.start)),
      end: Math.max(0, Math.min(duration, k.end)),
      disabled: !!k.disabled,
    }))
    .filter((k) => k.end - k.start > DEGENERATE_SECONDS)
    .sort((a, b) => a.start - b.start);

  const merged: Keep[] = [];
  for (const k of clipped) {
    const last = merged[merged.length - 1];
    if (last && k.start < last.end) {
      last.end = Math.max(last.end, k.end);
      // When an active keep overlaps a disabled keep, active content wins.
      // Otherwise a tiny drag overlap can silently remove good footage.
      last.disabled = last.disabled && k.disabled;
    } else {
      merged.push({ ...k });
    }
  }
  return merged;
}

export type OmittedInterval = { start: number; end: number };

/// Everything the export will drop: real removes plus keeps the user disabled.
/// Sorted and non-overlapping, because the cutlist it comes from tiles the
/// source in order.
export function omittedIntervals(cutlist: CutList): OmittedInterval[] {
  return cutlist.intervals
    .filter((c) => c.kind === "remove" || (c.kind === "keep" && c.disabled))
    .map((c) => ({ start: c.start, end: c.end }));
}

/// Once the playhead is this close to an interval's out-point it is already
/// leaving; re-issuing the same skip only makes the audio context click.
const SKIP_SETTLED_SECONDS = 0.005;

/// Where playback should jump to from `time`, or null to keep rolling.
///
/// `lookahead` is how far ahead to pre-empt an upcoming omitted interval, so
/// the dead frame is never rendered. Pass 0 while paused: that leaves only the
/// "the playhead is sitting inside a removed region" rule active, which is
/// what a manual seek into a gap needs.
///
/// Binary search rather than a scan: this runs once per decoded video frame,
/// and an hour of detected speech is easily a couple thousand intervals.
export function skipTargetFor(
  omitted: OmittedInterval[],
  time: number,
  lookahead: number,
): number | null {
  // First interval that hasn't already ended. Anything earlier is behind the
  // playhead and can't be relevant.
  let lo = 0;
  let hi = omitted.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (omitted[mid].end > time) hi = mid;
    else lo = mid + 1;
  }

  // Two candidates, not one: when the playhead is in the settled tail of an
  // interval that abuts the next one, the first candidate declines and the
  // second is the one inside the lookahead window.
  for (const c of [omitted[lo], omitted[lo + 1]]) {
    if (!c) break;
    if (time >= c.start && time < c.end - SKIP_SETTLED_SECONDS) return c.end;
    if (time < c.start && c.start - time < lookahead) return c.end;
  }
  return null;
}

/// Past this far into a keep, "previous" restarts it rather than stepping back
/// a keep — standard media-player behaviour.
const RESTART_WINDOW_SECONDS = 1.0;

/// "Next" ignores keeps starting within this of the playhead, so pressing it
/// while sitting exactly on an in-point doesn't land on the same keep again.
const NEXT_EPSILON_SECONDS = 0.05;

/// Disabled keeps are excluded from the export, so the transport treats them
/// as if they weren't there. Stopping on one would be navigating to footage
/// the user has already said they don't want.
function enabled(keeps: Keep[]): Keep[] {
  return keeps.filter((k) => !k.disabled);
}

/// Where the "previous cut" button should seek to, or null if there is nothing
/// enabled to seek to.
export function previousKeepTarget(keeps: Keep[], time: number): number | null {
  const active = enabled(keeps);
  if (!active.length) return null;

  const current = active.findIndex((k) => time >= k.start && time < k.end);
  if (current >= 0) {
    if (time - active[current].start > RESTART_WINDOW_SECONDS) {
      return active[current].start;
    }
    return current > 0 ? active[current - 1].start : active[current].start;
  }

  let previous = -1;
  for (let i = 0; i < active.length; i++) {
    if (active[i].end <= time) previous = i;
    else break;
  }
  return previous === -1 ? active[0].start : active[previous].start;
}

/// Where the "next cut" button should seek to, or null if there is nothing
/// enabled to seek to. Past the last keep this runs out to `duration` so the
/// button still does something at the tail of the timeline.
export function nextKeepTarget(
  keeps: Keep[],
  time: number,
  duration: number,
): number | null {
  const active = enabled(keeps);
  if (!active.length) return null;

  const upcoming = active.find((k) => k.start > time + NEXT_EPSILON_SECONDS);
  if (upcoming) return upcoming.start;

  const current = active.find((k) => time >= k.start && time < k.end);
  return current ? current.end : duration;
}

/// Turn normalized keeps into a cutlist that tiles [0, duration]: every gap
/// becomes a `remove`, and the keeps carry their `disabled` flag through.
export function tileCutList(keeps: Keep[], duration: number): CutList {
  const intervals: Cut[] = [];
  let cursor = 0;
  for (const k of keeps) {
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
  if (cursor < duration) {
    intervals.push({ start: cursor, end: duration, kind: "remove" });
  }
  return { source_duration: duration, intervals };
}

/// Collapse disabled keeps into the surrounding remove gaps so the Rust
/// exporter sees a clean cutlist where every interval is either truly kept or
/// truly removed. `disabled` is a frontend-only concept; Rust never sees it.
export function resolveForExport(cutlist: CutList): CutList {
  const out: Cut[] = [];
  for (const c of cutlist.intervals) {
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
  return { source_duration: cutlist.source_duration, intervals: out };
}
