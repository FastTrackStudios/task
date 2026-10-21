# Agents Spec

The agents a person runs — Hermes bots, Claude Code, anything that speaks
MCP — reach Task over its MCP server (`POST /mcp`, the account lane, and
`POST /org/{slug}/mcp`; the tool catalog is `docs/mcp-tools.md`). This spec
is about **who** an agent is when it gets there, and **what** it may reach.

| piece | where |
|---|---|
| bot accounts, keys, scopes | `apps/server/src/bots.rs` |
| a bot key at the permission gate | `apps/server/src/permits.rs` (`HomeOrBotResolver`) |
| a bot key on the account lane | `apps/server/src/central_auth.rs` (`home_principal`), `apps/server/src/mcp.rs` (`bot_tools`) |
| the app's surface | `BotService` on `/server/vox` (`features/org/org-proto/src/bots.rs`, `BotServiceImpl`), Settings → Bots (`crates/ui/src/bots.rs`) |
| the operator verbs | `task-server admin bot create\|list\|revoke`, and `admin grant` for a bot made with no orgs (`apps/server/src/admin_cli.rs`) |

---

### A bot is an account of its own, with a key

t[mcp.bots]
A person makes a bot by naming it, choosing the orgs it may reach and which
tools it is offered: **all** of an org's tools, or **library** — notes and
wikis (read and write) and the orientation tools, and nothing about tasks,
calendar, email or money. The bot is an account in the server's home auth
store with an address that cannot receive mail and a password nobody is
told; it holds a `member` row in each chosen org and nothing else; and it
is given one API key, shown once, which is what its MCP client presents.
A person can give a bot only orgs they are a member of. On every call the
key resolves to the bot's account behind the same membership fence a
person's session passes, so a bot reaches its orgs and no others, and what
it writes is attributed to it. Tools outside its scope are neither listed
nor callable. Revoking a bot removes its rows and its account, and its key
stops working.

A bot may also be made with **no** orgs, but only on purpose:
`task-server admin bot create --name <name> --no-orgs` (`no_orgs` on
`BotService`). An empty org list without that switch is refused, and so is
the switch given together with `--org`. A zero-org bot is a live key that
reaches nothing: on the account lane its `list_orgs` answers an empty list
(its other tools say it belongs to no organization yet, rather than calling
the key unknown), and every `POST /org/{slug}/mcp` refuses it. An operator
gives it an org later with
`task-server admin grant --org <slug> --principal <bot uuid>`. That
membership row is `local`, so no issuer sync takes it away; and the issuer
(FTS-auth) could not grant it in the first place, because a
`@bots.task.invalid` account exists only in this server's home store.
`task-server admin bot list` prints the bot's principal uuid in its first
column (what `admin grant --principal` and `bot revoke --bot` take), and
`(no orgs)` for a bot without any.

Bots are found by their account and key, not by membership rows, so a bot
with no orgs is still listed. Scoping is by owner and nothing else: a
person sees, and may revoke, exactly the bots they made, zero-org ones
included, and nobody else's; an operator reaches someone's bots only by
naming that owner with `--owner`. Settings → Bots shows such a bot as
"no orgs yet"; the app has no org picker for granting one, which stays an
operator's `admin grant`.

---

## Coverage

| rule | checked by |
|---|---|
| `mcp.bots` | e2e (`apps/server/tests/it/bots_e2e.rs`): a node with its person signed in makes Atlas (all tools) and a Librarian (library); over `POST /mcp` with only their keys, Atlas makes and lists a task, the Librarian is offered no task tools, is refused one by name and writes a note; an org the person is not in cannot be given; a revoked key reaches nothing. `a_bot_made_with_no_orgs_reaches_nothing_until_granted` drives the real `task-server admin` verbs: `bot create` refuses an empty org list without `--no-orgs` and refuses `--no-orgs` with `--org`; the zero-org bot is listed to its owner (principal first, `(no orgs)`) and to no one else, is refused by each `/org/{slug}/mcp` and gets an empty `list_orgs`; after `admin grant --principal` it reaches exactly that org, an issuer sync leaves the grant, and revoking kills the key. Unit: `bots` (library scope, bot addresses). Through the app's lane: `the_app_manages_bots_through_the_server_lane` makes, lists and revokes a bot with the person's session over `/server/vox` and refuses a bad token and a foreign org. Unit: `ui::bots` (the Hermes snippet). Not yet: OAuth for Hermes' Connect button |
