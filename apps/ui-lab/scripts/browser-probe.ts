/**
 * Headless time-to-first-data probe for the lab, driven over raw CDP
 * (no puppeteer/playwright dep; node >= 22 global WebSocket talks to
 * chromium's DevTools socket directly — plain `--dump-dom` stalls
 * websockets, so we drive a real page and poll the DOM).
 *
 *   pnpm vite-node scripts/browser-probe.ts [url]
 *
 * Measures a COLD load and then a REFRESH (Page.reload — the case the
 * 15s symptom was reported against), polling until project rows are
 * painted, then dumps the in-page vox telemetry ring buffer
 * (src/lib/telemetry.ts) for each run.
 */
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const PAGE_URL =
  process.argv[2] ?? "http://localhost:5173/projects?debug=vox";
const PAINT_TIMEOUT_MS = Number(process.env.PAINT_TIMEOUT_MS ?? 40_000);

interface CdpMessage {
  id?: number;
  method?: string;
  params?: Record<string, unknown>;
  sessionId?: string;
  result?: Record<string, unknown>;
  error?: { message: string };
}

class Cdp {
  private ws: WebSocket;
  private nextId = 1;
  private pending = new Map<
    number,
    { resolve: (r: Record<string, unknown>) => void; reject: (e: Error) => void }
  >();
  onEvent: ((msg: CdpMessage) => void) | null = null;

  private constructor(ws: WebSocket) {
    this.ws = ws;
    ws.addEventListener("message", (ev: MessageEvent) => {
      const msg = JSON.parse(String(ev.data)) as CdpMessage;
      if (msg.id !== undefined) {
        const entry = this.pending.get(msg.id);
        if (entry) {
          this.pending.delete(msg.id);
          if (msg.error) entry.reject(new Error(msg.error.message));
          else entry.resolve(msg.result ?? {});
        }
      } else if (msg.method) {
        this.onEvent?.(msg);
      }
    });
  }

  static async connect(url: string): Promise<Cdp> {
    const ws = new WebSocket(url);
    await new Promise<void>((resolve, reject) => {
      ws.addEventListener("open", () => resolve(), { once: true });
      ws.addEventListener("error", () => reject(new Error(`CDP connect failed: ${url}`)), { once: true });
    });
    return new Cdp(ws);
  }

  send(
    method: string,
    params: Record<string, unknown> = {},
    sessionId?: string,
  ): Promise<Record<string, unknown>> {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params, sessionId }));
    });
  }

  close(): void {
    this.ws.close();
  }
}

