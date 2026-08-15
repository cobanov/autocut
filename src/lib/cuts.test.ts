import { describe, expect, test } from "vitest";
import {
  MIN_KEEP_SECONDS,
  clampKeepEdge,
  nextKeepTarget,
  normalizeKeeps,
  omittedIntervals,
  previousKeepTarget,
  resolveForExport,
  skipTargetFor,
  tileCutList,
} from "./cuts";

describe("omittedIntervals", () => {
  test("collects removes and disabled keeps, and nothing else", () => {
    const out = omittedIntervals({
      source_duration: 12,
      intervals: [
        { start: 0, end: 2, kind: "remove" },
        { start: 2, end: 4, kind: "keep" },
        { start: 4, end: 6, kind: "keep", disabled: true },
        { start: 6, end: 8, kind: "remove" },
        { start: 8, end: 12, kind: "keep" },
      ],
    });
    expect(out).toEqual([
      { start: 0, end: 2 },
      { start: 4, end: 6 },
      { start: 6, end: 8 },
    ]);
  });
});

describe("skipTargetFor", () => {
  const omitted = [
    { start: 2, end: 3 },
    { start: 7, end: 9 },
    { start: 20, end: 24 },
  ];
  const LOOKAHEAD = 0.08;

  test("jumps out of an interval the playhead landed inside", () => {
    expect(skipTargetFor(omitted, 7.5, LOOKAHEAD)).toBe(9);
  });

  test("pre-rolls over an interval so its first frame never renders", () => {
    // 0.05s short of the region at [7, 9): inside the lookahead, so the jump
    // fires now and lands on the far side at 9 — landing on 7 would park the
    // playhead at the top of the dead zone we are trying to skip.
    expect(skipTargetFor(omitted, 6.95, LOOKAHEAD)).toBe(9);
  });

  test("stays put when the next interval is still far away", () => {
    expect(skipTargetFor(omitted, 5, LOOKAHEAD)).toBe(null);
  });

  test("a zero lookahead disables the pre-roll, for the paused case", () => {
    expect(skipTargetFor(omitted, 6.95, 0)).toBe(null);
  });

  test("a zero lookahead still escapes an interval the playhead is inside", () => {
    expect(skipTargetFor(omitted, 7.5, 0)).toBe(9);
  });

  test("does not re-fire on the last few ms of an interval it is leaving", () => {
    expect(skipTargetFor(omitted, 8.999, LOOKAHEAD)).toBe(null);
  });

  test("catches the following interval when leaving one that abuts it", () => {
    // Two omitted regions separated by a sliver of kept footage. The playhead
    // is in the dead zone at the tail of the first one, so the first rule
    // declines — but the second region is inside the lookahead and must
    // still be caught. A search that only ever considered one candidate
    // interval would play the gap.
    const abutting = [
      { start: 1, end: 2 },
      { start: 2.01, end: 3 },
    ];
    expect(skipTargetFor(abutting, 1.997, LOOKAHEAD)).toBe(3);
  });

  test("returns null past the last interval", () => {
    expect(skipTargetFor(omitted, 30, LOOKAHEAD)).toBe(null);
  });

  test("returns null when nothing is omitted", () => {
    expect(skipTargetFor([], 5, LOOKAHEAD)).toBe(null);
  });
});

