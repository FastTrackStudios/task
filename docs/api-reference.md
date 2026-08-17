# Task API reference

<!-- GENERATED — do not edit by hand. Regenerate with:
     cargo run -p task-cli -- api --markdown > apps/task/docs/api-reference.md
     Source: `task_server::permits::mounts()` (apps/task/server/src/permits.rs),
     the single registry the router, permit gate, and schema stamps derive from.
     Served live at `GET /org/{slug}/api`. -->

83 services mounted: 65 plain RPC, 18 `#[subscribe]` streams. Every method lists its permit — the `<action>` on `<resource>` the permissions gate checks (see `apps/task/server/src/permits.rs`). `audited` methods emit an audit line even when allowed. A `stream` method takes a `Tx` sink and pushes to the caller instead of returning once.

## `auth` (AuthService)

Plugin: `core` — schema stamp: `66044c970637a966`

| method | args | permit | notes |
|---|---|---|---|
| `sign_up_email_password` | `input` | `read` on `public/auth` | — |
| `sign_in_email_password` | `input` | `read` on `public/auth` | — |
| `current_session` | `token` | `read` on `public/auth` | — |
| `refresh_session` | `token` | `read` on `public/auth` | — |
| `whoami` | `token` | `read` on `public/auth` | — |
| `sign_out` | `token` | `read` on `public/auth` | — |
| `list_org_members` | `token` | `read` on `public/auth` | — |

## `permissions` (PermissionsService)

Plugin: `core` — schema stamp: `c20f2ef985520960`

| method | args | permit | notes |
|---|---|---|---|
| `can` | `resource`, `action` | `read` on `public/permissions` | — |
| `capabilities` | `token`, `prefix` | `read` on `public/permissions` | — |

## `attachments` (AttachmentService)

Plugin: `core` — schema stamp: `b210328c506e1690`

| method | args | permit | notes |
|---|---|---|---|
| `initiate_upload` | `req` | `write` on `attachments/**` | — |
| `complete_upload` | `req` | `write` on `attachments/**` | — |
| `get_download_url` | `arg` | `download` on `attachments/**` | audited |

## `media` (MediaService)

Plugin: `core` — schema stamp: `fde688afbb970039`

| method | args | permit | notes |
|---|---|---|---|
| `stat` | `content_hash` | `read` on `media/{content_hash}` | — |
| `read` | `content_hash`, `start`, `len`, `tx` | `read` on `media/{content_hash}` | stream, audited |

## `vault-sync` (VaultSyncRpc)

Plugin: `core` — schema stamp: `ba3389e731b79eb8`

| method | args | permit | notes |
|---|---|---|---|
| `manifest` | `vault_id` | `read` on `vault/**` | — |
| `get_file` | `vault_id`, `path` | `read` on `vault/{path}` | — |
| `put_file` | `vault_id`, `path`, `bytes`, `if_match` | `write` on `vault/{path}` | — |
| `delete_file` | `vault_id`, `path`, `if_match` | `write` on `vault/{path}` | — |
| `folder_index` | `vault_id` | `read` on `vault/**` | — |
| `set_folder` | `vault_id`, `path`, `parent`, `if_match` | `write` on `vault/{path}` | — |
| `open_collab` | `vault_id`, `path` | `read` on `vault/{path}` | — |
| `base_views` | `vault_id`, `base_path` | `read` on `vault/{path}` | — |

## `vault-sync-stream` (VaultSyncStream) — stream

Plugin: `core` — schema stamp: `1e7f7ff14fe06c56`

| method | args | permit | notes |
|---|---|---|---|
| `changes` | `sink` | `read` on `vault/**` | stream |

## `vault-graph` (VaultGraphRpc)

Plugin: `core` — schema stamp: `a052614cb08bb49a`

| method | args | permit | notes |
|---|---|---|---|
| `backlinks` | `vault_id`, `path` | `read` on `vault/**` | — |
| `links` | `vault_id`, `path` | `read` on `vault/**` | — |
| `orphans` | `vault_id` | `read` on `vault/**` | — |
| `unresolved` | `vault_id` | `read` on `vault/**` | — |
| `deadends` | `vault_id` | `read` on `vault/**` | — |
| `tags` | `vault_id` | `read` on `vault/**` | — |

## `share` (ShareService)

Plugin: `core` — schema stamp: `dab09d3f620a4c09`

| method | args | permit | notes |
|---|---|---|---|
| `create_link` | `note_path`, `label`, `capability` | `write` on `shares/**` | audited |
| `list_links` | — | `read` on `shares/**` | — |
| `links_for_note` | `note_path` | `read` on `shares/**` | — |
| `set_link_disabled` | `token`, `disabled` | `write` on `shares/**` | audited |
| `delete_link` | `token` | `write` on `shares/**` | audited |

## `doc-sync` (DocSync) — stream

Plugin: `core` — schema stamp: `adb32a8efbc4b061`

| method | args | permit | notes |
|---|---|---|---|
| `sync` | `doc_id`, `from`, `up`, `down` | `write` on `doc/**` | stream |

## `doc-presence` (DocPresence) — stream

Plugin: `core` — schema stamp: `5cffa57c1f8b156e`

| method | args | permit | notes |
|---|---|---|---|
| `presence` | `doc_id`, `up`, `down` | `read` on `doc/presence/**` | stream |

## `agent-tasks` (AgentTaskQueue)

Plugin: `agent` — schema stamp: `2901b1d751044c21`

