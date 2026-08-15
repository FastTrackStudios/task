/**
 * /projects/$org/$projectId — the project overview, designed from
 * scratch (Linear's project page is the reference: identity header,
 * one combined progress visual, properties rail, scoped issue views).
 *
 * The core problem: ONE project mixes a handful of human-scale tasks
 * with an agent-scale swarm of dispatched subtasks. The default lens
 * shows the HUMAN layer — each **Workstream** (the parent-with-swarm
 * construct, a real WorkstreamService entity) rolls its members up
 * into a collapsed strip — and the Agents lens opens the full swarm
 * as a claimant-grouped kanban. Parked review-required work gets its
 * own lane above everything.
 *
 * Status is never string-matched here: every status name resolves to
 * a state GROUP through the project's registry (`ProjectInfo.states`,
 * falling back to the default registry), and the bars/columns render
 * groups.
 *
 * This is the design the Dioxus app will port; visual parity rides on
 * the shared shadcn primitives + theme tokens.
 */
import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import {
  AlertCircle,
  ArrowLeft,
  ArrowUpRight,
  Bot,
  CalendarClock,
  Layers,
  Minus,
  RefreshCw,
  Target,
  User,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import type { StatesConfig } from "@/generated/projectservicerpc.generated";
import type { TaskInfo } from "@/generated/taskservicerpc.generated";
import { AccountAvatar } from "@/lib/auth";
import {
  projectMilestonesQuery,
  projectQuery,
  projectTasksQuery,
  projectWorkstreamsQuery,
} from "@/lib/queries";
import { resolveStateGroup, stateDefFor } from "@/lib/states";
import { cn } from "@/lib/utils";
import {
  buildModel,
  completionPct,
  deriveHealth,
  estimateLabel,
  formatDate,
  isAgentRef,
  milestoneProgress,
  rollupEstimates,
  statusBucket,
  type GroupCounts,
  type Health,
  type MilestoneProgress,
  type ProjectModel,
  type SwarmCounts,
} from "./model";
import {
  AgentsBoard,
  BucketChip,
  BUCKET_META,
  GROUP_META,
  GROUP_SEGMENT_ORDER,
  RelationChips,
  ReviewLane,
  WorkstreamRow,
} from "./swarm";

// ── shared bits ─────────────────────────────────────────────────────

function ErrorNote({
  error,
  onRetry,
}: {
  error: unknown;
  onRetry?: () => void;
}) {
  return (
    <div className="border-destructive/30 text-destructive bg-destructive/5 flex items-center gap-2 rounded-lg border px-4 py-3 text-sm">
      <AlertCircle className="size-4 shrink-0" />
      <span className="flex-1">
        {error instanceof Error ? error.message : String(error)}
      </span>
      {onRetry && (
        <Button variant="outline" size="sm" onClick={onRetry} className="gap-1">
          <RefreshCw className="size-3" /> Retry
        </Button>
      )}
    </div>
  );
}

function statusVariant(
  status: string,
): "default" | "secondary" | "destructive" | "outline" {
  switch (status) {
    case "in-progress":
    case "active":
      return "default";
    case "blocked":
    case "cancelled":
      return "destructive";
    case "done":
    case "completed":
      return "secondary";
    default:
      return "outline";
  }
}

/**
 * A task-status pill that goes through the state registry: group
 * decides the tone, a registered custom color (e.g. the test org's
 * `qa-review` #ffaa00) overrides it.
 */
export function StatusPill({
  status,
  states,
}: {
  status: string;
  states: StatesConfig | null;
}) {
  const def = stateDefFor(states, status);
  const group = resolveStateGroup(states, status);
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 rounded border px-1.5 py-px font-mono text-[10px]",
        GROUP_META[group].text,
      )}
      style={def?.color ? { color: def.color, borderColor: `${def.color}66` } : undefined}
    >
      {status}
    </span>
  );
}

const HEALTH_TONE: Record<Health["tone"], string> = {
  ok: "bg-emerald-500",
  warn: "bg-amber-500",
  bad: "bg-red-500",
};

