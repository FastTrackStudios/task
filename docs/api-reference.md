# Task API reference

<!-- GENERATED — do not edit by hand. Regenerate with:
     cargo run -p task-cli -- api --markdown > docs/api-reference.md
     Source: `task_server::permits::mounts()` (apps/server/src/permits.rs),
     the single registry the router, permit gate, and schema stamps derive from.
     Served live at `GET /org/{slug}/api`. -->

111 services mounted: 88 plain RPC, 23 `#[subscribe]` streams. Every method lists its permit — the `<action>` on `<resource>` the permissions gate checks (see `apps/server/src/permits.rs`). `audited` methods emit an audit line even when allowed. A `stream` method takes a `Tx` sink and pushes to the caller instead of returning once.

## `auth` (AuthService)

Plugin: `core` — schema stamp: `14f9a4c8473bda45`

| method | args | permit | notes |
|---|---|---|---|
| `sign_up_email_password` | `input` | `write` on `auth/signup` | audited |
| `sign_in_email_password` | `input` | `read` on `public/auth` | — |
| `current_session` | `token` | `read` on `public/auth` | — |
| `refresh_session` | `token` | `read` on `public/auth` | — |
| `whoami` | `token` | `read` on `public/auth` | — |
| `sign_out` | `token` | `read` on `public/auth` | — |
| `list_org_members` | `token` | `read` on `public/auth` | — |
| `migrate_user_email` | `input` | `write` on `auth/migrate` | audited |
| `list_email_history` | `input` | `read` on `auth/migrate` | audited |
| `change_password` | `input` | `write` on `auth/self` | audited |
| `change_email` | `input` | `write` on `auth/self` | audited |
| `update_profile` | `input` | `write` on `auth/self` | — |

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

## `media` (AttachmentMediaService)

Plugin: `core` — schema stamp: `08b88215b65bcc4d`

| method | args | permit | notes |
|---|---|---|---|
| `stat` | `content_hash` | `read` on `media/{content_hash}` | — |
| `read` | `content_hash`, `start`, `len`, `tx` | `read` on `media/{content_hash}` | stream, audited |
| `media_grant` | `prefix` | `download` on `media/**` | audited |

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

Plugin: `core` — schema stamp: `3a0a14f7bd088962`

| method | args | permit | notes |
|---|---|---|---|
| `create_link` | `target`, `options` | `write` on `shares/**` | audited |
| `update_link` | `token`, `options` | `write` on `shares/**` | audited |
| `list_links` | — | `read` on `shares/**` | — |
| `links_for_target` | `target` | `read` on `shares/**` | — |
| `set_link_disabled` | `token`, `disabled` | `write` on `shares/**` | audited |
| `delete_link` | `token` | `write` on `shares/**` | audited |
| `access_log` | `token` | `read` on `shares/**` | — |
| `set_sharing_disabled` | `disabled` | `write` on `shares/**` | audited |
| `sharing_disabled` | — | `read` on `shares/**` | — |
| `list_incoming` | `token` | `read` on `shares/**` | — |
| `promote_incoming` | `token`, `name`, `dest_path` | `write` on `shares/**` | audited |

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

## `agent-backends` (Backends)

Plugin: `agent` — schema stamp: `6d2ff6b4d624da00`

| method | args | permit | notes |
|---|---|---|---|
| `upsert_backend` | `backend` | `write` on `agent/runners/**` | audited |
| `remove_backend` | `backend_id` | `write` on `agent/runners/**` | audited |
| `list_backends` | — | `read` on `agent/runners/**` | — |
| `backend_health` | `backend_id` | `read` on `agent/runners/**` | — |
| `heartbeat_backend` | `backend_id` | `write` on `agent/runners/**` | — |
| `backends_by_kind` | `kind` | `read` on `agent/runners/**` | — |

## `agent-runs` (Runs)

Plugin: `agent` — schema stamp: `efa1e19841a84f06`

