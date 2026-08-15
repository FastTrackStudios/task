/**
 * Pure derivation layer for the project overview — classification of
 * the agent/human task layers, workstream rollups, state-group math,
 * milestone progress, estimates, and project health. No React in
 * here; everything is computed once per query result and memoized by
 * the page.
 *
 * THE core problem this feeds: agent-scale vs human-scale tasks. A
 * project like the test org's "Hermes Integration" carries a handful
 * of human tasks and a ~56-task agent swarm attached to a few
 * **Workstreams** (the parent-with-swarm construct — a real entity
 * served by WorkstreamService; tasks attach via
 * `workflow.workstream`). The human layer is the default view, with
 * each workstream rolling its swarm up into a strip.
 *
 * All status classification flows through the project's STATE
 * REGISTRY (`ProjectInfo.states`, lib/states.ts): free-form status
 * strings resolve to the five canonical groups, and blocked/review
 * are derived overlays on top of the groups.
 */
import type {
  AgentRef,
  Estimate,
  Relation,
  TaskInfo,
} from "@/generated/taskservicerpc.generated";
import type { Milestone } from "@/generated/milestoneservicerpc.generated";
import type { StatesConfig } from "@/generated/projectservicerpc.generated";
import type { Workstream } from "@/generated/workstreamservicerpc.generated";
import {
  isClosedGroup,
  isReviewStatus,
  resolveStateGroup,
  type GroupTag,
} from "@/lib/states";

// ── agent / human classification ────────────────────────────────────

/**
 * Is this assignee an agent? The wire decodes `AgentRef::Agent` as
 * `{ tag: 'Agent', name, model_version }`; the on-disk frontmatter
 * (and possibly older peers) spells it `{ kind: 'agent', ... }` — we
 * accept both shapes.
 */
export function isAgentRef(a: AgentRef): boolean {
  const raw = a as unknown as { tag?: string; kind?: string };
  return (
    raw.tag === "Agent" || raw.kind === "agent" || raw.kind === "Agent"
  );
}

/** The claiming agent (`name` + optional model), if any. */
export function agentClaimant(
  t: TaskInfo,
): { name: string; model: string | null } | null {
  for (const a of t.workflow?.assignees ?? []) {
    if (isAgentRef(a)) {
      const raw = a as unknown as {
        name?: string;
        model_version?: string | null;
      };
      return { name: raw.name ?? "agent", model: raw.model_version ?? null };
    }
  }
  return null;
}

/** Short agent-ref label, `name@model` (the `agent:claude@fable-5` form sans prefix). */
export function claimantKey(c: { name: string; model: string | null }): string {
  return c.model ? `${c.name}@${c.model}` : c.name;
}

/** Label an AgentRef (lead / member chips): humans by name, agents `name@model`. */
export function agentRefLabel(a: AgentRef): string {
  const raw = a as unknown as {
    name?: string;
    model_version?: string | null;
    user_id?: string;
    display_name?: string | null;
  };
  if (isAgentRef(a)) {
    return raw.model_version ? `${raw.name}@${raw.model_version}` : (raw.name ?? "agent");
  }
  return raw.display_name || raw.user_id || "human";
}

/**
 * Agent-layer task: carries an `AgentRef::Agent` assignee (the claim),
 * or — for unclaimed swarm members that have no assignee yet — the
 * `agent` tag the dispatcher stamps on dispatched subtasks.
 */
export function isAgentTask(t: TaskInfo): boolean {
  return agentClaimant(t) !== null || t.tags.includes("agent");
}

// ── state groups + status buckets ───────────────────────────────────

/** Per-group counts — the segmented progress bars render these. */
export type GroupCounts = Record<GroupTag, number> & { total: number };

export function emptyGroupCounts(): GroupCounts {
  return {
    Backlog: 0,
    Unstarted: 0,
    Started: 0,
    Completed: 0,
    Cancelled: 0,
    total: 0,
  };
}

