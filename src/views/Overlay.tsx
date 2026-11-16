import { useEffect, useReducer, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { api } from "../lib/api";
import { emptyLive, reduceLive } from "../lib/live";
import { OVERLAY_RESUME_MS, overlayLines, shouldStartOverlayDrag } from "../lib/overlay";
import { stickToBottom } from "../lib/scroll";
import type { Settings } from "../lib/types";

/** Floating, always-on-top caption box (excluded from screen sharing). Full translation history, newest at the bottom. */
export function Overlay() {
  const [live, dispatch] = useReducer(reduceLive, undefined, emptyLive);
  const [settings, setSettings] = useState<Settings | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const follow = useRef(true);
  const idleTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    document.body.classList.add("overlay-body");
    void api().getSettings().then(setSettings);
    let off: (() => void) | undefined;
    let cancelled = false;
    api().onEvent(dispatch).then((u) => (cancelled ? u() : (off = u)));
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);

  useEffect(() => {
    const el = box.current;
    if (!el) return;

    const clearIdle = () => {
      if (idleTimer.current != null) {
        clearTimeout(idleTimer.current);
        idleTimer.current = null;
      }
    };

    const scrollToLatest = () => {
      el.scrollTop = el.scrollHeight;
    };

    const onScroll = () => {
      if (stickToBottom(el.scrollTop, el.clientHeight, el.scrollHeight)) {
        follow.current = true;
        clearIdle();
        return;
      }
      // Reading older lines: pause follow until they return to the bottom or sit still for a bit.
      follow.current = false;
      clearIdle();
      idleTimer.current = setTimeout(() => {
        follow.current = true;
        scrollToLatest();
      }, OVERLAY_RESUME_MS);
    };

    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      el.removeEventListener("scroll", onScroll);
      clearIdle();
    };
  }, []);

  const lines = overlayLines(live.segments);
  const key = lines.map((l) => `${l.id}:${l.text}`).join("|");
  useEffect(() => {
    if (follow.current && box.current) {
      box.current.scrollTop = box.current.scrollHeight;
    }
  }, [key]);

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (!shouldStartOverlayDrag({ button: e.button, target: e.target, clientX: e.clientX, scroller: box.current })) return;
    void api().startDragging();
  };

  const size = settings?.overlay_font_px ?? 22;
  const alpha = settings?.overlay_opacity ?? 0.85;

  return (
    <div
      className="relative h-full cursor-grab select-none rounded-2xl px-5 py-3 active:cursor-grabbing"
      style={{ background: `rgba(10,12,18,${alpha})`, fontSize: size }}
      onPointerDown={onPointerDown}
    >
      <button
        type="button"
        data-no-drag
        aria-label="Hide overlay"
        onClick={() => api().showOverlay(false)}
        className="absolute right-3 top-2 z-10 cursor-pointer text-sm text-slate-400 hover:text-white"
      >
        ✕
      </button>
      <div
        ref={box}
        aria-label="Translation overlay"
        className="h-full overflow-y-auto overscroll-contain pr-1 [scrollbar-width:thin] [scrollbar-color:rgba(255,255,255,0.25)_transparent]"
      >
        <div className="flex min-h-full flex-col justify-end gap-1.5">
          {lines.length === 0 ? (
            <p className="text-center text-base text-slate-400">Translations of other speakers appear here.</p>
          ) : (
            lines.map((l) => (
              <p key={l.id} className={`leading-snug ${l.fresh ? "font-semibold text-white" : "text-slate-300/80"}`}>{l.text}</p>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