| method | args | permit | notes |
|---|---|---|---|
| `read_queue` | `queue_id`, `filter` | `read` on `agent/tasks/**` | — |
| `claim_agent_task` | `agent_task_id`, `handle` | `write` on `agent/tasks/**` | — |
| `set_agent_task_status` | `agent_task_id`, `new_status` | `write` on `agent/tasks/**` | — |
| `complete_agent_task` | `agent_task_id`, `result_blob` | `write` on `agent/tasks/**` | — |
| `link_agent_task_to_session` | `agent_task_id`, `session_id` | `write` on `agent/tasks/**` | — |
| `list_agent_task_links` | `queue_id` | `read` on `agent/tasks/**` | — |

## `agent-sessions` (SessionsRpc)

Plugin: `agent` — schema stamp: `20daebb18a1ca73e`

| method | args | permit | notes |
|---|---|---|---|
| `create_session` | `args` | `write` on `agent/sessions/**` | — |
| `read_session` | `session_id` | `read` on `agent/sessions/**` | — |
| `list_sessions` | `filter` | `read` on `agent/sessions/**` | — |
| `rename_session` | `session_id`, `title` | `write` on `agent/sessions/**` | — |
| `pin_session` | `session_id`, `pinned` | `write` on `agent/sessions/**` | — |
| `archive_session` | `session_id`, `archived` | `write` on `agent/sessions/**` | — |
| `delete_session` | `session_id` | `write` on `agent/sessions/**` | audited |
| `save_composer_draft` | `session_id`, `text`, `attachments` | `write` on `agent/sessions/**` | — |

## `agent-turns` (TurnDispatchRpc)

Plugin: `agent` — schema stamp: `2ae062b43ad63a9a`

| method | args | permit | notes |
|---|---|---|---|
| `dispatch_turn` | `args` | `write` on `agent/turns/**` | audited |
| `cancel_turn` | `session_id` | `write` on `agent/turns/**` | — |
| `resume_session` | `session_id` | `write` on `agent/turns/**` | — |

## `agent-threads` (ThreadsRpc)

Plugin: `agent` — schema stamp: `0133f034b611e516`

| method | args | permit | notes |
|---|---|---|---|
| `list_messages` | `session_id`, `limit`, `before_cursor` | `read` on `agent/threads/**` | — |
| `read_message` | `message_id` | `read` on `agent/threads/**` | — |
| `append_note` | `session_id`, `text` | `comment` on `agent/threads/**` | — |

## `agent-subscriptions` (SubscriptionsStream) — stream

Plugin: `agent` — schema stamp: `da7b504dda036f6e`

| method | args | permit | notes |
|---|---|---|---|
| `events` | `sink` | `read` on `agent/events/**` | stream |

## `agent-discovery` (DiscoveryRpc)

Plugin: `agent` — schema stamp: `405a046e1b7c4c80`

| method | args | permit | notes |
|---|---|---|---|
| `list_models` | `backend_id` | `read` on `agent/discovery/**` | — |
| `list_skills` | `backend_id` | `read` on `agent/discovery/**` | — |
| `list_capabilities` | `backend_id` | `read` on `agent/discovery/**` | — |
| `backend_health` | `backend_id` | `read` on `agent/discovery/**` | — |

## `agent-routines` (RoutinesRpc)

Plugin: `agent` — schema stamp: `2092551a25360d26`

| method | args | permit | notes |
|---|---|---|---|
| `list_routines` | `backend_id`, `include_disabled` | `read` on `agent/routines/**` | — |
| `create_routine` | `routine` | `write` on `agent/routines/**` | — |
| `set_routine_paused` | `backend_id`, `id`, `paused` | `write` on `agent/routines/**` | — |
| `run_routine` | `backend_id`, `id` | `write` on `agent/routines/**` | audited |
| `delete_routine` | `backend_id`, `id` | `write` on `agent/routines/**` | audited |

## `project` (ProjectServiceRpc)

Plugin: `core` — schema stamp: `4491227d91ff7b90`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `projects/**` | — |
| `get` | `id` | `read` on `projects/**` | — |
| `get_by_path` | `path` | `read` on `projects/**` | — |
| `create` | `project` | `write` on `projects/**` | — |
| `update` | `project` | `write` on `projects/**` | — |
| `rename` | `id`, `new_path` | `write` on `projects/**` | — |
| `delete` | `id` | `write` on `projects/**` | audited |

## `project-stream` (ProjectServiceStream) — stream

Plugin: `core` — schema stamp: `71314d5c2f2464a4`

| method | args | permit | notes |
|---|---|---|---|
| `events` | `sink` | `read` on `projects/**` | stream |

## `goal` (GoalServiceRpc)

Plugin: `core` — schema stamp: `5341902c6cc8cad2`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `goals/**` | — |
| `get` | `id` | `read` on `goals/**` | — |
| `get_by_path` | `path` | `read` on `goals/**` | — |
| `create` | `goal` | `write` on `goals/**` | — |
| `update` | `goal` | `write` on `goals/**` | — |
| `rename` | `id`, `new_path` | `write` on `goals/**` | — |
| `delete` | `id` | `write` on `goals/**` | audited |

## `goal-stream` (GoalServiceStream) — stream

Plugin: `core` — schema stamp: `f0e8f8eb34b6f71f`

| method | args | permit | notes |
|---|---|---|---|
| `events` | `sink` | `read` on `goals/**` | stream |

## `milestone` (MilestoneServiceRpc)

Plugin: `core` — schema stamp: `103db2d13bf34b5e`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `milestones/**` | — |
| `get` | `id` | `read` on `milestones/**` | — |
| `get_by_path` | `path` | `read` on `milestones/**` | — |
| `create` | `milestone` | `write` on `milestones/**` | — |
| `update` | `milestone` | `write` on `milestones/**` | — |
| `rename` | `id`, `new_path` | `write` on `milestones/**` | — |
| `delete` | `id` | `write` on `milestones/**` | audited |

## `milestone-stream` (MilestoneServiceStream) — stream

Plugin: `core` — schema stamp: `d5fcfa70ebcffefb`