| method | args | permit | notes |
|---|---|---|---|
| `start_run` | `start` | `write` on `agent/runs/**` | — |
| `beat_run` | `run_id` | `write` on `agent/runs/**` | — |
| `finish_run` | `finish` | `write` on `agent/runs/**` | — |
| `get_run` | `run_id` | `read` on `agent/runs/**` | — |
| `list_runs` | `filter` | `read` on `agent/runs/**` | — |
| `archive_run` | `run_id` | `write` on `agent/runs/**` | — |
| `sweep_stale_runs` | — | `write` on `agent/runs/**` | — |

## `agent-questions` (Questions)

Plugin: `agent` — schema stamp: `d371bdf0b1413d07`

| method | args | permit | notes |
|---|---|---|---|
| `ask_question` | `ask` | `write` on `agent/questions/**` | — |
| `unresolved_questions` | — | `read` on `agent/questions/**` | — |
| `questions_for_ticket` | `ticket` | `read` on `agent/questions/**` | — |
| `list_pending_questions` | `session_id` | `read` on `agent/questions/**` | — |
| `answer_question` | `request_id`, `answers` | `write` on `agent/questions/**` | — |
| `question_ticket` | `request_id` | `read` on `agent/questions/**` | — |

## `agent-run-stream` (RunStream)

Plugin: `agent` — schema stamp: `806bf5cce76887b7`

| method | args | permit | notes |
|---|---|---|---|
| `snapshot` | `run` | `read` on `agent/runs/**` | — |
| `publish` | `run`, `event` | `write` on `agent/runs/**` | — |

## `agent-run-events` (RunStreamStream) — stream

Plugin: `agent` — schema stamp: `7f06338c6e80c1c8`

| method | args | permit | notes |
|---|---|---|---|
| `run_events` | `sink` | `read` on `agent/runs/**` | stream |

## `project` (ProjectServiceRpc)

Plugin: `core` — schema stamp: `bc49b5b0da521891`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `projects/**` | — |
| `get` | `id` | `read` on `projects/**` | — |
| `get_by_path` | `path` | `read` on `projects/**` | — |
| `create` | `project` | `write` on `projects/**` | — |
| `update` | `project` | `write` on `projects/**` | — |
| `rename` | `id`, `new_path` | `write` on `projects/**` | — |
| `delete` | `id` | `write` on `projects/**` | audited |
| `parts` | `project` | `read` on `projects/**` | — |
| `add_part` | `project`, `name` | `write` on `projects/**` | — |
| `rename_part` | `project`, `part`, `name` | `write` on `projects/**` | — |
| `remove_part` | `project`, `part` | `write` on `projects/**` | — |
| `pieces` | `project` | `read` on `projects/**` | — |
| `promote_part` | `project`, `part` | `write` on `projects/**` | — |
| `demote_project` | `project` | `write` on `projects/**` | audited |
| `divergences` | `project` | `read` on `projects/**` | — |
| `attach_component` | `project`, `part`, `component` | `write` on `projects/**` | — |
| `detach_component` | `project`, `part`, `name` | `write` on `projects/**` | — |
| `deliverables` | `project` | `read` on `projects/**` | — |
| `declare_deliverable` | `project`, `deliverable` | `write` on `projects/**` | — |
| `withdraw_deliverable` | `project`, `deliverable` | `write` on `projects/**` | audited |
| `deliverable_items` | `project` | `read` on `projects/**` | — |
| `client_deliverables` | `project` | `read` on `projects/**` | — |
| `adopt` | `dir`, `title` | `write` on `projects/**` | — |
| `set_setlist` | `project`, `songs` | `write` on `projects/**` | — |
| `setlist` | `project` | `read` on `projects/**` | — |
| `merge` | `into`, `absorbed` | `write` on `projects/**` | audited |

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

## `files` (FilesService)

Plugin: `core` — schema stamp: `8647f4291f9cf1c0`

