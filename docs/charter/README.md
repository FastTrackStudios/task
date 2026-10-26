# Charters

Standing hard-requirement definitions: what each part of Task has to be
for it to be the one system you trust. A charter is never "done" — it is
evaluated against progress over time, each requirement a yes/no against
current behaviour, re-checked as the system evolves.

These used to be GitHub issues labelled `charter` (#1–#25). They moved
here so the issue tracker counts only work that needs doing; the issue
numbers are kept as the file prefixes so old references still resolve.

Where a charter has grown a testable form, that lives in a spec under
`docs/spec/` or `features/*/spec/` with `t[impl]`/`t[verify]` coverage;
the charter stays the map.

| # | Charter | Testable form |
|---|---|---|
| 1 | [The Temporal Contract](01-temporal-contract.md) | |
| 2 | [A Single Trusted System](02-single-trusted-system.md) | [storage](../spec/storage.md), [capture and process](../spec/capture-and-process.md) |
| 3 | [The Ideal File Manager](03-file-manager.md) | [files](../../features/files/spec/files.md) |
| 4 | [The Ideal Email Client](04-email-client.md) | |
| 5 | [The Ideal Scheduling System](05-scheduling.md) | |
| 6 | [The Ideal Time Tracking System](06-time-tracking.md) | |
| 7 | [The Ideal Knowledge Wiki](07-knowledge-wiki.md) | |
| 8 | [The Ideal Vault (PKM)](08-vault.md) | [storage](../spec/storage.md) |
| 9 | [The Ideal Finance Tracker](09-finance.md) | |
| 10 | [The Ideal Inventory System](10-inventory.md) | |
| 11 | [The Ideal Recall System](11-recall.md) | |
| 12 | [The Ideal Scripture Analysis Tool](12-scripture-analysis.md) | |
| 13 | [The Ideal Project Organization](13-project-organization.md) | [project](../../features/project/spec/project.md) |
| 14 | [The Ideal Client-Facing Deliverable Preview](14-deliverable-preview.md) | |
| 15 | [A Single Inbox - For All Things](15-single-inbox.md) | |
| 16 | [Does AI Belong in Daily Workflow?](16-ai-in-daily-workflow.md) | |
| 17 | [Searchable - Anywhere - Anything](17-search.md) | |
| 18 | [Project Domain Hierarchy](18-project-domain-hierarchy.md) | [project](../../features/project/spec/project.md) |
| 19 | [The Relationship Bank](19-relationship-bank.md) | |
| 20 | [The Ideal Editor](20-editor.md) | |
| 21 | [The Ideal Cooking System](21-cooking.md) | |
| 22 | [Self-Hostable - Federated Servers - Peer to Peer](22-self-host-federation-p2p.md) | |
| 23 | [An Ideal Way to Spend Your Time](23-spending-your-time.md) | |
| 24 | [The Ideal Fitness Tracker](24-fitness.md) | |
| 25 | [Local First - Multiplayer Real-Time Collaborative](25-local-first-multiplayer.md) | |

A gap a charter names is not an issue by itself. When one becomes work
someone will pick up, open an issue for that work and link the charter.
