/**
 * /projects — real projects fetched over vox-ws with the generated
 * ProjectServiceRpc client, fanned out across the org switcher's
 * selection (one query per org; rows are tagged with their org).
 */
import { useMemo, useState } from "react";
import { useQueries } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { AlertCircle, RefreshCw, Search } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { projectListQuery, type OrgProject } from "@/lib/queries";
import { ALL_ORGS, useOrgs } from "@/lib/orgs";

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

function ProjectTableRow({
  row,
  showOrg,
}: {
  row: OrgProject;
  showOrg: boolean;
}) {
  const { org, project } = row;
  const id = String(project.id);
  return (
    <TableRow className="group cursor-pointer">
      <TableCell className="font-medium">
        <Link
          to="/projects/$org/$projectId"
          params={{ org, projectId: id }}
          className="block hover:text-primary transition-colors"
        >
          {project.title || project.path}
        </Link>
      </TableCell>
      {showOrg && (
        <TableCell>
          <Badge variant="secondary" className="font-mono text-[10px]">
            {org}
          </Badge>
        </TableCell>
      )}
      <TableCell className="text-muted-foreground font-mono text-xs hidden md:table-cell">
        {project.path}
      </TableCell>
      <TableCell>
        <Badge variant="outline" className="font-mono text-[10px]">
          {project.project_type || "project"}
        </Badge>
      </TableCell>
      <TableCell>
        <Badge variant={statusVariant(project.status)}>{project.status}</Badge>
      </TableCell>
    </TableRow>
  );
}

export function ProjectsPage() {
  const { slugs, selection, label, isLoading: orgsLoading } = useOrgs();
  const [search, setSearch] = useState("");

  const results = useQueries({
    queries: slugs.map((org) => projectListQuery(org)),
  });

  const isPending = orgsLoading || results.some((r) => r.isPending);
  const isFetching = results.some((r) => r.isFetching);
  // Partial failure (one org down) renders as a soft note above the
  // rows that DID arrive — total failure as the full error card.
  const failures = results
    .map((r, i) => ({ org: slugs[i], error: r.error }))
    .filter((f) => f.error != null);
  const allFailed = slugs.length > 0 && failures.length === slugs.length;

  const data = results.map((r) => r.data);
  const rows = useMemo(() => {
    const all = data.flatMap((d) => d ?? []);
    return all
      .sort(
        (a, b) =>
          a.org.localeCompare(b.org) ||
          a.project.title.localeCompare(b.project.title),
      )
      .filter(
        (r) =>
          !search ||
          r.project.title.toLowerCase().includes(search.toLowerCase()) ||
          r.project.path.toLowerCase().includes(search.toLowerCase()) ||
          r.org.toLowerCase().includes(search.toLowerCase()),
      );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search, ...data]);

  const total = results.reduce((n, r) => n + (r.data?.length ?? 0), 0);
  const showOrg = selection === ALL_ORGS;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between gap-4">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Projects</h1>
          <p className="text-muted-foreground text-sm mt-0.5">
            {label} ·{" "}
            <span className="font-mono">
              {slugs.length} org{slugs.length === 1 ? "" : "s"}
            </span>{" "}
            over the generated ProjectServiceRpc client
          </p>
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={() => results.forEach((r) => void r.refetch())}
          disabled={isFetching}
          aria-label="Refresh"
          className="gap-1.5"
        >
          <RefreshCw className={isFetching ? "animate-spin" : ""} />
          Refresh
        </Button>
      </div>

      {failures.length > 0 && !allFailed && (
        <div className="border-destructive/30 text-destructive bg-destructive/5 flex items-center gap-2 rounded-lg border px-4 py-2.5 text-xs">
          <AlertCircle className="size-3.5 shrink-0" />
          {failures
            .map(
              (f) =>
                `${f.org}: ${f.error instanceof Error ? f.error.message : String(f.error)}`,
            )
            .join(" · ")}
        </div>
      )}

      <Card className="overflow-hidden py-0">
        <CardHeader className="px-4 py-3 border-b gap-0">
          <div className="flex items-center gap-3">
            <div className="relative flex-1 max-w-sm">
              <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground pointer-events-none" />
              <Input
                placeholder="Filter projects…"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                className="pl-8 h-8 text-sm"
                disabled={isPending || allFailed}
              />
            </div>
            {!isPending && !allFailed && (
              <span className="text-muted-foreground text-xs tabular-nums">
                {rows.length} / {total}
              </span>
            )}
          </div>
        </CardHeader>
        <CardContent className="p-0">
          {isPending ? (
            <div className="flex flex-col">
              {Array.from({ length: 6 }, (_, i) => (
                <div
                  key={i}
                  className="flex items-center gap-4 px-4 py-3 border-b last:border-0"
                >
                  <Skeleton className="h-4 flex-1 max-w-48" />
                  <Skeleton className="h-4 w-24 hidden md:block" />
                  <Skeleton className="h-5 w-16" />
                  <Skeleton className="h-5 w-20" />
                </div>
              ))}
            </div>
          ) : allFailed ? (
            <div className="border-destructive/30 text-destructive flex items-start gap-3 m-4 rounded-lg border bg-destructive/5 px-4 py-3 text-sm">
              <AlertCircle className="mt-0.5 size-4 shrink-0" />
              <div className="flex flex-col gap-2">
                <p className="font-medium">Couldn&apos;t reach the task-server</p>
                <p className="text-muted-foreground">
                  {failures[0].error instanceof Error
                    ? failures[0].error.message
                    : String(failures[0].error)}
                </p>
                <Button
                  variant="outline"
                  size="sm"
                  className="w-fit"
                  onClick={() => results.forEach((r) => void r.refetch())}
                >
                  Retry
                </Button>
              </div>
            </div>
          ) : rows.length === 0 ? (
            <p className="text-muted-foreground px-4 py-12 text-center text-sm">
              {search
                ? "No projects match the filter."
                : selection === ALL_ORGS
                  ? "No projects in any hosted org yet."
                  : "No projects in this org yet."}
            </p>
          ) : (
            <Table>
              <TableHeader>
                <TableRow className="hover:bg-transparent">
                  <TableHead>Title</TableHead>
                  {showOrg && <TableHead>Org</TableHead>}
                  <TableHead className="hidden md:table-cell">Path</TableHead>
                  <TableHead>Type</TableHead>
                  <TableHead>Status</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map((row) => (
                  <ProjectTableRow
                    key={`${row.org}/${String(row.project.id)}`}
                    row={row}
                    showOrg={showOrg}
                  />
                ))}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