| method | args | permit | notes |
|---|---|---|---|
| `create_root` | `path`, `name`, `flavor` | `write` on `files/**` | — |
| `list_roots` | — | `read` on `files/**` | — |
| `get_root` | `id` | `read` on `files/**` | — |
| `browse` | `root_id`, `subpath` | `read` on `files/**` | — |
| `drive_browse` | `path` | `read` on `files/**` | — |
| `tree_browse` | `path` | `read` on `files/**` | — |
| `chain` | `root_id`, `path` | `read` on `files/**` | — |
| `checkpoint_now` | `root_id`, `description` | `write` on `files/**` | — |
| `hint_activity` | `root_id`, `paths` | `write` on `files/**` | — |
| `snapshots` | `root_id` | `read` on `files/**` | — |
| `ignore_set` | `root_id` | `read` on `files/**` | — |
| `set_ignore_set` | `root_id`, `patterns` | `write` on `files/**` | — |
| `name_version` | `root_id`, `commit_id`, `name` | `write` on `files/**` | — |
| `list_named_versions` | `root_id` | `read` on `files/**` | — |
| `resolve_named_version` | `id` | `read` on `files/**` | — |
| `unname_version` | `id` | `write` on `files/**` | audited |
| `start_project_version` | `root_id`, `label` | `write` on `files/**` | — |
| `list_project_versions` | `root_id` | `read` on `files/**` | — |
| `restart_project_version` | `root_id`, `mode`, `label` | `write` on `files/**` | audited |
| `browse_at` | `root_id`, `commit_id`, `subpath` | `read` on `files/**` | — |
| `copy_forward` | `root_id`, `commit_id`, `paths` | `write` on `files/**` | audited |
| `gc_root` | `root_id`, `keep_newer_secs` | `write` on `files/**` | audited |
| `dehydrate` | `root_id`, `path` | `write` on `files/**` | audited |
| `hydrate` | `root_id`, `path` | `write` on `files/**` | — |
| `hydration_policy` | `root_id` | `read` on `files/**` | — |
| `set_hydration_policy` | `root_id`, `patterns` | `write` on `files/**` | — |
| `apply_hydration_policy` | `root_id` | `write` on `files/**` | audited |
| `divergences` | `root_id` | `read` on `files/**` | — |
| `resolve_divergence` | `root_id`, `path`, `choice` | `write` on `files/**` | audited |
| `rendition` | `root_id`, `path`, `kind` | `read` on `files/**` | — |
| `rendition_at` | `root_id`, `path`, `commit_id`, `kind` | `read` on `files/**` | — |
| `find_review` | `root_id`, `file_path` | `read` on `files/**` | — |
| `review_for_file` | `root_id`, `file_path` | `comment` on `files/**` | — |
| `list_reviews` | `root_id` | `read` on `files/**` | — |
| `review_comments` | `review_id` | `read` on `files/**` | — |
| `add_review_comment` | `review_id`, `comment` | `comment` on `files/**` | — |
| `delete_review_comment` | `id` | `write` on `files/**` | audited |

## `files-stream` (FilesServiceStream) — stream

Plugin: `core` — schema stamp: `7e4188c059889b46`

| method | args | permit | notes |
|---|---|---|---|
| `events` | `sink` | `read` on `files/**` | stream |

## `files-roots` (RootsService)

Plugin: `core` — schema stamp: `fcc51ced0d9a66c8`

| method | args | permit | notes |
|---|---|---|---|
| `adopt` | `request` | `write` on `files/**` | — |
| `resume_adoption` | `root_id` | `write` on `files/**` | — |
| `pause_adoption` | `root_id` | `write` on `files/**` | — |
| `adoption_progress` | `root_id` | `read` on `files/**` | — |
| `host_structure` | `root_id`, `name`, `flavor` | `write` on `files/**` | audited |
| `list` | — | `read` on `files/**` | — |
| `get` | `root_id` | `read` on `files/**` | — |
| `rename_root` | `root_id`, `name` | `write` on `files/**` | — |
| `release` | `root_id` | `write` on `files/**` | audited |

