import type { Segment } from "./types";

export interface OverlayLine { id: number; text: string; fresh: boolean }

/** How long after the user stops scrolling before the overlay follows new lines again. */
export const OVERLAY_RESUME_MS = 5_000;

/** Right-edge pixels reserved so the scrollbar can be dragged without moving the window. */
export const OVERLAY_SCROLLBAR_PX = 12;

/**
 * Translated non-mic lines, oldest → newest.
 * Pass `n` to keep only the last n (tests); omit for the full history.
 */
export function overlayLines(segments: Segment[], n?: number): OverlayLine[] {
  const translated = segments.filter((s) => s.translation && s.stream !== "mic");
  const shown = n == null ? translated : translated.slice(-n);
  return shown.map((s, i) => ({ id: s.id, text: s.translation as string, fresh: i === shown.length - 1 }));
}

/**
 * Whole-overlay window drag, without stealing scroll or the close control.
 * Wheel / trackpad scrolling stays on the scroller; only primary-button presses start a drag,
 * and hits on `[data-no-drag]` or the scrollbar gutter are ignored.
 */
export function shouldStartOverlayDrag(opts: {
  button: number;
  target: EventTarget | null;
  clientX: number;
  scroller: Pick<Element, "getBoundingClientRect" | "contains"> | null;
  scrollbarPx?: number;
}): boolean {
  if (opts.button !== 0) return false;
  const el = opts.target instanceof Element ? opts.target : null;
  if (el?.closest("[data-no-drag]")) return false;
  const scroller = opts.scroller;
  if (scroller && el && (el === scroller || scroller.contains(el))) {
    const r = scroller.getBoundingClientRect();
    const gutter = opts.scrollbarPx ?? OVERLAY_SCROLLBAR_PX;
    // Skip when layout has no real width (jsdom) so tests and first paint still drag.
    if (r.width > gutter * 2 && opts.clientX >= r.right - gutter) return false;
  }
  return true;
}
