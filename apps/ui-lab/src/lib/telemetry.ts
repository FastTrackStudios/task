/**
 * Lightweight vox/network telemetry for the lab.
 *
 * Always-on and cheap: every event lands in a fixed-size ring buffer
 * (no DOM, no formatting). Inspection is on demand:
 *
 *   - `window.voxPerf.table()`  — console.table of the buffer
 *   - `?debug=vox`              — tiny fixed overlay (bottom-right)
 *     listing events with ms deltas, updated as they arrive
 *
 * What gets timestamped:
 *   - WebSocket lifecycle (construct → open/error/close, with close
 *     code+reason) via a constructor wrapper installed over
 *     `globalThis.WebSocket` — this is the only way to see WHY a
 *     connect failed, since the vox ws transport collapses error and
 *     close into "failed to connect".
 *   - vox connect/handshake spans + RPC call start/end/outcome, fed
 *     from src/lib/vox.ts (telemetry middleware + spans).
 *   - vox internals (session reconnect attempts and their errors)
 *     via `setVoxLogger` — vox-core is silent by default; this bridge
 *     surfaces the otherwise-swallowed retry errors.
 */
import { setVoxLogger, type ClientMiddleware, type ClientContext, type CallRequest, type CallOutcome } from "@bearcove/vox-core";

export interface VoxTelemetryEvent {
  /** ms since timeOrigin (performance.now()). */
  t: number;
  name: string;
  detail: string;
  /** Span duration in ms, when the event closes a span. */
  durMs?: number;
}

const CAPACITY = 512;
const buffer: (VoxTelemetryEvent | undefined)[] = new Array(CAPACITY);
let head = 0; // next write slot
let total = 0;

type Listener = () => void;
const listeners = new Set<Listener>();

const now = (): number =>
  typeof performance !== "undefined" ? performance.now() : Date.now();

export function record(name: string, detail = "", durMs?: number): void {
  buffer[head] = { t: now(), name, detail, durMs };
  head = (head + 1) % CAPACITY;
  total++;
  for (const l of listeners) l();
}

/** Start a span; the returned function records `name` with its duration. */
export function span(name: string, detail = ""): (endDetail?: string) => void {
  const start = now();
  record(`${name}.start`, detail);
  return (endDetail?: string) => {
    record(`${name}.end`, endDetail ?? detail, now() - start);
  };
}

/** Snapshot of buffered events, oldest first. */
export function events(): VoxTelemetryEvent[] {
  const out: VoxTelemetryEvent[] = [];
  const count = Math.min(total, CAPACITY);
  for (let i = 0; i < count; i++) {
    const e = buffer[(head - count + i + CAPACITY) % CAPACITY];
    if (e) out.push(e);
  }
  return out;
}

export function clear(): void {
  buffer.fill(undefined);
  head = 0;
  total = 0;
  for (const l of listeners) l();
}

function rows(): Array<Record<string, string | number>> {
  const evs = events();
  const t0 = evs[0]?.t ?? 0;
  let prev = t0;
  return evs.map((e) => {
    const row = {
      "+ms": Math.round((e.t - t0) * 10) / 10,
      "Δms": Math.round((e.t - prev) * 10) / 10,
      event: e.name,
      detail: e.detail,
      dur: e.durMs !== undefined ? Math.round(e.durMs * 10) / 10 : "",
    };
    prev = e.t;
    return row;
  });
}

/** Dump the ring buffer with console.table. */
export function table(): void {
  // eslint-disable-next-line no-console
  console.table(rows());
}

export function errorMessage(e: unknown): string {
  if (e instanceof Error) return `${e.name}: ${e.message}`;
  return String(e);
}

// ---------------------------------------------------------------------------
// RPC middleware — per-call start/end/outcome.
// ---------------------------------------------------------------------------

const RPC_START = Symbol("voxPerf.rpcStart");

export const telemetryMiddleware: ClientMiddleware = {
  pre(ctx: ClientContext, request: CallRequest): void {
    ctx.extensions.set(RPC_START, now());
    record("rpc.start", request.method);
  },
  post(ctx: ClientContext, request: CallRequest, outcome: CallOutcome): void {
    const start = ctx.extensions.get<number>(RPC_START);
    const durMs = start !== undefined ? now() - start : undefined;
    const verdict = outcome.ok ? "ok" : `error: ${errorMessage(outcome.error)}`;
    record("rpc.end", `${request.method} → ${verdict}`, durMs);
  },
};

// ---------------------------------------------------------------------------
// WebSocket lifecycle — wrap the global constructor so every socket
// (vox AND vite HMR — the HMR ws is a suspect in dev-server races)
// reports construct/open/error/close with timing and close codes.
// ---------------------------------------------------------------------------

