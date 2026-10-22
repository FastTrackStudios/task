# task-server memory

What the server's memory is made of, how it is held to a budget, and how to
measure it. Written for t_37712151, after prod (seven small orgs) was
OOM-killed at its 2 GiB limit.

## The budget

`apps/server/tests/it/memory_budget.rs` boots the prod shape in one process
and fails when it grows. The scenario has seven orgs, and each vault holds
notes, tasks and a 4 MiB attachment. It adds a File Root with a laptop
syncing it over iroh, then runs fifteen rounds of agent traffic (MCP)
against every org, with a device sweep each round.

| measure | budget | why that number |
|---|---|---|
| peak live heap | 80 MiB | Measured at 58.6 MiB after this card, 49.3 MiB in zero-copy PR 3 (STA-305, which tightened it from 128 to 96 MiB) and 50.5 MiB before zero-copy PR 2 (STA-304, which tightened it to 80 MiB). The rest is headroom for the product to grow, and the figure is still an order of magnitude under prod's. |
| heap growth over the 12 measured rounds | 3 MiB | Identical traffic should retain nothing; 1.4 MiB was measured after this card and 0.0–1.1 MiB in zero-copy PRs 2 and 3. PR 2 tightened it from 4 MiB, keeping about twice the worst run. The allowance covers lazily filled caches, the inbox the rounds grow, and what background tasks hold at the moment of reading. Before this card the sweep leaked 9.1 MiB over these rounds. |
| anonymous RSS | 200 MiB | The card's figure for seven small orgs was 300 MiB; zero-copy PR 3 measured about 101 MiB and tightened it. It is loose, because RSS also counts the allocator's slack, thread stacks and the harness. |

Heap is counted by a global allocator installed in the `it` test binary
(`support::alloc::Counting`). That makes the heap numbers exact and
independent of the machine, so they move when code allocates more, not when
a box has more cores. A change that needs a bigger budget should say why on
its PR. A change that makes the scenario smaller should tighten the budget.

The same chapter pins one zero-copy guarantee on its own: `search_vault`
matches on names and must not read the vault to do it. With a 32 MiB file in
the vault, one call may take at most 2 MiB of heap.

Both tests run in `just ci` (the `task-server` nextest binary). Their
timeout is raised in `.config/nextest.toml`.

## Where the memory goes

Measured on seven seeded orgs with a 32-core host (heaptrack, then plain
runs; the card has the full table):

1. **Allocator slack.** glibc gives each allocating thread its own arena,
   up to eight per core, and an arena keeps the pages its frees leave
   behind. The server runs hundreds of threads because every File Root's
   iroh-blobs store builds a tokio runtime sized to the core count. Once
   the vault has been adopted, a boot reached 851 threads, 672 of them blob
   stores. With identical traffic, anonymous RSS read 106 MiB on the
   default allocator and 78 MiB with two arenas. Over six minutes it grew
   +14 MiB on the default against +1 MiB with two arenas. jemalloc came
   out worse than both, at 122 MiB. This card does not change the
   allocator. The shared, bounded blob-store runtime that removes the
   thread multiplier is t_4219725c. The prod mitigation, `MALLOC_ARENA_MAX=2`
   in the deployment, is t_911268fa.
2. **The device-sync op log.** Each sync cycle wrote a no-op jj operation
   (t_fe92ad56, fixed in #180).
3. **A connection leaked by every device sweep.** This one was found by the
   budget test, with #180 already applied. `initiator_on(link).establish()`
   drops the vox `ConnectionHandle` and returns only the client. Dropping
   the client closes its lane but not the connection, so the vox driver,
   the link and the QUIC connection under it stayed alive. So did the far
   side's accept task, which waits for that connection to end. A sweep
   dials twice (the replica lane and the document lane), and live heap grew
   by about 0.7 MiB per sweep. After 120 s idle none of it had come back.
   At the 60 s interval that is roughly 40 MiB/h for each reachable peer of
   each org.

   The sweep now dials through `files_sync::Dialled`, which keeps the
   handle and calls `shutdown()` on drop. Over 12 rounds, growth went from
   9.1 MiB to 1.4 MiB. The other callers of the same pattern are listed
   at the end of this page.
4. **iroh endpoints**, one per org. Each has a socket buffer of about
   3 MiB. The cost is bounded and grows with the number of orgs.
5. **SQLite pools**, about nine per org. The page cache is bounded.
6. **Copy-heavy request paths.** These are churn rather than retention,
   and churn is what fills the arenas in (1):
   - Every task, goal, milestone and workstream list opens the whole
     vault. It used to clone each page and parse its whole frontmatter to
     test one key. Pages now move instead of being cloned, and a two-key
     probe decides whether a page is a task.
   - Building a vault manifest read every file whole in order to hash
     it. Hashing now streams through a 16 KiB buffer.
   - `search_vault` and the doc peer built the full manifest. They now
     use a path-only listing and a markdown-only manifest.

## Measuring it yourself

```sh
TASK_DATA_ROOT=/tmp/seed task-server admin seed --orgs a,b,c,d,e,f,g   # debug build
heaptrack -o /tmp/hp task-server            # with TASK_DATA_ROOT set
heaptrack_print -f /tmp/hp.zst --flamegraph-cost-type peak -F stacks.txt
```

Compare `RssAnon` (`/proc/<pid>/status`) with the live heap. A gap that
keeps growing while the heap stays flat is allocator slack, not a leak.

To tell retention from churn in the budget scenario itself, run it longer
and then let it idle. It prints the live heap after each round's traffic,
after each round's sweep, and every 10 s while idle:

```sh
TASK_MEMORY_BUDGET_ROUNDS=30 TASK_MEMORY_BUDGET_SETTLE_SECS=120 \
  target/debug/deps/it-<hash> --exact \
  memory_budget::seven_small_orgs_stay_inside_the_memory_budget --nocapture
```

Under heaptrack, a 2-round run diffed against a 30-round run (collapsed
peak stacks) names whatever accumulates. That is how the connection leak
in (3) was found.

Not done yet:
- the other dial sites with the connection leak in (3): the files daemon's
  stored and redialled clients, `files::remotes` federation lanes, and
  others (t_67b4c081);
- one runtime per blob store (t_4219725c);
- redb's default 1 GiB cache per store (t_2274242e);
- caching the vault scan across list calls (t_2274242e);
- the Files catalogue being held twice and re-serialised on every write
  (t_2274242e).
