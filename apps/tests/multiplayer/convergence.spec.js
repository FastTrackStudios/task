// @ts-check
// Suite 1 — 5-way editor convergence (tracked issue dd824506).
//
// Two tests:
//
//  1. "baseline" — two peers, light traffic (under the vox credit
//     window, see below): proves the whole stack converges TODAY:
//     editor → replica → server → disk → other peers, presence
//     cursors, no console errors.
//
//  2. "storm" — the full 5-context concurrent edit storm from the
//     PRD. History: was test.fail'd on vox downstream credit
//     starvation (every server→client stream froze at the 16-message
//     initial window — DEFAULT_INITIAL_CHANNEL_CREDIT — because the
//     wasm client's GrantCredit control sends were dropped futures).
//     Fixed in the vox fork (23acdc0a); the storm's remaining
//     failure after that was a signal-ownership violation in the
//     vault lazy-fetch path (root-scope spawn_forever writing the
//     page's editor signal), fixed via the page-owned fetch worker
//     in crates/task/ui/src/vault_lookup.rs.
//
// The offline/return scenario is a separate spec —
// offline-return.spec.js (skipped until P0 6303584a merges).

const fs = require("fs");
const path = require("path");
const { test, expect } = require("@playwright/test");
const { loadState, settle, DEV_ACCOUNTS, Peer, dumpArtifacts, killServer, startServer } = require("./helpers");

const NOTE_TITLE = "Collab Test";
const ROUNDS = 6;

// Seeded note: one marker line per peer so concurrent edits target
// disjoint regions (offsets still shift across peers — that's the
// storm). ASCII only: the tile-walk caret helper works in byte
// offsets and the tokens must be index-comparable in JS strings.
const DELETE_TARGET = "XDELETEMEXXX"; // 12 chars, removed 4-per-round
const REPLACE_TARGET = "REPLACE_TARGET_AAA";
const SEED = `# Collab Test

alpha: .
bravo: .
charlie: ${DELETE_TARGET}
delta: .
echo: ${REPLACE_TARGET}

tail.
`;

/** Re-seed the note and stand up n peers on it. */
async function bringUp(browser, n, state) {
  fs.writeFileSync(path.join(state.vaultDir, state.collabNote), SEED);
  const identities = [...DEV_ACCOUNTS, DEV_ACCOUNTS[0]].slice(0, n);
  const peers = identities.map((a, i) => new Peer(browser, { id: i, email: a.email, name: a.name }));
  for (const peer of peers) await peer.join("/vault");
  for (const peer of peers) await peer.openNote(NOTE_TITLE);
  await settle(
    async () => {
      const texts = await Promise.all(peers.map((p) => p.docText()));
      return texts.every((t) => t === SEED) ? texts[0] : null;
    },
    { timeout: 30_000, label: "all peers see the seeded note" },
  );
  return peers;
}

/** Quiesce: all buffers identical and stable. */
async function quiesce(peers, timeout = 90_000) {
  return settle(
    async () => {
      const texts = await Promise.all(peers.map((p) => p.docText()));
      if (texts.some((t) => t === null)) return null;
      return texts.every((t) => t === texts[0]) ? texts[0] : null;
    },
    { timeout, stableFor: 2_500, label: "all peer buffers identical" },
  );
}