## `files-tree` (TreeService)

Plugin: `core` — schema stamp: `a90403091dca6ad1`

| method | args | permit | notes |
|---|---|---|---|
| `browse` | `root_id`, `path` | `read` on `files/**` | — |
| `resolve` | `path` | `read` on `files/**` | — |
| `entry` | `root_id`, `path` | `read` on `files/**` | — |
| `catalogue` | `root_id`, `cursor` | `read` on `files/**` | — |
| `changes_since` | `root_id`, `cursor` | `read` on `files/**` | — |
| `freshness` | — | `read` on `files/**` | — |

## `files-write` (WriteService)

Plugin: `core` — schema stamp: `132c223dc63eaf1d`

| method | args | permit | notes |
|---|---|---|---|
| `create_dirs` | `root_id`, `paths` | `write` on `files/**` | — |
| `rename` | `root_id`, `path`, `name` | `write` on `files/**` | — |
| `move_paths` | `root_id`, `moves`, `on_conflict` | `write` on `files/**` | — |
| `copy_paths` | `root_id`, `copies`, `on_conflict` | `write` on `files/**` | — |
| `delete_paths` | `root_id`, `paths` | `write` on `files/**` | audited |
| `archive` | `root_id`, `paths` | `download` on `files/**` | audited |

## `files-upload` (UploadService)

Plugin: `core` — schema stamp: `db526db319220972`

| method | args | permit | notes |
|---|---|---|---|
| `begin` | `spec` | `write` on `files/**` | — |
| `progress` | `upload_id` | `read` on `files/**` | — |
| `complete` | `upload_id`, `on_conflict` | `write` on `files/**` | audited |
| `abort` | `upload_id` | `write` on `files/**` | — |
| `pending` | — | `read` on `files/**` | — |
| `send_bytes` | `upload_id`, `frames` | `write` on `files/**` | stream |

## `files-version` (VersionService)

Plugin: `core` — schema stamp: `0f726632221d3e05`

| method | args | permit | notes |
|---|---|---|---|
| `chain` | `root_id`, `path` | `read` on `files/**` | — |
| `checkpoint` | `root_id`, `description` | `write` on `files/**` | — |
| `snapshots` | `root_id`, `limit` | `read` on `files/**` | — |
| `hold` | `root_id`, `path` | `write` on `files/**` | — |
| `occupancy` | `root_id`, `path` | `read` on `files/**` | — |
| `divergences` | `root_id` | `read` on `files/**` | — |
| `resolve_divergence` | `root_id`, `version`, `resolution` | `write` on `files/**` | audited |
| `restore` | `root_id`, `path`, `version` | `write` on `files/**` | audited |
| `keep_snapshot` | `root_id`, `snapshot` | `write` on `files/**` | — |

## `files-curation` (CurationService)

Plugin: `core` — schema stamp: `8656dec04fba5098`

| method | args | permit | notes |
|---|---|---|---|
| `name_version` | `root_id`, `version`, `name` | `write` on `files/**` | — |
| `unname_version` | `root_id`, `version` | `write` on `files/**` | audited |
| `named_versions` | `root_id`, `path` | `read` on `files/**` | — |
| `resolve_name` | `root_id`, `name` | `read` on `files/**` | — |
| `start_project_version` | `root_id`, `name` | `write` on `files/**` | — |
| `project_versions` | `root_id` | `read` on `files/**` | — |
| `restart_project_version` | `root_id`, `project_version`, `mode` | `write` on `files/**` | audited |

## `files-sync` (SyncService)

Plugin: `core` — schema stamp: `b2bf1fcccd0763d9`

