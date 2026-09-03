// @ts-check
const { test, expect } = require("@playwright/test");

/**
 * Regression guard: the app must not talk itself off its own connection.
 *
 * The bug this exists for (2026-09-02, seen against the deployed
 * server): every mounted consumer of a shared store owned its own
 * `use_resource`, so the shell, the command palette and the page each
 * issued the SAME `project/list` / `task/list`, and each of those
 * fanned out across every selected org. That put ~60 identical calls in
 * flight at once on one connection, on top of the ~11 long-lived
 * `*-stream/events` subscriptions.
 *
 * vox counts a live request against `max_concurrent_requests` (64 by
 * default) and treats crossing it as a PROTOCOL VIOLATION, not
 * backpressure — the server closes the connection:
 *
 *   WARN vox_core::driver: closing connection after protocol violation
 *     description=max_concurrent_requests exceeded ... (limit 64, in-flight 64)
 *
 * The client reconnected, re-ran every resource, and tripped the limit
 * again — a reconnect storm every ~2s that could never converge. The
 * shell still rendered throughout, so a DOM-only assertion sees nothing
 * wrong; the symptom lives entirely in the console + on the wire, which
 * is what this spec watches.
 *
 * Fixed by single-flighting the fan-out (`feeds::fan_out` in
 * `crates/ui-core/src/feeds.rs`) so concurrent callers wanting the same
 * rows share one call. The unit tests there cover the coalescing
 * contract; this covers the behaviour that actually broke.
 *
 * Needs a reachable `task-server` (see the config's note on data-driven
 * tests) — it skips rather than fails when the app never connects, so
 * the shell-only smoke run stays green.
 */

/** How long to watch a settled app for churn. */
const OBSERVE_MS = 25_000;

/** The storm's console signatures (`architect`'s reconnect + vox driver). */
const CONNECTION_LOST = /connection lost; reconnecting/i;
const PROTOCOL_ERROR = /received protocol error from peer/i;
const OVER_LIMIT = /max_concurrent_requests exceeded/i;
const ROOT_DEAD = /cached root is dead/i;
/** A dropped WebSocket callback surfaces as an uncaught wasm-bindgen error. */
const FATAL = /closure invoked recursively or after being dropped|unreachable executed/i;
/** Proof the app got a connection at all — otherwise there is nothing to test. */
const CONNECTED = /connection (established|re-established)/i;

test("the vox connection does not churn while the app sits idle", async ({
  page,
}) => {
  const lines = [];
  const fatal = [];
  page.on("console", (m) => lines.push(m.text()));
  page.on("pageerror", (e) => {
    if (FATAL.test(String(e))) fatal.push(String(e));
  });

  await page.goto("/projects", { waitUntil: "domcontentloaded" });

  // Let the app boot and settle. The FIRST connection is legitimately
  // replaced once: discovery has to resolve before auth can restore a
  // session, so generation 1 is dialled anonymously and re-dialled with
  // the bearer (see the ordering note in `crates/ui/src/app.rs`).
  await expect(page.getByRole("link", { name: "Projects" })).toBeVisible({
    timeout: 45_000,
  });
  await page.waitForTimeout(10_000);

  test.skip(
    !lines.some((l) => CONNECTED.test(l)),
    "no vox connection — run a task-server on the endpoint baked into this build",
  );

  // Only churn from here on counts: the boot re-dial is expected.
  const settled = lines.length;
  await page.waitForTimeout(OBSERVE_MS);
  const after = lines.slice(settled);

  const count = (re) => after.filter((l) => re.test(l)).length;
  const context = after
    .filter((l) => CONNECTION_LOST.test(l) || PROTOCOL_ERROR.test(l) || OVER_LIMIT.test(l))
    .slice(0, 10)
    .join("\n");

  // The server never rejecting a frame is the core invariant: these two
  // are how "we exceeded the concurrency limit" reaches the client.
  expect(count(PROTOCOL_ERROR), `protocol errors from the server:\n${context}`).toBe(0);
  expect(count(OVER_LIMIT), `concurrency limit exceeded:\n${context}`).toBe(0);

  // An idle, settled app should hold ONE connection. Allow a single
  // blip so an unrelated transient (a server restart, a flaky socket)
  // doesn't fail the suite; the storm produced one every ~2 seconds,
  // which is an order of magnitude clear of this bound.
  expect(
    count(CONNECTION_LOST),
    `reconnect churn while idle:\n${context}`,
  ).toBeLessThanOrEqual(1);
  expect(count(ROOT_DEAD), `connection roots died:\n${context}`).toBeLessThanOrEqual(1);

  expect(fatal, `fatal wasm/WS errors:\n${fatal.join("\n")}`).toHaveLength(0);
});
