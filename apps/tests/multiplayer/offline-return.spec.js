// @ts-check
// Suite 1b — offline/return re-convergence. EXPECTED-FAIL / SKIPPED.
//
// ┌─────────────────────────────────────────────────────────────────┐
// │ KNOWN BUG — P0 6303584a: dead connections don't recover.        │
// │ The per-org vox connection root is cached for the page's        │
// │ lifetime (crates/task/ui/src/vox_clients.rs: "Reconnection after a   │
// │ server restart is a follow-up — a stale client surfaces as a    │
// │ request error, and a reload re-establishes"), and the vault     │
// │ page tears the collab session down on Offline without re-arming │
// │ open_collab when connectivity returns. Until the fix merges,    │
// │ this spec is marked test.fixme() — UNSKIP IT (delete the fixme  │
// │ line) when 6303584a lands; the assertions below describe the    │
// │ post-fix contract.                                              │
// └─────────────────────────────────────────────────────────────────┘
//
// Scenario: two peers converge on a note; peer B drops offline
// (context-level network kill); A keeps editing; B types while
// offline (sha-save fallback path); B comes back; both must
// re-converge to one document that contains both edit streams, and
// B's collab session must return to live.

const fs = require("fs");
const path = require("path");
const { test, expect } = require("@playwright/test");
const { loadState, settle, DEV_ACCOUNTS, Peer, dumpArtifacts } = require("./helpers");

const NOTE_TITLE = "Collab Test";
const SEED = `# Collab Test

alpha: .
bravo: .

tail.
`;

test.describe("offline/return re-convergence", () => {
  test.setTimeout(300_000);

  test("a peer that drops offline and returns re-converges", async ({ browser }, testInfo) => {
    test.fixme(
      true,
      "KNOWN BUG P0 6303584a: dead connections don't recover (vox connection root cache + collab teardown never re-arm). Unskip when the fix merges.",
    );

    const state = loadState();
    const notePath = path.join(state.vaultDir, state.collabNote);
    fs.writeFileSync(notePath, SEED);

    const a = new Peer(browser, { id: 0, email: DEV_ACCOUNTS[0].email, name: DEV_ACCOUNTS[0].name });
    const b = new Peer(browser, { id: 1, email: DEV_ACCOUNTS[1].email, name: DEV_ACCOUNTS[1].name });

    try {
      await a.join("/vault");
      await b.join("/vault");
      await a.openNote(NOTE_TITLE);
      await b.openNote(NOTE_TITLE);

      // Baseline sync: A types, B must see it.
      await a.typeAtMarker("alpha:", " (a-before)");
      await settle(async () => (await b.docText())?.includes("(a-before)"), {
        timeout: 20_000,
        label: "B sees A's edit before the drop",
      });

      // ── B drops offline ────────────────────────────────────────
      await b.context.setOffline(true);
      // The vault page detects Offline and tears the session down
      // (collab falls back to sha-save mode).
      await settle(async () => {
        const v = await b.vaultState();
        return v && v.status !== "Live";
      }, { timeout: 30_000, label: "B's collab session leaves Live after the drop" });

      // Both sides keep editing across the partition.
      await a.typeAtMarker("alpha:", " (a-during)");
      await b.typeAtMarker("bravo:", " (b-during)");

      // ── B returns ──────────────────────────────────────────────
      await b.context.setOffline(false);

      // POST-FIX CONTRACT (P0 6303584a): connectivity returning must
      // re-establish the org connection and re-arm collab for the
      // open file — without a manual reload.
      await settle(async () => {
        const v = await b.vaultState();
        return v?.live === true && v?.status === "Live";
      }, { timeout: 60_000, label: "B's collab session returns to Live" });

      const converged = await settle(
        async () => {
          const ta = await a.docText();
          const tb = await b.docText();
          return ta !== null && ta === tb ? ta : null;
        },
        { timeout: 60_000, stableFor: 2_000, label: "A and B re-converge" },
      );
      expect(converged).toContain("(a-before)");
      expect(converged).toContain("(a-during)");
      expect(converged).toContain("(b-during)");

      await settle(() => fs.readFileSync(notePath, "utf8") === converged, {
        timeout: 20_000,
        label: "disk equals the re-converged text",
      });
    } catch (err) {
      await dumpArtifacts(testInfo, [a, b], { suite: "offline-return" });
      throw err;
    } finally {
      await a.leave().catch(() => {});
      await b.leave().catch(() => {});
    }
  });
});