describe("transport navigation", () => {
  const keeps = [
    { start: 0, end: 4, disabled: false },
    { start: 6, end: 10, disabled: false },
    { start: 12, end: 16, disabled: false },
  ];

  describe("previousKeepTarget", () => {
    test("restarts the current keep when well into it", () => {
      expect(previousKeepTarget(keeps, 8)).toBe(6);
    });

    test("steps back a keep when barely into the current one", () => {
      expect(previousKeepTarget(keeps, 6.2)).toBe(0);
    });

    test("stays put at the very first keep", () => {
      expect(previousKeepTarget(keeps, 0.2)).toBe(0);
    });

    test("from inside a gap, lands on the keep before it", () => {
      expect(previousKeepTarget(keeps, 11)).toBe(6);
    });

    test("skips a disabled keep, which is not in the export", () => {
      const withDisabled = [
        { start: 0, end: 4, disabled: false },
        { start: 6, end: 10, disabled: true },
        { start: 12, end: 16, disabled: false },
      ];
      expect(previousKeepTarget(withDisabled, 12.2)).toBe(0);
    });

    test("returns null when nothing is enabled", () => {
      expect(previousKeepTarget([{ start: 0, end: 4, disabled: true }], 2)).toBe(
        null,
      );
    });
  });

  describe("nextKeepTarget", () => {
    test("jumps to the next keep's in-point", () => {
      expect(nextKeepTarget(keeps, 2, 20)).toBe(6);
    });

    test("from inside a gap, jumps to the upcoming keep", () => {
      expect(nextKeepTarget(keeps, 5, 20)).toBe(6);
    });

    test("runs to the end of the current keep when it is the last one", () => {
      expect(nextKeepTarget(keeps, 13, 20)).toBe(16);
    });

    test("runs to the source duration past the last keep", () => {
      expect(nextKeepTarget(keeps, 18, 20)).toBe(20);
    });

    test("skips a disabled keep, which is not in the export", () => {
      const withDisabled = [
        { start: 0, end: 4, disabled: false },
        { start: 6, end: 10, disabled: true },
        { start: 12, end: 16, disabled: false },
      ];
      expect(nextKeepTarget(withDisabled, 2, 20)).toBe(12);
    });

    test("returns null when nothing is enabled", () => {
      expect(nextKeepTarget([{ start: 0, end: 4, disabled: true }], 2, 20)).toBe(
        null,
      );
    });
  });
});

describe("clampKeepEdge", () => {
  const keeps = [
    { start: 1, end: 3, disabled: false },
    { start: 5, end: 7, disabled: false },
    { start: 9, end: 11, disabled: false },
  ];

  test("an in-point cannot cross the previous keep's out-point", () => {
    expect(clampKeepEdge(keeps, 1, "in", 2, 20)).toBe(3);
  });

  test("an in-point cannot swallow its own keep", () => {
    expect(clampKeepEdge(keeps, 1, "in", 99, 20)).toBe(7 - MIN_KEEP_SECONDS);
  });

  test("an out-point cannot cross the next keep's in-point", () => {
    expect(clampKeepEdge(keeps, 1, "out", 10, 20)).toBe(9);
  });

  test("an out-point cannot swallow its own keep", () => {
    expect(clampKeepEdge(keeps, 1, "out", -5, 20)).toBe(5 + MIN_KEEP_SECONDS);
  });

  test("the first keep's in-point floors at zero", () => {
    expect(clampKeepEdge(keeps, 0, "in", -4, 20)).toBe(0);
  });

  test("the last keep's out-point ceils at the source duration", () => {
    expect(clampKeepEdge(keeps, 2, "out", 40, 20)).toBe(20);
  });

  test("a value already inside its bounds passes through untouched", () => {
    expect(clampKeepEdge(keeps, 1, "in", 4.2, 20)).toBe(4.2);
  });

  test("an out-of-range index returns the proposal unchanged", () => {
    expect(clampKeepEdge(keeps, 7, "in", 4.2, 20)).toBe(4.2);
  });
});

