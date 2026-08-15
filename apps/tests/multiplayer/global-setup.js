// @ts-check
// Global setup: seed an ISOLATED org + boot the whole stack.
//
//   1. throwaway TASK_DATA_ROOT (mktemp) — never the dev data root
//   2. `task org init` + 4 dev-account signups via the CLI binary
//      (the same 4 accounts `crates/task/ui/src/auth.rs::DEV_ACCOUNTS`
//      pre-fills in the web switcher)
//   3. seed vault notes (the collab note + a wikilink target)
//   4. spawn `task-server` on MP_SERVER_PORT (default 18091)
//   5. spawn the static SPA server (serve.js) on MP_WEB_PORT
//      (default 18092) hosting the dx-built bundle
//
// Fails fast (with instructions) when artifacts are missing or the
// wasm bundle was baked against the wrong vox URL — that's the #1
// way to silently test against the WRONG (dev) server.
//
// State handoff to specs/teardown: .mp-state.json next to this file.

const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawn, execFileSync } = require("child_process");

// The monorepo root — this suite lives at apps/task/tests/multiplayer,
// so it is four levels up. It was two when Task was its own repo; the
// 2026-07-10 subtree import moved the root without moving this, which
// is why every artifact path resolved under apps/task/ and the suite
// could not find a single binary.
const REPO_ROOT = path.resolve(__dirname, "..", "..", "..", "..");
const SERVER_PORT = parseInt(process.env.MP_SERVER_PORT || "18091", 10);
const WEB_PORT = parseInt(process.env.MP_WEB_PORT || "18092", 10);
// Per-run unique slug: a leaked page from a PREVIOUS run (a forgotten
// browser tab on the static-web port) keeps its reconnect supervisor
// dialing MP_SERVER_PORT forever and — with a fixed slug — joins the
// fresh run's org as a live, never-expiring presence peer ("ghost
// roster rows" that no timeout removes, because the tab heartbeats).
// A unique org per run strands such zombies in an org that no longer
// exists.
const ORG_SLUG = `mptest-${Date.now().toString(36)}${Math.floor(Math.random() * 1296).toString(36).padStart(2, "0")}`;
const STATE_FILE = path.join(__dirname, ".mp-state.json");

// Mirror of crates/task/ui/src/auth.rs::DEV_ACCOUNTS — keep in sync.
const DEV_ACCOUNTS = [
  { email: "cody@fasttrackstudios.com", password: "dev-cody-2026", name: "Cody Wright", username: "cody" },
  { email: "carter@fasttrackstudios.com", password: "dev-carter-2026", name: "Carter Whitlock", username: "carter" },
  { email: "tom@fasttrackstudios.com", password: "dev-tom-2026", name: "Tom Brooks", username: "tom" },
  { email: "guest@fasttrackstudios.com", password: "dev-guest-2026", name: "Guest", username: "guest" },
];

/** The note the convergence suite edits. Re-seeded per run. */
const COLLAB_NOTE = "Collab Test.md";
/** Wikilink target for the `[[` autocomplete flow. */
const LINK_NOTE = "Link Target.md";

function waitForHttp(url, timeoutMs) {
  const start = Date.now();
  return new Promise((resolve, reject) => {
    const tick = () => {
      fetch(url)
        .then((r) => (r.ok ? resolve(undefined) : retry()))
        .catch(retry);
      function retry() {
        if (Date.now() - start > timeoutMs) {
          reject(new Error(`timed out waiting for ${url}`));
        } else {
          setTimeout(tick, 300);
        }
      }
    };
    tick();
  });
}

/** Reject when something already listens on `port` — a stale server
 * there would be silently tested instead of OUR seeded stack. */
async function assertPortFree(port, what) {
  const inUse = await fetch(`http://127.0.0.1:${port}/`, { signal: AbortSignal.timeout(1500) })
    .then(() => true)
    .catch(() => false);
  if (inUse) {
    throw new Error(
      `port ${port} (${what}) is already in use — a previous run leaked, or you picked the dev ` +
        `server's port. Kill the squatter or set MP_SERVER_PORT/MP_WEB_PORT (the wasm bundle must ` +
        `be rebuilt if MP_SERVER_PORT changes).`,
    );
  }
}

