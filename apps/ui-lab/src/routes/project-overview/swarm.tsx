/**
 * Workstream/swarm rendering: the compact per-workstream strip (title,
 * member rollup, state-GROUP-segmented progress bar), the dense
 * virtualized member list it expands into, the claimant-grouped agent
 * kanban, and the review lane for parked review-required work.
 *
 * The swarm is the agent-scale side of the page — dozens of dispatched
 * subtasks per workstream. Rows are fixed-height and windowed by a
 * tiny hand-rolled virtualizer (no dep; 56 rows today, 500 by design),
 * so expanding a workstream or opening the Agents lens stays
 * O(viewport). Strips collapse by default, Linear/Plane-style — the
 * humans-first layer stays on top.
 */
import {
  useCallback,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { Link } from "@tanstack/react-router";
import {
  AlertTriangle,
  ArrowUpRight,
  Ban,
  Bot,
  CheckCircle2,
  ChevronRight,
  Circle,
  CircleDashed,
  CircleSlash,
  CopyX,
  Eye,
  Link2,
  Loader2,
  Wrench,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import type { StatesConfig } from "@/generated/projectservicerpc.generated";
import type { TaskInfo } from "@/generated/taskservicerpc.generated";
import { cn } from "@/lib/utils";
import type { GroupTag } from "@/lib/states";
import {
  agentClaimant,
  agentRefLabel,
  bucketRank,
  claimantKey,
  relativeAge,
  reviewReason,
  statusBucket,
  taskRelations,
  type GroupCounts,
  type ProjectModel,
  type RelationView,
  type StatusBucket,
  type SwarmCounts,
  type WorkstreamModel,
} from "./model";

// ── status visual language ──────────────────────────────────────────

/** The five canonical state groups — segmented bars render THESE. */
export const GROUP_META: Record<
  GroupTag,
  { label: string; bar: string; text: string }
> = {
  Completed: {
    label: "done",
    bar: "bg-emerald-500",
    text: "text-emerald-500",
  },
  Started: {
    label: "in progress",
    bar: "bg-blue-500",
    text: "text-blue-500",
  },
  Unstarted: {
    label: "unstarted",
    bar: "bg-muted-foreground/30",
    text: "text-muted-foreground",
  },
  Backlog: {
    label: "backlog",
    bar: "bg-muted-foreground/15",
    text: "text-muted-foreground/70",
  },
  Cancelled: {
    label: "cancelled",
    bar: "bg-muted-foreground/10",
    text: "text-muted-foreground/50",
  },
};

/** Bar paint order: shipped first, then in-flight, then waiting work. */
export const GROUP_SEGMENT_ORDER: readonly GroupTag[] = [
  "Completed",
  "Started",
  "Unstarted",
  "Backlog",
  "Cancelled",
] as const;

/** Buckets (groups + the review/blocked overlays) for icons + lanes. */
export const BUCKET_META: Record<
  StatusBucket,
  { label: string; bar: string; text: string; icon: ReactNode }
> = {
  done: {
    label: "done",
    bar: "bg-emerald-500",
    text: "text-emerald-500",
    icon: <CheckCircle2 className="size-3.5 text-emerald-500" />,
  },
  running: {
    label: "in progress",
    bar: "bg-blue-500",
    text: "text-blue-500",
    icon: <Loader2 className="size-3.5 text-blue-500" />,
  },
  blocked: {
    label: "blocked",
    bar: "bg-red-500",
    text: "text-red-500",
    icon: <CircleSlash className="size-3.5 text-red-500" />,
  },
  review: {
    label: "needs review",
    bar: "bg-purple-500",
    text: "text-purple-500",
    icon: <Eye className="size-3.5 text-purple-500" />,
  },
  open: {
    label: "open",
    bar: "bg-muted-foreground/30",
    text: "text-muted-foreground",
    icon: <Circle className="size-3.5 text-muted-foreground/70" />,
  },
  cancelled: {
    label: "cancelled",
    bar: "bg-muted-foreground/15",
    text: "text-muted-foreground/60",
    icon: <CircleDashed className="size-3.5 text-muted-foreground/50" />,
  },
};

/**
 * Segmented progress bar over the five state GROUPS — every status
 * string has been resolved through the project's registry before it
 * lands here.
 */
export function GroupBar({
  groups,
  className,
}: {
  groups: GroupCounts;
  className?: string;
}) {
  if (groups.total === 0) return null;
  return (
    <div
      className={cn(
        "flex h-1.5 overflow-hidden rounded-full bg-muted",
        className,
      )}
    >
      {GROUP_SEGMENT_ORDER.map((g) =>
        groups[g] === 0 ? null : (
          <div
            key={g}
            className={GROUP_META[g].bar}
            style={{ width: `${(groups[g] / groups.total) * 100}%` }}
          />
        ),
      )}
    </div>
  );
}

/**
 * The compact swarm rollup: group-segmented bar + "11/24 done · 5 in
 * progress · 2 blocked · 48 pts". This is how a 24-task swarm reads at
 * a glance on its workstream's strip.
 */
export function SwarmStrip({
  counts,
  groups,
  points,
  className,
}: {
  counts: SwarmCounts;
  groups: GroupCounts;
  points: number;
  className?: string;
}) {
  if (counts.total === 0) return null;
  const bits: string[] = [`${counts.done}/${counts.total} done`];
  if (counts.running > 0) bits.push(`${counts.running} in progress`);
  if (counts.review > 0) bits.push(`${counts.review} review`);
  if (counts.blocked > 0) bits.push(`${counts.blocked} blocked`);
  if (points > 0) bits.push(`${points} pts`);
  return (
    <div className={cn("flex min-w-0 items-center gap-2.5", className)}>
      <GroupBar groups={groups} className="w-28 shrink-0" />
      <span className="text-muted-foreground truncate text-[11px] tabular-nums">
        {bits.join(" · ")}
      </span>
    </div>
  );
}

// ── tiny fixed-row virtualizer ──────────────────────────────────────

/**
 * Windowed list over fixed-height rows. Renders only what's visible
 * (+overscan) inside its own scroll container — the page never mounts
 * the whole swarm at once.
 */
export function VirtualList<T>({
  items,
  rowHeight,
  maxHeight,
  renderRow,
  className,
}: {
  items: T[];
  rowHeight: number;
  maxHeight: number;
  renderRow: (item: T, index: number, style: CSSProperties) => ReactNode;
  className?: string;
}) {
  const [scrollTop, setScrollTop] = useState(0);
  const ref = useRef<HTMLDivElement>(null);
  const onScroll = useCallback(() => {
    if (ref.current) setScrollTop(ref.current.scrollTop);
  }, []);

  const viewport = Math.min(maxHeight, items.length * rowHeight);
  const overscan = 6;
  const first = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const last = Math.min(
    items.length,
    Math.ceil((scrollTop + viewport) / rowHeight) + overscan,
  );

  return (
    <div
      ref={ref}
      onScroll={onScroll}
      className={cn("overflow-y-auto overscroll-contain", className)}
      style={{ maxHeight }}
    >
      <div
        className="relative w-full"
        style={{ height: items.length * rowHeight }}
      >
        {items.slice(first, last).map((item, i) =>
          renderRow(item, first + i, {
            position: "absolute",
            top: (first + i) * rowHeight,
            left: 0,
            right: 0,
            height: rowHeight,
          }),
        )}
      </div>
    </div>
  );
}

// ── relations ───────────────────────────────────────────────────────

const RELATION_META: Record<
  RelationView["kind"],
  { label: string; icon: ReactNode; tone: string }
> = {
  Blocks: {
    label: "blocks",
    icon: <Ban className="size-2.5" />,
    tone: "text-red-500 border-red-500/40",
  },
  Duplicate: {
    label: "duplicate of",
    icon: <CopyX className="size-2.5" />,
    tone: "text-muted-foreground border-border",
  },
  Implements: {
    label: "implements",
    icon: <Wrench className="size-2.5" />,
    tone: "text-blue-500 border-blue-500/40",
  },
  Relates: {
    label: "relates to",
    icon: <Link2 className="size-2.5" />,
    tone: "text-muted-foreground border-border",
  },
};

/** Typed-relation chips (blocks / duplicate / implements / relates). */
export function RelationChips({
  task,
  byId,
}: {
  task: TaskInfo;
  byId: Map<string, TaskInfo>;
}) {
  const relations = taskRelations(task, byId);
  if (relations.length === 0) return null;
  return (
    <span className="flex shrink-0 items-center gap-1">
      {relations.map((r, i) => {
        const meta = RELATION_META[r.kind];
        return (
          <Tooltip key={`${r.kind}-${r.targetId}-${i}`}>
            <TooltipTrigger asChild>
              <span
                className={cn(
                  "flex cursor-default items-center gap-0.5 rounded border px-1 py-px text-[9px] font-medium",
                  meta.tone,
                )}
              >
                {meta.icon}
                {meta.label}
              </span>
            </TooltipTrigger>
            <TooltipContent>
              {meta.label} {r.targetTitle ?? r.targetId.slice(0, 8)}
            </TooltipContent>
          </Tooltip>
        );
      })}
    </span>
  );
}

// ── dense swarm rows (expanded workstream) ──────────────────────────

export const SWARM_ROW_HEIGHT = 34;

export function ClaimantChip({ task }: { task: TaskInfo }) {
  const claimant = agentClaimant(task);
  if (!claimant) return null;
  return (
    <span className="text-muted-foreground flex shrink-0 items-center gap-1 font-mono text-[10px]">
      <Bot className="size-3" />
      {claimantKey(claimant)}
    </span>
  );
}

export function SwarmRow({
  task,
  byId,
  states,
  style,
}: {
  task: TaskInfo;
  byId: Map<string, TaskInfo>;
  states: StatesConfig | null;
  style: CSSProperties;
}) {
  const bucket = statusBucket(task, byId, states);
  return (
    <div
      style={style}
      className="hover:bg-accent/50 flex items-center gap-2 border-b border-border/50 px-3 text-xs last:border-0"
    >
      <span className="shrink-0">{BUCKET_META[bucket].icon}</span>
      <span
        className={cn(
          "min-w-0 flex-1 truncate",
          bucket === "done" && "text-muted-foreground",
          bucket === "cancelled" && "text-muted-foreground/60 line-through",
        )}
      >
        {task.title || task.path}
      </span>
      <RelationChips task={task} byId={byId} />
      <ClaimantChip task={task} />
      <span className="text-muted-foreground/70 w-7 shrink-0 text-right font-mono text-[10px] tabular-nums">
        {relativeAge(task.date_created)}
      </span>
    </div>
  );
}

/**
 * Sorted swarm for the expanded list: attention first (review,
 * blocked, running), then open, done, cancelled.
 */
export function sortSwarm(
  swarm: TaskInfo[],
  byId: Map<string, TaskInfo>,
  states: StatesConfig | null,
): TaskInfo[] {
  return [...swarm].sort(
    (a, b) =>
      bucketRank(statusBucket(a, byId, states)) -
        bucketRank(statusBucket(b, byId, states)) ||
      a.title.localeCompare(b.title, undefined, { numeric: true }),
  );
}

const WS_STATUS_TONE: Record<string, string> = {
  "in-progress": "text-blue-500 border-blue-500/40",
  done: "text-emerald-500 border-emerald-500/40",
  cancelled: "text-muted-foreground/60 border-border",
  paused: "text-amber-500 border-amber-500/40",
};

/**
 * A workstream's human-scale strip: parent title (links to the detail
 * route), lead, member rollup + group-segmented bar — expandable into
 * its dense member list. Collapsed by default.
 */
export function WorkstreamRow({
  org,
  model,
  byId,
  states,
}: {
  org: string;
  model: WorkstreamModel;
  byId: Map<string, TaskInfo>;
  states: StatesConfig | null;
}) {
  const { workstream: ws, members, counts, groups, points } = model;
  const [open, setOpen] = useState(false);
  const sorted = useMemo(
    () => (open ? sortSwarm(members, byId, states) : []),
    [open, members, byId, states],
  );

  return (
    <div className="border-b last:border-0">
      <div className="hover:bg-accent/50 flex w-full items-center gap-2.5 px-3 py-2.5 transition-colors">
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          className="flex min-w-0 flex-1 items-center gap-2.5 text-left"
          aria-expanded={open}
        >
          <ChevronRight
            className={cn(
              "text-muted-foreground size-3.5 shrink-0 transition-transform",
              open && "rotate-90",
            )}
          />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-sm font-medium">
              {ws.title}
            </span>
          </span>
        </button>
        <Badge
          variant="outline"
          className={cn(
            "shrink-0 font-mono text-[10px]",
            WS_STATUS_TONE[ws.status] ?? "text-muted-foreground",
          )}
        >
          {ws.status}
        </Badge>
        {ws.lead && (
          <span className="text-muted-foreground hidden shrink-0 items-center gap-1 font-mono text-[10px] md:flex">
            <Bot className="size-3" />
            {agentRefLabel(ws.lead)}
          </span>
        )}
        <SwarmStrip
          counts={counts}
          groups={groups}
          points={points}
          className="hidden w-80 justify-end sm:flex"
        />
        <Link
          to="/workstreams/$org/$workstreamId"
          params={{ org, workstreamId: String(ws.id) }}
          className="text-muted-foreground hover:text-primary shrink-0 transition-colors"
          title="Open workstream"
        >
          <ArrowUpRight className="size-3.5" />
        </Link>
      </div>
      {open &&
        (members.length === 0 ? (
          <p className="text-muted-foreground border-t bg-card/50 px-9 py-3 text-xs">
            No tasks attached to this workstream yet.
          </p>
        ) : (
          <VirtualList
            items={sorted}
            rowHeight={SWARM_ROW_HEIGHT}
            maxHeight={SWARM_ROW_HEIGHT * 10.5}
            className="border-t bg-card/50"
            renderRow={(t, _i, style) => (
              <SwarmRow
                key={String(t.id)}
                task={t}
                byId={byId}
                states={states}
                style={style}
              />
            )}
          />
        ))}
    </div>
  );
}

// ── review lane ─────────────────────────────────────────────────────

/**
 * Parked review-required work, surfaced above everything else — the
 * human's queue. Only renders when such items exist.
 */
export function ReviewLane({
  tasks,
  byId,
  states,
}: {
  tasks: TaskInfo[];
  byId: Map<string, TaskInfo>;
  states: StatesConfig | null;
}) {
  if (tasks.length === 0) return null;
  return (
    <section className="rounded-lg border border-purple-500/40 bg-purple-500/5">
      <header className="flex items-center gap-2 border-b border-purple-500/20 px-3 py-2">
        <Eye className="size-3.5 text-purple-500" />
        <h3 className="text-sm font-medium">Needs your review</h3>
        <Badge className="bg-purple-500/15 text-purple-500" variant="secondary">
          {tasks.length}
        </Badge>
      </header>
      <div className="flex flex-col">
        {tasks.map((t) => (
          <div
            key={String(t.id)}
            className="flex items-center gap-2.5 border-b border-purple-500/10 px-3 py-2 text-xs last:border-0"
          >
            <AlertTriangle className="size-3.5 shrink-0 text-purple-500" />
            <span className="min-w-0 flex-1">
              <span className="block truncate font-medium">
                {t.title || t.path}
              </span>
              <span className="text-muted-foreground block truncate">
                {reviewReason(t, states)}
              </span>
            </span>
            <RelationChips task={t} byId={byId} />
            <ClaimantChip task={t} />
            <span className="text-muted-foreground/70 shrink-0 font-mono text-[10px]">
              {relativeAge(t.date_modified ?? t.date_created)}
            </span>
          </div>
        ))}
      </div>
    </section>
  );
}

// ── the Agents lens (kanban by claimant) ────────────────────────────

const CARD_HEIGHT = 64;
const UNCLAIMED = "· unclaimed";

function AgentCard({
  task,
  byId,
  states,
  style,
}: {
  task: TaskInfo;
  byId: Map<string, TaskInfo>;
  states: StatesConfig | null;
  style: CSSProperties;
}) {
  const bucket = statusBucket(task, byId, states);
  return (
    <div style={style} className="px-1.5 py-[3px]">
      <div className="bg-card hover:border-ring/40 flex h-full flex-col justify-center gap-1 rounded-md border px-2.5 py-1.5 transition-colors">
        <div className="flex items-center gap-1.5">
          <span className="shrink-0">{BUCKET_META[bucket].icon}</span>
          <span
            className={cn(
              "min-w-0 flex-1 truncate text-xs font-medium",
              bucket === "done" && "text-muted-foreground",
              bucket === "cancelled" &&
                "text-muted-foreground/60 line-through",
            )}
          >
            {task.title || task.path}
          </span>
        </div>
        <div className="flex items-center justify-between gap-2 pl-5">
          <span
            className={cn("text-[10px]", BUCKET_META[bucket].text)}
          >
            {task.status}
          </span>
          <span className="flex items-center gap-1.5">
            <RelationChips task={task} byId={byId} />
            <span className="text-muted-foreground/70 font-mono text-[10px] tabular-nums">
              {relativeAge(task.date_created)}
            </span>
          </span>
        </div>
      </div>
    </div>
  );
}

/**
 * The full agent swarm grouped by CLAIMANT: review lane on top, then
 * one column per claiming agent ref (`claude@fable-5`, `hermes@h4`,
 * …) plus an Unclaimed column, each with virtualized compact cards.
 * "Who's holding what" is the agent-ops question this lens answers.
 */
export function AgentsBoard({ model }: { model: ProjectModel }) {
  const { agents, byId, review, states } = model;
  const columns = useMemo(() => {
    const map = new Map<string, TaskInfo[]>();
    for (const t of agents) {
      if (statusBucket(t, byId, states) === "review") continue; // the lane above
      const claimant = agentClaimant(t);
      const key = claimant ? claimantKey(claimant) : UNCLAIMED;
      const bucket = map.get(key);
      if (bucket) bucket.push(t);
      else map.set(key, [t]);
    }
    for (const [, list] of map) {
      list.sort(
        (a, b) =>
          bucketRank(statusBucket(a, byId, states)) -
            bucketRank(statusBucket(b, byId, states)) ||
          a.title.localeCompare(b.title, undefined, { numeric: true }),
      );
    }
    // Busiest agents first; the unclaimed pool always last.
    return [...map.entries()].sort((a, b) => {
      if (a[0] === UNCLAIMED) return 1;
      if (b[0] === UNCLAIMED) return -1;
      return b[1].length - a[1].length || a[0].localeCompare(b[0]);
    });
  }, [agents, byId, states]);

  if (agents.length === 0) {
    return (
      <p className="text-muted-foreground rounded-lg border border-dashed px-4 py-12 text-center text-sm">
        No agent subtasks in this project — dispatch some with{" "}
        <span className="font-mono">task triage</span>.
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      <ReviewLane tasks={review} byId={byId} states={states} />
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        {columns.map(([claimant, cards]) => {
          const running = cards.filter(
            (t) => statusBucket(t, byId, states) === "running",
          ).length;
          return (
            <section
              key={claimant}
              className="bg-muted/40 flex min-w-0 flex-col rounded-lg border"
            >
              <header className="flex items-center gap-1.5 px-3 py-2">
                {claimant === UNCLAIMED ? (
                  <Circle className="size-3.5 text-muted-foreground/70" />
                ) : (
                  <Bot className="size-3.5 text-muted-foreground" />
                )}
                <h3 className="min-w-0 truncate font-mono text-xs font-medium">
                  {claimant === UNCLAIMED ? "Unclaimed" : claimant}
                </h3>
                {running > 0 && (
                  <span className="size-1.5 shrink-0 rounded-full bg-blue-500" />
                )}
                <span className="text-muted-foreground ml-auto font-mono text-[10px] tabular-nums">
                  {cards.length}
                </span>
              </header>
              <VirtualList
                items={cards}
                rowHeight={CARD_HEIGHT}
                maxHeight={CARD_HEIGHT * 8.5}
                className="pb-1.5"
                renderRow={(t, _i, style) => (
                  <AgentCard
                    key={String(t.id)}
                    task={t}
                    byId={byId}
                    states={states}
                    style={style}
                  />
                )}
              />
            </section>
          );
        })}
      </div>
    </div>
  );
}

/** Tooltipped legend chip used by the header progress visual. */
export function BucketChip({
  bucket,
  count,
}: {
  bucket: StatusBucket;
  count: number;
}) {
  if (count === 0) return null;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          className={cn(
            "flex cursor-default items-center gap-1 text-[11px] tabular-nums",
            BUCKET_META[bucket].text,
          )}
        >
          <span
            className={cn("size-1.5 rounded-full", BUCKET_META[bucket].bar)}
          />
          {count} {BUCKET_META[bucket].label}
        </span>
      </TooltipTrigger>
      <TooltipContent>
        {count} {BUCKET_META[bucket].label}
        {bucket === "blocked" && " (unresolved blockers)"}
      </TooltipContent>
    </Tooltip>
  );
}
