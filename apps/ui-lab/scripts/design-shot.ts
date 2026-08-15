/**
 * Headless design screenshots over raw CDP (same zero-dep approach as
 * browser-probe.ts). Drives the project overview on the test org's
 * Hermes Integration project and captures the Overview lens
 * (workstream strip expanded), the Agents lens (claimant kanban), and
 * the workstream detail route into design-shots/.
 *
 *   pnpm vite-node scripts/design-shot.ts [base-url]
 *
 * Expects a server (vite dev or preview) on base-url (default
 * http://127.0.0.1:4173) proxying to a live task-server.
 */
import { spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const BASE = process.argv[2] ?? "http://127.0.0.1:4173";
const ORG = process.env.SHOT_ORG ?? "test";
const PROJECT_TITLE = process.env.SHOT_PROJECT ?? "Hermes Integration";
const OUT_DIR = new URL("../design-shots/", import.meta.url).pathname;

interface CdpMessage {
  id?: number;
  method?: string;
  params?: Record<string, unknown>;
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
      }
    });
  }

  static async connect(url: string): Promise<Cdp> {
    const ws = new WebSocket(url);
    await new Promise<void>((resolve, reject) => {
      ws.addEventListener("open", () => resolve(), { once: true });
      ws.addEventListener(
        "error",
        () => reject(new Error(`CDP connect failed: ${url}`)),
        { once: true },
      );
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

function launchChromium(
  profileDir: string,
): Promise<{ proc: ChildProcess; wsUrl: string }> {
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
      "--window-size=1440,1200",
      "--force-device-scale-factor=2",
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
    setTimeout(
      () => reject(new Error(`chromium DevTools url not found:\n${stderr}`)),
      15_000,
    );
  });
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
  if ((r as { exceptionDetails?: unknown }).exceptionDetails) {
    throw new Error(
      `eval failed: ${JSON.stringify((r as { exceptionDetails: unknown }).exceptionDetails).slice(0, 400)}`,
    );
  }
  return (r.result as { value?: T } | undefined)?.value as T;
}

async function waitFor(
  cdp: Cdp,
  sessionId: string,
  expression: string,
  label: string,
  timeoutMs = 30_000,
): Promise<void> {
  const start = Date.now();
  for (;;) {
    if (await evalJson<boolean>(cdp, sessionId, expression).catch(() => false))
      return;
    if (Date.now() - start > timeoutMs)
      throw new Error(`timed out waiting for ${label}`);
    await new Promise((r) => setTimeout(r, 100));
  }
}

async function shoot(cdp: Cdp, sessionId: string, file: string): Promise<void> {
  // Let layout/fonts settle one frame.
  await new Promise((r) => setTimeout(r, 350));
  const shot = (await cdp.send(
    "Page.captureScreenshot",
    { format: "png", captureBeyondViewport: true },
    sessionId,
  )) as { data: string };
  const path = join(OUT_DIR, file);
  writeFileSync(path, Buffer.from(shot.data, "base64"));
  console.log(`wrote ${path}`);
}

async function main(): Promise<void> {
  mkdirSync(OUT_DIR, { recursive: true });
  const profileDir = mkdtempSync(join(tmpdir(), "ui-lab-shot-"));
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
    await cdp.send(
      "Emulation.setDeviceMetricsOverride",
      { width: 1440, height: 1200, deviceScaleFactor: 2, mobile: false },
      sessionId,
    );

    // Scope the org switcher to the test org before the app boots.
    await cdp.send(
      "Page.addScriptToEvaluateOnNewDocument",
      { source: `localStorage.setItem("ui-lab.org.selection", ${JSON.stringify(ORG)});` },
      sessionId,
    );

    // Find the project id via the projects list page.
    await cdp.send("Page.navigate", { url: `${BASE}/projects` }, sessionId);
    await waitFor(
      cdp,
      sessionId,
      `document.querySelectorAll('a[href^="/projects/${ORG}/"]').length > 0`,
      "project rows",
    );
    const href = await evalJson<string | null>(
      cdp,
      sessionId,
      `(() => {
        const a = [...document.querySelectorAll('a[href^="/projects/${ORG}/"]')]
          .find((el) => el.textContent?.includes(${JSON.stringify(PROJECT_TITLE)}));
        return a ? a.getAttribute("href") : null;
      })()`,
    );
    if (!href) throw new Error(`project "${PROJECT_TITLE}" not found in ${ORG}`);
    console.log(`project route: ${href}`);

    await shoot(cdp, sessionId, "projects-all-test-org.png");

    // Overview lens.
    await cdp.send("Page.navigate", { url: `${BASE}${href}` }, sessionId);
    await waitFor(
      cdp,
      sessionId,
      `document.querySelector('[data-slot="skeleton"]') === null && !!document.querySelector('h1')`,
      "overview painted",
    );
    await shoot(cdp, sessionId, "project-overview.png");

    // Expand the biggest workstream's swarm strip.
    const expanded = await evalJson<boolean>(
      cdp,
      sessionId,
      `(() => {
        // Workstream rows live inside <section>; the header's dropdown
        // triggers also carry aria-expanded, so scope past them.
        const btn = [...document.querySelectorAll('section button[aria-expanded="false"]')][0];
        if (!btn) return false;
        btn.click();
        return true;
      })()`,
    );
    if (expanded)
      await shoot(cdp, sessionId, "project-overview-workstream-expanded.png");

    // Grab a strip's detail-route link NOW — the Agents lens unmounts
    // the Overview tab content (and the strips with it).
    const wsHref = await evalJson<string | null>(
      cdp,
      sessionId,
      `(() => {
        const a = document.querySelector('a[href^="/workstreams/"]');
        return a ? a.getAttribute("href") : null;
      })()`,
    );

    // Agents lens.
    await evalJson(
      cdp,
      sessionId,
      `(() => {
        const tab = [...document.querySelectorAll('[role="tab"]')]
          .find((t) => t.textContent?.toLowerCase().includes("agents"));
        if (!tab) return false;
        // Radix tabs activate on mousedown — a bare .click() is ignored.
        for (const type of ["pointerdown", "mousedown", "pointerup", "mouseup", "click"]) {
          tab.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, view: window }));
        }
        return true;
      })()`,
    );
    await waitFor(
      cdp,
      sessionId,
      // Claimant kanban: columns are agent refs ("hermes@h4") or the
      // Unclaimed pool.
      `[...document.querySelectorAll('h3')].some((h) => /@|unclaimed/i.test(h.textContent ?? ""))`,
      "agents board",
    );
    await shoot(cdp, sessionId, "project-agents-board.png");

    // Workstream detail route — follow the strip link captured above.
    if (wsHref) {
      await cdp.send("Page.navigate", { url: `${BASE}${wsHref}` }, sessionId);
      await waitFor(
        cdp,
        sessionId,
        `document.querySelector('[data-slot="skeleton"]') === null && !!document.querySelector('h1')`,
        "workstream detail painted",
      );
      await shoot(cdp, sessionId, "workstream-detail.png");
    }

    console.log("design shots OK");
  } finally {
    cdp.close();
    proc.kill("SIGKILL");
    rmSync(profileDir, { recursive: true, force: true });
  }
}

main().catch((e) => {
  console.error("design-shot FAILED:", e);
  process.exit(1);
});