| method | args | permit | notes |
|---|---|---|---|
| `events` | `sink` | `read` on `milestones/**` | stream |

## `workstream` (WorkstreamServiceRpc)

Plugin: `core` — schema stamp: `790028acb3484c66`

| method | args | permit | notes |
|---|---|---|---|
| `list` | `project` | `read` on `workstreams/**` | — |
| `get` | `id` | `read` on `workstreams/**` | — |
| `get_by_path` | `path` | `read` on `workstreams/**` | — |
| `create` | `workstream` | `write` on `workstreams/**` | — |
| `update` | `workstream` | `write` on `workstreams/**` | — |
| `set_status` | `id`, `status` | `write` on `workstreams/**` | — |
| `delete` | `id` | `write` on `workstreams/**` | audited |
| `rollup` | `id` | `read` on `workstreams/**` | — |

## `workstream-stream` (WorkstreamServiceStream) — stream

Plugin: `core` — schema stamp: `17f8a666e6a2a9b3`

| method | args | permit | notes |
|---|---|---|---|
| `events` | `sink` | `read` on `workstreams/**` | stream |

## `task` (TaskServiceRpc)

Plugin: `core` — schema stamp: `c513b7e17a405951`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `tasks/**` | — |
| `get` | `id` | `read` on `tasks/**` | — |
| `get_by_path` | `path` | `read` on `tasks/**` | — |
| `create` | `task` | `write` on `tasks/**` | — |
| `update` | `task` | `write` on `tasks/**` | — |
| `try_claim` | `id`, `agent`, `force` | `write` on `tasks/**` | — |
| `reverse_relations` | `id` | `read` on `tasks/**` | — |
| `reverse_relations_batch` | `ids` | `read` on `tasks/**` | — |
| `query` | `filter` | `read` on `tasks/**` | — |
| `rename` | `id`, `new_path` | `write` on `tasks/**` | — |
| `delete` | `id` | `write` on `tasks/**` | audited |

## `task-stream` (TaskServiceStream) — stream

Plugin: `core` — schema stamp: `2e19ced670f4930b`

| method | args | permit | notes |
|---|---|---|---|
| `events` | `sink` | `read` on `tasks/**` | stream |

## `timer` (TimerService)

Plugin: `core` — schema stamp: `a48a5e0252767dcd`

| method | args | permit | notes |
|---|---|---|---|
| `start_timer` | `req` | `write` on `timer/**` | — |
| `stop_timer` | `user_id` | `write` on `timer/**` | — |
| `active_timer` | `user_id` | `read` on `timer/**` | — |
| `switch_timer` | `req` | `write` on `timer/**` | — |
| `log_session` | `req` | `write` on `timer/**` | — |
| `resolve_rate` | `user_id`, `project_id` | `read` on `timer/**` | — |
| `list_sessions` | `filter` | `read` on `timer/**` | — |
| `update_session` | `req` | `write` on `timer/**` | — |
| `delete_session` | `id` | `write` on `timer/**` | audited |
| `set_org_member_rate` | `org_id`, `user_id`, `hourly_cents`, `currency` | `write` on `timer/**` | audited |
| `set_project_member_rate` | `project_id`, `user_id`, `hourly_cents` | `write` on `timer/**` | audited |
| `list_org_member_rates` | `org_id` | `read` on `timer/**` | — |
| `list_project_member_rates` | `project_id` | `read` on `timer/**` | — |

## `timer-stream` (TimerServiceStream) — stream

Plugin: `core` — schema stamp: `420ab9513b52369b`

| method | args | permit | notes |
|---|---|---|---|
| `events` | `sink` | `read` on `timer/**` | stream |

## `threads` (ThreadsService)

Plugin: `core` — schema stamp: `c7dc2af6e1f9bcbc`

| method | args | permit | notes |
|---|---|---|---|
| `list_threads` | `entity_type`, `entity_id` | `read` on `threads/**` | — |
| `get_thread` | `id` | `read` on `threads/**` | — |
| `create_thread` | `req` | `comment` on `threads/**` | — |
| `list_messages` | `thread_id` | `read` on `threads/**` | — |
| `post_message` | `req` | `comment` on `threads/**` | — |
| `set_resolved` | `thread_id`, `resolved`, `by` | `write` on `threads/**` | — |
| `delete_thread` | `id` | `write` on `threads/**` | audited |
| `delete_message` | `id` | `write` on `threads/**` | audited |

## `prefs` (PrefsService)

Plugin: `core` — schema stamp: `c9fb51716d9fc2cd`

| method | args | permit | notes |
|---|---|---|---|
| `get` | `user_id` | `read` on `prefs/**` | — |
| `set` | `prefs` | `write` on `prefs/**` | — |

## `day-templates` (DayTemplatesRpc)

Plugin: `scheduling` — schema stamp: `446792622394beac`

| method | args | permit | notes |
|---|---|---|---|
| `list_day_templates` | — | `read` on `scheduling/day-templates/**` | — |
| `get_day_template` | `id` | `read` on `scheduling/day-templates/**` | — |
| `upsert_day_template` | `template` | `write` on `scheduling/day-templates/**` | — |
| `delete_day_template` | `id` | `write` on `scheduling/day-templates/**` | audited |

## `day-plans` (DayPlansRpc)

Plugin: `scheduling` — schema stamp: `89c5cc6f4e2ded99`

| method | args | permit | notes |
|---|---|---|---|
| `get_day_plan` | `date` | `read` on `scheduling/day-plans/**` | — |
| `upsert_day_plan` | `plan` | `write` on `scheduling/day-plans/**` | — |
| `delete_day_plan` | `date` | `write` on `scheduling/day-plans/**` | audited |

## `calendar-events` (CalendarEventsRpc)

