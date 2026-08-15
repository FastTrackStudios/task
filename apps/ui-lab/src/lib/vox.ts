/**
 * Lazy per-org vox clients against the live task-server.
 *
 * The server hosts several orgs, each behind its own vox mount
 * (`/org/<slug>/vox`) with one LayerRouter dispatching every feature
 * service by method id. The lab can look at ANY of them (see
 * lib/orgs.tsx — the switcher fans queries out across orgs), so each
 * generated client is cached per `(service, org)` in a Map rather
 * than as a single module-level singleton.
 *
 * In the browser the socket is opened same-origin and vite's dev/
 * preview proxy forwards it to the task-server (see vite.config.ts —
 * Chrome's Local Network Access checks stall cross-origin ws:// to
 * loopback). Under node (scripts/smoke.ts) there is no proxy, so it
 * dials the server directly. Override either with `VITE_TASK_SERVER` /
 * `VITE_TASK_ORG`.
 */
import {
  MiddlewareCaller,
  session,
  voxServiceMetadata,
  type Caller,
} from "@bearcove/vox-core";
import { wsConnector } from "@bearcove/vox-ws";

import { AuthServiceClient } from "@/generated/authservice.generated";
import { MilestoneServiceRpcClient } from "@/generated/milestoneservicerpc.generated";
import { ProjectServiceRpcClient } from "@/generated/projectservicerpc.generated";
import { TaskServiceRpcClient } from "@/generated/taskservicerpc.generated";
import { TaskServiceStreamClient } from "@/generated/taskservicestream.generated";
import { WorkstreamServiceRpcClient } from "@/generated/workstreamservicerpc.generated";
import { WorkstreamServiceStreamClient } from "@/generated/workstreamservicestream.generated";
import {
  errorMessage,
  installTelemetry,
  record,
  span,
  telemetryMiddleware,
} from "./telemetry";

// Before any WebSocket exists (idempotent; main.tsx also installs for
// the browser, this covers node/smoke).
installTelemetry();

export const SERVER: string =
  (import.meta.env.VITE_TASK_SERVER as string | undefined) ??
  (typeof location !== "undefined"
    ? location.origin.replace(/^http/, "ws") // browser: same-origin, proxied
    : "ws://127.0.0.1:18080"); // node smoke: direct

/** HTTP(S) origin for plain fetches (org discovery) — `ws→http`. */
export const SERVER_HTTP: string = SERVER.replace(/^ws/, "http");

/**
 * Fallback org before discovery resolves (and the org the node smoke
 * targets). The browser app itself never hardcodes an org — the
 * switcher in lib/orgs.tsx decides what to fetch.
 */
export const DEFAULT_ORG: string =
  (import.meta.env.VITE_TASK_ORG as string | undefined) ?? "codywright";

/** Per-org vox WebSocket endpoint. */
export function voxUrlFor(org: string): string {
  return `${SERVER}/org/${org}/vox`;
}

/**
 * Initial-connect policy. The vox runtime's ReconnectPolicy only
 * covers RESUMABLE sessions after a drop — the FIRST connect is a
 * single attempt with no timeout: a ws upgrade that stalls (dev-proxy
 * racing a page reload, dev-server restart mid-flight, Chrome LNA
 * stalling a socket) hangs `connect*` forever with zero signal, and a
 * fast first-attempt failure surfaces only through the query layer's
 * slow retry. So we bound each attempt and retry quickly with jitter:
 * a racing first attempt costs ~250ms, worst case ~1s — not 15s.
 */
const CONNECT_ATTEMPT_TIMEOUT_MS = 3_000;
const CONNECT_MAX_ATTEMPTS = 3;
const CONNECT_RETRY_BASE_MS = 250;

/**
 * Bound an attempt and, when we abandon it, dispose whatever it later
 * yields — a stalled ws upgrade can complete long after we've retried
 * (observed: five queued sockets all opening at +17s when a blocked
 * dev-server event loop flushed), and each orphan is a live session
 * that would keepalive until tab close.
 */
function attemptTimeout<T>(
  promise: Promise<T>,
  ms: number,
  disposeLate: (v: T) => void,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    let abandoned = false;
    const timer = setTimeout(() => {
      abandoned = true;
      reject(new Error(`connect attempt timed out after ${ms}ms`));
    }, ms);
    promise.then(
      (v) => {
        clearTimeout(timer);
        if (abandoned) {
          disposeLate(v);
        } else {
          resolve(v);
        }
      },
      (e) => {
        clearTimeout(timer);
        if (!abandoned) reject(e);
      },
    );
  });
}

/**
 * Instrumented mirror of the generated `connect*` helpers (which are
 * two-liners over `session.initiator` + the client class). Mirrored
 * here because the generated signature accepts SessionTransportOptions
 * but NO client middleware, NO initial-connect retry/timeout config,
 * and NO way to observe connect phases — that's a vox-codegen gap
 * (connect* should take `middleware?: ClientMiddleware[]` and an
 * initial-connect policy); until then we build the caller ourselves so
 * telemetry sees connect spans, per-attempt errors, and RPC outcomes.
 */