let wsInstalled = false;
let wsSeq = 0;

export function instrumentWebSocket(): void {
  if (wsInstalled || typeof globalThis.WebSocket !== "function") return;
  wsInstalled = true;

  const Native = globalThis.WebSocket;

  const Wrapped = function (
    this: WebSocket,
    url: string | URL,
    protocols?: string | string[],
  ): WebSocket {
    const id = `ws#${++wsSeq}`;
    const label = String(url);
    const start = now();
    record("ws.new", `${id} ${label}`);
    const ws =
      protocols === undefined ? new Native(url) : new Native(url, protocols);
    ws.addEventListener("open", () => {
      record("ws.open", `${id} ${label}`, now() - start);
    });
    ws.addEventListener("error", () => {
      // Browser error events carry no reason (security); the close
      // event's code is the real signal.
      record("ws.error", `${id} ${label}`, now() - start);
    });
    ws.addEventListener("close", (ev: CloseEvent) => {
      record(
        "ws.close",
        `${id} code=${ev.code}${ev.reason ? ` reason=${ev.reason}` : ""}${ev.wasClean ? " clean" : " unclean"} ${label}`,
        now() - start,
      );
    });
    return ws;
  } as unknown as typeof WebSocket;

  // Preserve statics (CONNECTING/OPEN/...) and prototype so
  // `instanceof WebSocket` and `WebSocket.OPEN` keep working.
  Wrapped.prototype = Native.prototype;
  Object.setPrototypeOf(Wrapped, Native);
  globalThis.WebSocket = Wrapped;
}

// ---------------------------------------------------------------------------
// vox-core internal logs (reconnect attempts + their errors).
// ---------------------------------------------------------------------------

let loggerInstalled = false;

export function instrumentVoxLogger(): void {
  if (loggerInstalled) return;
  loggerInstalled = true;
  setVoxLogger({
    debug(msg: string, ...args: unknown[]) {
      record("vox.debug", [msg, ...args.map(String)].join(" "));
    },
    error(msg: string, ...args: unknown[]) {
      record("vox.error", [msg, ...args.map(String)].join(" "));
    },
  });
}

// ---------------------------------------------------------------------------
// Debug overlay (?debug=vox) — render-only-when-asked; the buffer
// itself is always recording.
// ---------------------------------------------------------------------------

const OVERLAY_ROWS = 28;

function overlayEnabled(): boolean {
  if (typeof location === "undefined") return false;
  return new URLSearchParams(location.search).get("debug") === "vox";
}

function mountOverlay(): void {
  const el = document.createElement("div");
  el.id = "vox-perf-overlay";
  el.style.cssText = [
    "position:fixed",
    "bottom:8px",
    "right:8px",
    "z-index:99999",
    "max-width:560px",
    "max-height:45vh",
    "overflow-y:auto",
    "background:rgba(10,10,14,0.92)",
    "color:#d6e2f0",
    "font:10px/1.5 ui-monospace,monospace",
    "padding:6px 8px",
    "border:1px solid #334",
    "border-radius:6px",
    "pointer-events:auto",
    "white-space:pre",
  ].join(";");
  document.body.appendChild(el);

  let scheduled = false;
  const render = (): void => {
    scheduled = false;
    const evs = events();
    const t0 = evs[0]?.t ?? 0;
    const tail = evs.slice(-OVERLAY_ROWS);
    const lines = tail.map((e) => {
      const at = `+${(e.t - t0).toFixed(0)}ms`.padStart(9);
      const dur = e.durMs !== undefined ? ` (${e.durMs.toFixed(0)}ms)` : "";
      return `${at}  ${e.name}${dur}  ${e.detail}`;
    });
    el.textContent = `vox telemetry — ${evs.length} event(s)\n${lines.join("\n")}`;
    el.scrollTop = el.scrollHeight;
  };
  const schedule = (): void => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(render);
  };
  listeners.add(schedule);
  schedule();
}

/**
 * Install everything. Call once, as early as possible (before any
 * WebSocket is constructed) — main.tsx imports this first.
 */
export function installTelemetry(): void {
  instrumentWebSocket();
  instrumentVoxLogger();
  record("page.boot", typeof document !== "undefined" ? document.readyState : "node");

  if (typeof window !== "undefined") {
    (window as unknown as Record<string, unknown>).voxPerf = {
      table,
      events,
      clear,
      record,
    };
    if (overlayEnabled()) {
      if (document.body) mountOverlay();
      else addEventListener("DOMContentLoaded", mountOverlay, { once: true });
    }
  }
}
