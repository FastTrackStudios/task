/**
 * Wire-level smoke test against the LIVE task-server, run from node
 * (node >= 22 ships a browser-compatible global WebSocket, which is
 * all @bearcove/vox-ws needs).
 *
 *   pnpm smoke            # expects the server on ws://127.0.0.1:18080
 *   VITE_TASK_ORG=... VITE_TASK_SERVER=... pnpm smoke
 *
 * Exercises what the lab routes exercise: discover orgs (well-known),
 * connect per org, list projects, get one project, list its tasks +
 * milestones, and a real AuthService sign-in against the home org.
 */
import { channel } from "@bearcove/vox-core";

import type { TaskEvent } from "../src/generated/taskservicestream.generated";
import { fetchOrgs, homeSlug } from "../src/lib/orgs";
import { checkSchemaStamps, isLikelySkewError } from "../src/lib/schema";
import {
  DEFAULT_ORG,
  authFor,
  milestonesFor,
  projectsFor,
  taskStreamFor,
  tasksFor,
  unwrap,
  voxUrlFor,
  workstreamsFor,
} from "../src/lib/vox";

/**
 * Services the schema-stamp check flagged as stale/unverified.
 * Calls against them run inside `guarded()`: a skew-shaped
 * RpcError (unknown method / invalid payload) becomes a loud
 * SKIP instead of a smoke failure — the server is stale, not the
 * app. When the stamps all match, nothing is skippable and every
 * error stays fatal.
 */
const skewSuspects = new Set<string>();

async function guarded<T>(
  service: string,
  what: string,
  run: () => Promise<T>,
): Promise<T | null> {
  try {
    return await run();
  } catch (e) {
    if (skewSuspects.has(service) && isLikelySkewError(e)) {
      console.warn(
        `  SKEW SKIP: ${what} failed with ${String(e)} and ${service}'s ` +
          `schema stamp doesn't match the running server — rebuild + ` +
          `restart task-server (see \`task doctor\`).`,
      );
      return null;
    }
    throw e;
  }
}