| method | args | permit | notes |
|---|---|---|---|
| `facets` | `root_id` | `read` on `files/**` | — |
| `map_facet` | `root_id`, `path`, `facet` | `write` on `files/**` | — |
| `ignore_set` | `root_id` | `read` on `files/**` | — |
| `set_project_ignores` | `root_id`, `patterns` | `write` on `files/**` | — |
| `subscription` | `root_id` | `read` on `files/**` | — |
| `subscribe` | `root_id`, `facets` | `write` on `files/**` | — |
| `pin` | `root_id`, `paths`, `pinned` | `write` on `files/**` | — |
| `hydrate` | `root_id`, `paths`, `resident` | `write` on `files/**` | audited |
| `devices` | — | `read` on `files/**` | — |
| `enroll_device` | `endpoint`, `name` | `write` on `files/**` | audited |
| `coordinator` | — | `read` on `files/**` | — |
| `set_transfer_policy` | `device`, `policy` | `write` on `files/**` | — |
| `revoke_device` | `device` | `write` on `files/**` | audited |

## `files-replica` (SyncService)

Plugin: `core` — schema stamp: `72b9fb2e22b51f12`

| method | args | permit | notes |
|---|---|---|---|
| `roots` | — | `read` on `files/replica` | — |
| `heads` | `root_id` | `read` on `files/replica` | — |
| `object` | `root_id`, `id` | `read` on `files/replica` | — |
| `manifest` | `root_id`, `file_id` | `read` on `files/replica` | — |
| `chunks` | `root_id`, `hashes` | `download` on `files/replica` | audited |
| `chunk_ranges` | `root_id`, `hash`, `from_chunk`, `chunks` | `download` on `files/replica` | audited |

## `files-access` (AccessService)

Plugin: `core` — schema stamp: `a3f88bea9630b812`

| method | args | permit | notes |
|---|---|---|---|
| `grant` | `subject`, `root_id`, `path`, `capabilities` | `write` on `files/**` | audited |
| `revoke` | `grant` | `write` on `files/**` | audited |
| `grants` | `root_id`, `path` | `read` on `files/**` | — |
| `effective` | `root_id`, `path` | `read` on `files/**` | — |
| `create_share` | `root_id`, `path`, `capabilities` | `write` on `files/**` | audited |
| `set_share_disabled` | `share`, `disabled` | `write` on `files/**` | audited |
| `shares` | `root_id` | `read` on `files/**` | — |

## `files-organise` (OrganiseService)

Plugin: `core` — schema stamp: `79adf0779c9ab38c`

| method | args | permit | notes |
|---|---|---|---|
| `marks` | `root_id`, `path` | `read` on `files/**` | — |
| `set_tags` | `root_id`, `path`, `tags` | `write` on `files/**` | — |
| `set_favourite` | `root_id`, `path`, `favourite` | `write` on `files/**` | — |
| `tagged` | `tags`, `root_id` | `read` on `files/**` | — |
| `all_tags` | `root_id` | `read` on `files/**` | — |
| `activity` | `root_id`, `under`, `limit` | `read` on `files/**` | — |

## `files-federation` (FederationService)

Plugin: `core` — schema stamp: `e7babcb3f93fe801`

| method | args | permit | notes |
|---|---|---|---|
| `offer` | `root_id`, `path`, `to`, `capabilities` | `write` on `files/**` | audited |
| `withdraw` | `grant` | `write` on `files/**` | audited |
| `offered` | — | `read` on `files/**` | — |
| `accept` | `offer` | `write` on `files/**` | audited |
| `remotes` | — | `read` on `files/**` | — |
| `forget` | `root_id` | `write` on `files/**` | audited |
| `read_offered` | `secret`, `path` | `read` on `public/files-offer` | audited |
| `fetch_offered` | `secret`, `token`, `range` | `read` on `public/files-offer` | audited |
| `browse_offered` | `secret`, `path` | `read` on `public/files-offer` | audited |

## `files-media` (MediaService)

Plugin: `core` — schema stamp: `549c6b8f83b05140`