module.exports = async () => {
  // ── artifact checks ────────────────────────────────────────────
  const serverBin = path.join(REPO_ROOT, "target", "debug", "task-server");
  const cliBin = path.join(REPO_ROOT, "target", "debug", "task");
  const publicDir = path.join(REPO_ROOT, "target", "dx", "task-app-web", "debug", "web", "public");
  for (const p of [serverBin, cliBin]) {
    if (!fs.existsSync(p)) {
      throw new Error(`missing ${p} — run ./run.sh (it builds task-server + task-cli first)`);
    }
  }
  const wasmPath = path.join(publicDir, "wasm", "task-app-web_bg.wasm");
  if (!fs.existsSync(wasmPath)) {
    throw new Error(`missing wasm bundle at ${wasmPath} — run ./run.sh (dx build with TASK_VOX_URL_WEB)`);
  }
  const expectedUrl = `ws://127.0.0.1:${SERVER_PORT}/vox`;
  const wasmBytes = fs.readFileSync(wasmPath);
  if (!wasmBytes.includes(Buffer.from(expectedUrl))) {
    throw new Error(
      `wasm bundle was not built with TASK_VOX_URL_WEB=${expectedUrl} ` +
        `(the vox URL is baked at BUILD time — rebuild via ./run.sh, ` +
        `or set MP_SERVER_PORT to the port the bundle was baked with)`,
    );
  }
  if (!wasmBytes.includes(Buffer.from("__taskVault"))) {
    throw new Error(
      "wasm bundle predates the __taskVault test hook (crates/task/ui/src/pages/note_view.rs) — rebuild via ./run.sh",
    );
  }
  // A bundle built OUTSIDE the nix dev shell links the tree-sitter
  // grammars as unresolved `env` imports and the app fails to load.
  const glue = fs.readFileSync(path.join(publicDir, "wasm", "task-app-web.js"), "utf8");
  if (glue.includes('from "env"')) {
    throw new Error(
      "wasm glue imports from \"env\" (tree-sitter C grammars didn't compile for wasm) — " +
        "rebuild inside the dev shell: nix develop --command tests/multiplayer/run.sh",
    );
  }

  await assertPortFree(SERVER_PORT, "task-server");
  await assertPortFree(WEB_PORT, "static web");

  // ── throwaway data root + seeded org ───────────────────────────
  const dataRoot = fs.mkdtempSync(path.join(os.tmpdir(), "task-mp-"));
  const xdgData = path.join(dataRoot, "xdg");
  fs.mkdirSync(xdgData, { recursive: true });
  const cliEnv = {
    ...process.env,
    TASK_DATA_ROOT: dataRoot,
    // Redirect the CLI's session.json so seeding NEVER touches the
    // developer's real ~/.local/share/task session.
    XDG_DATA_HOME: xdgData,
  };
  const cli = (args) => execFileSync(cliBin, args, { env: cliEnv, stdio: "pipe" }).toString();

  cli(["org", "init", ORG_SLUG, "--name", "MP Conformance", "--home"]);

  const vaultDir = path.join(dataRoot, "orgs", ORG_SLUG, "vault");
  fs.mkdirSync(vaultDir, { recursive: true });
  // Content is overwritten by the convergence spec before peers open
  // the note; this seeds the folder index.
  fs.writeFileSync(path.join(vaultDir, COLLAB_NOTE), "# Collab Test\n\nseed.\n");
  fs.writeFileSync(path.join(vaultDir, LINK_NOTE), "# Link Target\n\nLinked from the storm.\n");

  // ── task-server ────────────────────────────────────────────────
  const serverLog = fs.openSync(path.join(dataRoot, "server.log"), "a");
  const server = spawn(serverBin, [], {
    env: {
      ...process.env,
      TASK_DATA_ROOT: dataRoot,
      TASK_SERVER_BIND: `127.0.0.1:${SERVER_PORT}`,
      RUST_LOG: process.env.MP_SERVER_LOG || "task_server=info",
    },
    stdio: ["ignore", serverLog, serverLog],
    detached: false,
  });
  await waitForHttp(`http://127.0.0.1:${SERVER_PORT}/.well-known/task-server.json`, 30_000);

  // Dev-account signups go over the wire: the CLI's sqlite-direct
  // signup was removed with the remote-login rework (sessions are
  // server-aware now), so accounts are created via the org's
  // AuthService once the server hosts the seeded org.
  for (const a of DEV_ACCOUNTS) {
    cli([
      "auth", "signup", "--org", ORG_SLUG,
      "--server", `ws://127.0.0.1:${SERVER_PORT}/vox`,
      "--email", a.email, "--password", a.password,
      "--username", a.username, "--name", a.name,
    ]);
  }

  // ── static web server ──────────────────────────────────────────
  const webLog = fs.openSync(path.join(dataRoot, "web.log"), "a");
  const web = spawn(process.execPath, [path.join(__dirname, "serve.js"), publicDir, String(WEB_PORT)], {
    stdio: ["ignore", webLog, webLog],
    detached: false,
  });
  await waitForHttp(`http://127.0.0.1:${WEB_PORT}/`, 15_000);

  const state = {
    dataRoot,
    vaultDir,
    orgSlug: ORG_SLUG,
    collabNote: COLLAB_NOTE,
    linkNote: LINK_NOTE,
    serverPort: SERVER_PORT,
    webPort: WEB_PORT,
    serverPid: server.pid,
    webPid: web.pid,
    serverLog: path.join(dataRoot, "server.log"),
  };
  fs.writeFileSync(STATE_FILE, JSON.stringify(state, null, 2));
  process.env.MP_STATE_FILE = STATE_FILE;
  console.log(`[mp-setup] org '${ORG_SLUG}' seeded at ${dataRoot}`);
  console.log(`[mp-setup] task-server :${SERVER_PORT} (pid ${server.pid}), web :${WEB_PORT} (pid ${web.pid})`);
};
