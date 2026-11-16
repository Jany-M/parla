import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { setApi } from "../lib/api";
import { createMockApi, type MockApi } from "../lib/mockApi";
import { Overlay } from "./Overlay";

let mock: MockApi;
beforeEach(() => {
  mock = createMockApi({ installed: true });
  setApi(mock);
});
afterEach(() => {
  cleanup();
  setApi(null);
});

async function ready(onboarded = true) {
  if (onboarded) await mock.saveSettings({ ...(await mock.getSettings()), onboarding_done: true });
  render(<App />);
  if (onboarded) {
    // Wait for AppProvider + sidebar update check so later assertions aren't racing act().
    await screen.findByRole("navigation", { name: /main/i });
    await waitFor(() => expect(mock.calls).toContain("checkForUpdate"));
  }
}

async function openSettings() {
  const nav = await screen.findByRole("navigation", { name: /main/i });
  await userEvent.click(within(nav).getByRole("button", { name: /settings/i }));
  await screen.findByRole("heading", { name: /^settings$/i });
}

describe("wizard", () => {
  it("downloads missing models and then shows the app", async () => {
    mock = createMockApi({ installed: false });
    setApi(mock);
    await ready(false);
    expect(await screen.findByRole("heading", { name: /this computer/i })).toBeInTheDocument();
    expect(screen.getByText((t) => /8\s*gb/i.test(t))).toBeInTheDocument();
    expect(screen.getAllByText(/qwen3\.5 2b/i).length).toBeGreaterThan(0);
    expect(screen.queryByText(/qwen3\.5 4b/i)).not.toBeInTheDocument();
    await userEvent.click(await screen.findByRole("button", { name: /download and continue/i }));
    await screen.findByRole("button", { name: /start recording/i });
    expect(mock.calls.filter((c) => c.startsWith("install:")).length).toBeGreaterThanOrEqual(4);
    expect(mock.calls).toContain("install:sum-qwen-2b");
    expect(mock.calls).not.toContain("install:sum-qwen-4b");
  });
  it("can skip optional features", async () => {
    mock = createMockApi({ installed: false });
    setApi(mock);
    await ready(false);
    await userEvent.click(await screen.findByLabelText(/live translation overlay/i));
    await userEvent.click(screen.getByLabelText(/meeting summaries/i));
    await userEvent.click(screen.getByRole("button", { name: /download and continue/i }));
    await screen.findByRole("button", { name: /start recording/i });
    expect(mock.calls).not.toContain("install:mt-hy-mt2");
    expect(mock.calls).not.toContain("install:sum-qwen-2b");
  });
  it("recommends the 4B summary model on high-RAM machines", async () => {
    mock = createMockApi({ installed: false, ramMb: 16000 });
    setApi(mock);
    await ready(false);
    expect(await screen.findByText((t) => /16\s*gb/i.test(t))).toBeInTheDocument();
    expect(screen.getAllByText(/qwen3\.5 4b/i).length).toBeGreaterThan(0);
  });
});

describe("recording", () => {
  it("records, shows live text, translates, and stops", async () => {
    await ready();
    await userEvent.click(await screen.findByRole("button", { name: /start recording/i }));
    await screen.findByRole("button", { name: /stop/i });
    await act(async () => void mock.addSegment({ id: 100, text: "Good morning everyone" }));
    expect(await screen.findByText("Good morning everyone")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /start translating/i }));
    expect(mock.calls).toContain("startTranslation:en");
    expect(mock.calls).toContain("overlay:true");
    await act(async () => mock.emit({ type: "translation", segment_id: 100, text: "Buongiorno a tutti", lang: "it" }));
    expect(await screen.findByText(/Buongiorno a tutti/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /stop$/i }));
    await screen.findByText(/saved to your library/i);
    expect(mock.calls).toContain("stopLive");
  });
  it("tells you when nothing was heard and keeps it out of the library", async () => {
    await ready();
    await userEvent.click(await screen.findByRole("button", { name: /start recording/i }));
    await screen.findByRole("button", { name: /stop/i });
    await act(async () => mock.emit({ type: "stopped", meeting_id: 1, saved: false }));
    expect(await screen.findByText(/no speech was detected/i)).toBeInTheDocument();
    expect(screen.queryByText(/^Saved to your library/)).not.toBeInTheDocument();
  });
  it("pause and resume keep the same recording", async () => {
    await ready();
    await userEvent.click(await screen.findByRole("button", { name: /start recording/i }));
    await userEvent.click(await screen.findByRole("button", { name: /pause/i }));
    expect(mock.calls).toContain("setPaused:true");
    expect(await screen.findByText("Paused")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /resume/i }));
    expect(mock.calls).toContain("setPaused:false");
    expect(mock.calls.filter((c) => c === "startLive")).toHaveLength(1);
    expect(mock.calls).not.toContain("stopLive");
  });
  it("translation never starts on its own", async () => {
    await ready();
    await userEvent.click(await screen.findByRole("button", { name: /start recording/i }));
    await screen.findByRole("button", { name: /stop/i });
    expect(mock.calls.some((c) => c.startsWith("startTranslation"))).toBe(false);
  });
  it("disables translating when no recording", async () => {
    await ready();
    expect(await screen.findByRole("button", { name: /start translating/i })).toBeDisabled();
  });
});