| method | args | permit | notes |
|---|---|---|---|
| `read` | `root_id`, `path` | `read` on `files/**` | — |
| `read_at` | `root_id`, `path`, `version` | `read` on `files/**` | — |
| `read_content` | `content` | `read` on `files/**` | — |
| `renditions` | `root_id`, `path` | `read` on `files/**` | — |
| `rendition` | `root_id`, `path`, `kind` | `read` on `files/**` | — |
| `handoff` | `name`, `target`, `items` | `write` on `files/**` | — |

## `files-media-stream` (MediaServiceStream) — stream

Plugin: `core` — schema stamp: `058d6a5c85afaac2`

| method | args | permit | notes |
|---|---|---|---|
| `bytes` | `request`, `sink` | `download` on `files/**` | stream, audited |

## `files-search` (SearchService)

Plugin: `core` — schema stamp: `0a7795144f0ae614`

| method | args | permit | notes |
|---|---|---|---|
| `search` | `query` | `read` on `files/**` | — |
| `extract_state` | `root_id`, `path` | `read` on `files/**` | — |
| `pending` | `root_id` | `read` on `files/**` | — |
| `extract` | `root_id`, `paths`, `kinds` | `write` on `files/**` | — |

## `files-review` (ReviewService)

Plugin: `core` — schema stamp: `f628aa2d4d23af71`

| method | args | permit | notes |
|---|---|---|---|
| `scope` | — | `read` on `files/**` | — |
| `review` | `review` | `read` on `files/**` | — |
| `playback` | `review`, `version` | `read` on `files/**` | — |
| `comments` | `review` | `read` on `files/**` | — |
| `comment` | `comment` | `comment` on `files/**` | — |
| `delete_comment` | `comment` | `write` on `files/**` | audited |
| `for_file` | `root_id`, `path` | `read` on `files/**` | — |

## `storage` (StorageService)

Plugin: `core` — schema stamp: `1ac4f2de58320881`

| method | args | permit | notes |
|---|---|---|---|
| `list_locations` | — | `read` on `files/**` | — |
| `list_grants` | — | `read` on `files/**` | — |
| `place_root` | `root_id`, `location_id`, `relative_path` | `write` on `files/**` | — |
| `placement` | `root_id` | `read` on `files/**` | — |
| `list_placements` | — | `read` on `files/**` | — |
| `add_blob_replica` | `root_id`, `location_id` | `write` on `files/**` | — |
| `refresh_usage` | `root_id` | `write` on `files/**` | — |
| `usage` | `location_id` | `read` on `files/**` | — |

## `storage-stream` (StorageServiceStream) — stream

Plugin: `core` — schema stamp: `1e93d244256f3166`

| method | args | permit | notes |
|---|---|---|---|
| `events` | `sink` | `read` on `files/**` | stream |

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

Plugin: `core` — schema stamp: `c6aece882f950c6e`

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
| `list_tags` | `org_id` | `read` on `timer/**` | — |
| `create_tag` | `org_id`, `name`, `color` | `write` on `timer/**` | — |
| `delete_tag` | `org_id`, `name` | `write` on `timer/**` | audited |
| `attach_tags` | `session_id`, `org_id`, `names` | `write` on `timer/**` | — |
| `detach_tags` | `session_id`, `org_id`, `names`, `all` | `write` on `timer/**` | — |

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

## `notify` (Notify)

Plugin: `core` — schema stamp: `8c6528e77d7cf602`

| method | args | permit | notes |
|---|---|---|---|
| `list` | `filter` | `read` on `notifications/**` | — |
| `mark_read` | `id` | `write` on `notifications/**` | — |
| `mark_all_read` | — | `write` on `notifications/**` | — |
| `delete` | `id` | `write` on `notifications/**` | audited |

## `notify-stream` (NotifyStream) — stream

Plugin: `core` — schema stamp: `f61bff918b362048`

| method | args | permit | notes |
|---|---|---|---|
| `events` | `sink` | `read` on `notifications/**` | stream |

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