Plugin: `scheduling` — schema stamp: `4e6f60ff24118b6f`

| method | args | permit | notes |
|---|---|---|---|
| `list_events` | — | `read` on `scheduling/events/**` | — |
| `upsert_event` | `event` | `write` on `scheduling/events/**` | — |
| `delete_event` | `id` | `write` on `scheduling/events/**` | audited |

## `event-types` (EventTypesRpc)

Plugin: `scheduling` — schema stamp: `0407815bc7b28566`

| method | args | permit | notes |
|---|---|---|---|
| `list_event_types` | — | `read` on `scheduling/event-types/**` | — |
| `get_event_type` | `id` | `read` on `scheduling/event-types/**` | — |
| `upsert_event_type` | `event_type` | `write` on `scheduling/event-types/**` | — |
| `delete_event_type` | `id` | `write` on `scheduling/event-types/**` | audited |

## `schedules` (SchedulesRpc)

Plugin: `scheduling` — schema stamp: `a5153cadde47a680`

| method | args | permit | notes |
|---|---|---|---|
| `list_schedules` | — | `read` on `scheduling/schedules/**` | — |
| `get_schedule` | `id` | `read` on `scheduling/schedules/**` | — |
| `upsert_schedule` | `schedule` | `write` on `scheduling/schedules/**` | — |
| `delete_schedule` | `id` | `write` on `scheduling/schedules/**` | audited |

## `slots` (SlotsRpc)

Plugin: `scheduling` — schema stamp: `20b0cb35cabc4f87`

| method | args | permit | notes |
|---|---|---|---|
| `list_open_slots` | `query` | `read` on `scheduling/slots/**` | — |

## `bookings` (BookingsRpc)

Plugin: `scheduling` — schema stamp: `2f094cd467e8f67d`

| method | args | permit | notes |
|---|---|---|---|
| `list_bookings` | — | `read` on `scheduling/bookings/**` | — |
| `get_booking` | `id` | `read` on `scheduling/bookings/**` | — |
| `create_booking` | `booking` | `write` on `scheduling/bookings/**` | — |
| `update_booking_status` | `id`, `status` | `write` on `scheduling/bookings/**` | — |

## `scheduling-events` (SchedulingEventsStream) — stream

Plugin: `scheduling` — schema stamp: `aacb88fceb226e1d`

| method | args | permit | notes |
|---|---|---|---|
| `events` | `sink` | `read` on `scheduling/**` | stream |

## `inbox` (InboxRpc)

Plugin: `core` — schema stamp: `e3309d34aacd2f4b`

| method | args | permit | notes |
|---|---|---|---|
| `list_inbox` | — | `read` on `inbox/**` | — |
| `review_queue` | `today` | `read` on `inbox/**` | — |
| `get_inbox_item` | `id` | `read` on `inbox/**` | — |
| `upsert_inbox_item` | `item` | `write` on `inbox/**` | — |
| `delete_inbox_item` | `id` | `write` on `inbox/**` | audited |

## `inbox-stream` (InboxStream) — stream

Plugin: `core` — schema stamp: `18590160ce7459bd`

| method | args | permit | notes |
|---|---|---|---|
| `events` | `sink` | `read` on `inbox/**` | stream |

## `recall` (RecallRpc)

Plugin: `recall` — schema stamp: `b273280d67ef5c84`

| method | args | permit | notes |
|---|---|---|---|
| `list_cards` | — | `read` on `recall/**` | — |
| `review_queue` | `today` | `read` on `recall/**` | — |
| `upsert_card` | `card` | `write` on `recall/**` | — |
| `delete_card` | `id` | `write` on `recall/**` | audited |

## `recall-stream` (RecallStream) — stream

Plugin: `recall` — schema stamp: `69e79dd5c6df9d63`

| method | args | permit | notes |
|---|---|---|---|
| `events` | `sink` | `read` on `recall/**` | stream |

## `contacts` (ContactsRpc)

Plugin: `contacts` — schema stamp: `3183ca6a232e423f`

| method | args | permit | notes |
|---|---|---|---|
| `list_contacts` | — | `read` on `contacts/**` | — |
| `get_contact` | `id` | `read` on `contacts/**` | — |
| `upsert_contact` | `contact` | `write` on `contacts/**` | — |
| `delete_contact` | `id` | `write` on `contacts/**` | audited |
| `list_accounts` | — | `read` on `contacts/**` | — |
| `upsert_account` | `account` | `write` on `contacts/**` | — |
| `delete_account` | `id` | `write` on `contacts/**` | audited |
| `sync_account` | `id` | `write` on `contacts/**` | audited |

## `contacts-stream` (ContactsStream) — stream

Plugin: `contacts` — schema stamp: `68bf42f299f1ff31`

| method | args | permit | notes |
|---|---|---|---|
| `events` | `sink` | `read` on `contacts/**` | stream |

## `tags` (TagServiceRpc)

Plugin: `core` — schema stamp: `72f2a6ff1e78a186`

| method | args | permit | notes |
|---|---|---|---|
| `list_tags` | — | `read` on `tags/**` | — |
| `get_tag` | `id` | `read` on `tags/**` | — |
| `upsert_tag` | `tag` | `write` on `tags/**` | — |
| `delete_tag` | `id` | `write` on `tags/**` | audited |

## `scripture` (ScriptureServiceRpc)

Plugin: `scripture` — schema stamp: `464df04f44aeb954`