async function connectInstrumented<T>(
  service: string,
  org: string,
  make: (caller: Caller) => T,
): Promise<T> {
  const url = voxUrlFor(org);
  const end = span(`vox.connect.${service}`, url);
  let lastError: unknown;
  for (let attempt = 1; attempt <= CONNECT_MAX_ATTEMPTS; attempt++) {
    try {
      const established = await attemptTimeout(
        session.initiator(wsConnector(url), {
          metadata: voxServiceMetadata(service),
        }),
        CONNECT_ATTEMPT_TIMEOUT_MS,
        // Late completion of an abandoned attempt: close the session so
        // the socket doesn't linger as a zombie connection.
        (late) => {
          record("vox.connect.late-dispose", `${service} @ ${org}`);
          (late as { close?: () => void }).close?.();
        },
      );
      end(attempt === 1 ? "connected" : `connected (attempt ${attempt})`);
      const caller = new MiddlewareCaller(
        established.rootConnection().caller(),
        [telemetryMiddleware],
      );
      return make(caller);
    } catch (e) {
      lastError = e;
      // THE key signal: why this attempt failed (pre-telemetry this
      // error was swallowed into a generic failure 15s later).
      record(
        "vox.connect.retry",
        `${service} @ ${org} attempt ${attempt}/${CONNECT_MAX_ATTEMPTS} failed: ${errorMessage(e)}`,
      );
      if (attempt < CONNECT_MAX_ATTEMPTS) {
        // 125-250ms, 250-500ms, ... (full jitter on a doubling base).
        const ceiling = CONNECT_RETRY_BASE_MS * 2 ** (attempt - 1);
        const delay = ceiling / 2 + Math.random() * (ceiling / 2);
        await new Promise((r) => setTimeout(r, delay));
      }
    }
  }
  end(`error: ${errorMessage(lastError)}`);
  throw lastError;
}

/**
 * Per-`(service, org)` client cache. Caches the *promise* so
 * concurrent callers share one in-flight connect; a failed connect
 * evicts itself so the next call retries instead of replaying the
 * cached rejection forever.
 */
function clientCache<T>(
  service: string,
  make: (caller: Caller) => T,
): (org: string) => Promise<T> {
  const byOrg = new Map<string, Promise<T>>();
  return (org: string) => {
    let client = byOrg.get(org);
    if (!client) {
      client = connectInstrumented(service, org, make).catch((e) => {
        byOrg.delete(org); // retry on next call instead of caching the failure
        throw e;
      });
      byOrg.set(org, client);
    }
    return client;
  };
}

export const projectsFor = clientCache(
  "ProjectServiceRpc",
  (caller) => new ProjectServiceRpcClient(caller),
);

export const tasksFor = clientCache(
  "TaskServiceRpc",
  (caller) => new TaskServiceRpcClient(caller),
);

export const milestonesFor = clientCache(
  "MilestoneServiceRpc",
  (caller) => new MilestoneServiceRpcClient(caller),
);

export const workstreamsFor = clientCache(
  "WorkstreamServiceRpc",
  (caller) => new WorkstreamServiceRpcClient(caller),
);

/**
 * `#[subscribe]` stream siblings — live change feeds. Subscribe by
 * creating a channel pair and handing the tx to the stream verb:
 *
 *   const [tx, rx] = channel<TaskEvent>();   // @bearcove/vox-core
 *   const stream = await taskStreamFor(org);
 *   await stream.events(tx);                  // resolves once attached
 *   for await (const ev of rx) { ... }        // Upserted / Deleted
 *
 * Fetch-once-then-fold contract: subscribe FIRST, then `list()`, then
 * fold events into the local copy (events are idempotent full-state
 * payloads, so re-applying one already in the fetched list is fine).
 */
export const taskStreamFor = clientCache(
  "TaskServiceStream",
  (caller) => new TaskServiceStreamClient(caller),
);

export const workstreamStreamFor = clientCache(
  "WorkstreamServiceStream",
  (caller) => new WorkstreamServiceStreamClient(caller),
);

export const authFor = clientCache(
  "AuthService",
  (caller) => new AuthServiceClient(caller),
);

/** The generated fallible methods return `{ok,value}|{ok,error}`. */
type VoxResult<T, E> = { ok: true; value: T } | { ok: false; error: E };

/** Unwrap a vox result, throwing a readable Error on the user-error arm. */
export function unwrap<T, E>(result: VoxResult<T, E>): T {
  if (result.ok) return result.value;
  throw new Error(formatVoxError(result.error));
}

function formatVoxError(error: unknown): string {
  if (error && typeof error === "object" && "tag" in error) {
    const e = error as { tag: string; value?: unknown };
    return e.value === undefined ? e.tag : `${e.tag}: ${String(e.value)}`;
  }
  return String(error);
}