Plugin: `finance` — schema stamp: `c68659b9e2165b18`

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
| `commit_invoice` | `book`, `party`, `invoice`, `session_ids` | `write` on `finance/invoicing/**` | audited |
| `void_invoice` | `id` | `write` on `finance/invoicing/**` | audited |

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

Plugin: `wiki` — schema stamp: `5ef2be11fa237516`

| method | args | permit | notes |
|---|---|---|---|
| `import_raw_source` | `wiki_id`, `source` | `write` on `wiki/raw/**` | audited |
| `list_raw_sources` | `wiki_id` | `read` on `wiki/raw/**` | — |
| `read_raw_source` | `wiki_id`, `path` | `read` on `wiki/raw/**` | — |
| `delete_raw_source` | `wiki_id`, `path` | `write` on `wiki/raw/**` | audited |
| `rescan_sources` | `wiki_id` | `write` on `wiki/raw/**` | — |
| `rescan_diff` | `wiki_id` | `write` on `wiki/raw/**` | — |

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

Plugin: `mealplan` — schema stamp: `7f61764cd066b347`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `mealplan/cookbook/**` | — |
| `get` | `path` | `read` on `mealplan/cookbook/**` | — |
| `create` | `recipe` | `write` on `mealplan/cookbook/**` | — |
| `update` | `recipe` | `write` on `mealplan/cookbook/**` | — |
| `rename` | `old_path`, `new_path` | `write` on `mealplan/cookbook/**` | — |
| `delete` | `path` | `write` on `mealplan/cookbook/**` | audited |
| `import` | `url` | `write` on `mealplan/cookbook/**` | — |
| `image` | `path` | `read` on `mealplan/cookbook/**` | — |
| `put_image` | `path`, `bytes` | `write` on `mealplan/cookbook/**` | — |

## `mealplan` (MealplanServiceRpc)

Plugin: `mealplan` — schema stamp: `fe64446d5f7aa37c`

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
| `eat_out` | `id` | `write` on `mealplan/plan/**` | — |
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

Plugin: `mealplan` — schema stamp: `2da7f4ef4bf91844`

| method | args | permit | notes |
|---|---|---|---|
| `list` | — | `read` on `mealplan/shopping/**` | — |
| `get` | `id` | `read` on `mealplan/shopping/**` | — |
| `create` | `list` | `write` on `mealplan/shopping/**` | — |
| `update` | `list` | `write` on `mealplan/shopping/**` | — |
| `delete` | `id` | `write` on `mealplan/shopping/**` | audited |
| `add_missing_for_recipe` | `list_id`, `recipe_path`, `servings` | `write` on `mealplan/shopping/**` | — |
| `add_recipe_ingredients` | `list_id`, `recipe_path`, `servings` | `write` on `mealplan/shopping/**` | — |
| `add_low_stock` | `list_id` | `write` on `mealplan/shopping/**` | — |
| `add_expired_or_overdue` | `list_id`, `today` | `write` on `mealplan/shopping/**` | — |
| `clear` | `id` | `write` on `mealplan/shopping/**` | — |
| `mark_purchased` | `list_id`, `entry_id` | `write` on `mealplan/shopping/**` | — |
| `mark_have` | `list_id`, `entry_id`, `have` | `write` on `mealplan/shopping/**` | — |
| `reset` | `id` | `write` on `mealplan/shopping/**` | — |
| `start_from_template` | `template_id`, `name` | `write` on `mealplan/shopping/**` | — |
| `save_as_template` | `list_id`, `name` | `write` on `mealplan/shopping/**` | — |

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

## `email-product` (EmailProductRpc)

Plugin: `email` — schema stamp: `2e3a196f2d85deff`