describe("library", () => {
  it("imports a file and summarizes it", async () => {
    await ready();
    await userEvent.click(await screen.findByRole("button", { name: /library/i }));
    await userEvent.click(await screen.findByRole("button", { name: /import audio/i }));
    await userEvent.click(await screen.findByRole("button", { name: /imported/i }));
    expect((await screen.findByLabelText("Meeting title")) as HTMLInputElement).toHaveValue("Imported");
    expect(mock.calls).toContain("import:C:\\audio\\call.wav");
  });
});

describe("logs", () => {
  it("opens the logs folder from the sidebar", async () => {
    await ready();
    await userEvent.click(await screen.findByRole("button", { name: /open logs/i }));
    expect(mock.calls).toContain("openLogs");
  });
});

describe("credits", () => {
  it("opens profile, studio, and issues links from the sidebar", async () => {
    await ready();
    await userEvent.click(await screen.findByRole("button", { name: /jany martelli/i }));
    await userEvent.click(await screen.findByRole("button", { name: /^shambix$/i }));
    await userEvent.click(await screen.findByRole("button", { name: /feedback & issues/i }));
    await userEvent.click(await screen.findByRole("button", { name: /parla on github/i }));
    expect(mock.calls).toContain("openUrl:https://github.com/Jany-M");
    expect(mock.calls).toContain("openUrl:https://www.shambix.com/?utm_source=parla&utm_medium=referral&utm_campaign=projects&utm_content=app-sidebar");
    expect(mock.calls.filter((c) => c === "openUrl:https://github.com/Jany-M/parla/issues")).toHaveLength(2);
  });
});