export function countGroups(
  tasks: TaskInfo[],
  states: StatesConfig | null,
): GroupCounts {
  const c = emptyGroupCounts();
  for (const t of tasks) {
    c[resolveStateGroup(states, t.status)] += 1;
    c.total += 1;
  }
  return c;
}

export type StatusBucket =
  | "done"
  | "running"
  | "blocked"
  | "review"
  | "cancelled"
  | "open";

/**
 * Park-for-review detection. Two conventions feed the review lane:
 * a `review-required[:reason]` tag (or details prefix) on the task,
 * and a registry state whose name contains "review" (the test org's
 * `qa-review`).
 */
export function reviewReason(
  t: TaskInfo,
  states: StatesConfig | null,
): string | null {
  for (const tag of t.tags) {
    if (tag.startsWith("review-required")) {
      return tag.includes(":") ? tag.slice(tag.indexOf(":") + 1).trim() : "review required";
    }
  }
  const firstLine = t.details.trimStart().split("\n", 1)[0] ?? "";
  if (firstLine.toLowerCase().startsWith("review-required:")) {
    return firstLine.slice("review-required:".length).trim();
  }
  if (isReviewStatus(states, t.status)) {
    return `in ${t.status}`;
  }
  return null;
}

/** Unresolved blockers: blocker tasks that aren't closed (done/cancelled). */
export function unresolvedBlockers(
  t: TaskInfo,
  byId: Map<string, TaskInfo>,
  states: StatesConfig | null,
): string[] {
  const out: string[] = [];
  for (const blocker of t.workflow?.blockers ?? []) {
    const id = String(blocker);
    const other = byId.get(id);
    // Unknown blocker id still blocks — absence of evidence isn't done.
    if (!other || !isClosedGroup(resolveStateGroup(states, other.status))) {
      out.push(id);
    }
  }
  // `Blocks` relations point the OTHER way (this task blocks target),
  // so only `blockers` feeds blocked-ness here; reverse relations are
  // surfaced separately on rows.
  return out;
}

/**
 * Bucket a task for kanban lanes / row icons. Group-derived
 * (registry-aware), with two overlays the groups can't express:
 * review (parked for a human) and blocked (unresolved blockers).
 */
export function statusBucket(
  t: TaskInfo,
  byId: Map<string, TaskInfo>,
  states: StatesConfig | null,
): StatusBucket {
  if (reviewReason(t, states) !== null) return "review";
  const group = resolveStateGroup(states, t.status);
  switch (group) {
    case "Completed":
      return "done";
    case "Cancelled":
      return "cancelled";
    default:
      break;
  }
  if (unresolvedBlockers(t, byId, states).length > 0) return "blocked";
  return group === "Started" ? "running" : "open";
}

export interface SwarmCounts {
  done: number;
  running: number;
  blocked: number;
  review: number;
  open: number;
  cancelled: number;
  total: number;
}

export function emptyCounts(): SwarmCounts {
  return {
    done: 0,
    running: 0,
    blocked: 0,
    review: 0,
    open: 0,
    cancelled: 0,
    total: 0,
  };
}

export function countTasks(
  tasks: TaskInfo[],
  byId: Map<string, TaskInfo>,
  states: StatesConfig | null,
): SwarmCounts {
  const c = emptyCounts();
  for (const t of tasks) {
    c[statusBucket(t, byId, states)] += 1;
    c.total += 1;
  }
  return c;
}

/** Completion ratio that ignores cancelled work (Linear convention). */
export function completionPct(c: SwarmCounts): number {
  const denom = c.total - c.cancelled;
  return denom === 0 ? 0 : Math.round((c.done / denom) * 100);
}

// ── relations ───────────────────────────────────────────────────────

export interface RelationView {
  kind: "Blocks" | "Duplicate" | "Implements" | "Relates";
  targetId: string;
  /** Resolved target title when the task is in this project's set. */
  targetTitle: string | null;
}

