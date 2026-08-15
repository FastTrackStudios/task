// @ts-check
// Suite 2 — 20-peer presence churn (tracked issue dd824506).
//
// Two tests:
//
//  1. "baseline" — three peers join, rosters must agree with ground
//     truth (names + statuses, derived rows excluded), a status
//     change must propagate. Deliberately FAST: the whole check runs
//     inside the vox 16-message credit window (see the finding in
//     convergence.spec.js) so it proves the presence stack works
//     today. Departed-peer expiry can't fit inside that window
//     (heartbeats alone exhaust it), so it's only covered by the
//     full churn below.
//
//  2. "churn" — the full 20-context seeded-schedule churn from the
//     PRD (~2 min: joins/leaves/status changes/route-change activity
//     republish + a refresh-rejoin). Used to be test.fail'd on vox
//     downstream credit starvation (streams frozen at the 16-message
//     initial window); the wasm GrantCredit fix (vox fork 23acdc0a)
//     flipped it to passing.
//
// Schedules are DETERMINISTIC per seed (override with MP_SEED).
// Idle (5-minute heuristic) can't be reached in a 2-minute churn;
// the schedule exercises the same publisher path via route changes
// ("viewing X" republish) and asserts names+statuses.

const { test, expect } = require("@playwright/test");
const { settle, mulberry32, DEV_ACCOUNTS, STATUS_TITLE, Peer, dumpArtifacts } = require("./helpers");

const N_PEERS = 20;
const SEED = parseInt(process.env.MP_SEED || "3551505", 10);
const STATUS_LABELS = Object.keys(STATUS_TITLE);
const NAV_LABELS = ["Home", "Tasks", "Projects", "Goals", "Gantt"];
/** PRD bound: departed peers must be gone within 45s. */
const CHECKPOINT_TIMEOUT = 45_000;

/** Expected roster multiset from ground truth. */
const wantRows = (expected, peers) =>
  [...expected.entries()].map(([id, st]) => `${peers[id].name}|${st}`).sort();

/** One peer's parsed roster as a comparable multiset. */
async function gotRows(peer) {
  const r = await peer.roster();
  if (!r) return null;
  return r.rows
    .filter((row) => !row.agent)
    .map((row) => `${row.name}|${row.status}`)
    .sort();
}

