import { describe, expect, it } from "vitest";
import { fileSafe, formatBytes, formatClock, formatDuration } from "./format";
import { emptyLive, groupTurns, reduceLive, speakerName, unknownSpeakers } from "./live";
import { createMockApi } from "./mockApi";
import { batchProgress, essentials, featureItems, isLocalUrl, isReady, missing, totalBytes } from "./models";
import { overlayLines } from "./overlay";
import { stickToBottom } from "./scroll";
import { startupMessage } from "./warmup";
import type { Segment, Speaker } from "./types";

const seg = (o: Partial<Segment>): Segment => ({ id: 1, meeting_id: 1, stream: "system", speaker_id: 1, t0: 0, t1: 2, lang: "en", text: "x", translation: null, translation_lang: null, ...o });
const spk = (o: Partial<Speaker>): Speaker => ({ id: 1, meeting_id: 1, label: "Speaker 1", name: null, color: "#fff", is_me: false, ...o }) as Speaker;

describe("format", () => {
  it("formats bytes", () => {
    expect(formatBytes(500)).toBe("1 KB");
    expect(formatBytes(660_000_000)).toBe("660 MB");
    expect(formatBytes(1_500_000_000)).toBe("1.5 GB");
  });
  it("formats clock", () => {
    expect(formatClock(65)).toBe("01:05");
    expect(formatClock(3725)).toBe("1:02:05");
    expect(formatClock(-3)).toBe("00:00");
  });
  it("duration and file names", () => {
    expect(formatDuration(0, null)).toBe("recording…");
    expect(formatDuration(0, 90)).toBe("01:30");
    expect(fileSafe('a/b:c*"d')).toBe("a_b_c_d");
    expect(fileSafe("  ")).toBe("meeting");
  });
});

describe("live reducer", () => {
  it("adds segments once", () => {
    let s = reduceLive(emptyLive(), { type: "segment", segment: seg({ id: 1 }) });
    s = reduceLive(s, { type: "segment", segment: seg({ id: 1 }) });
    expect(s.segments).toHaveLength(1);
  });
  it("attaches translations", () => {
    let s = reduceLive(emptyLive(), { type: "segment", segment: seg({ id: 7 }) });
    s = reduceLive(s, { type: "translation", segment_id: 7, text: "ciao", lang: "it" });
    expect(s.segments[0].translation).toBe("ciao");
  });
  it("meter rises fast and falls slowly", () => {
    let s = reduceLive(emptyLive(), { type: "level", stream: "mic", level: 0.8 });
    expect(s.levels.mic).toBe(0.8);
    s = reduceLive(s, { type: "level", stream: "mic", level: 0 });
    expect(s.levels.mic).toBeCloseTo(0.64);
  });
  it("dedups warnings", () => {
    let s = reduceLive(emptyLive(), { type: "warning", message: "w" });
    s = reduceLive(s, { type: "warning", message: "w" });
    expect(s.warnings).toEqual(["w"]);
  });
});

describe("turns and speakers", () => {
  it("merges same-speaker segments within the gap", () => {
    const t = groupTurns([seg({ id: 1, t1: 2 }), seg({ id: 2, t0: 4, t1: 6 }), seg({ id: 3, t0: 7, speaker_id: 2 }), seg({ id: 4, t0: 40, t1: 42, speaker_id: 2 })]);
    expect(t.map((x) => x.segments.length)).toEqual([2, 1, 1]);
  });
  it("names speakers", () => {
    expect(speakerName([spk({ name: " Anna " })], 1)).toBe(" Anna ");
    expect(speakerName([spk({})], 1)).toBe("Speaker 1");
    expect(speakerName([], 5)).toBe("Speaker");
  });
  it("detects unknown speakers", () => {
    expect(unknownSpeakers([seg({ speaker_id: 9 })], [spk({ id: 1 })])).toBe(true);
    expect(unknownSpeakers([seg({ speaker_id: 1 })], [spk({ id: 1 })])).toBe(false);
  });
});

describe("overlay", () => {
  it("shows only translated non-mic lines, newest fresh", () => {
    expect(overlayLines([seg({ id: 1, translation: "a" }), seg({ id: 2, translation: "b" }), seg({ id: 3, translation: "c" })]).map((x) => x.text)).toEqual(["b", "c"]);
    const l = overlayLines([
      seg({ id: 1, translation: "a" }), seg({ id: 2, stream: "mic", translation: "me" }), seg({ id: 3 }),
      seg({ id: 4, translation: "b" }), seg({ id: 5, translation: "c" }), seg({ id: 6, translation: "d" }),
    ], 3);
    expect(l.map((x) => x.text)).toEqual(["b", "c", "d"]);
    expect(l.map((x) => x.fresh)).toEqual([false, false, true]);
  });
});

describe("models", () => {
  it("computes essentials, missing and progress", async () => {
    const m = await createMockApi().modelStatus();
    expect(essentials(m).map((x) => x.slot).sort()).toEqual(["speaker", "stt", "vad"]);
    expect(isReady(m)).toBe(false);
    expect(missing(m)).toHaveLength(m.length);
    const f = featureItems(m, "translate");
    expect(f.length).toBeGreaterThan(0);
    expect(batchProgress(f, null)).toBe(0);
    expect(batchProgress(f, { id: f[0].id, done: f[0].size_bytes, total: f[0].size_bytes })).toBeGreaterThan(0);
    expect(totalBytes(f)).toBeGreaterThan(0);
    expect(batchProgress([], null)).toBe(1);
  });
  it("recognises loopback URLs", () => {
    for (const u of ["http://127.0.0.1:1234", "http://localhost:11434/v1", "http://[::1]:8080", "localhost:80"]) expect(isLocalUrl(u)).toBe(true);
    for (const u of ["https://api.example.com", "http://192.168.1.5:1234", "http://localhost.evil.com", "http://127.0.0.1@evil.com"]) expect(isLocalUrl(u)).toBe(false);
  });
});

describe("startup notice", () => {
  it("counts down when the last start time is known", () => {
    expect(startupMessage(8000, 2000)).toContain("about 6 s left");
  });
  it("counts up the first time and when slower than expected", () => {
    expect(startupMessage(null, 3500)).toContain("(3 s)");
    expect(startupMessage(5000, 7000)).toContain("Almost ready");
  });
});

describe("autoscroll", () => {
  it("follows new text only when the reader is at the bottom", () => {
    expect(stickToBottom(900, 100, 1000)).toBe(true);
    expect(stickToBottom(880, 100, 1000)).toBe(true);
    expect(stickToBottom(200, 100, 1000)).toBe(false);
  });
});
