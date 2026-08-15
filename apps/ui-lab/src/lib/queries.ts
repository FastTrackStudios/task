/**
 * TanStack Query options for the vox-backed data this lab renders.
 * Every query is **org-scoped** (the org slug leads the key); pages
 * fan out across the org switcher's selection with `useQueries` and
 * tag rows with their org. Mutations (task create / estimate / claim,
 * workstream status) live with their pages and invalidate these keys.
 */
import { queryOptions } from "@tanstack/react-query";

import type { Milestone } from "@/generated/milestoneservicerpc.generated";
import type { ProjectInfo } from "@/generated/projectservicerpc.generated";
import type { TaskInfo } from "@/generated/taskservicerpc.generated";
import type {
  Workstream,
  WorkstreamWithRollup,
} from "@/generated/workstreamservicerpc.generated";
import { milestonesFor, projectsFor, tasksFor, unwrap, workstreamsFor } from "./vox";

/** A row tagged with the org it came from (the "All orgs" fan-out). */
export interface OrgProject {
  org: string;
  project: ProjectInfo;
}

export const projectListQuery = (org: string) =>
  queryOptions({
    queryKey: [org, "projects"],
    queryFn: async (): Promise<OrgProject[]> => {
      const client = await projectsFor(org);
      return unwrap(await client.list()).map((project) => ({ org, project }));
    },
  });

export const projectQuery = (org: string, id: string) =>
  queryOptions({
    queryKey: [org, "projects", id],
    queryFn: async () => {
      const client = await projectsFor(org);
      // Uuid crosses the wire as a string; the generated signature is
      // `unknown` because codegen has no TS mapping for uuid::Uuid yet.
      return unwrap(await client.get(id));
    },
  });

/**
 * Tasks belonging to one project. `TaskService.list()` has no filters
 * by design ("clients filter client-side after fetching" — see
 * features/task/task/src/service.rs), so that's what we do. The
 * unfiltered list is cached once per org under `[org, "tasks"]`.
 */
export const orgTasksQuery = (org: string) =>
  queryOptions({
    queryKey: [org, "tasks"],
    queryFn: async (): Promise<TaskInfo[]> => {
      const client = await tasksFor(org);
      return unwrap(await client.list());
    },
  });

export const projectTasksQuery = (org: string, projectId: string) =>
  queryOptions({
    ...orgTasksQuery(org),
    select: (all: TaskInfo[]) =>
      all.filter((t) => String(t.project_id ?? "") === projectId),
  });

/** Every workstream in the org (cached once, filtered per page). */
export const orgWorkstreamsQuery = (org: string) =>
  queryOptions({
    queryKey: [org, "workstreams"],
    queryFn: async (): Promise<Workstream[]> => {
      const client = await workstreamsFor(org);
      return unwrap(await client.list(null));
    },
  });

/** Workstreams owned by one project. */
export const projectWorkstreamsQuery = (org: string, projectId: string) =>
  queryOptions({
    ...orgWorkstreamsQuery(org),
    select: (all: Workstream[]) =>
      all.filter((w) => String(w.project_id) === projectId),
  });

/**
 * One workstream PLUS its derived progress (done / total /
 * in-progress / blocked / estimate-points sum) — the server-side
 * `rollup` verb, one round-trip.
 */
export const workstreamRollupQuery = (org: string, id: string) =>
  queryOptions({
    queryKey: [org, "workstreams", id, "rollup"],
    queryFn: async (): Promise<WorkstreamWithRollup> => {
      const client = await workstreamsFor(org);
      return unwrap(await client.rollup(id));
    },
  });

/** Milestones of one project — same client-side-filter convention. */
export const orgMilestonesQuery = (org: string) =>
  queryOptions({
    queryKey: [org, "milestones"],
    queryFn: async (): Promise<Milestone[]> => {
      const client = await milestonesFor(org);
      return unwrap(await client.list());
    },
  });

export const projectMilestonesQuery = (org: string, projectId: string) =>
  queryOptions({
    ...orgMilestonesQuery(org),
    select: (all: Milestone[]) =>
      all.filter((m) => String(m.project_id ?? "") === projectId),
  });
