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
| the operator verbs | `task-server admin bot create\|list\|revoke` (`apps/server/src/admin_cli.rs`) |

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

---

## Coverage

| rule | checked by |
|---|---|
| `mcp.bots` | e2e (`apps/server/tests/it/bots_e2e.rs`): a node with its person signed in makes Atlas (all tools) and a Librarian (library); over `POST /mcp` with only their keys, Atlas makes and lists a task, the Librarian is offered no task tools, is refused one by name and writes a note; an org the person is not in cannot be given; a revoked key reaches nothing. Unit: `bots` (library scope, bot addresses). Through the app's lane: `the_app_manages_bots_through_the_server_lane` makes, lists and revokes a bot with the person's session over `/server/vox` and refuses a bad token and a foreign org. Unit: `ui::bots` (the Hermes snippet). Not yet: OAuth for Hermes' Connect button |