test.describe("presence", () => {
  test("baseline: three peers' rosters agree and a status change propagates", async ({ browser }, testInfo) => {
    test.setTimeout(240_000);
    const peers = [0, 1, 2].map(
      (i) => new Peer(browser, { id: i, email: DEV_ACCOUNTS[i].email, name: DEV_ACCOUNTS[i].name }),
    );
    const expected = new Map();
    try {
      for (const p of peers) {
        await p.join("/");
        expected.set(p.id, "Active");
      }
      // Status change EARLY — everything below must complete within
      // ~45s of the first join (see header).
      await peers[1].setStatus("Do not disturb");
      expected.set(1, "Do not disturb");

      const want = wantRows(expected, peers);
      const flaps = [];
      await settle(
        async () => {
          for (const p of peers) {
            const got = await gotRows(p);
            if (!got || JSON.stringify(got) !== JSON.stringify(want)) {
              flaps.push({ t: Date.now(), peer: p.label, got });
              return false;
            }
          }
          return true;
        },
        // 60s: peers from a PREVIOUS spec in the same run linger on
        // the org roster for up to PRESENCE_TIMEOUT_MS (30s) after
        // their contexts close — the window must outlast those
        // ghosts' expiry, or this flaps while they drop out.
        { timeout: 60_000, stableFor: 1_500, label: "all three rosters agree (incl. DND)" },
      ).catch((err) => {
        console.log("[presence-baseline] flap trail:", JSON.stringify(flaps.slice(-10), null, 1));
        throw err;
      });

      for (const p of peers) {
        expect(p.errors, `${p.label} console errors:\n${p.errors.join("\n")}`).toHaveLength(0);
      }
    } catch (err) {
      await dumpArtifacts(testInfo, peers, { suite: "presence-baseline" });
      throw err;
    } finally {
      for (const p of peers) await p.leave().catch(() => {});
    }
  });

  test("churn: twenty peers, rosters agree at every quiescence checkpoint", async ({ browser }, testInfo) => {
    test.setTimeout(900_000);
    // (Was test.fail'd on vox downstream credit starvation; the wasm
    // GrantCredit fix — vox fork 23acdc0a — flipped it to passing.)

    const rng = mulberry32(SEED);
    const pick = (arr) => arr[Math.floor(rng() * arr.length)];

    const peers = Array.from({ length: N_PEERS }, (_, i) => {
      const a = DEV_ACCOUNTS[i % DEV_ACCOUNTS.length];
      return new Peer(browser, { id: i, email: a.email, name: a.name });
    });

    /** Ground truth: peer id → expected roster status title. */
    const expected = new Map();
    /** Peer ids that have not joined yet (churn joins draw these). */
    const joinPool = [];
    const t0 = Date.now();
    const log = (msg) => console.log(`[churn +${((Date.now() - t0) / 1000).toFixed(1)}s] ${msg}`);

    const join = async (id) => {
      await peers[id].join("/");
      expected.set(id, "Active");
      log(`join    ${peers[id].label}`);
    };
    const leave = async (id) => {
      await peers[id].leave();
      expected.delete(id);
      log(`leave   ${peers[id].label}`);
    };

    /**
     * Quiescence checkpoint: every CONNECTED peer's roster (derived
     * rows excluded) must equal the expected multiset, stable, within
     * the 45s budget (covers the departed-within-45s bound).
     */
    let lastMismatch = null;
    const checkpoint = async (tag) => {
      const want = wantRows(expected, peers);
      log(`checkpoint ${tag}: expecting ${want.length} rows on ${expected.size} peers`);
      await settle(
        async () => {
          for (const [id] of expected) {
            const got = await gotRows(peers[id]);
            if (!got) {
              lastMismatch = { tag, peer: peers[id].label, roster: null };
              return false;
            }
            if (JSON.stringify(got) !== JSON.stringify(want)) {
              lastMismatch = { tag, peer: peers[id].label, want, got };
              return false;
            }
          }
          lastMismatch = null;
          return { tag, rows: want.length };
        },
        { timeout: CHECKPOINT_TIMEOUT, stableFor: 2_000, label: `checkpoint ${tag} roster agreement` },
      );
      log(`checkpoint ${tag}: PASS`);
    };

    /** One churn phase: `count` seeded random events. */
    const phase = async (count) => {
      for (let i = 0; i < count; i++) {
        const connected = [...expected.keys()];
        const roll = rng();
        if (roll < 0.3 && joinPool.length > 0) {
          await join(joinPool.shift());
        } else if (roll < 0.45 && connected.length > 6) {
          // Never churn out peer 0 — it's the refresh-rejoin peer.
          await leave(pick(connected.filter((id) => id !== 0)));
        } else if (roll < 0.75) {
          const id = pick(connected);
          const label = pick(STATUS_LABELS);
          await peers[id].setStatus(label);
          expected.set(id, STATUS_TITLE[label]);
          log(`status  ${peers[id].label} -> ${label}`);
        } else {
          const id = pick(connected);
          const label = pick(NAV_LABELS);
          await peers[id].navigate(label);
          log(`nav     ${peers[id].label} -> ${label}`);
        }
        // Seeded inter-event gap (1–3s) — schedule pacing, not an
        // assertion wait.
        await new Promise((r) => setTimeout(r, 1_000 + Math.floor(rng() * 2_000)));
      }
    };

    try {
      // ── ramp-up: first 10 peers join; the rest feed the churn ──
      for (let i = 0; i < 10; i++) await join(i);
      for (let i = 10; i < N_PEERS; i++) joinPool.push(i);
      await checkpoint("0-after-rampup");

      // ── churn phase 1 ──────────────────────────────────────────
      await phase(12);
      await checkpoint("1-mid-churn");

      // ── churn phase 2 + refresh-rejoin ─────────────────────────
      await phase(8);
      log(`reload  ${peers[0].label} (refresh-rejoin — must not duplicate)`);
      await peers[0].reload();
      await phase(4);
      await checkpoint("2-after-refresh-rejoin");

      // ── churn phase 3: drain the join pool, then settle ────────
      while (joinPool.length > 0) {
        await join(joinPool.shift());
        await new Promise((r) => setTimeout(r, 800 + Math.floor(rng() * 1_200)));
      }
      await phase(6);
      await checkpoint("3-final");

      // ── refresh-rejoin duplicate guard, explicitly ─────────────
      // Multiset equality at checkpoints already catches duplicates;
      // this pins the named assertion: the reloaded peer's account
      // appears exactly as many times as ground truth says.
      const dupeName = peers[0].name;
      const expectCount = [...expected.keys()].filter((id) => peers[id].name === dupeName).length;
      const roster0 = await peers[0].roster();
      const gotCount = roster0.rows.filter((r) => !r.agent && r.name === dupeName).length;
      expect(gotCount, "refresh-rejoin must not duplicate the peer").toBe(expectCount);

      // ── console hygiene on the still-connected peers ───────────
      for (const [id] of expected) {
        const peer = peers[id];
        expect(peer.errors, `${peer.label} console errors:\n${peer.errors.join("\n")}`).toHaveLength(0);
      }
      log(`done — runtime ${((Date.now() - t0) / 1000).toFixed(1)}s, seed ${SEED}`);
    } catch (err) {
      await dumpArtifacts(testInfo, peers, { suite: "presence-churn", seed: SEED, lastMismatch });
      throw err;
    } finally {
      for (const peer of peers) await peer.leave().catch(() => {});
    }
  });
});