function launchChromium(profileDir: string): Promise<{ proc: ChildProcess; wsUrl: string }> {
  const proc = spawn(
    "chromium",
    [
      "--headless=new",
      "--remote-debugging-port=0",
      `--user-data-dir=${profileDir}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-gpu",
      "--no-sandbox",
      "about:blank",
    ],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  return new Promise((resolve, reject) => {
    let stderr = "";
    const onData = (chunk: Buffer): void => {
      stderr += chunk.toString();
      const m = stderr.match(/DevTools listening on (ws:\/\/\S+)/);
      if (m) {
        proc.stderr?.off("data", onData);
        resolve({ proc, wsUrl: m[1] });
      }
    };
    proc.stderr?.on("data", onData);
    proc.on("exit", (code) =>
      reject(new Error(`chromium exited early (code ${code}): ${stderr}`)),
    );
    setTimeout(() => reject(new Error(`chromium DevTools url not found:\n${stderr}`)), 15_000);
  });
}

interface PerfEvent {
  t: number;
  name: string;
  detail: string;
  durMs?: number;
}

async function evalJson<T>(
  cdp: Cdp,
  sessionId: string,
  expression: string,
): Promise<T> {
  const r = await cdp.send(
    "Runtime.evaluate",
    { expression, returnByValue: true, awaitPromise: true },
    sessionId,
  );
  const res = r.result as { value?: T; description?: string } | undefined;
  return res?.value as T;
}

function printEvents(label: string, events: PerfEvent[]): void {
  console.log(`\n=== telemetry: ${label} (${events.length} events) ===`);
  const t0 = events[0]?.t ?? 0;
  let prev = t0;
  for (const e of events) {
    const at = `+${(e.t - t0).toFixed(1)}ms`.padStart(11);
    const delta = `Δ${(e.t - prev).toFixed(1)}ms`.padStart(11);
    const dur = e.durMs !== undefined ? ` dur=${e.durMs.toFixed(1)}ms` : "";
    console.log(`${at} ${delta}  ${e.name}${dur}  ${e.detail}`);
    prev = e.t;
  }
}

async function waitForData(
  cdp: Cdp,
  sessionId: string,
  label: string,
): Promise<number> {
  const start = Date.now();
  for (;;) {
    const elapsed = Date.now() - start;
    if (elapsed > PAINT_TIMEOUT_MS) {
      console.log(`!! ${label}: TIMED OUT after ${elapsed}ms without painted data`);
      return elapsed;
    }
    // "Data painted" = at least one successful RPC recorded AND no
    // loading skeletons left in the DOM (works for both lab routes).
    const state = await evalJson<
      { ok: number; skeletons: number; old: boolean } | undefined
    >(
      cdp,
      sessionId,
      `({ ok: (window.voxPerf ? window.voxPerf.events() : []).filter(e => e.name === 'rpc.end' && e.detail.includes('→ ok')).length, skeletons: document.querySelectorAll('[data-slot="skeleton"]').length, old: !!window.__probeOldDoc })`,
    ).catch(() => undefined);
    // `old` guards the reload case: don't count paint from the
    // pre-reload document.
    if (state && !state.old && state.ok > 0 && state.skeletons === 0) {
      const ms = Date.now() - start;
      console.log(
        `${label}: data painted in ${ms}ms (${state.ok} ok RPC(s), 0 skeletons)`,
      );
      return ms;
    }
    await new Promise((r) => setTimeout(r, 50));
  }
}

async function dumpTelemetry(cdp: Cdp, sessionId: string, label: string): Promise<void> {
  const events = await evalJson<PerfEvent[]>(
    cdp,
    sessionId,
    `(window.voxPerf ? window.voxPerf.events() : [])`,
  );
  printEvents(label, events ?? []);
  // Telemetry starts at main.tsx; navigation timing covers the window
  // before that (network, vite transforms, module exec).
  const nav = await evalJson<Record<string, number> | undefined>(
    cdp,
    sessionId,
    `(() => { const n = performance.getEntriesByType('navigation')[0]; return n && { fetchStart: n.fetchStart, responseEnd: n.responseEnd, domContentLoaded: n.domContentLoadedEventEnd, loadEvent: n.loadEventEnd, resources: performance.getEntriesByType('resource').length }; })()`,
  );
  if (nav) {
    console.log(
      `  nav: fetchStart=${nav.fetchStart?.toFixed(0)}ms responseEnd=${nav.responseEnd?.toFixed(0)}ms DCL=${nav.domContentLoaded?.toFixed(0)}ms load=${nav.loadEvent?.toFixed(0)}ms resources=${nav.resources}`,
    );
  }
  const overlay = await evalJson<boolean>(
    cdp,
    sessionId,
    `!!document.getElementById('vox-perf-overlay')`,
  );
  console.log(`  debug overlay (?debug=vox): ${overlay ? "mounted" : "absent"}`);
}

async function main(): Promise<void> {
  const profileDir = mkdtempSync(join(tmpdir(), "ui-lab-probe-"));
  const { proc, wsUrl } = await launchChromium(profileDir);
  const cdp = await Cdp.connect(wsUrl);

  try {
    const { targetId } = (await cdp.send("Target.createTarget", {
      url: "about:blank",
    })) as { targetId: string };
    const { sessionId } = (await cdp.send("Target.attachToTarget", {
      targetId,
      flatten: true,
    })) as { sessionId: string };

    await cdp.send("Page.enable", {}, sessionId);
    await cdp.send("Runtime.enable", {}, sessionId);
    cdp.onEvent = (msg) => {
      if (msg.method === "Runtime.consoleAPICalled") {
        const p = msg.params as {
          type: string;
          args: Array<{ value?: unknown; description?: string }>;
        };
        if (p.type === "error" || p.type === "warning") {
          const text = p.args
            .map((a) => a.value ?? a.description ?? "")
            .join(" ");
          console.log(`  [console.${p.type}] ${text}`);
        }
      }
    };

    console.log(`navigating to ${PAGE_URL}`);
    await cdp.send("Page.navigate", { url: PAGE_URL }, sessionId);
    const coldMs = await waitForData(cdp, sessionId, "COLD LOAD");
    await dumpTelemetry(cdp, sessionId, "cold load");

    // Let the session settle the way a user's would before they hit refresh.
    await new Promise((r) => setTimeout(r, 1_000));

    const refreshCount = Number(process.env.REFRESH_COUNT ?? 1);
    let worstReload = 0;
    let worstEvents: PerfEvent[] = [];
    for (let i = 1; i <= refreshCount; i++) {
      console.log(`\nreloading page (${i}/${refreshCount})`);
      // Tag the outgoing document so waitForData only accepts the new one.
      await evalJson(cdp, sessionId, `(window.__probeOldDoc = true)`);
      await cdp.send("Page.reload", { ignoreCache: false }, sessionId);
      const ms = await waitForData(cdp, sessionId, `REFRESH ${i}`);
      if (ms >= worstReload) {
        worstReload = ms;
        worstEvents =
          (await evalJson<PerfEvent[]>(
            cdp,
            sessionId,
            `(window.voxPerf ? window.voxPerf.events() : [])`,
          )) ?? [];
      }
      await new Promise((r) => setTimeout(r, 300));
    }
    printEvents(`worst refresh (${worstReload}ms)`, worstEvents);
    await dumpTelemetry(cdp, sessionId, "last refresh");

    console.log(
      `\nsummary: cold=${coldMs}ms worst-refresh=${worstReload}ms over ${refreshCount} refresh(es) (budget: <2000ms each)`,
    );
    if (coldMs > 2_000 || worstReload > 2_000) process.exitCode = 1;
  } finally {
    cdp.close();
    proc.kill("SIGKILL");
    rmSync(profileDir, { recursive: true, force: true });
  }
}

main().catch((e) => {
  console.error("probe FAILED:", e);
  process.exit(1);
});
