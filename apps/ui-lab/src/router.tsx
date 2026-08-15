/**
 * Code-based TanStack Router tree — one lab route per prototype, per
 * the prototype→port loop (see README.md). File-based routing + the
 * router vite plugin are deliberately skipped; a lab this size doesn't
 * need generated route trees.
 *
 * Detail routes are org-qualified (`/projects/$org/$projectId`) —
 * with the org switcher fanning the list out across orgs, a project
 * id alone no longer identifies where to fetch from.
 */
import {
  Link,
  Outlet,
  createRootRoute,
  createRoute,
  createRouter,
  redirect,
} from "@tanstack/react-router";
import { FlaskConical } from "lucide-react";

import { AccountSwitcher } from "./components/account-switcher";
import { OrgSwitcher } from "./components/org-switcher";
import { Separator } from "./components/ui/separator";
import { TooltipProvider } from "./components/ui/tooltip";
import { ProjectOverviewPage } from "./routes/project-overview";
import { ProjectsPage } from "./routes/projects";
import { WorkstreamDetailPage } from "./routes/workstream";

const rootRoute = createRootRoute({
  component: () => (
    <TooltipProvider>
      <div className="min-h-screen flex flex-col">
        <header className="border-b bg-card/50 backdrop-blur-sm sticky top-0 z-10">
          <div className="mx-auto flex max-w-6xl items-center gap-3 px-6 h-12">
            <div className="flex items-center gap-2">
              <FlaskConical className="size-4 text-muted-foreground" />
              <span className="text-sm font-semibold tracking-tight">
                Task <span className="text-muted-foreground font-normal">ui-lab</span>
              </span>
            </div>
            <Separator orientation="vertical" className="h-4" />
            <OrgSwitcher />
            <nav className="flex items-center gap-1 text-sm">
              <Link
                to="/projects"
                className="px-3 py-1.5 rounded-md text-muted-foreground transition-colors hover:text-foreground hover:bg-accent [&.active]:text-foreground [&.active]:bg-accent"
              >
                Projects
              </Link>
            </nav>
            <div className="ml-auto">
              <AccountSwitcher />
            </div>
          </div>
        </header>
        <main className="mx-auto w-full max-w-6xl px-6 py-8 flex-1">
          <Outlet />
        </main>
      </div>
    </TooltipProvider>
  ),
});

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  beforeLoad: () => {
    throw redirect({ to: "/projects" });
  },
});

const projectsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/projects",
  component: ProjectsPage,
});

const projectOverviewRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/projects/$org/$projectId",
  component: ProjectOverviewPage,
});

// Org-qualified like the project overview — the workstream id alone
// doesn't say which org's vox mount to fetch from.
const workstreamRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/workstreams/$org/$workstreamId",
  component: WorkstreamDetailPage,
});

const routeTree = rootRoute.addChildren([
  indexRoute,
  projectsRoute,
  projectOverviewRoute,
  workstreamRoute,
]);

export function createAppRouter() {
  return createRouter({
    routeTree,
    defaultPreload: "intent",
  });
}
