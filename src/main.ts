import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ButtonAudio } from "./audio";
import { formatSize, isActive, isDownloadState, transition, type DownloadState } from "./state";
import "./style.css";

function element<T extends HTMLElement>(id: string): T {
  const result = document.getElementById(id);
  if (!result) throw new Error(`Missing application element: ${id}`);
  return result as T;
}

const app = element<HTMLElement>("app");
const button = element<HTMLButtonElement>("button");
const label = button.querySelector<HTMLSpanElement>(".button-label")!;
const cancelButton = element<HTMLButtonElement>("cancel");
const status = element<HTMLParagraphElement>("status");
const hint = element<HTMLParagraphElement>("hint");
const audio = new ButtonAudio();
let state: DownloadState = { phase: "idle" };
let ready = false;
let actionPending = false;
let confirmationSubmitted = false;
let resetTimer: number | undefined;
let pressTimer: number | undefined;
let unlisten: UnlistenFn | undefined;
let stopped = false;

function removeNativeListener(): void {
  // Tauri currently types this callback as returning void but returns a promise
  // at runtime. Assimilate both forms so a closing IPC channel cannot leak an
  // unhandled rejection.
  void Promise.resolve().then(() => unlisten?.()).catch((error: unknown) => {
    console.warn("Native listener shutdown failed", error);
  });
}

function render(): void {
  app.dataset.phase = state.phase;
  button.disabled = !ready || actionPending || confirmationSubmitted || (state.phase !== "idle" && state.phase !== "confirming");
  button.setAttribute("aria-busy", String(isActive(state.phase)));
  button.setAttribute("aria-label", state.phase === "confirming" ? "Confirm this large download" : "Download a surprise file");
  label.textContent = state.phase === "confirming" ? "DOWNLOAD" : "BOTÓN";
  // Cancellation only appears when it does something. Escape works as well.
  cancelButton.hidden = !isActive(state.phase);
  cancelButton.disabled = actionPending;
  status.removeAttribute("title");
  status.classList.toggle("sr-only", state.phase === "discovering" || state.phase === "downloading");

  switch (state.phase) {
    case "idle":
      status.textContent = "A little uncertainty.";
      hint.textContent = "Press to discover.";
      break;
    case "discovering":
      status.textContent = "Discovering a file. Press Escape to cancel.";
      hint.textContent = "";
      break;
    case "confirming":
      status.textContent = state.total === undefined
        ? "This file may be large."
        : `A ${formatSize(state.total)} surprise.`;
      hint.textContent = "Press again to download.";
      break;
    case "downloading":
      status.textContent = "Downloading a file. Press Escape to cancel.";
      hint.textContent = "";
      break;
    case "complete":
      status.textContent = state.filename || "Your surprise has arrived.";
      status.title = state.filename || "";
      hint.textContent = "Saved to Downloads.";
      break;
    case "error":
      status.textContent = state.message || "That one slipped away.";
      hint.textContent = "You can try again in a moment.";
      break;
  }
}

function update(next: DownloadState): void {
  const updated = transition(state, next);
  if (updated === state) return;
  const previousPhase = state.phase;
  window.clearTimeout(resetTimer);
  state = updated;
  if (state.phase !== "confirming") confirmationSubmitted = false;
  render();
  if (state.phase !== previousPhase && (state.phase === "confirming" || state.phase === "idle")) {
    button.focus({ preventScroll: true });
  }
  if (state.phase === "complete" || state.phase === "error") {
    resetTimer = window.setTimeout(() => {
      update({ phase: "idle" });
      button.focus({ preventScroll: true });
    }, state.phase === "complete" ? 3200 : 4500);
  }
}

function showCommandError(error: unknown): void {
  console.error("BOTÓN command failed", error);
  update({ phase: "error", message: "Couldn't finish this time." });
}

async function activate(): Promise<void> {
  if (!ready || actionPending || confirmationSubmitted || (state.phase !== "idle" && state.phase !== "confirming")) return;
  const confirming = state.phase === "confirming";
  // A command acknowledgement is not a download-state transition. Keep the
  // confirmation locked until the native worker leaves this phase.
  confirmationSubmitted = confirming;
  actionPending = true;
  // Audio is optional: a system audio failure must never prevent a download.
  void audio.click().catch((error: unknown) => console.warn("Click sound unavailable", error));
  window.clearTimeout(pressTimer);
  app.classList.add("pressed");
  pressTimer = window.setTimeout(() => app.classList.remove("pressed"), 240);
  if (!confirming) update({ phase: "discovering" });
  render();
  try {
    if (confirming) await invoke("confirm_download", { accept: true });
    else await invoke("start_download");
  } catch (error: unknown) {
    showCommandError(error);
  } finally {
    actionPending = false;
    render();
  }
}

async function cancel(): Promise<void> {
  if (!ready || actionPending || !isActive(state.phase)) return;
  actionPending = true;
  render();
  let failed = false;
  try {
    await invoke("cancel_download");
    // The backend owns the transition to idle, after partial-file cleanup.
  } catch (error: unknown) {
    console.error("BOTÓN cancellation failed", error);
    // A failed IPC cancellation does not mean the transfer stopped. Keep its
    // state and the cancellation control until the native worker reports back.
    failed = true;
  } finally {
    actionPending = false;
    render();
    if (failed && isActive(state.phase)) {
      status.classList.remove("sr-only");
      status.textContent = "Couldn't cancel. Try again.";
    }
    if (state.phase === "idle") button.focus({ preventScroll: true });
  }
}

function onActivate(): void { void activate(); }
function onCancel(): void { void cancel(); }
function onKeyDown(event: KeyboardEvent): void {
  if (event.key === "Escape" && isActive(state.phase)) {
    event.preventDefault();
    void cancel();
  }
}
button.addEventListener("click", onActivate);
cancelButton.addEventListener("click", onCancel);
window.addEventListener("keydown", onKeyDown);

async function initialize(): Promise<void> {
  if (!isTauri()) {
    status.textContent = "Open BOTÓN as a desktop app.";
    hint.textContent = "The browser cannot download a surprise.";
    return;
  }
  try {
    // Subscribe before enabling activation so even a very fast backend is observed.
    unlisten = await listen<unknown>("download-state", ({ payload }) => {
      if (isDownloadState(payload)) update(payload);
      else console.error("Invalid BOTÓN state event", payload);
    });
    if (stopped) {
      removeNativeListener();
      return;
    }
    ready = true;
    render();
  } catch (error: unknown) {
    console.error("BOTÓN initialization failed", error);
    status.textContent = "Couldn't connect to the download engine.";
    hint.textContent = "Close and reopen BOTÓN.";
  }
}

function shutdown(): void {
  stopped = true;
  window.clearTimeout(resetTimer);
  window.clearTimeout(pressTimer);
  removeNativeListener();
  button.removeEventListener("click", onActivate);
  cancelButton.removeEventListener("click", onCancel);
  window.removeEventListener("keydown", onKeyDown);
  window.removeEventListener("pagehide", shutdown);
  void audio.close().catch((error: unknown) => console.warn("Audio shutdown failed", error));
  // Native window-close handling also cancels, including forced webview teardown.
  if (ready && isActive(state.phase)) {
    void invoke("cancel_download").catch((error: unknown) => console.warn("Shutdown cancellation failed", error));
  }
}
window.addEventListener("pagehide", shutdown);

void initialize();