| method | args | permit | notes |
|---|---|---|---|
| `translations` | — | `read` on `scripture/**` | — |
| `chapter` | `translation`, `book`, `chapter` | `read` on `scripture/**` | — |
| `verse` | `translation`, `reference` | `read` on `scripture/**` | — |
| `compare` | `reference`, `translations` | `read` on `scripture/**` | — |
| `chapter_backlinks` | `book`, `chapter` | `read` on `scripture/**` | — |
| `lexicon` | `strongs` | `read` on `scripture/**` | — |
| `word_study` | `translation`, `reference` | `read` on `scripture/**` | — |
| `occurrences` | `strongs`, `translation`, `limit` | `read` on `scripture/**` | — |
| `original_editions` | — | `read` on `scripture/**` | — |
| `interlinear` | `edition`, `reference` | `read` on `scripture/**` | — |
| `study` | `strongs`, `limit` | `read` on `scripture/**` | — |
| `cross_refs` | `reference`, `min_votes` | `read` on `scripture/**` | — |
| `topics_of` | `reference` | `read` on `scripture/**` | — |
| `verses_for_topic` | `topic`, `limit` | `read` on `scripture/**` | — |

## `links` (LinksServiceRpc)

Plugin: `core` — schema stamp: `c5c32de59903f282`

| method | args | permit | notes |
|---|---|---|---|
| `create` | `link` | `write` on `links/**` | — |
| `delete` | `id` | `write` on `links/**` | audited |
| `get` | `id` | `read` on `links/**` | — |
| `links_for` | `node` | `read` on `links/**` | — |
| `graph` | `min_confidence`, `include_private` | `read` on `links/**` | — |

## `collection` (CollectionServiceRpc)

Plugin: `fasttrackstudio` — schema stamp: `0ce252dab5a62852`

| method | args | permit | notes |
|---|---|---|---|
| `create` | `org`, `title`, `kind` | `write` on `collections/**` | — |
| `get` | `id` | `read` on `collections/**` | — |
| `list` | `org`, `kind` | `read` on `collections/**` | — |
| `add_item` | `placement` | `write` on `collections/**` | — |
| `remove_item` | `collection_id`, `node` | `write` on `collections/**` | — |
| `reorder` | `placement` | `write` on `collections/**` | — |

## `resources` (ResourcesServiceRpc)

Plugin: `core` — schema stamp: `b324784fa40b557b`

| method | args | permit | notes |
|---|---|---|---|
| `transcript` | `rel_path` | `read` on `resources/**` | — |

## `invoicing` (InvoicingRpc)

Plugin: `finance` — schema stamp: `473a14f2f1f08794`

| method | args | permit | notes |
|---|---|---|---|
| `generate_invoice` | `req` | `write` on `finance/invoicing/**` | audited |
| `list_invoices` | — | `read` on `finance/invoicing/**` | — |
| `get_invoice` | `id` | `read` on `finance/invoicing/**` | — |
| `delete_invoice` | `id` | `write` on `finance/invoicing/**` | audited |
| `record_invoice_payment` | `id`, `amount_minor`, `date` | `write` on `finance/invoicing/**` | audited |
| `uninvoiced` | — | `read` on `finance/invoicing/**` | — |
| `mark_sent` | `id` | `write` on `finance/invoicing/**` | audited |
| `void_with_credit` | `id`, `reason` | `write` on `finance/invoicing/**` | audited |
| `record_payment` | `payload` | `write` on `finance/invoicing/**` | audited |
| `refund_payment` | `id`, `amount_minor`, `reason` | `write` on `finance/invoicing/**` | audited |
| `run_schedule_once` | `id` | `write` on `finance/invoicing/**` | audited |

## `ledger` (LedgerRpc)

Plugin: `finance` — schema stamp: `af1147b7f586cc62`

| method | args | permit | notes |
|---|---|---|---|
| `post_transaction` | `payload` | `write` on `finance/ledger/**` | audited |
| `account_transactions` | `account_id`, `since`, `until`, `limit` | `read` on `finance/ledger/**` | — |
| `balances` | `book_id`, `as_of` | `read` on `finance/ledger/**` | — |
| `books` | — | `read` on `finance/ledger/**` | — |
| `accounts` | `book_id` | `read` on `finance/ledger/**` | — |

## `wiki-schema` (SchemaRpc)

Plugin: `wiki` — schema stamp: `658bb14de7a9c4a4`

| method | args | permit | notes |
|---|---|---|---|
| `bootstrap` | `wiki_id` | `write` on `wiki/schema/**` | audited |
| `read_schema` | `wiki_id` | `read` on `wiki/schema/**` | — |
| `read_purpose` | `wiki_id` | `read` on `wiki/schema/**` | — |
| `write_schema` | `wiki_id`, `markdown` | `write` on `wiki/schema/**` | — |
| `write_purpose` | `wiki_id`, `markdown` | `write` on `wiki/schema/**` | — |
| `health` | `wiki_id` | `read` on `wiki/schema/**` | — |

## `wiki-catalog` (CatalogRpc)

Plugin: `wiki` — schema stamp: `1efe171502d83278`

| method | args | permit | notes |
|---|---|---|---|
| `read_index` | `wiki_id` | `read` on `wiki/catalog/**` | — |
| `rebuild_index` | `wiki_id` | `write` on `wiki/catalog/**` | audited |
| `append_log` | `wiki_id`, `entry` | `write` on `wiki/catalog/**` | — |

## `wiki-raw` (RawLayerRpc)

Plugin: `wiki` — schema stamp: `c3f86f7199ee96c3`

| method | args | permit | notes |
|---|---|---|---|
| `import_raw_source` | `wiki_id`, `source` | `write` on `wiki/raw/**` | audited |
| `list_raw_sources` | `wiki_id` | `read` on `wiki/raw/**` | — |
| `read_raw_source` | `wiki_id`, `path` | `read` on `wiki/raw/**` | — |
| `delete_raw_source` | `wiki_id`, `path` | `write` on `wiki/raw/**` | audited |
| `rescan_sources` | `wiki_id` | `write` on `wiki/raw/**` | — |

## `wiki-graph` (GraphRpc)