export function taskRelations(
  t: TaskInfo,
  byId: Map<string, TaskInfo>,
): RelationView[] {
  return (t.workflow?.relations ?? []).map((r: Relation) => {
    const targetId = String(r.target);
    return {
      kind: r.kind.tag,
      targetId,
      targetTitle: byId.get(targetId)?.title ?? null,
    };
  });
}

// ── the two layers + workstream rollups ─────────────────────────────

export interface WorkstreamModel {
  workstream: Workstream;
  /** Member tasks (attached via `workflow.workstream`). */
  members: TaskInfo[];
  /** Bucketed counts (review/blocked overlays included). */
  counts: SwarmCounts;
  /** Per-state-group counts — the strip's segmented bar. */
  groups: GroupCounts;
  /** Client-computed estimate-point sum over members (mirrors the
   * server rollup's XS/S/M/L/XL → 1/2/3/5/8 weights). */
  points: number;
}

export interface ProjectModel {
  byId: Map<string, TaskInfo>;
  states: StatesConfig | null;
  /** Workstreams with their member rollups (largest swarm first). */
  workstreams: WorkstreamModel[];
  /** Human-layer rows attached to NO workstream. */
  humans: TaskInfo[];
  /** Every agent-layer task in the project (the full swarm). */
  agents: TaskInfo[];
  /** Tasks parked for human review — the review lane. */
  review: TaskInfo[];
  all: SwarmCounts;
  allGroups: GroupCounts;
}

export function buildModel(
  tasks: TaskInfo[],
  workstreams: Workstream[],
  states: StatesConfig | null,
): ProjectModel {
  const byId = new Map<string, TaskInfo>();
  for (const t of tasks) byId.set(String(t.id), t);

  const membersByWs = new Map<string, TaskInfo[]>();
  const loose: TaskInfo[] = [];
  for (const t of tasks) {
    const ws = t.workflow?.workstream ? String(t.workflow.workstream) : "";
    if (ws) {
      const bucket = membersByWs.get(ws);
      if (bucket) bucket.push(t);
      else membersByWs.set(ws, [t]);
    } else {
      loose.push(t);
    }
  }

  const wsModels: WorkstreamModel[] = workstreams.map((w) => {
    const members = membersByWs.get(String(w.id)) ?? [];
    return {
      workstream: w,
      members,
      counts: countTasks(members, byId, states),
      groups: countGroups(members, states),
      points: members.reduce(
        (sum, t) => sum + estimatePoints(t.workflow?.estimate),
        0,
      ),
    };
  });
  wsModels.sort((a, b) => b.members.length - a.members.length);

  // Tasks attached to a workstream the service didn't return (cross-
  // project attach) still belong to the human/agent layers below.
  const agents = tasks.filter(isAgentTask);
  const humans = loose
    .filter((t) => !isAgentTask(t))
    .sort(
      (a, b) =>
        bucketRank(statusBucket(a, byId, states)) -
          bucketRank(statusBucket(b, byId, states)) ||
        priorityRank(a.priority) - priorityRank(b.priority) ||
        a.title.localeCompare(b.title),
    );

  const review = tasks.filter((t) => reviewReason(t, states) !== null);

  return {
    byId,
    states,
    workstreams: wsModels,
    humans,
    agents,
    review,
    all: countTasks(tasks, byId, states),
    allGroups: countGroups(tasks, states),
  };
}

export function bucketRank(b: StatusBucket): number {
  switch (b) {
    case "review":
      return 0;
    case "blocked":
      return 1;
    case "running":
      return 2;
    case "open":
      return 3;
    case "done":
      return 4;
    case "cancelled":
      return 5;
  }
}

export function priorityRank(p: string): number {
  switch (p) {
    case "urgent":
      return 0;
    case "high":
      return 1;
    case "normal":
    case "medium":
      return 2;
    case "low":
      return 3;
    default:
      return 4;
  }
}