function HealthDot({ health }: { health: Health }) {
  return (
    <span className="flex items-center gap-1.5 text-xs font-medium">
      <span
        className={cn(
          "size-2 rounded-full",
          HEALTH_TONE[health.tone],
          health.tone !== "ok" && "animate-pulse",
        )}
      />
      {health.label}
    </span>
  );
}

/** Linear-style priority glyph: three bars of increasing height. */
function PriorityGlyph({ priority }: { priority: string }) {
  const level =
    priority === "urgent"
      ? 3
      : priority === "high"
        ? 3
        : priority === "normal" || priority === "medium"
          ? 2
          : priority === "low"
            ? 1
            : 0;
  if (level === 0) return <Minus className="text-muted-foreground size-3" />;
  const tone =
    priority === "urgent"
      ? "bg-red-500"
      : priority === "high"
        ? "bg-orange-400"
        : "bg-muted-foreground";
  return (
    <span className="flex h-3 items-end gap-[2px]" title={priority}>
      {[1, 2, 3].map((bar) => (
        <span
          key={bar}
          className={cn(
            "w-[3px] rounded-[1px]",
            bar <= level ? tone : "bg-muted-foreground/25",
          )}
          style={{ height: `${4 + bar * 3}px` }}
        />
      ))}
    </span>
  );
}

// ── the ONE progress visual ─────────────────────────────────────────

/**
 * Combined progress: a single bar segmented by state GROUP (across
 * EVERY task, swarm included) with milestone markers riding on top of
 * it, plus blocked/review overlay chips. One glance = delivery state
 * + where the checkpoints sit.
 */
function CombinedProgress({
  counts,
  groups,
  milestones,
}: {
  counts: SwarmCounts;
  groups: GroupCounts;
  milestones: MilestoneProgress[];
}) {
  const denom = groups.total - groups.Cancelled;
  const pct = completionPct(counts);
  const doneMs = milestones.filter((m) => m.pct >= 100).length;
  // Milestones order by target date when present (undated ones last,
  // stable), and spread evenly along the bar — the marker carries its
  // own per-milestone completion in a tooltip.
  const ordered = [...milestones].sort((a, b) =>
    a.due === b.due ? 0 : a.due === null ? 1 : b.due === null ? -1 : a.due < b.due ? -1 : 1,
  );

  return (
    <div className="flex flex-col gap-1.5">
      <div className="relative">
        <div className="flex h-2 overflow-hidden rounded-full bg-muted">
          {denom > 0 &&
            GROUP_SEGMENT_ORDER.map((g) =>
              g === "Cancelled" || groups[g] === 0 ? null : (
                <div
                  key={g}
                  className={cn(GROUP_META[g].bar, "transition-all")}
                  style={{ width: `${(groups[g] / denom) * 100}%` }}
                />
              ),
            )}
        </div>
        {ordered.map((m, i) => {
          const left = ((i + 1) / (ordered.length + 1)) * 100;
          const reached = m.pct >= 100;
          return (
            <Tooltip key={String(m.milestone.id)}>
              <TooltipTrigger asChild>
                <span
                  className={cn(
                    "absolute top-1/2 size-2.5 -translate-x-1/2 -translate-y-1/2 rotate-45 cursor-default rounded-[2px] border-2 border-background shadow-sm",
                    reached ? "bg-emerald-500" : "bg-foreground/80",
                  )}
                  style={{ left: `${left}%` }}
                />
              </TooltipTrigger>
              <TooltipContent className="flex flex-col gap-0.5">
                <span className="font-medium">{m.milestone.title}</span>
                <span className="text-[11px] opacity-80">
                  {m.counts.total > 0
                    ? `${m.counts.done}/${m.counts.total} tasks · ${m.pct}%`
                    : "no linked tasks"}
                  {m.due ? ` · due ${formatDate(m.due)}` : " · no target date"}
                  {m.overdue ? " · OVERDUE" : ""}
                </span>
              </TooltipContent>
            </Tooltip>
          );
        })}
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="text-sm font-semibold tabular-nums">{pct}%</span>
        <span className="text-muted-foreground text-[11px] tabular-nums">
          {groups.Completed}/{denom} tasks
        </span>
        {(["Started", "Unstarted", "Backlog"] as const).map((g) =>
          groups[g] === 0 ? null : (
            <span
              key={g}
              className={cn(
                "flex items-center gap-1 text-[11px] tabular-nums",
                GROUP_META[g].text,
              )}
            >
              <span className={cn("size-1.5 rounded-full", GROUP_META[g].bar)} />
              {groups[g]} {GROUP_META[g].label}
            </span>
          ),
        )}
        <BucketChip bucket="review" count={counts.review} />
        <BucketChip bucket="blocked" count={counts.blocked} />
        {milestones.length > 0 && (
          <span className="text-muted-foreground ml-auto flex items-center gap-1 text-[11px] tabular-nums">
            <Target className="size-3" />
            {doneMs}/{milestones.length} milestones
          </span>
        )}
      </div>
    </div>
  );
}