describe("normalizeKeeps", () => {
  test("keeps two touching intervals separate", () => {
    // Dragging keep #2's in-point all the way left until it meets keep #1's
    // out-point must not fuse them. Fusing renumbers every later keep, and
    // the timeline drag handler is holding the pre-fuse index — the rest of
    // the drag then edits somebody else's cut.
    const out = normalizeKeeps(
      [
        { start: 0, end: 5 },
        { start: 5, end: 9 },
      ],
      10,
    );
    expect(out).toEqual([
      { start: 0, end: 5, disabled: false },
      { start: 5, end: 9, disabled: false },
    ]);
  });

  test("merges genuinely overlapping intervals", () => {
    const out = normalizeKeeps(
      [
        { start: 0, end: 5 },
        { start: 4, end: 9 },
      ],
      10,
    );
    expect(out).toEqual([{ start: 0, end: 9, disabled: false }]);
  });

  test("does not re-enable a disabled keep that merely touches an active one", () => {
    const out = normalizeKeeps(
      [
        { start: 0, end: 5, disabled: false },
        { start: 5, end: 9, disabled: true },
      ],
      10,
    );
    expect(out).toEqual([
      { start: 0, end: 5, disabled: false },
      { start: 5, end: 9, disabled: true },
    ]);
  });

  test("active content wins when an active keep truly overlaps a disabled one", () => {
    const out = normalizeKeeps(
      [
        { start: 0, end: 5, disabled: false },
        { start: 4, end: 9, disabled: true },
      ],
      10,
    );
    expect(out).toEqual([{ start: 0, end: 9, disabled: false }]);
  });

  test("clips to the source duration and drops degenerate intervals", () => {
    const out = normalizeKeeps(
      [
        { start: -3, end: 2 },
        { start: 4, end: 4.005 },
        { start: 8, end: 40 },
      ],
      10,
    );
    expect(out).toEqual([
      { start: 0, end: 2, disabled: false },
      { start: 8, end: 10, disabled: false },
    ]);
  });

  test("sorts unordered input", () => {
    const out = normalizeKeeps(
      [
        { start: 6, end: 8 },
        { start: 1, end: 3 },
      ],
      10,
    );
    expect(out.map((k) => k.start)).toEqual([1, 6]);
  });
});

describe("tileCutList", () => {
  test("fills the gaps between keeps with removes and tiles [0, duration]", () => {
    const cutlist = tileCutList(
      [
        { start: 2, end: 4, disabled: false },
        { start: 6, end: 8, disabled: false },
      ],
      10,
    );
    expect(cutlist.source_duration).toBe(10);
    expect(cutlist.intervals).toEqual([
      { start: 0, end: 2, kind: "remove" },
      { start: 2, end: 4, kind: "keep" },
      { start: 4, end: 6, kind: "remove" },
      { start: 6, end: 8, kind: "keep" },
      { start: 8, end: 10, kind: "remove" },
    ]);
  });

  test("carries the disabled flag onto the keep interval", () => {
    const cutlist = tileCutList([{ start: 0, end: 10, disabled: true }], 10);
    expect(cutlist.intervals).toEqual([
      { start: 0, end: 10, kind: "keep", disabled: true },
    ]);
  });

  test("emits adjacent keeps with no remove between them", () => {
    const cutlist = tileCutList(
      [
        { start: 0, end: 5, disabled: false },
        { start: 5, end: 10, disabled: false },
      ],
      10,
    );
    expect(cutlist.intervals).toEqual([
      { start: 0, end: 5, kind: "keep" },
      { start: 5, end: 10, kind: "keep" },
    ]);
  });
});

describe("resolveForExport", () => {
  test("turns disabled keeps into removes and coalesces them with neighbours", () => {
    const resolved = resolveForExport({
      source_duration: 10,
      intervals: [
        { start: 0, end: 2, kind: "remove" },
        { start: 2, end: 4, kind: "keep", disabled: true },
        { start: 4, end: 6, kind: "remove" },
        { start: 6, end: 10, kind: "keep" },
      ],
    });
    expect(resolved.intervals).toEqual([
      { start: 0, end: 6, kind: "remove" },
      { start: 6, end: 10, kind: "keep" },
    ]);
  });

  test("strips the frontend-only disabled flag from surviving keeps", () => {
    const resolved = resolveForExport({
      source_duration: 4,
      intervals: [{ start: 0, end: 4, kind: "keep" }],
    });
    expect(resolved.intervals[0]).not.toHaveProperty("disabled");
  });
});