test.describe("5-way editor convergence", () => {
  test.setTimeout(420_000);

  test("baseline: two peers converge bidirectionally (and on disk)", async ({ browser }, testInfo) => {
    const state = loadState();
    const notePath = path.join(state.vaultDir, state.collabNote);
    const peers = await bringUp(browser, 2, state);
    try {
      // Three edits each, interleaved — comfortably inside the vox
      // 16-message credit window so the known stream-freeze finding
      // (see header) doesn't mask basic conformance.
      for (let r = 0; r < 3; r++) {
        await Promise.all([
          peers[0].typeAtMarker("alpha:", ` (a${r})`),
          peers[1].typeAtMarker("bravo:", ` (b${r})`),
        ]);
        await Promise.all(peers.map((p) => p.pollRemoteCursors()));
        await new Promise((res) => setTimeout(res, 400));
        await Promise.all(peers.map((p) => p.pollRemoteCursors()));
      }

      const converged = await quiesce(peers, 30_000);
      for (const tokn of ["(a0)", "(a1)", "(a2)", "(b0)", "(b1)", "(b2)"]) {
        expect(converged.split(tokn).length - 1, `${tokn} exactly once`).toBe(1);
      }
      await settle(() => fs.readFileSync(notePath, "utf8") === converged, {
        timeout: 20_000,
        label: "note on disk equals the converged text",
      });
      for (const peer of peers) {
        const vault = await peer.vaultState();
        expect(vault?.live, `${peer.label} collab live`).toBe(true);
        expect(peer.sawRemoteCursor, `${peer.label} saw a remote cursor`).toBe(true);
        expect(peer.errors, `${peer.label} console errors:\n${peer.errors.join("\n")}`).toHaveLength(0);
      }
    } catch (err) {
      await dumpArtifacts(testInfo, peers, { suite: "convergence-baseline" });
      throw err;
    } finally {
      for (const peer of peers) await peer.leave().catch(() => {});
    }
  });

  // Regression guard for the 2026-06 signal-ownership bug: the collab
  // session's signals used to be created inside the keyed CollabSession
  // scope, so any session re-key (file switch, reconnect-generation
  // bump, Live→Offline teardown) dropped them while the still-mounted
  // Editor kept reading them from its keydown path — backspace and vim
  // died after the first re-key, with "Copy Value … not a descendant of
  // the owning scope" warnings flooding the console (now suite-fatal
  // via helpers.js isSignalOwnershipViolation). The handles are now
  // page-owned (crates/task/ui/src/collab.rs::use_collab_handles +
  // architect's crdt use_doc_slot/use_synced_doc_into split).
  test("input lifecycle: vim Normal boot, backspace round-trip, file-switch + reconnect re-key keep input alive", async ({ browser }, testInfo) => {
    const state = loadState();
    const peers = await bringUp(browser, 2, state);
    const [a, b] = peers;
    try {
      // ── vim: Normal mode active on load ──────────────────────
      // openNote's probe pressed `i` on the fresh page and recorded
      // whether it switched modes WITHOUT inserting — i.e. VimState
      // was mounted and booted in Normal mode, the vault default.
      for (const p of peers) {
        expect(p.vimNormalOnBoot, `${p.label} vim Normal mode on load`).toBe(true);
      }

      // ── backspace deletes locally and round-trips ────────────
      await a.typeAtMarker("alpha:", " DELME");
      await settle(async () => (await b.docText())?.includes("DELME"), {
        timeout: 20_000,
        label: "B sees A's insert before the backspaces",
      });
      await a.backspaceAtMarker("alpha:", " DELME".length);
      const afterBackspace = await quiesce(peers, 30_000);
      expect(afterBackspace, "backspace deletion converged on both peers").toBe(SEED);

      // ── file switch (collab session re-key #1 and #2) ────────
      // Away to another note and back: two keyed remounts of the
      // collab session while the SAME page (and its Editor closures)
      // stays alive. Input must survive both.
      await a.openNote("Link Target");
      await a.openNote(NOTE_TITLE);
      // (sentinel "ZZ": must not collide with SEED's XDELETEMEXXX)
      await a.typeAtMarker("alpha:", " (post-switch)ZZ");
      await a.backspaceAtMarker("alpha:", 2);
      const afterSwitch = await quiesce(peers, 30_000);
      expect(afterSwitch, "typing alive after file switch").toContain("(post-switch)");
      expect(afterSwitch, "backspace alive after file switch").not.toContain("ZZ");

      // ── reconnect-generation re-key (the original regression) ─
      // A real server restart (SIGKILL) severs every vox socket —
      // context.setOffline can't (Chromium keeps established
      // WebSockets alive under offline emulation). The supervised
      // connection detects the death → vault tears collab down →
      // reconnect bumps the generation → the open-effect re-arms a
      // fresh replica. Typing AND backspace must still work after.
      const notePath = path.join(state.vaultDir, state.collabNote);
      await settle(() => fs.readFileSync(notePath, "utf8") === afterSwitch, {
        timeout: 20_000,
        label: "disk caught up before the server restart",
      });
      for (const p of peers) p.expectedOutage = true;
      killServer();
      await settle(async () => {
        const v = await a.vaultState();
        return v && v.live !== true;
      }, { timeout: 30_000, label: "A's collab session torn down after socket death" });
      await startServer();
      await settle(async () => {
        const va = await a.vaultState();
        const vb = await b.vaultState();
        return va?.live === true && va?.status === "Live" && vb?.live === true && vb?.status === "Live";
      }, { timeout: 90_000, label: "both peers re-key to Live after the restart" });
      for (const p of peers) p.expectedOutage = false;
      await a.typeAtMarker("bravo:", " (post-rekey)YY");
      await a.backspaceAtMarker("bravo:", 2);
      const afterRekey = await quiesce(peers, 30_000);
      expect(afterRekey, "typing alive after reconnect re-key").toContain("(post-rekey)");
      expect(afterRekey, "backspace alive after reconnect re-key").not.toContain("YY");

      // ── console hygiene: zero errors INCLUDING the ownership
      //    warning class (suite-fatal via helpers.js) ────────────
      for (const peer of peers) {
        expect(peer.errors, `${peer.label} console errors:\n${peer.errors.join("\n")}`).toHaveLength(0);
      }
    } catch (err) {
      await dumpArtifacts(testInfo, peers, { suite: "convergence-input-lifecycle" });
      throw err;
    } finally {
      for (const peer of peers) await peer.leave().catch(() => {});
    }
  });

  test("storm: five concurrent editors converge everywhere (and on disk)", async ({ browser }, testInfo) => {
    // (Was test.fail'd — see the header. Credit fix + page-owned
    // vault fetch worker flipped it to passing.)
    const state = loadState();
    const notePath = path.join(state.vaultDir, state.collabNote);
    const peers = await bringUp(browser, 5, state);

    /** Tokens every peer types; asserted present exactly once. */
    const tokens = [];
    const tok = (s) => {
      tokens.push(s);
      return s;
    };

    try {
      // ── the storm ────────────────────────────────────────────
      // Each round fires all five peers' edits concurrently at
      // different document positions: p0 short chunks, p1 longer
      // chunks, p2 deletes (then types), p3 a `[[` autocomplete
      // wikilink (then types), p4 select-and-replace (then types).
      for (let r = 0; r < ROUNDS; r++) {
        const actions = [
          peers[0].typeAtMarker("alpha:", tok(` (a${r})`)),
          peers[1].typeAtMarker("bravo:", tok(` (b${r}-some-longer-chunk)`)),
          r < 3
            ? peers[2].backspaceAtMarker("charlie:", 4)
            : peers[2].typeAtMarker("charlie:", tok(` (c${r})`)),
          r === 1
            ? peers[3].insertWikilink("delta:", "Link")
            : peers[3].typeAtMarker("delta:", tok(` (d${r})`)),
          r === 0
            ? peers[4].replaceText(REPLACE_TARGET, tok("(replaced-by-echo)"))
            : peers[4].typeAtMarker("echo:", tok(` (e${r})`)),
        ];
        await Promise.all(actions);
        // Cursor presence is debounced 200ms; poll between rounds so
        // every peer gets a chance to observe a remote caret.
        await Promise.all(peers.map((p) => p.pollRemoteCursors()));
        await new Promise((res) => setTimeout(res, 400));
        await Promise.all(peers.map((p) => p.pollRemoteCursors()));
      }

      // ── quiesce: all five buffers identical & stable ─────────
      const converged = await quiesce(peers);

      // ── disk agrees (server write-behind, ~1s debounce) ──────
      await settle(() => fs.readFileSync(notePath, "utf8") === converged, {
        timeout: 20_000,
        label: "note on disk equals the converged text",
      });

      // ── content invariants ───────────────────────────────────
      for (const token of tokens) {
        const count = converged.split(token).length - 1;
        expect(count, `token ${JSON.stringify(token)} must appear exactly once`).toBe(1);
      }
      expect(converged, "deletion target fully removed").not.toContain(DELETE_TARGET);
      expect(converged, "wikilink autocomplete inserted the full link").toContain("[[Link Target]]");
      expect(converged, "markers intact").toContain("alpha:");

      // ── collab still live everywhere ─────────────────────────
      for (const peer of peers) {
        const vault = await peer.vaultState();
        expect(vault?.live, `${peer.label} collab live`).toBe(true);
        expect(vault?.status, `${peer.label} sync status`).toBe("Live");
      }

      // ── presence cursors: every peer saw a remote caret ──────
      for (const peer of peers) {
        expect(peer.sawRemoteCursor, `${peer.label} saw at least one remote cursor`).toBe(true);
      }

      // ── console hygiene ──────────────────────────────────────
      for (const peer of peers) {
        expect(peer.errors, `${peer.label} console errors:\n${peer.errors.join("\n")}`).toHaveLength(0);
      }
    } catch (err) {
      await dumpArtifacts(testInfo, peers, { suite: "convergence-storm" });
      throw err;
    } finally {
      for (const peer of peers) await peer.leave().catch(() => {});
    }
  });
});