// ── estimates ───────────────────────────────────────────────────────

/** Bucket weights per t-shirt size (matches the server rollup); `Points` passes through. */
export function estimatePoints(e: Estimate | null | undefined): number {
  switch (e?.tag) {
    case "XS":
      return 1;
    case "S":
      return 2;
    case "M":
      return 3;
    case "L":
      return 5;
    case "XL":
      return 8;
    case "Points":
      return e.value;
    default:
      return 0;
  }
}

export interface EstimateRollup {
  total: number;
  done: number;
  /** How many tasks actually carry an estimate. */
  estimated: number;
}

export function rollupEstimates(
  tasks: TaskInfo[],
  byId: Map<string, TaskInfo>,
  states: StatesConfig | null,
): EstimateRollup {
  let total = 0;
  let done = 0;
  let estimated = 0;
  for (const t of tasks) {
    const pts = estimatePoints(t.workflow?.estimate);
    if (pts === 0) continue;
    estimated += 1;
    total += pts;
    if (statusBucket(t, byId, states) === "done") done += pts;
  }
  return { total, done, estimated };
}

// ── milestones ──────────────────────────────────────────────────────

export interface MilestoneProgress {
  milestone: Milestone;
  counts: SwarmCounts;
  pct: number;
  /** ISO date string when the milestone carries one. */
  due: string | null;
  overdue: boolean;
}

export function milestoneProgress(
  milestones: Milestone[],
  tasks: TaskInfo[],
  byId: Map<string, TaskInfo>,
  states: StatesConfig | null,
): MilestoneProgress[] {
  return milestones.map((m) => {
    const mine = tasks.filter(
      (t) => String(t.milestone_id ?? "") === String(m.id),
    );
    const counts = countTasks(mine, byId, states);
    const due = m.due_date == null ? null : String(m.due_date);
    const closed = m.status === "done" || m.status === "closed";
    return {
      milestone: m,
      counts,
      pct: closed ? 100 : completionPct(counts),
      due,
      overdue: !closed && due !== null && due < todayIso(),
    };
  });
}

function todayIso(): string {
  return new Date().toISOString().slice(0, 10);
}

// ── health ──────────────────────────────────────────────────────────

export type Health = {
  label: "On track" | "At risk" | "Off track";
  tone: "ok" | "warn" | "bad";
};

/**
 * Derived health (the wire has no first-class health field yet):
 * overdue milestone or >15% of live work blocked → off track;
 * any blocked/review-parked work → at risk; else on track.
 */
export function deriveHealth(
  all: SwarmCounts,
  milestones: MilestoneProgress[],
): Health {
  const live = all.total - all.done - all.cancelled;
  if (
    milestones.some((m) => m.overdue) ||
    (live > 0 && all.blocked / live > 0.15)
  ) {
    return { label: "Off track", tone: "bad" };
  }
  if (all.blocked > 0 || all.review > 0) {
    return { label: "At risk", tone: "warn" };
  }
  return { label: "On track", tone: "ok" };
}

// ── misc formatting ─────────────────────────────────────────────────

/** Compact relative age ("3h", "2d") from an ISO timestamp. */
export function relativeAge(iso: unknown): string {
  if (iso == null) return "";
  const then = Date.parse(String(iso));
  if (Number.isNaN(then)) return "";
  const s = Math.max(0, (Date.now() - then) / 1000);
  if (s < 60) return `${Math.floor(s)}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)}d`;
  return `${Math.floor(s / (86400 * 30))}mo`;
}

export function formatDate(iso: unknown): string {
  if (iso == null) return "";
  const d = new Date(String(iso));
  if (Number.isNaN(d.getTime())) return String(iso);
  return d.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: d.getFullYear() === new Date().getFullYear() ? undefined : "numeric",
  });
}

export function estimateLabel(e: Estimate | null | undefined): string | null {
  if (!e) return null;
  return e.tag === "Points" ? `${e.value}pt` : e.tag;
}
