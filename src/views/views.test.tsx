import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
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

describe("updates", () => {
  it("shows an update button above Open logs and installs on click", async () => {
    mock = createMockApi({
      installed: true,
      update: { version: "0.2.0", tag: "v0.2.0", asset_name: "Parla_0.2.0_x64-setup.exe", size: 1_000_000 },
    });
    setApi(mock);
    await ready();
    const btn = await screen.findByRole("button", { name: /update to v0\.2\.0/i });
    const logs = screen.getByRole("button", { name: /open logs/i });
    expect(btn.compareDocumentPosition(logs) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await userEvent.click(btn);
    await waitFor(() => expect(mock.calls).toContain("installUpdate"));
  });

  it("hides the update button when already current", async () => {
    await ready();
    expect(screen.queryByRole("button", { name: /update to/i })).not.toBeInTheDocument();
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
    const list = await screen.findByLabelText("Transcript").catch(() => null);
    expect(list).toBeNull(); // empty state has no list yet
    const sc = document.querySelector(".overflow-y-auto.min-h-0") as HTMLElement;
    sc.style.overflowY = "auto"; // jsdom does not load the stylesheet
    Object.defineProperty(sc, "scrollHeight", { configurable: true, value: 4000 });
    await act(async () => void mock.addSegment({ id: 500, text: "line one" }));
    await screen.findByText("line one");
    await waitFor(() => expect(sc.scrollTop).toBe(4000));
  });
});

describe("overlay", () => {
  it("shows the last two sentences", async () => {
    render(<Overlay />);
    await act(async () => {
      mock.addSegment({ id: 1, text: "a", translation: "uno" });
      mock.addSegment({ id: 2, text: "b", translation: "due" });
      mock.addSegment({ id: 3, text: "c", translation: "tre" });
    });
    expect(await screen.findByText("tre")).toBeInTheDocument();
    expect(screen.getByText("due")).toBeInTheDocument();
    expect(screen.queryByText("uno")).not.toBeInTheDocument();
  });
  it("shows translated lines only", async () => {
    setApi(mock);
    render(<Overlay />);
    await act(async () => void mock.addSegment({ id: 1, text: "orig", translation: "tradotto" }));
    expect(await screen.findByText("tradotto")).toBeInTheDocument();
    expect(screen.queryByText("orig")).not.toBeInTheDocument();
  });
});