// ── properties rail ─────────────────────────────────────────────────

function RailRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-3 py-1.5">
      <dt className="text-muted-foreground shrink-0 text-xs">{label}</dt>
      <dd className="flex min-w-0 items-center justify-end gap-1.5 text-xs font-medium">
        {children}
      </dd>
    </div>
  );
}

function PropertiesRail({
  project,
  model,
  milestones,
  health,
}: {
  project: import("@/generated/projectservicerpc.generated").ProjectInfo;
  model: ProjectModel;
  milestones: MilestoneProgress[];
  health: Health;
}) {
  const estimates = rollupEstimates(
    [...model.byId.values()],
    model.byId,
    model.states,
  );
  const tags = new Set<string>(project.tags);

  return (
    <aside className="flex w-full flex-col gap-4 lg:w-60 lg:shrink-0">
      <section>
        <h3 className="text-muted-foreground mb-1 text-[11px] font-medium uppercase tracking-wide">
          Properties
        </h3>
        <dl className="divide-y divide-border/60">
          <RailRow label="Status">
            <Badge variant={statusVariant(project.status)}>
              {project.status}
            </Badge>
          </RailRow>
          <RailRow label="Health">
            <HealthDot health={health} />
          </RailRow>
          <RailRow label="Priority">
            <PriorityGlyph priority={project.priority} />
            <span className="capitalize">{project.priority || "none"}</span>
          </RailRow>
          <RailRow label="Lead">
            {project.lead ? (
              <>
                <AccountAvatar name={project.lead} email="" size={16} />
                <span className="truncate">{project.lead}</span>
              </>
            ) : (
              <span className="text-muted-foreground font-normal">
                Unassigned
              </span>
            )}
          </RailRow>
          <RailRow label="Target">
            {project.target_date != null ? (
              <span className="flex items-center gap-1">
                <CalendarClock className="size-3" />
                {formatDate(project.target_date)}
              </span>
            ) : (
              <span className="text-muted-foreground font-normal">
                No target date
              </span>
            )}
          </RailRow>
          <RailRow label="Estimate">
            {estimates.total > 0 ? (
              <span className="tabular-nums">
                {estimates.done}/{estimates.total} pts
              </span>
            ) : (
              <span className="text-muted-foreground font-normal">
                Unestimated
              </span>
            )}
          </RailRow>
          <RailRow label="Workstreams">
            {model.workstreams.length > 0 ? (
              <span className="flex items-center gap-1 tabular-nums">
                <Layers className="size-3" />
                {model.workstreams.length}
              </span>
            ) : (
              <span className="text-muted-foreground font-normal">None</span>
            )}
          </RailRow>
          <RailRow label="States">
            <span className="text-muted-foreground font-normal">
              {project.states && project.states.length > 0
                ? `${project.states.length} custom`
                : "default registry"}
            </span>
          </RailRow>
          <RailRow label="Tags">
            {tags.size > 0 ? (
              <span className="flex max-w-44 flex-wrap justify-end gap-1">
                {[...tags].map((t) => (
                  <Badge key={t} variant="outline" className="text-[10px]">
                    {t}
                  </Badge>
                ))}
              </span>
            ) : (
              <span className="text-muted-foreground font-normal">—</span>
            )}
          </RailRow>
          <RailRow label="Links">
            {project.same_as ? (
              <a
                href={project.same_as}
                target="_blank"
                rel="noreferrer"
                className="text-primary flex items-center gap-1 truncate hover:underline"
              >
                <ArrowUpRight className="size-3 shrink-0" />
                <span className="truncate">{project.same_as}</span>
              </a>
            ) : (
              <span className="text-muted-foreground font-normal">—</span>
            )}
          </RailRow>
        </dl>
      </section>

      <section>
        <h3 className="text-muted-foreground mb-2 text-[11px] font-medium uppercase tracking-wide">
          Milestones
        </h3>
        {milestones.length === 0 ? (
          <p className="text-muted-foreground rounded-md border border-dashed px-3 py-4 text-center text-xs">
            No milestones yet.
          </p>
        ) : (
          <div className="flex flex-col gap-3">
            {milestones.map((m) => (
              <div key={String(m.milestone.id)} className="flex flex-col gap-1">
                <div className="flex items-baseline justify-between gap-2">
                  <span className="truncate text-xs font-medium">
                    {m.milestone.title}
                  </span>
                  <span className="text-muted-foreground shrink-0 font-mono text-[10px] tabular-nums">
                    {m.counts.total > 0 ? `${m.pct}%` : "—"}
                  </span>
                </div>
                <div className="flex h-1 overflow-hidden rounded-full bg-muted">
                  <div
                    className={cn(
                      "rounded-full transition-all",
                      m.overdue ? "bg-red-500" : "bg-emerald-500",
                    )}
                    style={{ width: `${m.pct}%` }}
                  />
                </div>
                <span
                  className={cn(
                    "text-[10px]",
                    m.overdue ? "font-medium text-red-500" : "text-muted-foreground",
                  )}
                >
                  {m.counts.total > 0
                    ? `${m.counts.done}/${m.counts.total} tasks`
                    : "no linked tasks"}
                  {m.due
                    ? ` · ${m.overdue ? "overdue " : ""}${formatDate(m.due)}`
                    : " · no target date"}
                </span>
              </div>
            ))}
          </div>
        )}
      </section>
    </aside>
  );
}

