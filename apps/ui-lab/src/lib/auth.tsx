/**
 * Account context + real sign-in (the React mirror of the Dioxus
 * app's crates/ui/src/auth.rs).
 *
 * The server mounts architect-auth's `AuthService` per org, so the
 * lab signs in over the same per-org vox socket every other service
 * rides — always against the HOME org (auth is org-global; the org
 * switcher only scopes *data*).
 *
 * Flow: `AccountProvider` restores the persisted account on boot
 * (localStorage `ui-lab.auth.active`), defaulting to Guest — the
 * account used for anonymous sessions. `switchAccount` first tries
 * the cached token (`ui-lab.auth.token.<email>` → `whoami`
 * validates), and only on a miss performs a real
 * `signInEmailPassword`. Switching never signs the previous account
 * out — its token stays cached so switching back is instant;
 * `signOut` is the only explicit revocation.
 *
 * Identity is display-only for now (requests aren't auth-gated yet),
 * but the active account — token included — is available via
 * `useAccount()` for anything that wants to attach it.
 */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import type { AuthUser } from "@/generated/authservice.generated";
import { useOrgs } from "./orgs";
import { authFor, unwrap } from "./vox";

// ── dev accounts (DEV-ONLY SECTION) ─────────────────────────────────
//
// The switcher performs the REAL sign-in flow — real session tokens
// issued by the org's auth engine — it just has these dev credentials
// pre-filled so switching is one click. Production replaces the
// account picker with a login form; everything else (token cache,
// whoami validation, context) stays.

export interface DevAccount {
  email: string;
  password: string;
  name: string;
  username: string;
}

/** The four dev accounts seeded into the home org's `auth.sqlite`. */
export const DEV_ACCOUNTS: readonly DevAccount[] = [
  {
    email: "cody@fasttrackstudios.com",
    password: "dev-cody-2026",
    name: "Cody Wright",
    username: "cody",
  },
  {
    email: "carter@fasttrackstudios.com",
    password: "dev-carter-2026",
    name: "Carter Whitlock",
    username: "carter",
  },
  {
    email: "tom@fasttrackstudios.com",
    password: "dev-tom-2026",
    name: "Tom Brooks",
    username: "tom",
  },
  {
    email: "guest@fasttrackstudios.com",
    password: "dev-guest-2026",
    name: "Guest",
    username: "guest",
  },
];

/** A fresh browser lands on Guest — the anonymous-ish shared identity. */
export const GUEST_EMAIL = "guest@fasttrackstudios.com";

// ── localStorage persistence ────────────────────────────────────────

const ACTIVE_KEY = "ui-lab.auth.active";
const tokenKey = (email: string) => `ui-lab.auth.token.${email}`;

function storageGet(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function storageSet(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // private mode / quota — degrade to per-session auth
  }
}

function storageRemove(key: string) {
  try {
    localStorage.removeItem(key);
  } catch {
    // ignore
  }
}

// ── active account context ──────────────────────────────────────────

/** The signed-in identity, derived from the server's session bundle. */
export interface ActiveAccount {
  /** `AuthUser.id` — the auth system's user uuid. */
  userId: string;
  email: string;
  name: string;
  /** Raw session token (also cached for instant switch-back). */
  token: string;
}

export interface AccountContextValue {
  /** `null` until the boot sign-in resolves. */
  account: ActiveAccount | null;
  /** True while a switch/sign-in is in flight. */
  busy: boolean;
  /** Last auth error — surfaced softly, never blocks the app. */
  error: string | null;
  switchAccount: (email: string) => void;
  signOut: () => void;
}

const AccountContext = createContext<AccountContextValue | null>(null);

function accountFrom(
  user: AuthUser,
  email: string,
  token: string,
): ActiveAccount {
  const dev = DEV_ACCOUNTS.find((a) => a.email === email);
  return {
    userId: String(user.id),
    email: user.email ?? email,
    name: user.name?.trim() || dev?.name || email,
    token,
  };
}

/**
 * Token-cache-first session resolution against the home org:
 * cached token → `whoami` validates; miss/invalid → real
 * `signInEmailPassword` with the pre-filled dev credentials.
 */
async function resolveSession(
  home: string,
  email: string,
): Promise<ActiveAccount> {
  const client = await authFor(home);

  const cached = storageGet(tokenKey(email));
  if (cached) {
    const r = await client.whoami(cached);
    if (r.ok) return accountFrom(r.value, email, cached);
    storageRemove(tokenKey(email)); // expired/revoked — fall through
  }

  const dev = DEV_ACCOUNTS.find((a) => a.email === email);
  if (!dev) throw new Error(`no credentials on file for ${email}`);
  const bundle = unwrap(
    await client.signInEmailPassword({
      email,
      password: dev.password,
      ip_address: null,
      user_agent: "task-ui-lab",
    }),
  );
  storageSet(tokenKey(email), bundle.token);
  return accountFrom(bundle.user, email, bundle.token);
}

