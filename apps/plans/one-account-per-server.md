# One account per server

**Status:** designed, not started (2026-08-13)

One principal per *server*, with real org memberships — so signing in
once shows you every org you belong to on that server.

Distinct from `federated-task-platform.md` phase 3, which is one account
across *servers* (the identity locker, cross-server links). This is the
inside-one-server half, and phase 3 sits on top of it: a locker that
links servers is much simpler when each server has exactly one principal
per human.

## The problem, concretely

`AppState` opens one `AuthState` per org (`lib.rs:562`, from
`org_root.auth_db()`). Six orgs on production, and
`acodywright@gmail.com` has an account in all six — six *distinct* user
ids that share a login and nothing else.

Nothing joins them, so:

- `.well-known/task-server.json` answers `member` by "does this token
  validate here", which is true for exactly the org that issued it
  (`lib.rs:2411` says so outright: "no cross-org membership table to
  consult").
- The client's `my_orgs_with_links` keeps orgs where `member || linked`,
  so **"All organizations" collapses to the home org**.
- Every multi-org view — projects, tasks, sessions, invoices — funnels
  through `orgs::selected_slugs`, so they are all one-org views in
  practice. The fan-out code is correct and is simply handed a
  one-element list.

The permission gate compounds it: `RoleEngine::with_default_user_role("member")`
means *any* user validated by an org's own store is a member of that
org. Membership is currently a side effect of which database answered.

## Target

**Revised 2026-08-13**, after Cody pointed out he is the only principal:
build no second auth store. The `merge-principals` dry run had already
shown why — the canonical id is always the HOME org's, so home-org data
(which is nearly all the data) never moves. The destination was the
expensive half, and it turns out we own one already.

```
<data_root>/orgs/<home>/auth.sqlite         the server's identity authority
<data_root>/orgs/<home>/memberships.sqlite  (user_id, org_slug, role, created_at)
<data_root>/orgs/<slug>/…                   unchanged, auth.sqlite included
```

- **Sign-in** is unchanged: against the home org.
- **Org lane**: try this org's own auth store, then the home org's. A
  home-issued token that validates gets its role from
  `(user_id, org_slug)` in `memberships`. No row = not a member = the
  gate refuses. Membership stops being a side effect of which database
  answered.
- **Discovery** answers `member` from `memberships`.
- **Client**: still no change.

What this buys over a merged store: no account migration, no id
rewriting, no session loss, no new blast radius — and identity stays
*inside* the home org, which is what `federated-task-platform.md`'s open
decision #1 leaned toward, so "rsync the org elsewhere" keeps working
with the transfer functions rather than against them.

Per-org accounts stay where they are, unused for sign-in but intact —
they are the rollback, and they are what a future "detach this org onto
its own server" reads to hand the org back its own identity.

### Superseded target (kept for the record)

One server-level `<data_root>/identity/auth.sqlite` holding every user,
with the per-org stores merged into it. Rejected as more machinery than
a single-principal server needs, and because moving identity out of the
home org contradicts org portability.

- **Sign-in** issues a server session, not an org session.
- **Org lane** resolves the bearer against the server store → `user_id`,
  then looks up `(user_id, org_slug)` for the role. No membership row =
  not a member = the gate refuses. Membership becomes an explicit fact
  instead of a database-routing accident.
- **Discovery** answers `member` from the membership table.
- **Client**: no change at all. `member: true` for six orgs makes
  `selected_slugs` return six, and every existing fan-out fills in.

## Staging

**S1 — memberships store.** `memberships.sqlite` in the home org, and
`admin adopt-principal --email <e>`: for every org holding an account
with that email, write `(home_user_id, slug, role)`, taking the role from
that org's own account so an admin stays an admin. Idempotent, and
`--dry-run` prints the rows first. Nothing else reads the table yet, so
this cannot break a running server.

**S2 — org lane falls back to home.** When a bearer token does not
validate against the org's own store, try the home org's; on success,
require a membership row and take the role from it. A token that is
neither is refused exactly as it is today.

**S3 — discovery answers from memberships.** `.well-known` reports
`member` per membership row instead of "does this token validate here".
**"All organizations" starts working here** — the client already unions
`member || linked` and fans out over `selected_slugs`.

**S4 — later, as more people arrive.** Per-org accounts for OTHER humans
become memberships too, and `admin merge-principals --apply` (the dry
run already exists) grows the id remap for orgs where someone's rows
predate their membership. Not needed while there is one principal.

`merge-principals` (dry run, shipped) stays useful throughout: it is the
report of who exists where, and it is what says whether S4 has anything
to do.

## Risks, named

- **The home org becomes load-bearing for the whole server.** Losing
  `orgs/<home>/auth.sqlite` locks every org out, not one. It is already
  the most-backed-up thing here, and identity riding *with* the home org
  is the point — but it is a real concentration and the
  `task-git-backup` CronJob should be checked against it.
- **A membership row is now the thing standing between an org's data and
  a signed-in stranger.** Today the org's own auth store is that fence.
  The S2 fallback must require the row, not merely a valid home token —
  a home token is easy to get if we ever open sign-up.
- **Roles are copied once, at adopt time.** Change a role in an org's own
  auth store afterwards and nothing notices, because nothing reads it
  any more. `adopt-principal` re-run updates the row; that is the only
  path, and it should stay obvious.
- **Merging by email stays a judgement call** for S4, when other humans
  arrive. Two people sharing an address would become one principal. The
  `merge-principals` dry run prints every group for a human to read
  before `--apply` exists.
- **Portability is preserved, deliberately.** Identity lives in the home
  org, so moving a NON-home org elsewhere still means copying its
  directory — it keeps its own untouched `auth.sqlite`, and the transfer
  function's job is to hand back the membership rows as local accounts.
  Moving the *home* org moves the server's identity, which is the
  intended mental model.

## Not doing

- Cross-*server* identity (that is phase 3).
- Self-registration. Accounts stay admin-provisioned.
- Merging accounts with *different* emails. If one human has two emails,
  they get two principals until they say otherwise.
