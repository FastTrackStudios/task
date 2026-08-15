/**
 * Multi-org selection model + discovery (the React mirror of the
 * Dioxus app's crates/ui/src/orgs.rs).
 *
 * The server hosts several orgs; `/.well-known/task-server.json`
 * lists them (proxied same-origin by vite, see vite.config.ts). The
 * lab can view **all** of them at once or scope to a single slug;
 * the selection lives in React context and persists to localStorage.
 * Data fetchers resolve the selection to a list of slugs via
 * `selectedSlugs` and fan out per org, tagging rows with their org.
 */
import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { useQuery } from "@tanstack/react-query";

import { SERVER_HTTP } from "./vox";

/** One hosted org, as surfaced by the server's well-known endpoint. */
export interface OrgMeta {
  slug: string;
  name: string;
  isHome: boolean;
  /** Org's stable UUID (`org.toml` `id`); absent on older servers. */
  id: string | null;
}

/**
 * What the org switcher is pointed at: `"all"` aggregates every
 * hosted org; any other string scopes to that slug.
 */
export type OrgSelection = "all" | (string & {});

export const ALL_ORGS: OrgSelection = "all";

const SELECTION_KEY = "ui-lab.org.selection";

interface WellKnownOrg {
  slug: string;
  display_name: string;
  is_home: boolean;
  id?: string | null;
}

/**
 * Fetch the hosted org list. In the browser the URL is same-origin
 * (`/.well-known/...` → vite proxy → task-server); under node it
 * goes straight at the server.
 */
export async function fetchOrgs(): Promise<OrgMeta[]> {
  const base =
    typeof location !== "undefined" && SERVER_HTTP.startsWith(location.origin)
      ? "" // same-origin: relative, through the dev proxy
      : SERVER_HTTP;
  const resp = await fetch(`${base}/.well-known/task-server.json`);
  if (!resp.ok) {
    throw new Error(`org discovery failed: HTTP ${resp.status}`);
  }
  const body = (await resp.json()) as { orgs?: WellKnownOrg[] };
  return (body.orgs ?? []).map((o) => ({
    slug: o.slug,
    name: o.display_name,
    isHome: o.is_home,
    id: o.id ?? null,
  }));
}

export const orgsQuery = {
  queryKey: ["orgs"] as const,
  queryFn: fetchOrgs,
  staleTime: 5 * 60_000, // the hosted-org set changes ~never mid-session
};

/**
 * Resolve a selection to the concrete slugs to fetch from. `"all"`
 * expands to every hosted org, home org first for stable ordering.
 */
export function selectedSlugs(
  selection: OrgSelection,
  orgs: OrgMeta[],
): string[] {
  if (selection !== ALL_ORGS) return [selection];
  return [...orgs]
    .sort((a, b) => Number(b.isHome) - Number(a.isHome))
    .map((o) => o.slug);
}

/** The home org's slug (falls back to the first hosted org). */
export function homeSlug(orgs: OrgMeta[]): string {
  return (orgs.find((o) => o.isHome) ?? orgs[0])?.slug ?? "";
}

function loadSelection(): OrgSelection {
  try {
    return localStorage.getItem(SELECTION_KEY) ?? ALL_ORGS;
  } catch {
    return ALL_ORGS;
  }
}

function saveSelection(selection: OrgSelection) {
  try {
    localStorage.setItem(SELECTION_KEY, selection);
  } catch {
    // private mode / quota — selection just won't persist
  }
}

export interface OrgContextValue {
  /** Hosted orgs, once discovery resolves (empty while loading). */
  orgs: OrgMeta[];
  /** Discovery state, for header affordances. */
  isLoading: boolean;
  error: unknown;
  /** Current switcher position. */
  selection: OrgSelection;
  setSelection: (selection: OrgSelection) => void;
  /** The selection resolved to concrete slugs (home org first). */
  slugs: string[];
  /** Where auth signs in — always the home org. */
  home: string;
  /** Display label for the switcher trigger. */
  label: string;
}

const OrgContext = createContext<OrgContextValue | null>(null);

export function OrgProvider({ children }: { children: ReactNode }) {
  const discovery = useQuery(orgsQuery);
  const [selection, setSelectionState] = useState<OrgSelection>(loadSelection);

  const setSelection = useCallback((next: OrgSelection) => {
    setSelectionState(next);
    saveSelection(next);
  }, []);

  const orgs = useMemo(() => discovery.data ?? [], [discovery.data]);

  const value = useMemo<OrgContextValue>(() => {
    // A persisted slug the server no longer hosts degrades to "all"
    // rather than rendering an eternally-empty workspace.
    const effective: OrgSelection =
      selection !== ALL_ORGS &&
      orgs.length > 0 &&
      !orgs.some((o) => o.slug === selection)
        ? ALL_ORGS
        : selection;
    return {
      orgs,
      isLoading: discovery.isPending,
      error: discovery.error,
      selection: effective,
      setSelection,
      slugs: selectedSlugs(effective, orgs),
      home: homeSlug(orgs),
      label:
        effective === ALL_ORGS
          ? "All organizations"
          : (orgs.find((o) => o.slug === effective)?.name ?? effective),
    };
  }, [orgs, discovery.isPending, discovery.error, selection, setSelection]);

  return <OrgContext.Provider value={value}>{children}</OrgContext.Provider>;
}

export function useOrgs(): OrgContextValue {
  const ctx = useContext(OrgContext);
  if (!ctx) throw new Error("useOrgs requires <OrgProvider> above it");
  return ctx;
}