Plugin: `wiki` — schema stamp: `b42fc42808db4045`

| method | args | permit | notes |
|---|---|---|---|
| `build_graph` | `wiki_id`, `opts` | `write` on `wiki/graph/**` | — |
| `relevance` | `wiki_id`, `from`, `to` | `read` on `wiki/graph/**` | — |
| `clusters` | `wiki_id` | `read` on `wiki/graph/**` | — |
| `gaps` | `wiki_id` | `read` on `wiki/graph/**` | — |

## `wiki-pages` (PagesRpc)

Plugin: `wiki` — schema stamp: `f3e7b11714a44bd6`

| method | args | permit | notes |
|---|---|---|---|
| `list_pages` | `wiki_id` | `read` on `wiki/pages/**` | — |
| `read_page` | `wiki_id`, `path` | `read` on `wiki/pages/**` | — |
| `write_page` | `wiki_id`, `path`, `markdown`, `base_sha256` | `write` on `wiki/pages/**` | — |

## `wiki-ingest` (IngestRpc)

Plugin: `wiki` — schema stamp: `e8df02e00385891e`

| method | args | permit | notes |
|---|---|---|---|
| `enqueue_ingest` | `wiki_id`, `source_path`, `change` | `write` on `wiki/ingest/**` | — |
| `list_ingest` | `wiki_id` | `read` on `wiki/ingest/**` | — |
| `claim_next_ingest` | `wiki_id` | `write` on `wiki/ingest/**` | — |
| `record_analysis` | `wiki_id`, `task_id`, `analysis` | `write` on `wiki/ingest/**` | — |
| `record_pages` | `wiki_id`, `task_id`, `pages` | `write` on `wiki/ingest/**` | — |
| `fail_ingest` | `wiki_id`, `task_id`, `error` | `write` on `wiki/ingest/**` | — |
| `cancel_ingest` | `wiki_id`, `task_id` | `write` on `wiki/ingest/**` | — |
| `retry_ingest` | `wiki_id`, `task_id` | `write` on `wiki/ingest/**` | — |

## `wiki-lint` (LintRpc)

Plugin: `wiki` — schema stamp: `2f9dbfadd3eb4a5a`

| method | args | permit | notes |
|---|---|---|---|
| `lint` | `wiki_id`, `scope` | `write` on `wiki/lint/**` | — |
| `list_findings` | `wiki_id` | `read` on `wiki/lint/**` | — |
| `resolve_finding` | `wiki_id`, `finding_id`, `action` | `write` on `wiki/lint/**` | — |

## `wiki-search` (SearchRpc)

Plugin: `wiki` — schema stamp: `2d0d4ffacab1e1ce`

| method | args | permit | notes |
|---|---|---|---|
| `search` | `wiki_id`, `opts` | `read` on `wiki/search/**` | — |

## `wiki-events` (EventsStream) — stream

Plugin: `wiki` — schema stamp: `a82bddd598991545`

| method | args | permit | notes |
|---|---|---|---|
| `changes` | `sink` | `read` on `wiki/**` | stream |

## `wiki-watcher` (WatcherRpc)

Plugin: `wiki` — schema stamp: `337ca6dde0d51a6f`

| method | args | permit | notes |
|---|---|---|---|
| `set_watch` | `wiki_id`, `enabled` | `write` on `wiki/watcher/**` | — |
| `is_watching` | `wiki_id` | `read` on `wiki/watcher/**` | — |

## `wiki-multimodal` (MultimodalRpc)

Plugin: `wiki` — schema stamp: `74971e036ac473fd`

| method | args | permit | notes |
|---|---|---|---|
| `extract_images` | `wiki_id`, `source_path`, `opts` | `write` on `wiki/multimodal/**` | — |

## `wiki-review` (ReviewRpc)

Plugin: `wiki` — schema stamp: `0881527ef447a035`

| method | args | permit | notes |
|---|---|---|---|
| `enqueue_review` | `wiki_id`, `item` | `write` on `wiki/review/**` | — |
| `list_review` | `wiki_id` | `read` on `wiki/review/**` | — |
| `apply_review` | `wiki_id`, `item_id`, `action` | `write` on `wiki/review/**` | audited |

## `locations` (LocationsServiceRpc)

Plugin: `home` — schema stamp: `c265c7bd8380a2b7`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `locations/**` | — |
| `get` | `id` | `read` on `locations/**` | — |
| `create` | `loc` | `write` on `locations/**` | — |
| `update` | `loc` | `write` on `locations/**` | — |
| `rename` | `id`, `new_path` | `write` on `locations/**` | — |
| `delete` | `id` | `write` on `locations/**` | audited |

## `inventory` (InventoryServiceRpc)

Plugin: `home` — schema stamp: `1de6386e67e35a85`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `inventory/**` | — |
| `list_at` | `location_id` | `read` on `inventory/**` | — |
| `get` | `id` | `read` on `inventory/**` | — |
| `create` | `item` | `write` on `inventory/**` | — |
| `update` | `item` | `write` on `inventory/**` | — |
| `rename` | `id`, `new_path` | `write` on `inventory/**` | — |
| `delete` | `id` | `write` on `inventory/**` | audited |
| `set_status` | `id`, `status` | `write` on `inventory/**` | — |
| `set_condition` | `id`, `condition` | `write` on `inventory/**` | — |
| `set_location` | `id`, `location_id` | `write` on `inventory/**` | — |

## `cookbook` (CookbookServiceRpc)

