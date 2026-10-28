import { describe, expect, it } from "vitest";
import { formatSize, isActive, isDownloadState, transition, type DownloadState } from "./state";

describe("download lifecycle", () => {
  it("accepts a complete download and resets", () => {
    let state: DownloadState = { phase: "idle" };
    for (const next of [
      { phase: "discovering" },
      { phase: "downloading", bytes: 0, total: 200 },
      { phase: "downloading", bytes: 200, total: 200 },
      { phase: "complete", filename: "unexpected.jpg" },
      { phase: "idle" },
    ] satisfies DownloadState[]) {
      state = transition(state, next);
      expect(state).toEqual(next);
    }
  });

  it("requires discovery before downloading and downloading before completion", () => {
    const idle: DownloadState = { phase: "idle" };
    expect(transition(idle, { phase: "downloading" })).toBe(idle);
    const discovery: DownloadState = { phase: "discovering" };
    expect(transition(discovery, { phase: "complete" })).toBe(discovery);
  });

  it("rejects overlapping activations and late progress after completion", () => {
    const active: DownloadState = { phase: "downloading", bytes: 100 };
    expect(transition(active, { phase: "discovering" })).toBe(active);
    const complete: DownloadState = { phase: "complete", filename: "file.pdf" };
    expect(transition(complete, { phase: "downloading", bytes: 200 })).toBe(complete);
  });

  it("supports large-file confirmation and cancellation", () => {
    const confirm = transition({ phase: "discovering" }, { phase: "confirming", total: 300_000_000 });
    expect(confirm.phase).toBe("confirming");
    expect(transition(confirm, { phase: "downloading" }).phase).toBe("downloading");
    expect(transition(confirm, { phase: "idle" }).phase).toBe("idle");
  });

  it.each(["discovering", "confirming", "downloading"] as const)("handles cancellation and errors while %s", (phase) => {
    expect(transition({ phase }, { phase: "idle" }).phase).toBe("idle");
    const error = transition({ phase }, { phase: "error", message: "Connection interrupted" });
    expect(error.phase).toBe("error");
    expect(transition(error, { phase: "idle" }).phase).toBe("idle");
    expect(isActive(phase)).toBe(true);
  });

  it.each(["idle", "complete", "error"] as const)("does not permit cancellation while %s", (phase) => {
    expect(isActive(phase)).toBe(false);
  });
});

describe("IPC validation", () => {
  it("accepts the backend contract", () => {
    expect(isDownloadState({ phase: "downloading", bytes: 42, total: 100 })).toBe(true);
    expect(isDownloadState({ phase: "complete", filename: "<img onerror=alert(1)>" })).toBe(true);
    expect(isDownloadState({ phase: "error", message: "Try again" })).toBe(true);
  });

  it.each([null, [], "idle", {}, { phase: "unknown" }, { phase: "toString" },
    { phase: "downloading", bytes: -1 }, { phase: "downloading", total: Infinity },
    { phase: "downloading", bytes: 0.5 }, { phase: "complete", filename: {} },
    { phase: "error", message: ["bad"] }, { phase: "downloading", total: Number.MAX_SAFE_INTEGER + 1 },
  ])("rejects malformed payload %j", (payload) => {
    expect(isDownloadState(payload)).toBe(false);
  });
});

describe("size confirmation", () => {
  it("uses binary units and rounds small units up", () => {
    expect(formatSize(1023)).toBe("1023 B");
    expect(formatSize(1025)).toBe("2 KiB");
    expect(formatSize(100 * 1024 ** 2)).toBe("100 MiB");
    expect(formatSize(1.5 * 1024 ** 3)).toBe("1.5 GiB");
  });
});
