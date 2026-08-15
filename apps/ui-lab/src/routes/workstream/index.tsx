/**
 * /workstreams/$org/$workstreamId — one Workstream (the parent-with-
 * swarm construct): rollup header served by the server-side `rollup`
 * verb, the parent PRD body, the member list grouped by state GROUP
 * (through the owning project's state registry), and blocked-by
 * callouts.
 *
 * This page is also the lab's REAL-MUTATION surface: create a task in
 * the workstream (`TaskService.create` with `workflow.workstream`
 * set), set a member's estimate (`update`), claim a member as the
 * signed-in account (`tryClaim`), and move the workstream's lifecycle
 * status (`WorkstreamService.setStatus`). Everything invalidates the
 * org-scoped query keys, so the overview strips update on navigation.
 */
import { useMemo, useState } from "react";
import {
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import {
  AlertCircle,
  ArrowLeft,
  ArrowUpRight,
  Bot,
  CalendarClock,
  CircleSlash,
  Hand,
  Layers,
  Loader2,
  Plus,
  RefreshCw,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import type { StatesConfig } from "@/generated/projectservicerpc.generated";
import type {
  Estimate,
  TaskInfo,
  WorkflowAttrs,
} from "@/generated/taskservicerpc.generated";
import type { Workstream } from "@/generated/workstreamservicerpc.generated";
import { useAccount } from "@/lib/auth";
import {
  orgTasksQuery,
  projectQuery,
  workstreamRollupQuery,
} from "@/lib/queries";
import {
  defaultStateName,
  GROUP_ORDER,
  resolveStateGroup,
  type GroupTag,
} from "@/lib/states";
import { tasksFor, unwrap, workstreamsFor } from "@/lib/vox";
import { cn } from "@/lib/utils";
import {
  agentClaimant,
  agentRefLabel,
  bucketRank,
  estimateLabel,
  formatDate,
  relativeAge,
  statusBucket,
  unresolvedBlockers,
} from "../project-overview/model";
import { StatusPill } from "../project-overview";
import {
  BUCKET_META,
  ClaimantChip,
  GROUP_META,
  GroupBar,
  RelationChips,
} from "../project-overview/swarm";
import { countGroups } from "../project-overview/model";

const NIL_UUID = "00000000-0000-0000-0000-000000000000";

/** Canonical workstream lifecycle (workstream-proto `Status`). */
const WS_STATUSES = [
  "backlog",
  "planned",
  "in-progress",
  "paused",
  "done",
  "cancelled",
] as const;

const ESTIMATES: readonly Estimate["tag"][] = ["XS", "S", "M", "L", "XL"];

function emptyWorkflow(): WorkflowAttrs {
  return {
    cycle: null,
    workstream: null,
    estimate: null,
    parent: null,
    assignees: [],
    blockers: [],
    relates_to: [],
    relations: [],
    session: null,
  };
}

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

// ── PRD body ────────────────────────────────────────────────────────

/**
 * Minimal markdown-ish renderer for the workstream's PRD/charter —
 * headers, bullets, paragraphs. Deliberately tiny (no dep): the
 * Dioxus port renders the same body with its own markdown pipeline.
 */
function PrdBody({ details }: { details: string }) {
  const blocks = useMemo(() => {
    const out: { kind: "h" | "li" | "p"; depth: number; text: string }[] = [];
    for (const raw of details.split("\n")) {
      const line = raw.trimEnd();
      if (!line.trim()) continue;
      const h = /^(#{1,6})\s+(.*)$/.exec(line);
      if (h) {
        out.push({ kind: "h", depth: h[1].length, text: h[2] });
        continue;
      }
      const li = /^\s*[-*]\s+(.*)$/.exec(line);
      if (li) {
        out.push({ kind: "li", depth: 0, text: li[1] });
        continue;
      }
      out.push({ kind: "p", depth: 0, text: line.trim() });
    }
    return out;
  }, [details]);

  if (blocks.length === 0) {
    return (
      <p className="text-muted-foreground text-xs italic">
        No PRD body yet — the parent page is empty.
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-1.5 text-sm leading-relaxed">
      {blocks.map((b, i) =>
        b.kind === "h" ? (
          <h3
            key={i}
            className={cn(
              "font-semibold tracking-tight",
              b.depth <= 2 ? "mt-2 text-sm" : "mt-1 text-xs uppercase text-muted-foreground",
            )}
          >
            {b.text}
          </h3>
        ) : b.kind === "li" ? (
          <p key={i} className="flex gap-2 pl-2 text-sm">
            <span className="text-muted-foreground">•</span>
            <span>{b.text}</span>
          </p>
        ) : (
          <p key={i}>{b.text}</p>
        ),
      )}
    </div>
  );
}

// ── member rows ─────────────────────────────────────────────────────

function MemberRow({
  task,
  byId,
  states,
  onEstimate,
  onClaim,
  mutating,
}: {
  task: TaskInfo;
  byId: Map<string, TaskInfo>;
  states: StatesConfig | null;
  onEstimate: (task: TaskInfo, tag: Estimate["tag"]) => void;
  onClaim: (task: TaskInfo) => void;
  mutating: boolean;
}) {
  const bucket = statusBucket(task, byId, states);
  const claimant = agentClaimant(task);
  const est = estimateLabel(task.workflow?.estimate);
  const claimed = (task.workflow?.assignees ?? []).length > 0;
  return (
    <div className="hover:bg-accent/40 flex items-center gap-2.5 border-b border-border/50 px-3 py-2 text-xs last:border-0">
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
      <StatusPill status={task.status} states={states} />
      {claimant ? (
        <ClaimantChip task={task} />
      ) : (
        !claimed && (
          <Button
            variant="outline"
            size="sm"
            className="h-6 gap-1 px-2 text-[10px]"
            disabled={mutating}
            onClick={() => onClaim(task)}
            title="Claim as the signed-in account"
          >
            <Hand className="size-3" /> Claim
          </Button>
        )
      )}
      <Select
        value={task.workflow?.estimate?.tag === "Points" ? "" : (task.workflow?.estimate?.tag ?? "")}
        onValueChange={(v) => onEstimate(task, v as Estimate["tag"])}
        disabled={mutating}
      >
        <SelectTrigger
          size="sm"
          className="h-6 w-16 shrink-0 px-2 font-mono text-[10px]"
          aria-label="Estimate"
        >
          <SelectValue placeholder={est ?? "est"} />
        </SelectTrigger>
        <SelectContent>
          {ESTIMATES.map((e) => (
            <SelectItem key={e} value={e} className="font-mono text-xs">
              {e}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <span className="text-muted-foreground/70 w-7 shrink-0 text-right font-mono text-[10px] tabular-nums">
        {relativeAge(task.date_created)}
      </span>
    </div>
  );
}

// ── page ────────────────────────────────────────────────────────────

export function WorkstreamDetailPage() {
  const { org, workstreamId } = useParams({
    from: "/workstreams/$org/$workstreamId",
  });
  const queryClient = useQueryClient();
  const { account } = useAccount();

  // Server-side rollup verb: workstream + derived progress, one trip.
  const rollupQ = useQuery(workstreamRollupQuery(org, workstreamId));
  const ws: Workstream | undefined = rollupQ.data?.workstream;

  const tasksQ = useQuery(orgTasksQuery(org));
  const projectQ = useQuery({
    ...projectQuery(org, String(ws?.project_id ?? "")),
    enabled: ws != null,
    retry: false,
  });
  const states = projectQ.data?.states ?? null;

  const members = useMemo(
    () =>
      (tasksQ.data ?? []).filter(
        (t) => String(t.workflow?.workstream ?? "") === workstreamId,
      ),
    [tasksQ.data, workstreamId],
  );
  const byId = useMemo(() => {
    const m = new Map<string, TaskInfo>();
    for (const t of tasksQ.data ?? []) m.set(String(t.id), t);
    return m;
  }, [tasksQ.data]);

  const groups = useMemo(
    () => countGroups(members, states),
    [members, states],
  );
  const grouped = useMemo(() => {
    const m = new Map<GroupTag, TaskInfo[]>(GROUP_ORDER.map((g) => [g, []]));
    for (const t of members) {
      m.get(resolveStateGroup(states, t.status))?.push(t);
    }
    for (const [, list] of m) {
      list.sort(
        (a, b) =>
          bucketRank(statusBucket(a, byId, states)) -
            bucketRank(statusBucket(b, byId, states)) ||
          a.title.localeCompare(b.title, undefined, { numeric: true }),
      );
    }
    return m;
  }, [members, byId, states]);

  const blocked = useMemo(
    () =>
      members
        .map((t) => ({
          task: t,
          blockers: unresolvedBlockers(t, byId, states),
        }))
        .filter((b) => b.blockers.length > 0),
    [members, byId, states],
  );

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: [org, "tasks"] });
    void queryClient.invalidateQueries({ queryKey: [org, "workstreams"] });
  };

  // ── mutations (real writes over the live vox client) ──────────────

  const [composerTitle, setComposerTitle] = useState("");
  const createTask = useMutation({
    mutationFn: async (title: string) => {
      if (!ws) throw new Error("workstream not loaded");
      const client = await tasksFor(org);
      const task: TaskInfo = {
        id: NIL_UUID, // nil → backend assigns id + path
        path: "",
        title,
        status: defaultStateName(states),
        priority: "normal",
        due: null,
        scheduled: null,
        tags: [],
        contexts: [],
        projects: [],
        project_id: ws.project_id,
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
        workflow: { ...emptyWorkflow(), workstream: ws.id },
      };
      return unwrap(await client.create(task));
    },
    onSuccess: () => {
      setComposerTitle("");
      invalidate();
    },
  });

  const setEstimate = useMutation({
    mutationFn: async ({
      task,
      tag,
    }: {
      task: TaskInfo;
      tag: Estimate["tag"];
    }) => {
      const client = await tasksFor(org);
      const estimate: Estimate =
        tag === "Points" ? { tag, value: 0 } : { tag };
      return unwrap(
        await client.update({
          ...task,
          workflow: { ...(task.workflow ?? emptyWorkflow()), estimate },
        }),
      );
    },
    onSuccess: invalidate,
  });

  const claim = useMutation({
    mutationFn: async (task: TaskInfo) => {
      if (!account) throw new Error("no signed-in account");
      const client = await tasksFor(org);
      // `agent` is a JSON-encoded workflows_proto::AgentRef
      // (kind-tagged serde form).
      const ref = JSON.stringify({
        kind: "human",
        user_id: account.userId,
        display_name: account.name,
      });
      const result = unwrap(await client.tryClaim(task.id, ref, false));
      if (result.tag === "Lost") {
        throw new Error(`claim lost — held by ${result.holder}`);
      }
      return result;
    },
    onSuccess: invalidate,
  });

  const setWsStatus = useMutation({
    mutationFn: async (status: string) => {
      const client = await workstreamsFor(org);
      return unwrap(await client.setStatus(workstreamId, status));
    },
    onSuccess: () => {
      invalidate();
      void queryClient.invalidateQueries({
        queryKey: [org, "workstreams", workstreamId],
      });
    },
  });

  const mutating =
    createTask.isPending ||
    setEstimate.isPending ||
    claim.isPending ||
    setWsStatus.isPending;
  const mutationError =
    createTask.error ?? setEstimate.error ?? claim.error ?? setWsStatus.error;

  const rollup = rollupQ.data?.rollup;

  return (
    <div className="flex flex-col gap-5">
      <div className="flex items-center gap-2 text-sm">
        <Button asChild variant="ghost" size="sm" className="-ml-2">
          {ws ? (
            <Link
              to="/projects/$org/$projectId"
              params={{ org, projectId: String(ws.project_id) }}
            >
              <ArrowLeft className="size-3.5" />
              {projectQ.data?.title ?? "Project"}
            </Link>
          ) : (
            <Link to="/projects">
              <ArrowLeft className="size-3.5" /> Projects
            </Link>
          )}
        </Button>
        <Separator orientation="vertical" className="h-4" />
        <Badge variant="secondary" className="font-mono text-[10px]">
          {org}
        </Badge>
        {ws && (
          <span className="text-muted-foreground truncate font-mono text-xs">
            {ws.path || `workstream/${String(ws.id).slice(0, 8)}`}
          </span>
        )}
      </div>

      {rollupQ.isPending ? (
        <div className="flex flex-col gap-4">
          <Skeleton className="h-8 w-80" />
          <Skeleton className="h-2 w-full" />
          <Skeleton className="h-40 w-full" />
        </div>
      ) : rollupQ.isError ? (
        <ErrorNote
          error={rollupQ.error}
          onRetry={() => void rollupQ.refetch()}
        />
      ) : ws && rollup ? (
        <div className="flex flex-col gap-6 lg:flex-row">
          {/* main column */}
          <div className="flex min-w-0 flex-1 flex-col gap-5">
            <header className="flex flex-col gap-3">
              <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
                <Layers className="text-muted-foreground size-5" />
                <h1 className="text-2xl font-semibold tracking-tight">
                  {ws.title}
                </h1>
                <Select
                  value={ws.status}
                  onValueChange={(v) => setWsStatus.mutate(v)}
                  disabled={setWsStatus.isPending}
                >
                  <SelectTrigger
                    size="sm"
                    className="h-7 w-32 font-mono text-xs"
                    aria-label="Workstream status"
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {WS_STATUSES.map((s) => (
                      <SelectItem key={s} value={s} className="font-mono text-xs">
                        {s}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {setWsStatus.isPending && (
                  <Loader2 className="size-3.5 animate-spin text-muted-foreground" />
                )}
              </div>

              <div className="text-muted-foreground flex flex-wrap items-center gap-x-4 gap-y-1 text-xs">
                {ws.lead ? (
                  <span className="flex items-center gap-1.5 font-mono">
                    <Bot className="size-3" />
                    lead {agentRefLabel(ws.lead)}
                  </span>
                ) : (
                  <span>No lead</span>
                )}
                {ws.members.length > 0 && (
                  <span className="flex items-center gap-1.5">
                    swarm:
                    {ws.members.map((m, i) => (
                      <span key={i} className="font-mono">
                        {agentRefLabel(m)}
                        {i < ws.members.length - 1 ? "," : ""}
                      </span>
                    ))}
                  </span>
                )}
                <span className="flex items-center gap-1.5">
                  <CalendarClock className="size-3" />
                  {ws.start_date != null ? formatDate(ws.start_date) : "unstarted"}
                  {" → "}
                  {ws.target_date != null
                    ? formatDate(ws.target_date)
                    : "open-ended"}
                </span>
                {ws.links.map((l) => (
                  <a
                    key={l}
                    href={l}
                    target="_blank"
                    rel="noreferrer"
                    className="text-primary flex items-center gap-1 hover:underline"
                  >
                    <ArrowUpRight className="size-3" />
                    {l.replace(/^https?:\/\//, "").slice(0, 40)}
                  </a>
                ))}
              </div>

              {/* server-side rollup header + group-segmented bar */}
              <div className="flex flex-col gap-1.5">
                <GroupBar groups={groups} className="h-2" />
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] tabular-nums">
                  <span className="text-sm font-semibold">
                    {rollup.total > 0
                      ? Math.round((rollup.done / rollup.total) * 100)
                      : 0}
                    %
                  </span>
                  <span className="text-muted-foreground">
                    {rollup.done}/{rollup.total} done
                  </span>
                  {rollup.in_progress > 0 && (
                    <span className={GROUP_META.Started.text}>
                      {rollup.in_progress} in progress
                    </span>
                  )}
                  {rollup.blocked > 0 && (
                    <span className="text-red-500">
                      {rollup.blocked} blocked
                    </span>
                  )}
                  {rollup.estimate_points_sum > 0 && (
                    <span className="text-muted-foreground">
                      {rollup.estimate_points_sum} pts
                    </span>
                  )}
                  <span className="text-muted-foreground/60 ml-auto">
                    rollup computed server-side
                  </span>
                </div>
              </div>
            </header>

            {/* blocked-by callouts */}
            {blocked.length > 0 && (
              <section className="rounded-lg border border-red-500/40 bg-red-500/5">
                <header className="flex items-center gap-2 border-b border-red-500/20 px-3 py-2">
                  <CircleSlash className="size-3.5 text-red-500" />
                  <h3 className="text-sm font-medium">Blocked members</h3>
                  <Badge variant="secondary" className="bg-red-500/15 text-red-500">
                    {blocked.length}
                  </Badge>
                </header>
                <div className="flex flex-col">
                  {blocked.map(({ task, blockers }) => (
                    <div
                      key={String(task.id)}
                      className="flex flex-col gap-0.5 border-b border-red-500/10 px-3 py-2 text-xs last:border-0"
                    >
                      <span className="font-medium">{task.title}</span>
                      <span className="text-muted-foreground">
                        blocked by{" "}
                        {blockers
                          .map((id) => byId.get(id)?.title ?? `${id.slice(0, 8)}… (not in org list)`)
                          .join(", ")}
                      </span>
                    </div>
                  ))}
                </div>
              </section>
            )}

            {mutationError != null && (
              <ErrorNote error={mutationError} />
            )}

            {/* composer — a real TaskService.create into this stream */}
            <form
              className="flex items-center gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                const title = composerTitle.trim();
                if (title) createTask.mutate(title);
              }}
            >
              <Input
                value={composerTitle}
                onChange={(e) => setComposerTitle(e.target.value)}
                placeholder="Add a task to this workstream…"
                className="h-8 text-sm"
                disabled={createTask.isPending}
              />
              <Button
                type="submit"
                size="sm"
                className="h-8 gap-1"
                disabled={createTask.isPending || !composerTitle.trim()}
              >
                {createTask.isPending ? (
                  <Loader2 className="size-3.5 animate-spin" />
                ) : (
                  <Plus className="size-3.5" />
                )}
                Add
              </Button>
            </form>

            {/* members, grouped by state group */}
            {tasksQ.isPending ? (
              <div className="flex flex-col gap-2">
                {Array.from({ length: 5 }, (_, i) => (
                  <Skeleton key={i} className="h-9 w-full" />
                ))}
              </div>
            ) : tasksQ.isError ? (
              <ErrorNote
                error={tasksQ.error}
                onRetry={() => void tasksQ.refetch()}
              />
            ) : members.length === 0 ? (
              <p className="text-muted-foreground rounded-lg border border-dashed px-4 py-10 text-center text-sm">
                No tasks attached yet — add the first one above.
              </p>
            ) : (
              <div className="flex flex-col gap-3">
                {GROUP_ORDER.map((g) => {
                  const list = grouped.get(g) ?? [];
                  if (list.length === 0) return null;
                  return (
                    <section
                      key={g}
                      className="overflow-hidden rounded-lg border"
                    >
                      <header className="bg-muted/50 flex items-center gap-2 border-b px-3 py-2">
                        <span
                          className={cn(
                            "size-2 rounded-full",
                            GROUP_META[g].bar,
                          )}
                        />
                        <h2 className="text-xs font-medium capitalize">
                          {GROUP_META[g].label}
                        </h2>
                        <span className="text-muted-foreground font-mono text-[10px] tabular-nums">
                          {list.length}
                        </span>
                      </header>
                      {list.map((t) => (
                        <MemberRow
                          key={String(t.id)}
                          task={t}
                          byId={byId}
                          states={states}
                          mutating={mutating}
                          onEstimate={(task, tag) =>
                            setEstimate.mutate({ task, tag })
                          }
                          onClaim={(task) => claim.mutate(task)}
                        />
                      ))}
                    </section>
                  );
                })}
              </div>
            )}
          </div>

          {/* PRD rail */}
          <aside className="flex w-full flex-col gap-3 lg:w-72 lg:shrink-0">
            <section className="rounded-lg border p-4">
              <h3 className="text-muted-foreground mb-2 text-[11px] font-medium uppercase tracking-wide">
                PRD / charter
              </h3>
              <PrdBody details={ws.details} />
            </section>
          </aside>
        </div>
      ) : null}
    </div>
  );
}
