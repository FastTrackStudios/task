/**
 * Header org switcher — "All organizations" or one hosted org.
 * Discovery comes from the well-known endpoint (lib/orgs.tsx); the
 * selection persists and every data fetcher fans out over it.
 */
import { Building2, Check, ChevronsUpDown, Globe } from "lucide-react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import { ALL_ORGS, useOrgs } from "@/lib/orgs";
import { cn } from "@/lib/utils";

export function OrgSwitcher() {
  const { orgs, isLoading, error, selection, setSelection, label } = useOrgs();

  if (isLoading) return <Skeleton className="h-7 w-40 rounded-md" />;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        className={cn(
          "flex h-7 items-center gap-1.5 rounded-md border px-2.5 text-sm font-medium",
          "hover:bg-accent transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
          error != null && "border-destructive/50 text-destructive",
        )}
        title={error != null ? `org discovery failed: ${String(error)}` : undefined}
      >
        {selection === ALL_ORGS ? (
          <Globe className="size-3.5 text-muted-foreground" />
        ) : (
          <Building2 className="size-3.5 text-muted-foreground" />
        )}
        <span className="max-w-44 truncate">
          {error != null ? "Orgs unavailable" : label}
        </span>
        <ChevronsUpDown className="size-3 text-muted-foreground" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-64">
        <DropdownMenuLabel className="text-muted-foreground text-xs">
          Organization
        </DropdownMenuLabel>
        <DropdownMenuItem onSelect={() => setSelection(ALL_ORGS)}>
          <Globe className="size-3.5" />
          <span className="flex-1">All organizations</span>
          {selection === ALL_ORGS && <Check className="size-3.5" />}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        {orgs.map((org) => (
          <DropdownMenuItem
            key={org.slug}
            onSelect={() => setSelection(org.slug)}
          >
            <Building2 className="size-3.5" />
            <span className="flex min-w-0 flex-1 flex-col">
              <span className="truncate">{org.name}</span>
              <span className="text-muted-foreground truncate font-mono text-[10px]">
                {org.slug}
                {org.isHome ? " · home" : ""}
              </span>
            </span>
            {selection === org.slug && <Check className="size-3.5" />}
          </DropdownMenuItem>
        ))}
        {orgs.length === 0 && (
          <p className="text-muted-foreground px-2 py-3 text-center text-xs">
            No hosted orgs discovered.
          </p>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