// ── human-layer task rows ───────────────────────────────────────────

function humanAssignee(t: TaskInfo): string | null {
  for (const a of t.workflow?.assignees ?? []) {
    if (!isAgentRef(a)) {
      const raw = a as unknown as {
        display_name?: string | null;
        user_id?: string;
      };
      return raw.display_name || raw.user_id || null;
    }
  }
  return null;
}

function HumanRow({
  task,
  byId,
  states,
}: {
  task: TaskInfo;
  byId: Map<string, TaskInfo>;
  states: StatesConfig | null;
}) {
  const bucket = statusBucket(task, byId, states);
  const est = estimateLabel(task.workflow?.estimate);
  const assignee = humanAssignee(task);
  return (
    <div className="hover:bg-accent/50 flex items-center gap-2.5 border-b px-3 py-2.5 transition-colors last:border-0">
      {BUCKET_META[bucket].icon}
      <span
        className={cn(
          "min-w-0 flex-1 truncate text-sm",
          bucket === "done" && "text-muted-foreground",
          bucket === "cancelled" && "text-muted-foreground/60 line-through",
        )}
      >
        {task.title || task.path}
      </span>
      <RelationChips task={task} byId={byId} />
      <StatusPill status={task.status} states={states} />
      {task.due && (
        <span className="text-muted-foreground hidden shrink-0 items-center gap-1 text-[11px] sm:flex">
          <CalendarClock className="size-3" />
          {formatDate(task.due)}
        </span>
      )}
      {est && (
        <Badge
          variant="outline"
          className="text-muted-foreground hidden shrink-0 font-mono text-[10px] sm:inline-flex"
        >
          {est}
        </Badge>
      )}
      <PriorityGlyph priority={task.priority} />
      {assignee ? (
        <AccountAvatar name={assignee} email="" size={18} />
      ) : (
        <span
          className="border-muted-foreground/30 flex size-[18px] shrink-0 items-center justify-center rounded-full border border-dashed"
          title="Unassigned"
        >
          <User className="text-muted-foreground/50 size-2.5" />
        </span>
      )}
    </div>
  );
}

// ── page ────────────────────────────────────────────────────────────

