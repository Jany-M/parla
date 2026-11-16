import { useEffect, useReducer, useRef, useState } from "react";
import { api } from "../lib/api";
import { emptyLive, reduceLive } from "../lib/live";
import { overlayLines } from "../lib/overlay";
import type { Settings } from "../lib/types";

/** Floating, always-on-top caption box (excluded from screen sharing). Shows the last two sentences. */
export function Overlay() {
  const [live, dispatch] = useReducer(reduceLive, undefined, emptyLive);
  const [settings, setSettings] = useState<Settings | null>(null);
  const box = useRef<HTMLDivElement>(null);

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

  const lines = overlayLines(live.segments, 2);
  // if two long sentences do not fit, the newest text stays in view
  const key = lines.map((l) => `${l.id}:${l.text}`).join("|");
  useEffect(() => {
    if (box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [key]);
  const size = settings?.overlay_font_px ?? 22;
  const alpha = settings?.overlay_opacity ?? 0.85;

  return (
    <div data-tauri-drag-region className="relative h-full select-none rounded-2xl px-5 py-3" style={{ background: `rgba(10,12,18,${alpha})`, fontSize: size }}>
      <button aria-label="Hide overlay" onClick={() => api().showOverlay(false)} className="absolute right-3 top-2 z-10 text-sm text-slate-400 hover:text-white">✕</button>
      <div ref={box} className="h-full overflow-y-auto [scrollbar-width:none]">
        <div className="flex min-h-full flex-col justify-end">
          {lines.length === 0 ? (
            <p data-tauri-drag-region className="text-center text-base text-slate-400">Translations of other speakers appear here.</p>
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