async function main() {
  // Org discovery — the same well-known fetch the org switcher uses.
  const orgs = await fetchOrgs();
  console.log(
    `well-known -> ${orgs.length} org(s): ${orgs.map((o) => o.slug).join(", ")}`,
  );
  if (orgs.length === 0) throw new Error("well-known returned zero orgs");
  const home = homeSlug(orgs);

  // Proto/server skew guard — compare this bundle's generated
  // descriptors against the stamps the RUNNING server publishes.
  const wellKnown = (await (
    await fetch(
      `${(await import("../src/lib/vox")).SERVER_HTTP}/.well-known/task-server.json`,
    )
  ).json()) as { schema_stamps?: Record<string, string> };
  const check = checkSchemaStamps(wellKnown.schema_stamps ?? null);
  if (check.stale.length > 0 || check.unverified.length > 0) {
    console.warn(
      `SCHEMA SKEW WARNING — the running task-server disagrees with this ` +
        `bundle:\n` +
        (check.stale.length > 0
          ? `  stale stamps: ${check.stale.join(", ")}\n`
          : "") +
        (check.unverified.length > 0
          ? `  unverified (server has no stamp): ${check.unverified.join(", ")}\n`
          : "") +
        `  Rebuild + restart task-server; affected calls below are skipped, ` +
        `not failed.`,
    );
    for (const name of [...check.stale, ...check.unverified]) {
      skewSuspects.add(name);
    }
  } else {
    console.log(
      `schema stamps -> all ${check.ok.length} generated services match the server`,
    );
  }

  const org = DEFAULT_ORG;
  console.log(`connecting to ${voxUrlFor(org)} ...`);

  const projectClient = await projectsFor(org);
  const all = unwrap(await projectClient.list());
  console.log(`ProjectServiceRpc.list -> ${all.length} project(s)`);
  for (const p of all.slice(0, 5)) {
    console.log(
      `  - ${p.title || p.path} [${p.status} / ${p.project_type || "project"}] ${String(p.id)}`,
    );
  }

  const taskClient = await tasksFor(org);
  const allTasks = unwrap(await taskClient.list());
  console.log(`TaskServiceRpc.list -> ${allTasks.length} task(s)`);

  const milestoneClient = await milestonesFor(org);
  const allMilestones = unwrap(await milestoneClient.list());
  console.log(`MilestoneServiceRpc.list -> ${allMilestones.length} milestone(s)`);

  // Find a project whose `get` round-trips. A vault page without a
  // persisted `id:` in its frontmatter gets a fresh backfilled UUID on
  // every scan, so `get(list()[i].id)` can legitimately be NotFound
  // for such pages — skip those rather than failing the smoke.
  let fetchedOne = false;
  for (const p of all) {
    const r = await projectClient.get(p.id);
    if (!r.ok) {
      console.log(
        `  (skip ${p.title}: get -> ${r.error.tag}; unpersisted frontmatter id)`,
      );
      continue;
    }
    const its = allTasks.filter(
      (t) => String(t.project_id ?? "") === String(p.id),
    );
    console.log(
      `ProjectServiceRpc.get(${String(p.id)}) -> "${r.value.title}", ${its.length} task(s) attached`,
    );
    fetchedOne = true;
    break;
  }
  if (all.length > 0 && !fetchedOne) {
    throw new Error("get() failed for every project returned by list()");
  }

  // Workstream round-trip: list + the server-side rollup verb. The
  // `test` org carries the synthetic workstream fixtures; fall back to
  // the default org when it isn't hosted. Read-only — smoke never
  // mutates.
  const wsOrg = orgs.some((o) => o.slug === "test") ? "test" : org;
  const wsClient = await workstreamsFor(wsOrg);
  const workstreams = unwrap(await wsClient.list(null));
  console.log(
    `WorkstreamServiceRpc.list(${wsOrg}) -> ${workstreams.length} workstream(s)`,
  );
  if (workstreams.length > 0) {
    const first = workstreams[0];
    const got = await guarded("WorkstreamServiceRpc", "rollup()", async () =>
      unwrap(await wsClient.rollup(first.id)),
    );
    if (got) {
      const { workstream: w, rollup } = got;
      if (String(w.id) !== String(first.id)) {
        throw new Error("rollup returned a different workstream");
      }
      if (rollup.done > rollup.total || rollup.in_progress > rollup.total) {
        throw new Error(`rollup arithmetic is off: ${JSON.stringify(rollup)}`);
      }
      const g = rollup.groups;
      const groupSum =
        g.backlog + g.unstarted + g.started + g.completed + g.cancelled;
      if (groupSum !== rollup.total) {
        throw new Error(
          `rollup groups don't sum to total: ${JSON.stringify(rollup)}`,
        );
      }
      console.log(
        `WorkstreamServiceRpc.rollup("${w.title}") -> ${rollup.done}/${rollup.total} done, ` +
          `${rollup.in_progress} in progress, ${rollup.blocked} blocked, ${rollup.estimate_points_sum} pts; ` +
          `groups b/u/s/c/x = ${g.backlog}/${g.unstarted}/${g.started}/${g.completed}/${g.cancelled}`,
      );
    }
  }

  // Server-side filtered list + batch reverse relations (the
  // list-page round-trip without shipping the whole org).
  {
    const sample = allTasks.slice(0, 3);
    const page = await guarded("TaskServiceRpc", "query()", async () =>
      unwrap(await taskClient.query({
        project: null,
        workstream: null,
        status: null,
        limit: 5,
        offset: 0,
      })),
    );
    if (page) {
      if (page.length > 5) throw new Error("query ignored limit");
      console.log(`TaskServiceRpc.query(limit 5) -> ${page.length} task(s)`);
    }
    if (sample.length > 0) {
      const batch = await guarded(
        "TaskServiceRpc",
        "reverseRelationsBatch()",
        async () =>
          unwrap(
            await taskClient.reverseRelationsBatch(sample.map((t) => t.id)),
          ),
      );
      if (batch) {
        if (batch.length !== sample.length) {
          throw new Error(
            `reverseRelationsBatch returned ${batch.length} entries for ${sample.length} ids`,
          );
        }
        console.log(
          `TaskServiceRpc.reverseRelationsBatch(${sample.length} ids) -> ` +
            batch.map((e) => e.relations.length).join("/") +
            " incoming edge(s)",
        );
      }
    }
  }

  // `#[subscribe]` stream smoke: subscribe to TaskServiceStream
  // events, mutate, and assert the events arrive (fetch-once-then-
  // fold is the board's live path). Mutations are confined to the
  // `test` org; when it isn't hosted the stream check is skipped.
  if (orgs.some((o) => o.slug === "test")) {
    const stream = await taskStreamFor("test");
    const [tx, rx] = channel<TaskEvent>();
    // Subscribe BEFORE mutating — the awaited call returns once the
    // sink is attached to the backend hub, so nothing is missed.
    const subscribed = await guarded(
      "TaskServiceStream",
      "events() subscribe",
      async () => {
        await stream.events(tx);
        return true;
      },
    );
    if (!subscribed) {
      console.log("(skip stream smoke: subscribe unavailable on this server)");
    } else {
      await streamRoundTrip(rx);
    }
  } else {
    console.log("(skip stream smoke: `test` org not hosted)");
  }

  async function streamRoundTrip(rx: import("@bearcove/vox-core").Rx<TaskEvent>) {
    const testTasks = await tasksFor("test");
    const created = unwrap(
      await testTasks.create({
        id: "00000000-0000-0000-0000-000000000000", // nil -> backend assigns
        path: "",
        title: `smoke stream probe ${Date.now()}`,
        status: "open",
        priority: "normal",
        due: null,
        scheduled: null,
        tags: ["smoke"],
        contexts: [],
        projects: [],
        project_id: null,
        milestone_id: null,
        time_estimate: null,
        time_entries: [],
        recurrence: null,
        recurrence_anchor: null,
        complete_instances: [],
        completed_date: null,
        agent_profile: "",
        dispatched_agent_tasks: [],
        date_created: null,
        date_modified: null,
        details: "",
        workflow: null,
      }),
    );
    const next = async (): Promise<TaskEvent> => {
      const timeout = new Promise<never>((_, reject) =>
        setTimeout(
          () => reject(new Error("no TaskEvent within 10s of a mutation")),
          10_000,
        ),
      );
      const ev = await Promise.race([rx.recv(), timeout]);
      if (ev === null) throw new Error("task event stream closed early");
      return ev;
    };

    const upserted = await next();
    if (
      upserted.tag !== "Upserted" ||
      String(upserted.value.id) !== String(created.id)
    ) {
      throw new Error(
        `expected Upserted(${String(created.id)}) after create, got ${JSON.stringify(upserted).slice(0, 200)}`,
      );
    }
    unwrap(await testTasks.delete(created.id));
    const deleted = await next();
    if (deleted.tag !== "Deleted" || String(deleted.value) !== String(created.id)) {
      throw new Error(
        `expected Deleted(${String(created.id)}) after delete, got ${JSON.stringify(deleted).slice(0, 200)}`,
      );
    }
    console.log(
      `TaskServiceStream.events(test) -> Upserted + Deleted observed for ${String(created.id)}`,
    );
  }

  // Real sign-in against the home org — the account switcher's path.
  const auth = await authFor(home);
  const bundle = unwrap(
    await auth.signInEmailPassword({
      email: "guest@fasttrackstudios.com",
      password: "dev-guest-2026",
      ip_address: null,
      user_agent: "ui-lab-smoke",
    }),
  );
  const who = unwrap(await auth.whoami(bundle.token));
  console.log(
    `AuthService.signInEmailPassword(guest) -> whoami ${who.email} (${who.name ?? "?"}) @ ${home}`,
  );
  unwrap(await auth.signOut(bundle.token));

  console.log("smoke OK");
  process.exit(0);
}

main().catch((e) => {
  console.error("smoke FAILED:", e);
  process.exit(1);
});