function HeaderSkeleton() {
  return (
    <div className="flex flex-col gap-4">
      <Skeleton className="h-4 w-56" />
      <Skeleton className="h-8 w-80" />
      <Skeleton className="h-2 w-full" />
      <div className="flex gap-2">
        <Skeleton className="h-4 w-24" />
        <Skeleton className="h-4 w-32" />
      </div>
    </div>
  );
}

function ListSkeleton({ rows }: { rows: number }) {
  return (
    <div className="flex flex-col gap-2">
      {Array.from({ length: rows }, (_, i) => (
        <Skeleton key={i} className="h-10 w-full" />
      ))}
    </div>
  );
}

export function ProjectOverviewPage() {
  const { org, projectId } = useParams({ from: "/projects/$org/$projectId" });
  const project = useQuery(projectQuery(org, projectId));
  const tasks = useQuery(projectTasksQuery(org, projectId));
  const workstreamsQ = useQuery(projectWorkstreamsQuery(org, projectId));
  const milestonesQ = useQuery(projectMilestonesQuery(org, projectId));
  const [lens, setLens] = useState("overview");

  const states = project.data?.states ?? null;
  const model = useMemo(
    () => buildModel(tasks.data ?? [], workstreamsQ.data ?? [], states),
    [tasks.data, workstreamsQ.data, states],
  );
  const milestones = useMemo(
    () =>
      milestoneProgress(
        milestonesQ.data ?? [],
        tasks.data ?? [],
        model.byId,
        states,
      ),
    [milestonesQ.data, tasks.data, model.byId, states],
  );
  const health = useMemo(
    () => deriveHealth(model.all, milestones),
    [model.all, milestones],
  );

  const agentTotal = model.agents.length;

  return (
    <div className="flex flex-col gap-5">
      <div className="flex items-center gap-2 text-sm">
        <Button asChild variant="ghost" size="sm" className="-ml-2">
          <Link to="/projects">
            <ArrowLeft className="size-3.5" /> Projects
          </Link>
        </Button>
        <Separator orientation="vertical" className="h-4" />
        <Badge variant="secondary" className="font-mono text-[10px]">
          {org}
        </Badge>
        {project.data && (
          <span className="text-muted-foreground truncate font-mono text-xs">
            {project.data.path}
          </span>
        )}
      </div>

      {project.isPending ? (
        <HeaderSkeleton />
      ) : project.isError ? (
        <ErrorNote
          error={project.error}
          onRetry={() => void project.refetch()}
        />
      ) : (
        <div className="flex flex-col gap-6 lg:flex-row">
          {/* main column */}
          <div className="flex min-w-0 flex-1 flex-col gap-5">
            <header className="flex flex-col gap-3">
              <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
                <h1 className="text-2xl font-semibold tracking-tight">
                  {project.data.title}
                </h1>
                <Badge variant={statusVariant(project.data.status)}>
                  {project.data.status}
                </Badge>
                <HealthDot health={health} />
              </div>
              <div className="text-muted-foreground flex flex-wrap items-center gap-x-4 gap-y-1 text-xs">
                {project.data.lead ? (
                  <span className="flex items-center gap-1.5">
                    <AccountAvatar name={project.data.lead} email="" size={16} />
                    {project.data.lead}
                  </span>
                ) : (
                  <span className="flex items-center gap-1.5">
                    <User className="size-3" /> No lead
                  </span>
                )}
                <span className="flex items-center gap-1.5">
                  <CalendarClock className="size-3" />
                  {project.data.target_date != null
                    ? `Target ${formatDate(project.data.target_date)}`
                    : "No target date"}
                </span>
                {model.workstreams.length > 0 && (
                  <span className="flex items-center gap-1.5">
                    <Layers className="size-3" />
                    {model.workstreams.length} workstreams
                  </span>
                )}
                {agentTotal > 0 && (
                  <span className="flex items-center gap-1.5">
                    <Bot className="size-3" />
                    {agentTotal} agent subtasks
                  </span>
                )}
              </div>
              {tasks.isSuccess && (
                <CombinedProgress
                  counts={model.all}
                  groups={model.allGroups}
                  milestones={milestones}
                />
              )}
              {tasks.isPending && <Skeleton className="h-9 w-full" />}
            </header>

            <Tabs value={lens} onValueChange={setLens}>
              <TabsList>
                <TabsTrigger value="overview">Overview</TabsTrigger>
                <TabsTrigger value="agents" className="gap-1.5">
                  Agents
                  {tasks.isSuccess && (
                    <span className="text-muted-foreground font-mono text-[10px] tabular-nums">
                      {agentTotal}
                    </span>
                  )}
                  {model.review.length > 0 && (
                    <span className="size-1.5 rounded-full bg-purple-500" />
                  )}
                </TabsTrigger>
              </TabsList>

              <TabsContent value="overview" className="mt-3">
                {tasks.isPending ? (
                  <ListSkeleton rows={6} />
                ) : tasks.isError ? (
                  <ErrorNote
                    error={tasks.error}
                    onRetry={() => void tasks.refetch()}
                  />
                ) : model.all.total === 0 && model.workstreams.length === 0 ? (
                  <p className="text-muted-foreground rounded-lg border border-dashed px-4 py-12 text-center text-sm">
                    No tasks point at this project yet.
                  </p>
                ) : (
                  <div className="flex flex-col gap-4">
                    <ReviewLane
                      tasks={model.review}
                      byId={model.byId}
                      states={states}
                    />

                    {workstreamsQ.isError && (
                      <ErrorNote
                        error={workstreamsQ.error}
                        onRetry={() => void workstreamsQ.refetch()}
                      />
                    )}

                    {model.workstreams.length > 0 && (
                      <section className="overflow-hidden rounded-lg border">
                        <header className="bg-muted/50 flex items-center gap-2 border-b px-3 py-2">
                          <Layers className="text-muted-foreground size-3.5" />
                          <h2 className="text-xs font-medium">Workstreams</h2>
                          <span className="text-muted-foreground font-mono text-[10px] tabular-nums">
                            {model.workstreams.length}
                          </span>
                          <span className="text-muted-foreground/70 ml-auto text-[10px]">
                            each strip rolls up its swarm — expand inline or
                            open the stream
                          </span>
                        </header>
                        {model.workstreams.map((w) => (
                          <WorkstreamRow
                            key={String(w.workstream.id)}
                            org={org}
                            model={w}
                            byId={model.byId}
                            states={states}
                          />
                        ))}
                      </section>
                    )}

                    <section className="overflow-hidden rounded-lg border">
                      <header className="bg-muted/50 flex items-center gap-2 border-b px-3 py-2">
                        <User className="text-muted-foreground size-3.5" />
                        <h2 className="text-xs font-medium">Tasks</h2>
                        <span className="text-muted-foreground font-mono text-[10px] tabular-nums">
                          {model.humans.length}
                        </span>
                      </header>
                      {model.humans.length === 0 ? (
                        <p className="text-muted-foreground px-4 py-8 text-center text-xs">
                          No standalone tasks — everything lives under the
                          workstreams above.
                        </p>
                      ) : (
                        model.humans.map((t) => (
                          <HumanRow
                            key={String(t.id)}
                            task={t}
                            byId={model.byId}
                            states={states}
                          />
                        ))
                      )}
                    </section>
                  </div>
                )}
              </TabsContent>

              <TabsContent value="agents" className="mt-3">
                {tasks.isPending ? (
                  <ListSkeleton rows={6} />
                ) : tasks.isError ? (
                  <ErrorNote
                    error={tasks.error}
                    onRetry={() => void tasks.refetch()}
                  />
                ) : (
                  <AgentsBoard model={model} />
                )}
              </TabsContent>
            </Tabs>
          </div>

          {/* properties rail */}
          {milestonesQ.isError ? (
            <aside className="w-full lg:w-60 lg:shrink-0">
              <ErrorNote
                error={milestonesQ.error}
                onRetry={() => void milestonesQ.refetch()}
              />
            </aside>
          ) : milestonesQ.isPending || tasks.isPending ? (
            <aside className="flex w-full flex-col gap-2 lg:w-60 lg:shrink-0">
              {Array.from({ length: 7 }, (_, i) => (
                <Skeleton key={i} className="h-6 w-full" />
              ))}
            </aside>
          ) : (
            <PropertiesRail
              project={project.data}
              model={model}
              milestones={milestones}
              health={health}
            />
          )}
        </div>
      )}
    </div>
  );
}
