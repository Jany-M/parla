import { useEffect, useRef } from "react";
import { stickToBottom } from "../lib/scroll";
import { formatClock } from "../lib/format";
import { groupTurns, speakerColor, speakerName } from "../lib/live";
import type { Segment, Speaker } from "../lib/types";

export function Transcript({ segments, speakers, autoScroll = false, empty }: { segments: Segment[]; speakers: Speaker[]; autoScroll?: boolean; empty?: string }) {
  const end = useRef<HTMLDivElement>(null);
  // follow the speech unless the reader scrolled up to look at something earlier
  const follow = useRef(true);
  useEffect(() => {
    if (!autoScroll) return;
    const box = scroller(end.current);
    if (!box) return;
    const onScroll = () => (follow.current = stickToBottom(box.scrollTop, box.clientHeight, box.scrollHeight));
    box.addEventListener("scroll", onScroll);
    return () => box.removeEventListener("scroll", onScroll);
  }, [autoScroll]);
  useEffect(() => {
    if (!autoScroll || !follow.current) return;
    const box = scroller(end.current);
    if (box) box.scrollTop = box.scrollHeight;
  }, [segments, autoScroll]);

  if (segments.length === 0) return <p className="py-10 text-center text-sm text-mute">{empty ?? "Nothing here yet."}</p>;
  return (
    <ol className="space-y-4" aria-label="Transcript">
      {groupTurns(segments).map((t) => (
        <li key={t.key} className="grid grid-cols-[3.5rem_1fr] gap-3">
          <span className="pt-0.5 text-xs tabular-nums text-mute">{formatClock(t.t0)}</span>
          <div>
            <div className="mb-0.5 text-sm font-semibold" style={{ color: speakerColor(speakers, t.speakerId) }}>
              {t.stream === "mic" ? "Me" : speakerName(speakers, t.speakerId)}
            </div>
            {t.segments.map((s) => (
              <p key={s.id} className="leading-relaxed text-slate-200">
                {s.text}
                {s.translation && <span className="mt-0.5 block text-[0.92em] italic text-indigo-300">↳ {s.translation}</span>}
              </p>
            ))}
          </div>
        </li>
      ))}
      <div ref={end} />
    </ol>
  );
}

/** Nearest ancestor that scrolls. */
function scroller(el: HTMLElement | null): HTMLElement | null {
  for (let p = el?.parentElement ?? null; p; p = p.parentElement) {
    const o = getComputedStyle(p).overflowY;
    if (o === "auto" || o === "scroll") return p;
  }
  return null;
}
