/**
 * Client mirror of the per-project state registry
 * (features/project/project/src/states.rs).
 *
 * `TaskInfo.status` stays a free-form string (the vault is
 * hand-editable); this module only adds MEANING: every status name
 * resolves to one of the five canonical state GROUPS (Backlog /
 * Unstarted / Started / Completed / Cancelled), and all progress
 * classification — rollups, segmented bars, kanban buckets — goes
 * through the group, never the raw string.
 *
 * Resolution order mirrors the server exactly:
 *   1. the project's own registry (`ProjectInfo.states`), when
 *      present — alias-tolerant name match;
 *   2. the built-in alias table over the canonical statuses;
 *   3. fallback: `Unstarted` — unknown work is presumed not yet
 *      begun (documented server contract, keeps progress
 *      conservative).
 */
import type {
  StateDef,
  StateGroup,
  StatesConfig,
} from "@/generated/projectservicerpc.generated";

export type GroupTag = StateGroup["tag"];

/** Canonical display order of the five groups. */
export const GROUP_ORDER: readonly GroupTag[] = [
  "Backlog",
  "Unstarted",
  "Started",
  "Completed",
  "Cancelled",
] as const;

/**
 * Case / separator / whitespace-insensitive form used for ALL status
 * name matching — mirrors `states::normalize`.
 */
export function normalizeStatus(s: string): string {
  return s
    .trim()
    .toLowerCase()
    .replace(/[_\s]+/g, "-");
}

/**
 * The registry used when a project declares no `states:` config —
 * mirrors `default_states()` (note `waiting` → Started: claimed work
 * paused on a dependency stays in the in-flight bucket).
 */
export const DEFAULT_STATES: StatesConfig = [
  { name: "triage", group: { tag: "Backlog" }, color: "", default: false, order: 0 },
  { name: "open", group: { tag: "Unstarted" }, color: "", default: true, order: 1 },
  { name: "in-progress", group: { tag: "Started" }, color: "", default: false, order: 2 },
  { name: "waiting", group: { tag: "Started" }, color: "", default: false, order: 3 },
  { name: "done", group: { tag: "Completed" }, color: "", default: false, order: 4 },
  { name: "cancelled", group: { tag: "Cancelled" }, color: "", default: false, order: 5 },
];

/** Built-in alias table (input already normalized) — mirrors `builtin_group`. */
const BUILTIN_GROUPS: Record<string, GroupTag> = {
  triage: "Backlog",
  backlog: "Backlog",
  open: "Unstarted",
  todo: "Unstarted",
  none: "Unstarted",
  unstarted: "Unstarted",
  "in-progress": "Started",
  doing: "Started",
  started: "Started",
  active: "Started",
  waiting: "Started",
  blocked: "Started",
  done: "Completed",
  completed: "Completed",
  complete: "Completed",
  shipped: "Completed",
  cancelled: "Cancelled",
  canceled: "Cancelled",
  abandoned: "Cancelled",
};

/** The effective registry: the project's own, or the default mirror. */
export function effectiveStates(
  states: StatesConfig | null | undefined,
): StatesConfig {
  return states && states.length > 0 ? states : DEFAULT_STATES;
}

/** The registry in display order (`order` asc, declaration tiebreak). */
export function orderedStates(
  states: StatesConfig | null | undefined,
): StateDef[] {
  return [...effectiveStates(states)].sort((a, b) => a.order - b.order);
}

/** The registered StateDef for a raw status, if any (color, group). */
export function stateDefFor(
  states: StatesConfig | null | undefined,
  status: string,
): StateDef | null {
  const norm = normalizeStatus(status);
  return (
    effectiveStates(states).find((d) => normalizeStatus(d.name) === norm) ??
    null
  );
}

/** Resolve a raw status string to its state group — mirrors `resolve_state_group`. */
export function resolveStateGroup(
  states: StatesConfig | null | undefined,
  status: string,
): GroupTag {
  const def = states && states.length > 0 ? stateDefFor(states, status) : null;
  if (def) return def.group.tag;
  return BUILTIN_GROUPS[normalizeStatus(status)] ?? "Unstarted";
}

/**
 * Closed for blocker-resolution purposes (`done` OR `cancelled`
 * unblocks dependents) — mirrors `StateGroup::is_closed` and the
 * `task issue ready` rule the server rollup uses.
 */
export function isClosedGroup(g: GroupTag): boolean {
  return g === "Completed" || g === "Cancelled";
}

/** The state newly-created tasks default to (first `default: true`). */
export function defaultStateName(
  states: StatesConfig | null | undefined,
): string {
  return effectiveStates(states).find((d) => d.default)?.name ?? "open";
}

/**
 * Review-ish state convention: a registered state whose name contains
 * "review" (e.g. the test org's `qa-review`) parks work for a human —
 * it joins the review lane even without a `review-required` tag.
 */
export function isReviewStatus(
  states: StatesConfig | null | undefined,
  status: string,
): boolean {
  return (
    normalizeStatus(status).includes("review") &&
    resolveStateGroup(states, status) === "Started"
  );
}