Plugin: `mealplan` — schema stamp: `fb790207efbcd9b3`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `mealplan/cookbook/**` | — |
| `get` | `path` | `read` on `mealplan/cookbook/**` | — |
| `create` | `recipe` | `write` on `mealplan/cookbook/**` | — |
| `update` | `recipe` | `write` on `mealplan/cookbook/**` | — |
| `rename` | `old_path`, `new_path` | `write` on `mealplan/cookbook/**` | — |
| `delete` | `path` | `write` on `mealplan/cookbook/**` | audited |
| `import` | `url` | `write` on `mealplan/cookbook/**` | — |

## `mealplan` (MealplanServiceRpc)

Plugin: `mealplan` — schema stamp: `5f8b448efff1207f`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `mealplan/plan/**` | — |
| `get` | `id` | `read` on `mealplan/plan/**` | — |
| `create` | `meal` | `write` on `mealplan/plan/**` | — |
| `update` | `meal` | `write` on `mealplan/plan/**` | — |
| `rename` | `id`, `new_path` | `write` on `mealplan/plan/**` | — |
| `delete` | `id` | `write` on `mealplan/plan/**` | audited |
| `cook` | `id`, `deductions` | `write` on `mealplan/plan/**` | — |
| `skip` | `id` | `write` on `mealplan/plan/**` | — |
| `can_cook` | `recipe_path`, `servings` | `read` on `mealplan/plan/**` | — |
| `cook_recipe` | `recipe_path`, `servings` | `write` on `mealplan/plan/**` | — |

## `pantry` (PantryServiceRpc)

Plugin: `mealplan` — schema stamp: `f33d579e96fc53e9`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `mealplan/pantry/**` | — |
| `get` | `id` | `read` on `mealplan/pantry/**` | — |
| `create` | `item` | `write` on `mealplan/pantry/**` | — |
| `update` | `item` | `write` on `mealplan/pantry/**` | — |
| `rename` | `id`, `new_path` | `write` on `mealplan/pantry/**` | — |
| `delete` | `id` | `write` on `mealplan/pantry/**` | audited |
| `consume` | `id`, `amount` | `write` on `mealplan/pantry/**` | — |
| `restock` | `id`, `amount` | `write` on `mealplan/pantry/**` | — |
| `open` | `id` | `write` on `mealplan/pantry/**` | — |
| `find_by_barcode` | `barcode` | `read` on `mealplan/pantry/**` | — |
| `resolve_barcode` | `barcode` | `read` on `mealplan/pantry/**` | — |
| `add_stock` | `id`, `entry` | `write` on `mealplan/pantry/**` | — |
| `consume_stock` | `id`, `amount` | `write` on `mealplan/pantry/**` | — |
| `transfer_stock` | `id`, `entry_id`, `location_id` | `write` on `mealplan/pantry/**` | — |
| `inventory_set` | `id`, `qty` | `write` on `mealplan/pantry/**` | — |

## `shopping` (ShoppingServiceRpc)

Plugin: `mealplan` — schema stamp: `c8a4a56f4e9b76cb`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `mealplan/shopping/**` | — |
| `get` | `id` | `read` on `mealplan/shopping/**` | — |
| `create` | `list` | `write` on `mealplan/shopping/**` | — |
| `update` | `list` | `write` on `mealplan/shopping/**` | — |
| `delete` | `id` | `write` on `mealplan/shopping/**` | audited |
| `add_missing_for_recipe` | `list_id`, `recipe_path`, `servings` | `write` on `mealplan/shopping/**` | — |
| `add_low_stock` | `list_id` | `write` on `mealplan/shopping/**` | — |
| `add_expired_or_overdue` | `list_id`, `today` | `write` on `mealplan/shopping/**` | — |
| `clear` | `id` | `write` on `mealplan/shopping/**` | — |
| `mark_purchased` | `list_id`, `entry_id` | `write` on `mealplan/shopping/**` | — |

## `substitutions` (SubstitutionServiceRpc)

Plugin: `mealplan` — schema stamp: `c74bff9435eec316`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `mealplan/substitutions/**` | — |
| `get` | `id` | `read` on `mealplan/substitutions/**` | — |
| `create` | `rule` | `write` on `mealplan/substitutions/**` | — |
| `update` | `rule` | `write` on `mealplan/substitutions/**` | — |
| `delete` | `id` | `write` on `mealplan/substitutions/**` | audited |
| `for_item` | `from_item_id` | `read` on `mealplan/substitutions/**` | — |

## `body` (BodyServiceRpc)

Plugin: `fitness` — schema stamp: `81afcaa6c3012ca1`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `fitness/body/**` | — |
| `get` | `id` | `read` on `fitness/body/**` | — |
| `find_by_kind` | `kind` | `read` on `fitness/body/**` | — |
| `create` | `metric` | `write` on `fitness/body/**` | — |
| `update` | `metric` | `write` on `fitness/body/**` | — |
| `delete` | `id` | `write` on `fitness/body/**` | audited |
| `log_entry` | `metric_id`, `entry` | `write` on `fitness/body/**` | — |

## `exercises` (ExercisesServiceRpc)

Plugin: `fitness` — schema stamp: `9c04f00c785c8c8e`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `fitness/exercises/**` | — |
| `get` | `id` | `read` on `fitness/exercises/**` | — |
| `find_by_name` | `name` | `read` on `fitness/exercises/**` | — |
| `create` | `exercise` | `write` on `fitness/exercises/**` | — |
| `update` | `exercise` | `write` on `fitness/exercises/**` | — |
| `rename` | `id`, `new_path` | `write` on `fitness/exercises/**` | — |
| `delete` | `id` | `write` on `fitness/exercises/**` | audited |

## `workouts` (WorkoutsServiceRpc)

Plugin: `fitness` — schema stamp: `73c6d5281212e007`