| method | args | permit | notes |
|---|---|---|---|
| `derivations` | `account`, `ids` | `read` on `email/outbox/**` | — |
| `list_outbox` | `account` | `read` on `email/outbox/**` | — |
| `submit_draft` | `account`, `draft`, `origin` | `write` on `email/outbox/**` | — |
| `approve` | `account`, `id` | `write` on `email/outbox/**` | audited |
| `cancel` | `account`, `id` | `write` on `email/outbox/**` | — |
| `unnotified` | `account`, `limit` | `read` on `email/outbox/**` | — |
| `mark_notified` | `account`, `ids` | `write` on `email/outbox/**` | — |

## `email-stream` (EmailSyncStream) — stream

Plugin: `email` — schema stamp: `f2a53b26b14f4fae`

| method | args | permit | notes |
|---|---|---|---|
| `changes` | `sink` | `read` on `email/**` | stream |

## `email-links` (EmailLinksRpc)

Plugin: `email` — schema stamp: `2f392bad3a6f7579`

| method | args | permit | notes |
|---|---|---|---|
| `link` | `message_id`, `target`, `linked_by` | `write` on `email/links/**` | — |
| `unlink` | `message_id`, `target` | `write` on `email/links/**` | audited |
| `links_for_message` | `message_id` | `read` on `email/links/**` | — |
| `links_for_target` | `target` | `read` on `email/links/**` | — |

## `git-repos` (RepoCatalogRpc)

Plugin: `git` — schema stamp: `b36a0493f396cec0`

| method | args | permit | notes |
|---|---|---|---|
| `list_repos` | — | `read` on `git/repos/**` | — |
| `get_repo` | `repo` | `read` on `git/repos/**` | — |

## `git-issues` (IssueTrackerRpc)

Plugin: `git` — schema stamp: `80593cb8612de6a2`

| method | args | permit | notes |
|---|---|---|---|
| `list_issues` | `repo`, `filter` | `read` on `git/issues/**` | — |
| `get_issue` | `repo`, `issue` | `read` on `git/issues/**` | — |
| `create_issue` | `repo`, `title`, `body` | `write` on `git/issues/**` | — |
| `update_issue` | `repo`, `issue`, `update` | `write` on `git/issues/**` | — |
| `list_comments` | `repo`, `issue` | `read` on `git/issues/**` | — |
| `add_comment` | `repo`, `issue`, `body` | `comment` on `git/issues/**` | — |

## `git-reviews` (ReviewSurfaceRpc)

Plugin: `git` — schema stamp: `fcace7b88f4e0257`

| method | args | permit | notes |
|---|---|---|---|
| `list_pull_requests` | `repo` | `read` on `git/reviews/**` | — |
| `get_pull_request` | `repo`, `pr` | `read` on `git/reviews/**` | — |
| `create_pull_request` | `repo`, `new` | `write` on `git/reviews/**` | — |
| `update_pull_request` | `repo`, `pr`, `update` | `write` on `git/reviews/**` | — |
| `list_reviews` | `repo`, `pr` | `read` on `git/reviews/**` | — |
| `request_reviewers` | `repo`, `pr`, `reviewers` | `write` on `git/reviews/**` | — |
| `merge_pull_request` | `repo`, `pr`, `method` | `write` on `git/reviews/**` | audited |

## `git-issues-stream` (IssueTrackerStream) — stream

Plugin: `git` — schema stamp: `d1959584664f569e`

| method | args | permit | notes |
|---|---|---|---|
| `issue_events` | `sink` | `read` on `git/issues/**` | stream |

## `git-reviews-stream` (ReviewSurfaceStream) — stream

Plugin: `git` — schema stamp: `610bcf152bad7ba9`

| method | args | permit | notes |
|---|---|---|---|
| `review_events` | `sink` | `read` on `git/reviews/**` | stream |

## `git-connections` (RepoConnections)

Plugin: `git` — schema stamp: `b37ad30d0297cb44`

| method | args | permit | notes |
|---|---|---|---|
| `list_connected_repos` | — | `read` on `git/connections/**` | — |
| `repos_for_project` | `project_id` | `read` on `git/connections/**` | — |