describe("updates", () => {
  it("shows an update button below the version row and installs on click", async () => {
    mock = createMockApi({
      installed: true,
      update: { version: "0.2.0", tag: "v0.2.0", asset_name: "Parla_0.2.0_x64-setup.exe", size: 1_000_000 },
    });
    setApi(mock);
    await ready();
    const btn = await screen.findByRole("button", { name: /update to v0\.2\.0/i });
    const logs = screen.getByRole("button", { name: /open logs/i });
    expect(logs.compareDocumentPosition(btn) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await userEvent.click(btn);
    await waitFor(() => expect(mock.calls).toContain("installUpdate"));
  });

  it("hides the update button when already current", async () => {
    await ready();
    expect(screen.queryByRole("button", { name: /update to/i })).not.toBeInTheDocument();
  });

  it("checks again when the version label is clicked and on the hourly timer", async () => {
    const { UPDATE_CHECK_MS } = await import("../App");
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      await ready();
      const before = mock.calls.filter((c) => c === "checkForUpdate").length;
      await userEvent.click(screen.getByRole("button", { name: /version .*check for updates/i }));
      await waitFor(() => expect(mock.calls.filter((c) => c === "checkForUpdate").length).toBe(before + 1));
      const afterClick = mock.calls.filter((c) => c === "checkForUpdate").length;
      await act(async () => {
        vi.advanceTimersByTime(UPDATE_CHECK_MS);
      });
      await waitFor(() => expect(mock.calls.filter((c) => c === "checkForUpdate").length).toBe(afterClick + 1));
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("settings", () => {
  it("warns when an endpoint is not local", async () => {
    await ready();
    await openSettings();
    await userEvent.selectOptions(await screen.findByLabelText("Translation"), "endpoint");
    await waitFor(() => expect(screen.getByLabelText(/translation address/i)).toBeInTheDocument());
    const addr = screen.getByLabelText(/translation address/i);
    expect(screen.queryByText(/not on your computer/i)).not.toBeInTheDocument();
    await userEvent.clear(addr);
    await userEvent.type(addr, "https://api.example.com");
    expect(await screen.findByText(/not on your computer/i)).toBeInTheDocument();
  });
  it("delete-all needs confirmation", async () => {
    await ready();
    await openSettings();
    await userEvent.click(await screen.findByRole("button", { name: /delete all my data/i }));
    expect(mock.calls).not.toContain("deleteAllData");
  });
  it("hides the 4B summary download on low-RAM machines", async () => {
    mock = createMockApi({ installed: false, ramMb: 8000 });
    setApi(mock);
    await ready();
    await openSettings();
    expect(await screen.findByText(/larger summary model is hidden/i)).toBeInTheDocument();
    expect(screen.getAllByText((t) => t.includes("Qwen3.5 2B")).length).toBeGreaterThan(0);
    expect(screen.queryByText((t) => t.includes("Qwen3.5 4B"))).not.toBeInTheDocument();
  });
  it("lets high-RAM machines pick either summary model", async () => {
    mock = createMockApi({ installed: true, ramMb: 16000 });
    setApi(mock);
    await ready();
    await openSettings();
    expect(screen.getAllByText((t) => t.includes("Qwen3.5 4B")).length).toBeGreaterThan(0);
    expect(screen.getAllByText((t) => t.includes("Qwen3.5 2B")).length).toBeGreaterThan(0);
    const pick = await screen.findByLabelText("Summaries model");
    await userEvent.selectOptions(pick, "sum-qwen-2b");
    await waitFor(() => expect(mock.calls).toContain("saveSettings"));
  });
});

describe("transcript autoscroll", () => {
  it("scrolls the live transcript to the newest line", async () => {
    await ready();
    await userEvent.click(await screen.findByRole("button", { name: /start recording/i }));
    await screen.findByRole("button", { name: /stop/i });
    // Empty state is a <p>, not the labeled <ol> - queryBy (not findBy) so we don't wait 1s.
    expect(screen.queryByLabelText("Transcript")).toBeNull();
    const sc = document.querySelector(".overflow-y-auto.min-h-0") as HTMLElement;
    sc.style.overflowY = "auto"; // jsdom does not load the stylesheet
    Object.defineProperty(sc, "scrollHeight", { configurable: true, value: 4000 });
    await act(async () => void mock.addSegment({ id: 500, text: "line one" }));
    expect(await screen.findByText("line one")).toBeInTheDocument();
    expect(await screen.findByLabelText("Transcript")).toBeInTheDocument();
    await waitFor(() => expect(sc.scrollTop).toBe(4000));
  });
});

describe("overlay", () => {
  it("shows the full translation history, newest last", async () => {
    render(<Overlay />);
    await act(async () => {
      mock.addSegment({ id: 1, text: "a", translation: "uno" });
      mock.addSegment({ id: 2, text: "b", translation: "due" });
      mock.addSegment({ id: 3, text: "c", translation: "tre" });
    });
    expect(await screen.findByText("uno")).toBeInTheDocument();
    expect(screen.getByText("due")).toBeInTheDocument();
    expect(screen.getByText("tre")).toBeInTheDocument();
  });
  it("shows translated lines only", async () => {
    setApi(mock);
    render(<Overlay />);
    await act(async () => void mock.addSegment({ id: 1, text: "orig", translation: "tradotto" }));
    expect(await screen.findByText("tradotto")).toBeInTheDocument();
    expect(screen.queryByText("orig")).not.toBeInTheDocument();
  });
  it("resumes following the latest line after scroll idle", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      render(<Overlay />);
      await act(async () => {
        mock.addSegment({ id: 1, text: "a", translation: "one" });
        mock.addSegment({ id: 2, text: "b", translation: "two" });
      });
      const box = await screen.findByLabelText("Translation overlay");
      Object.defineProperty(box, "clientHeight", { configurable: true, value: 100 });
      Object.defineProperty(box, "scrollHeight", { configurable: true, value: 400 });
      box.scrollTop = 0;
      box.dispatchEvent(new Event("scroll"));
      await act(async () => void mock.addSegment({ id: 3, text: "c", translation: "three" }));
      expect(box.scrollTop).toBe(0);
      await act(async () => {
        vi.advanceTimersByTime(5_000);
      });
      expect(box.scrollTop).toBe(box.scrollHeight);
    } finally {
      vi.useRealTimers();
    }
  });
  it("drags the window from the body but not from the close button", async () => {
    render(<Overlay />);
    const line = await screen.findByText(/translations of other speakers/i);
    await act(async () => {
      line.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0, clientX: 40 }));
    });
    expect(mock.calls).toContain("startDragging");
    mock.calls.length = 0;
    await userEvent.click(screen.getByRole("button", { name: /hide overlay/i }));
    expect(mock.calls).toContain("overlay:false");
    expect(mock.calls).not.toContain("startDragging");
  });
});