export function AccountProvider({ children }: { children: ReactNode }) {
  const { home } = useOrgs();
  const [account, setAccount] = useState<ActiveAccount | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Monotonic switch id — a slow sign-in must not clobber a newer one.
  const switchSeq = useRef(0);

  const switchAccount = useCallback(
    (email: string) => {
      if (!home) {
        setError("org discovery hasn't resolved yet");
        return;
      }
      const seq = ++switchSeq.current;
      setBusy(true);
      setError(null);
      void resolveSession(home, email).then(
        (next) => {
          if (switchSeq.current !== seq) return;
          storageSet(ACTIVE_KEY, next.email);
          setAccount(next);
          setBusy(false);
        },
        (e) => {
          if (switchSeq.current !== seq) return;
          setError(e instanceof Error ? e.message : String(e));
          setBusy(false);
        },
      );
    },
    [home],
  );

  const signOut = useCallback(() => {
    const current = account;
    setAccount(null);
    if (current) {
      storageRemove(tokenKey(current.email));
      storageRemove(ACTIVE_KEY);
      if (home) {
        // Best-effort server-side revocation — sign_out is idempotent.
        void authFor(home)
          .then((c) => c.signOut(current.token))
          .catch(() => {});
      }
    }
    // Fall back to Guest, the anonymous default.
    switchAccount(GUEST_EMAIL);
  }, [account, home, switchAccount]);

  // Boot restore: wait for org discovery (home slug resolves), then
  // validate the persisted account — or auto sign-in as Guest. Once.
  const booted = useRef(false);
  useEffect(() => {
    if (!home || booted.current) return;
    booted.current = true;
    switchAccount(storageGet(ACTIVE_KEY) ?? GUEST_EMAIL);
  }, [home, switchAccount]);

  const value = useMemo<AccountContextValue>(
    () => ({ account, busy, error, switchAccount, signOut }),
    [account, busy, error, switchAccount, signOut],
  );

  return (
    <AccountContext.Provider value={value}>{children}</AccountContext.Provider>
  );
}

export function useAccount(): AccountContextValue {
  const ctx = useContext(AccountContext);
  if (!ctx) throw new Error("useAccount requires <AccountProvider> above it");
  return ctx;
}

// ── avatars ─────────────────────────────────────────────────────────

/** Tasteful gradient palette — `[from, to]` CSS colors. */
const AVATAR_GRADIENTS: ReadonlyArray<readonly [string, string]> = [
  ["#f59e0b", "#ef4444"], // amber → red
  ["#8b5cf6", "#6366f1"], // violet → indigo
  ["#06b6d4", "#3b82f6"], // cyan → blue
  ["#10b981", "#14b8a6"], // emerald → teal
  ["#ec4899", "#f43f5e"], // pink → rose
  ["#84cc16", "#22c55e"], // lime → green
];

/**
 * FNV-1a over the key, mod the palette size — the same account always
 * gets the same gradient (matches the Dioxus app's hash exactly).
 */
export function gradientIndex(key: string): number {
  let hash = 0xcbf29ce484222325n;
  for (const byte of new TextEncoder().encode(key)) {
    hash ^= BigInt(byte);
    hash = (hash * 0x00000100000001b3n) & 0xffffffffffffffffn;
  }
  return Number(hash % BigInt(AVATAR_GRADIENTS.length));
}

/** Two-letter initials: first letters of the first two words. */
export function initials(name: string): string {
  const words = name.split(/\s+/).filter(Boolean);
  if (words.length >= 2) {
    return (words[0][0] + words[1][0]).toUpperCase();
  }
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase();
  return "?";
}

/** Round initials avatar with a deterministic per-account gradient. */
export function AccountAvatar({
  name,
  email,
  size = 24,
}: {
  name: string;
  email: string;
  size?: number;
}) {
  const key = email || name;
  const [from, to] = AVATAR_GRADIENTS[gradientIndex(key)];
  const font = Math.max(7, Math.floor((size * 2) / 5));
  return (
    <span
      className="flex shrink-0 select-none items-center justify-center rounded-full font-semibold leading-none text-white"
      style={{
        width: size,
        height: size,
        fontSize: font,
        background: `linear-gradient(135deg, ${from}, ${to})`,
      }}
      title={name}
    >
      {initials(name)}
    </span>
  );
}