| method | args | permit | notes |
|---|---|---|---|
| `list_routines` | — | `read` on `fitness/workouts/**` | — |
| `get_routine` | `id` | `read` on `fitness/workouts/**` | — |
| `create_routine` | `routine` | `write` on `fitness/workouts/**` | — |
| `update_routine` | `routine` | `write` on `fitness/workouts/**` | — |
| `delete_routine` | `id` | `write` on `fitness/workouts/**` | audited |
| `list_sessions` | — | `read` on `fitness/workouts/**` | — |
| `get_session` | `id` | `read` on `fitness/workouts/**` | — |
| `create_session` | `session` | `write` on `fitness/workouts/**` | — |
| `update_session` | `session` | `write` on `fitness/workouts/**` | — |
| `delete_session` | `id` | `write` on `fitness/workouts/**` | audited |
| `log_set` | `session_id`, `set` | `write` on `fitness/workouts/**` | — |
| `start_from_routine` | `routine_id`, `day_name`, `date` | `write` on `fitness/workouts/**` | — |

## `intake` (IntakeServiceRpc)

Plugin: `fitness` — schema stamp: `35915551e8628c59`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `fitness/intake/**` | — |
| `get` | `id` | `read` on `fitness/intake/**` | — |
| `for_day` | `date` | `read` on `fitness/intake/**` | — |
| `create` | `log` | `write` on `fitness/intake/**` | — |
| `update` | `log` | `write` on `fitness/intake/**` | — |
| `delete` | `id` | `write` on `fitness/intake/**` | audited |
| `log_recipe` | `date`, `recipe_id`, `servings`, `slot` | `write` on `fitness/intake/**` | — |
| `log_pantry` | `date`, `item_id`, `qty`, `slot` | `write` on `fitness/intake/**` | — |
| `log_freeform` | `date`, `name`, `nutrition`, `slot` | `write` on `fitness/intake/**` | — |
| `log_entry` | `date`, `entry` | `write` on `fitness/intake/**` | — |

## `email` (EmailSyncRpc)

Plugin: `email` — schema stamp: `df6d5c18087eba09`

| method | args | permit | notes |
|---|---|---|---|
| `accounts` | — | `read` on `email/**` | — |
| `list_folders` | `account` | `read` on `email/**` | — |
| `fetch_envelopes` | `account`, `folder`, `range` | `read` on `email/**` | — |
| `fetch_message` | `account`, `message_id` | `read` on `email/**` | — |
| `fetch_attachment` | `account`, `message_id`, `part` | `download` on `email/**` | audited |
| `set_flags` | `account`, `message_id`, `delta` | `write` on `email/**` | — |
| `move_message` | `account`, `message_id`, `dest_folder` | `write` on `email/**` | — |
| `delete_message` | `account`, `message_id` | `write` on `email/**` | audited |
| `append_draft` | `account`, `draft` | `write` on `email/**` | — |
| `send` | `account`, `draft` | `write` on `email/**` | audited |

## `email-stream` (EmailSyncStream) — stream

Plugin: `email` — schema stamp: `f2a53b26b14f4fae`

| method | args | permit | notes |
|---|---|---|---|
| `changes` | `sink` | `read` on `email/**` | stream |

## `forge-repos` (RepoCatalogRpc)

Plugin: `forge` — schema stamp: `b36a0493f396cec0`

| method | args | permit | notes |
|---|---|---|---|
| `list_repos` | — | `read` on `forge/repos/**` | — |
| `get_repo` | `repo` | `read` on `forge/repos/**` | — |

## `forge-issues` (IssueTrackerRpc)

Plugin: `forge` — schema stamp: `80593cb8612de6a2`

| method | args | permit | notes |
|---|---|---|---|
| `list_issues` | `repo`, `filter` | `read` on `forge/issues/**` | — |
| `get_issue` | `repo`, `issue` | `read` on `forge/issues/**` | — |
| `create_issue` | `repo`, `title`, `body` | `write` on `forge/issues/**` | — |
| `update_issue` | `repo`, `issue`, `update` | `write` on `forge/issues/**` | — |
| `list_comments` | `repo`, `issue` | `read` on `forge/issues/**` | — |
| `add_comment` | `repo`, `issue`, `body` | `comment` on `forge/issues/**` | — |

## `forge-reviews` (ReviewSurfaceRpc)

Plugin: `forge` — schema stamp: `fcace7b88f4e0257`

| method | args | permit | notes |
|---|---|---|---|
| `list_pull_requests` | `repo` | `read` on `forge/reviews/**` | — |
| `get_pull_request` | `repo`, `pr` | `read` on `forge/reviews/**` | — |
| `create_pull_request` | `repo`, `new` | `write` on `forge/reviews/**` | — |
| `update_pull_request` | `repo`, `pr`, `update` | `write` on `forge/reviews/**` | — |
| `list_reviews` | `repo`, `pr` | `read` on `forge/reviews/**` | — |
| `request_reviewers` | `repo`, `pr`, `reviewers` | `write` on `forge/reviews/**` | — |
| `merge_pull_request` | `repo`, `pr`, `method` | `write` on `forge/reviews/**` | audited |

## `forge-issues-stream` (IssueTrackerStream) — stream

Plugin: `forge` — schema stamp: `d1959584664f569e`

| method | args | permit | notes |
|---|---|---|---|
| `issue_events` | `sink` | `read` on `forge/issues/**` | stream |

## `forge-reviews-stream` (ReviewSurfaceStream) — stream

Plugin: `forge` — schema stamp: `610bcf152bad7ba9`

| method | args | permit | notes |
|---|---|---|---|
| `review_events` | `sink` | `read` on `forge/reviews/**` | stream |

## `forge-connections` (RepoConnections)

Plugin: `forge` — schema stamp: `b37ad30d0297cb44`

| method | args | permit | notes |
|---|---|---|---|
| `list_connected_repos` | — | `read` on `forge/connections/**` | — |
| `repos_for_project` | `project_id` | `read` on `forge/connections/**` | — |

