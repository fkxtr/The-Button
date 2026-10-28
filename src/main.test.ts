// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import markup from "../index.html?raw";
import type { DownloadState } from "./state";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  unlisten: vi.fn(),
  isTauri: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke, isTauri: mocks.isTauri }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("./audio", () => ({
  ButtonAudio: class {
    async click(): Promise<void> { /* No hardware in deterministic DOM tests. */ }
    async close(): Promise<void> { /* No hardware in deterministic DOM tests. */ }
  },
}));

let receive: (event: { payload: DownloadState }) => void;
const mainButton = () => document.getElementById("button") as HTMLButtonElement;
const cancelButton = () => document.getElementById("cancel") as HTMLButtonElement;
const phase = () => document.getElementById("app")!.dataset.phase;
const status = () => document.getElementById("status")!;
const flush = async () => { await Promise.resolve(); await Promise.resolve(); };
const emit = (payload: DownloadState) => receive({ payload });

beforeEach(async () => {
  vi.resetModules();
  vi.useFakeTimers();
  vi.clearAllMocks();
  mocks.invoke.mockResolvedValue(undefined);
  mocks.unlisten.mockResolvedValue(undefined);
  mocks.isTauri.mockReturnValue(true);
  mocks.listen.mockImplementation(async (_name, callback) => {
    receive = callback;
    return mocks.unlisten;
  });
  // Parse the real shell inertly and remove browser entrypoints before mounting.
  // Vitest's explicit import below must be the only module initialization.
  const fixture = new DOMParser().parseFromString(markup, "text/html");
  fixture.querySelectorAll("script").forEach((script) => script.remove());
  document.body.replaceChildren(
    ...Array.from(fixture.body.childNodes, (node) => document.importNode(node, true)),
  );
  await import("./main");
  await flush();
});

afterEach(async () => {
  window.dispatchEvent(new Event("pagehide"));
  await flush();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("native IPC interactions", () => {
  it("keeps a submitted confirmation locked until the native worker leaves that phase", async () => {
    mainButton().click();
    await flush();
    emit({ phase: "confirming", total: 200 * 1024 ** 2 });
    expect(mainButton().disabled).toBe(false);
    mainButton().click();
    await flush(); // Native acknowledgement precedes the downloading event.
    expect(phase()).toBe("confirming");
    expect(mainButton().disabled).toBe(true);
    mainButton().click();
    expect(mocks.invoke.mock.calls.filter(([name]) => name === "confirm_download")).toHaveLength(1);
    emit({ phase: "downloading", bytes: 0 });
    emit({ phase: "complete", filename: "surprise.pdf" });
    expect(status().textContent).toBe("surprise.pdf");
    await vi.advanceTimersByTimeAsync(3200);
    expect(phase()).toBe("idle");
    expect(mainButton().disabled).toBe(false);
  });

  it("does not start overlapping downloads and announces loading to assistive technology", async () => {
    mainButton().click();
    mainButton().click();
    await flush();
    expect(mocks.invoke).toHaveBeenCalledTimes(1);
    expect(status().textContent).toContain("Discovering a file");
    expect(status().classList.contains("sr-only")).toBe(true);
    emit({ phase: "downloading", bytes: 100 });
    mainButton().click();
    expect(mocks.invoke).toHaveBeenCalledTimes(1);
    expect(status().textContent).toContain("Downloading a file");
  });

  it("keeps cancellation available after a transport failure, then unlocks after native cleanup", async () => {
    const diagnostic = vi.spyOn(console, "error").mockImplementation(() => {});
    mainButton().click();
    await flush();
    emit({ phase: "downloading", bytes: 0 });
    mocks.invoke.mockRejectedValueOnce(new Error("IPC unavailable"));
    cancelButton().click();
    await flush();
    expect(diagnostic).toHaveBeenCalled();
    expect(phase()).toBe("downloading");
    expect(mainButton().disabled).toBe(true);
    expect(cancelButton().disabled).toBe(false);
    expect(status().textContent).toContain("Couldn't cancel");
    mocks.invoke.mockImplementationOnce(async () => {
      emit({ phase: "idle" }); // Cleanup can complete before the IPC response.
    });
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await flush();
    expect(phase()).toBe("idle");
    expect(mainButton().disabled).toBe(false);
    expect(cancelButton().hidden).toBe(true);
    expect(document.activeElement).toBe(mainButton());
  });

  it("offers confirmation for unknown lengths and safely renders a returned filename", async () => {
    mainButton().click();
    await flush();
    emit({ phase: "confirming" });
    expect(status().textContent).toBe("This file may be large.");
    expect(document.activeElement).toBe(mainButton());
    mainButton().click();
    await flush();
    emit({ phase: "downloading" });
    const filename = "<img src=x onerror=alert(1)>.txt";
    emit({ phase: "complete", filename });
    expect(status().textContent).toBe(filename);
    expect(status().querySelector("img")).toBeNull();
    expect(status().classList.contains("sr-only")).toBe(false);
  });

  it("recovers from a failed start and permits a retry", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    mocks.invoke.mockRejectedValueOnce(new Error("No download engine"));
    mainButton().click();
    await flush();
    expect(phase()).toBe("error");
    await vi.advanceTimersByTimeAsync(4500);
    expect(mainButton().disabled).toBe(false);
    mainButton().click();
    await flush();
    expect(phase()).toBe("discovering");
    expect(mocks.invoke).toHaveBeenCalledTimes(2);
  });

  it("handles asynchronous listener cleanup rejection during native shutdown", async () => {
    const diagnostic = vi.spyOn(console, "warn").mockImplementation(() => {});
    mocks.unlisten.mockRejectedValueOnce(new Error("Webview already closed"));
    window.dispatchEvent(new Event("pagehide"));
    await flush();
    await flush();
    expect(mocks.unlisten).toHaveBeenCalledTimes(1);
    expect(diagnostic).toHaveBeenCalledWith("Native listener shutdown failed", expect.any(Error));
  });
});
