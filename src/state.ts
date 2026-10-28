export type Phase =
  | "idle"
  | "discovering"
  | "confirming"
  | "downloading"
  | "complete"
  | "error";

export interface DownloadState {
  phase: Phase;
  filename?: string;
  bytes?: number;
  total?: number;
  message?: string;
}

const transitions: Record<Phase, readonly Phase[]> = {
  idle: ["idle", "discovering", "error"],
  discovering: ["discovering", "confirming", "downloading", "error", "idle"],
  confirming: ["confirming", "downloading", "error", "idle"],
  downloading: ["downloading", "complete", "error", "idle"],
  complete: ["complete", "idle"],
  error: ["error", "idle"],
};

/** Reject impossible late events, including a second start during a download. */
export function transition(current: DownloadState, next: DownloadState): DownloadState {
  return transitions[current.phase].includes(next.phase) ? next : current;
}

/** IPC is a trust boundary too: never render arbitrary event objects. */
export function isDownloadState(value: unknown): value is DownloadState {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Record<string, unknown>;
  if (typeof candidate.phase !== "string" || !Object.hasOwn(transitions, candidate.phase)) return false;
  for (const field of ["filename", "message"]) {
    if (candidate[field] !== undefined && typeof candidate[field] !== "string") return false;
  }
  for (const field of ["bytes", "total"]) {
    const size = candidate[field];
    if (size !== undefined && (typeof size !== "number" || !Number.isSafeInteger(size) || size < 0)) return false;
  }
  return true;
}

export function isActive(phase: Phase): boolean {
  return phase === "discovering" || phase === "confirming" || phase === "downloading";
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${Math.ceil(bytes / 1024)} KiB`;
  if (bytes < 1024 ** 3) return `${Math.ceil(bytes / 1024 ** 2)} MiB`;
  return `${(bytes / 1024 ** 3).toFixed(1)} GiB`;
}
