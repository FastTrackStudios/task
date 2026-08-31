//! Multi-org data feeds.
//!
//! Each fetcher takes the resolved list of org slugs (from
//! [`crate::orgs::selected_slugs`]) and fans out **concurrently** —
//! establishing each org's service client and concatenating the rows.
//! "All" mode passes every hosted slug; single-org mode passes one.
//! Per-org failures are tolerated in multi-org mode (a down/empty org
//! doesn't blank the whole view); an error only surfaces if *nothing*
//! came back.

use project_proto::ProjectInfo;
use task_proto::TaskInfo as DbTask;
// The `feeds!` declaration macro and the multi-org fan-out helpers live
// in `task-ui-core`, so feature UI crates declare their own calls the
// same way — see that module's docs for the shape.
use task_ui_core::feeds;
use task_ui_core::feeds::{collect, fan_out, fan_out_tagged};

/// Active projects across the selected orgs (concurrent fan-out).
pub async fn fetch_projects(slugs: &[String]) -> Result<Vec<ProjectInfo>, String> {
    fan_out(
        slugs,
        "list",
        |c: project_proto::ProjectServiceClient| async move { c.list().await },
    )
    .await
}

/// Tasks across the selected orgs (concurrent fan-out).
pub async fn fetch_tasks(slugs: &[String]) -> Result<Vec<DbTask>, String> {
    Ok(fetch_tasks_tagged(slugs)
        .await?
        .into_iter()
        .map(|(_, t)| t)
        .collect())
}

/// Projects across the selected orgs, each paired with the slug of the
/// org it came from — feeds the shared project store so mutations and
/// the detail page can route back to the owning org.
pub async fn fetch_projects_tagged(slugs: &[String]) -> Result<Vec<(String, ProjectInfo)>, String> {
    fan_out_tagged(
        slugs,
        "list",
        |c: project_proto::ProjectServiceClient| async move { c.list().await },
    )
    .await
}

/// Tasks across the selected orgs, each paired with the slug of the org
/// it came from — so mutations can be routed back to the right org's
/// `TaskService` when viewing "All".
pub async fn fetch_tasks_tagged(slugs: &[String]) -> Result<Vec<(String, DbTask)>, String> {
    fan_out_tagged(
        slugs,
        "list",
        |c: task_proto::TaskServiceClient| async move { c.list().await },
    )
    .await
}

/// Goals across the selected orgs, each paired with the slug of the
/// org it came from — feeds the shared goal store (the `/goals` page
/// renders the merged hierarchy; the slug tag keys the live fold).
pub async fn fetch_goals_tagged(
    slugs: &[String],
) -> Result<Vec<(String, goal_proto::Goal)>, String> {
    fan_out_tagged(
        slugs,
        "list",
        |c: goal_proto::GoalServiceClient| async move { c.list().await },
    )
    .await
}

feeds! {
    scheduling_proto::DayTemplatesClient {
        /// Fetch one org's day-plan templates (drives the calendar schedule
        /// overlay), in the order the backend lists them.
        fetch_day_templates() -> Vec<scheduling_proto::DayTemplate>
            = list_day_templates() as "day templates";
    }

    scheduling_proto::DayPlansClient {
        /// The saved per-date plan for `date` (ISO `YYYY-MM-DD`), or `None`
        /// when the date hasn't been edited (caller materializes a default).
        fetch_day_plan(date: &str) -> Option<scheduling_proto::DayPlan>
            = get_day_plan(date.to_string()) as format!("day plan {date}");

        /// Save (replacing) a per-date plan.
        save_day_plan(plan: scheduling_proto::DayPlan) -> ()
            = upsert_day_plan(plan) as "save day plan";

        /// Delete a per-date plan, reverting that date to the template.
        delete_day_plan(date: &str) -> ()
            = delete_day_plan(date.to_string()) as format!("delete day plan {date}");
    }

    scheduling_proto::CalendarEventsClient {
        /// All persisted calendar events for the org.
        list_events() -> Vec<scheduling_proto::CalEvent>
            = list_events() as "list events";

        /// Save (replacing) one calendar event.
        upsert_event(event: scheduling_proto::CalEvent) -> ()
            = upsert_event(event) as "save event";

        /// Delete one calendar event.
        delete_event(id: &str) -> ()
            = delete_event(id.to_string()) as "delete event";
    }
}

// ── Bookings (Cal.com-style booking half) ───────────────────────────

feeds! {
    scheduling_proto::EventTypesClient {
        /// All bookable event types for the org (30-min consults, etc.).
        fetch_event_types() -> Vec<scheduling_proto::EventType>
            = list_event_types() as "list event types";

        /// Create (upsert) a bookable event type, returning the persisted draft
        /// so optimistic stores can reconcile against it. The backend derives
        /// the vault `path` from the slug/id; the caller builds the entity (see
        /// `stores::draft_event_type`).
        create_event_type(event_type: scheduling_proto::EventType) -> scheduling_proto::EventType
            = upsert_event_type(event_type.clone()) map |()| event_type, as "create event type";
    }

    scheduling_proto::BookingsClient {
        /// All bookings for the org (every status), oldest start first.
        fetch_bookings() -> Vec<scheduling_proto::Booking>
            = list_bookings() as "list bookings";

        /// Cancel a booking by id (sets status to `Cancelled`).
        cancel_booking(id: &str) -> ()
            = update_booking_status(scheduling_proto::BookingId(id.to_owned()), scheduling_proto::BookingStatus::Cancelled) as "cancel booking";
    }
}

/// Lowercase, hyphenate, strip non-url-safe chars for an event-type slug.
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_owned()
}

feeds! {
    inbox_proto::InboxClient {
        /// Every inbox item (open + processed + archived), oldest first.
        /// Consumers filter by `status` / `resurface_on` for the daily queue.
        fetch_inbox() -> Vec<inbox_proto::InboxItem>
            = list_inbox() as "list inbox";

        /// Capture or update one inbox item (keyed by id). Capture, snooze,
        /// process, and archive all flow through here.
        upsert_inbox_item(item: inbox_proto::InboxItem) -> ()
            = upsert_inbox_item(item) as "save inbox item";

        /// Delete one inbox item.
        delete_inbox_item(id: &str) -> ()
            = delete_inbox_item(id.to_string()) as "delete inbox item";
    }
}

feeds! {
    notify_proto::NotifyClient {
        /// Recent notifications, newest first (one default server
        /// page) — the bell's backing fetch; the live fold keeps it
        /// current after that.
        fetch_notifications() -> Vec<notify_proto::Notification>
            = list(notify_proto::NotifyListFilter::recent()) as "list notifications";

        /// Flip one notification read (returns the post-write row;
        /// the stream folds it everywhere).
        mark_notification_read(id: uuid::Uuid) -> notify_proto::Notification
            = mark_read(id) as "mark notification read";

        /// Flip every unread notification read.
        mark_all_notifications_read() -> u64
            = mark_all_read() as "mark all notifications read";
    }
}

// ── Recall (spaced-repetition learning deck) ─────────────────────────

feeds! {
    recall_proto::RecallClient {
        /// Every recall card (all decks, archived + active), oldest first.
        /// Consumers filter by `project` / `archived` / due-date client-side.
        fetch_recall_cards() -> Vec<recall_proto::RecallCard>
            = list_cards() as "list recall cards";

        /// The due, non-archived review queue for `today` (ISO `YYYY-MM-DD`).
        fetch_recall_due(today: &str) -> Vec<recall_proto::RecallCard>
            = review_queue(today.to_string()) as "recall review queue";

        /// Create or update one card (keyed by id). Authoring, edits, and
        /// review-reschedules all flow through here.
        upsert_recall_card(card: recall_proto::RecallCard) -> ()
            = upsert_card(card) as "save recall card";
    }
}

/// Read one vault note's text (UTF-8, lossy) — backs the recall
/// "generate cards from note" action.
pub async fn fetch_note_text(slug: &str, path: &str) -> Result<String, String> {
    let client = crate::vox_clients::establish_for::<vault_proto::VaultSyncClient>(slug).await?;
    let bytes = client
        .get_file("default".to_owned(), path.to_string())
        .await
        .map_err(|e| format!("{slug}: read {path}: {e:?}"))?;
    Ok(String::from_utf8_lossy(&bytes.0).into_owned())
}

feeds! {
    recall_proto::RecallClient {
        /// Delete one card.
        delete_recall_card(id: &str) -> ()
            = delete_card(id.to_string()) as "delete recall card";
    }
}

// ── Contacts (vault-backed people directory) ─────────────────────────

feeds! {
    contacts_proto::ContactsClient {
        /// Every contact in the directory (archived + active). Consumers filter
        /// by group / archived / source client-side.
        fetch_contacts() -> Vec<contacts_proto::Contact>
            = list_contacts() as "list contacts";

        /// One contact by id, or `None` if the file is gone.
        get_contact(id: &str) -> Option<contacts_proto::Contact>
            = get_contact(id.to_string()) as "get contact";

        /// Create or update one contact (keyed by id). Author, edit, link, and
        /// archive all flow through here.
        upsert_contact(contact: contacts_proto::Contact) -> ()
            = upsert_contact(contact) as "save contact";

        /// Permanently remove a contact from the vault.
        delete_contact(id: &str) -> ()
            = delete_contact(id.to_string()) as "delete contact";

        /// Every configured CardDAV sync account (passwords blanked).
        fetch_carddav_accounts() -> Vec<contacts_proto::CardDavAccount>
            = list_accounts() as "list carddav accounts";

        /// Create or update one CardDAV sync account (keyed by id).
        upsert_carddav_account(account: contacts_proto::CardDavAccount) -> ()
            = upsert_account(account) as "save carddav account";

        /// Remove a CardDAV sync account (its imported contacts stay).
        delete_carddav_account(id: &str) -> ()
            = delete_account(id.to_string()) as "delete carddav account";

        /// Run a one-way pull for one account, returning its [`SyncReport`].
        sync_carddav_account(id: &str) -> contacts_proto::SyncReport
            = sync_account(id.to_string()) as "sync carddav account";
    }
}

// ── Threads (conversations on tasks/projects) ────────────────────────
//
// Single cross-target impl — the architect-generated `ThreadsServiceClient`
// is one API, established via `vox_clients::establish_for` (which already
// compiles for both wasm + native). No per-target duplication.

feeds! {
    threads::ThreadsServiceClient {
        /// Threads anchored to `(entity_type, entity_id)`, newest first.
        fetch_threads(entity_type: &str, entity_id: uuid::Uuid) -> Vec<threads::Thread>
            = list_threads(entity_type.to_string(), entity_id) as "list threads";

        /// Messages of one thread, oldest first.
        fetch_thread_messages(thread_id: uuid::Uuid) -> Vec<threads::Message>
            = list_messages(thread_id) as "list messages";

        /// Open a new thread.
        create_thread(req: threads::CreateThreadRequest) -> threads::Thread
            = create_thread(req) as "create thread";

        /// Post a message to a thread.
        post_thread_message(req: threads::PostMessageRequest) -> threads::Message
            = post_message(req) as "post message";
    }

    git_proto::connections::RepoConnectionsClient {
        /// Repos *connected* (project-bound) in this org — the `/repos`
        /// "connected" view, distinct from the raw forge catalog.
        fetch_connected_repos() -> Vec<git_proto::RepoId>
            = list_connected_repos() as "list connected repos";

        /// Repos bound to a specific project (its connected repos).
        fetch_repos_for_project(project_id: uuid::Uuid) -> Vec<git_proto::RepoId>
            = repos_for_project(project_id.to_string()) as "repos for project";
    }

    git_proto::issues::IssueTrackerClient {
        /// Comments on a forge issue — the issue's conversation, rendered under
        /// it in the `/repos` view. Works for PRs too (Gitea shares the index).
        fetch_issue_comments(repo: git_proto::RepoId, number: u64) -> Vec<git_proto::Comment>
            = list_comments(repo, git_proto::IssueId(number)) as format!("list comments #{number}");

        /// Post a comment to an issue or PR conversation (PRs share the issue
        /// index). Authored as the server's configured forge identity.
        post_issue_comment(repo: git_proto::RepoId, number: u64, body: String) -> git_proto::Comment
            = add_comment(repo, git_proto::IssueId(number), body) as format!("add comment #{number}");
    }

    project_proto::ProjectServiceClient {
        /// Update a project (write-through to its markdown). Used to change the
        /// project type from the detail page.
        create_project(project: project_proto::ProjectInfo) -> project_proto::ProjectInfo
            = create(project) as "create project";

        update_project(project: project_proto::ProjectInfo) -> project_proto::ProjectInfo
            = update(project) as "update project";
    }

    git_proto::reviews::ReviewSurfaceClient {
        /// Pull requests on a connected repo.
        fetch_pull_requests(repo: git_proto::RepoId) -> Vec<git_proto::PullRequest>
            = list_pull_requests(repo) as "list pull requests";

        /// Reviews on a PR (summary state + body per reviewer).
        fetch_pr_reviews(repo: git_proto::RepoId, pr: u64) -> Vec<git_proto::Review>
            = list_reviews(repo, git_proto::PullRequestId(pr)) as format!("list reviews #{pr}");
    }
}

/// Open or close an issue (state-only update).
pub async fn set_issue_state(
    slug: &str,
    repo: git_proto::RepoId,
    number: u64,
    state: git_proto::IssueState,
) -> Result<git_proto::Issue, String> {
    let client =
        crate::vox_clients::establish_for::<git_proto::issues::IssueTrackerClient>(slug).await?;
    let update = git_proto::IssueUpdate {
        state: Some(state),
        ..Default::default()
    };
    client
        .update_issue(repo, git_proto::IssueId(number), update)
        .await
        .map_err(|e| format!("{slug}: set state #{number}: {e:?}"))
}

feeds! {
    git_proto::reviews::ReviewSurfaceClient {
        /// Merge a pull request.
        merge_pull_request(repo: git_proto::RepoId, number: u64, method: git_proto::MergeMethod) -> Option<String>
            = merge_pull_request(repo, git_proto::PullRequestId(number), method) as format!("merge PR #{number}");
    }
}

/// Promote an inbox item into a Task — `title` is the headline, `details`
/// the markdown body. Returns the created task (its `path` is the
/// provenance back-link to store in `processed_into`).
pub async fn create_task(
    slug: &str,
    title: &str,
    details: &str,
) -> Result<task_proto::TaskInfo, String> {
    let client = crate::vox_clients::establish_for::<task_proto::TaskServiceClient>(slug).await?;
    let t = task_proto::TaskInfo {
        id: uuid::Uuid::nil(),
        path: String::new(),
        title: title.to_owned(),
        status: "open".into(),
        priority: "normal".into(),
        due: None,
        scheduled: None,
        tags: task_proto::model::StringList(vec!["task".into()]),
        contexts: task_proto::model::StringList::default(),
        projects: task_proto::model::StringList::default(),
        project_id: None,
        milestone_id: None,
        time_estimate: None,
        time_entries: task_proto::model::TimeEntries::default(),
        recurrence: None,
        recurrence_anchor: None,
        complete_instances: task_proto::model::StringList::default(),
        completed_date: None,
        agent_profile: String::new(),
        dispatched_agent_tasks: task_proto::model::StringList::default(),
        date_created: None,
        date_modified: None,
        details: details.to_owned(),
        workflow: None,
    };
    client
        .create(t)
        .await
        .map_err(|e| format!("{slug}: create task: {e:?}"))
}

feeds! {
    vault_proto::VaultSyncClient {
        /// Promote an inbox item into an atomic note: write `markdown` to
        /// `path` (vault-relative, e.g. `Wiki/Atomic/<slug>.md`) in the org's
        /// `"default"` vault. `CreateOnly` so a re-promote doesn't clobber.
        create_wiki_note(path: &str, markdown: &str) -> ()
            = put_file("default".to_owned(), path.to_owned(), markdown.as_bytes().to_vec(), vault_proto::IfMatch::CreateOnly) map |_| (), as "write note";
    }
}

// ── Locations ───────────────────────────────────────────────────────

feeds! {
    locations_proto::LocationsServiceClient {
        /// Every location in the org's vault (studios / rooms / storage /
        /// venues / homes), in the order the backend lists them.
        fetch_locations() -> Vec<locations_proto::Location>
            = list() as "list locations";

        /// Create one location from a caller-built draft (see
        /// `stores::draft_location` — the backend assigns the real `id` and
        /// vault `path`). Returns the persisted location.
        create_location(loc: locations_proto::Location) -> locations_proto::Location
            = create(loc) as "create location";
    }
}

// ── Scripture ───────────────────────────────────────────────────────

// ── Inventory ───────────────────────────────────────────────────────

feeds! {
    inventory_proto::InventoryServiceClient {
        /// Every inventory item in the org's vault (`type: item` gear /
        /// equipment pages), in the order the backend lists them.
        fetch_inventory() -> Vec<inventory_proto::Item>
            = list() as "list inventory";

        /// Create one inventory item from a caller-built draft (see
        /// `stores::draft_item` — the backend assigns the real `id` and vault
        /// `path`). Returns the persisted item.
        create_item(item: inventory_proto::Item) -> inventory_proto::Item
            = create(item) as "create item";

        /// Move an item along its lifecycle (in-use / stored / loaned /
        /// in-repair / missing / retired). Returns the updated item.
        set_item_status(id: &str, status: &str) -> inventory_proto::Item
            = set_status(id.to_owned(), status.to_owned()) as "set item status";
    }
}

// ── Milestones ──────────────────────────────────────────────────────

feeds! {
    milestone_proto::MilestoneServiceClient {
        /// Every milestone in the org's vault (project-scoped checkpoints),
        /// in the order the backend lists them. Filter client-side by
        /// `project_id` / `status` as needed.
        fetch_milestones() -> Vec<milestone_proto::Milestone>
            = list() as "list milestones";

        /// Create one milestone from a caller-built draft (see
        /// `stores::draft_milestone` — the backend derives the vault `path` and
        /// assigns the real `id`). Returns the persisted milestone.
        create_milestone(ms: milestone_proto::Milestone) -> milestone_proto::Milestone
            = create(ms) as "create milestone";
    }
}

// ── Mealplan ────────────────────────────────────────────────────────

feeds! {
    cookbook_proto::CookbookServiceClient {
        /// Every recipe in the org's cookbook (`<wiki>/Cookbook/*.cook`),
        /// in the order the backend lists them.
        fetch_recipes() -> Vec<cookbook_proto::Recipe>
            = list() as "list recipes";

        /// Create one recipe from a caller-built draft (see
        /// `stores::draft_recipe` — identity is the vault-relative `path`; the
        /// backend parses the cooklang `source`). Returns the persisted recipe.
        create_recipe(recipe: cookbook_proto::Recipe) -> cookbook_proto::Recipe
            = create(recipe) as "create recipe";

        /// Import a recipe from a web URL — the server fetches the page,
        /// extracts the recipe, and synthesizes a cooklang `.cook` draft (not
        /// yet saved). Returns the parsed draft for review.
        import_recipe(url: String) -> cookbook_proto::Recipe
            = import(url) as "import recipe";

        /// Raw bytes of one recipe image, addressed by the wiki-relative
        /// path carried on `Recipe::images`. Served over the org's RPC
        /// rather than a public HTTP route, so it inherits the same
        /// permit gate as the recipes themselves.
        fetch_recipe_image(path: String) -> Vec<u8>
            = image(path) as "recipe image";

        /// Save edits to a recipe's `.cook` source. The server writes the
        /// source verbatim then re-parses, so the returned recipe carries fresh
        /// structured steps / ingredients / timers.
        update_recipe(recipe: cookbook_proto::Recipe) -> cookbook_proto::Recipe
            = update(recipe) as "update recipe";
    }

    pantry_proto::PantryServiceClient {
        /// Every pantry item in the org's vault (food-on-hand pages), in
        /// the order the backend lists them.
        fetch_pantry() -> Vec<pantry_proto::PantryItem>
            = list() as "list pantry";

        /// Create one pantry item from a caller-built draft (see
        /// `stores::draft_pantry_item` — the backend assigns the real `id` and
        /// vault `path`). Returns the persisted item.
        create_pantry_item(item: pantry_proto::PantryItem) -> pantry_proto::PantryItem
            = create(item) as "create pantry item";
    }

    mealplan_proto::MealplanServiceClient {
        /// Cook a recipe directly: the server computes the pantry deductions
        /// for `servings` and consumes them from stock, returning a receipt of
        /// what was deducted (matched + convertible + in-stock ingredients) and
        /// what was skipped (with the reason).
        cook_recipe(recipe_path: String, servings: u32) -> mealplan_proto::CookReceipt
            = cook_recipe(recipe_path, servings) as "cook recipe";

        /// "Can I cook this right now?" — the server checks the recipe (and any
        /// nested recipes) against current pantry stock for `servings` and
        /// returns the full `Fulfillment`: whether it's cookable, the
        /// have/need partition, and the per-shortage substitution suggestions.
        /// All derivation is server-side — this is a thin client call.
        can_cook(recipe_path: String, servings: u32) -> mealplan_proto::Fulfillment
            = can_cook(recipe_path, servings) as "can cook";

        /// Every planned meal in the org's vault, in the order the
        /// backend lists them.
        fetch_meal_plans() -> Vec<mealplan_proto::Meal>
            = list() as "list meal plans";
    }

    mealplan_proto::ShoppingServiceClient {
        /// Every shopping list in the org's vault — live runs and the
        /// reusable templates alongside them (tell them apart by
        /// `is_template`).
        fetch_shopping_lists() -> Vec<mealplan_proto::ShoppingList>
            = list() as "list shopping lists";

        /// Create a list from a caller-built draft; the backend assigns
        /// the vault `path` and stamps the dates.
        create_shopping_list(list: mealplan_proto::ShoppingList) -> mealplan_proto::ShoppingList
            = create(list) as "create shopping list";

        /// Save edits (renames, hand-added rows) verbatim.
        update_shopping_list(list: mealplan_proto::ShoppingList) -> mealplan_proto::ShoppingList
            = update(list) as "update shopping list";

        /// First pass: tick a row off because it's already in the
        /// kitchen. Deliberately no pantry write — pass `have = false`
        /// to put it back on the list.
        mark_have(list_id: String, entry_id: String, have: bool) -> mealplan_proto::ShoppingList
            = mark_have(list_id, entry_id, have) as "mark have";

        /// Second pass: bought it. Restocks the pantry when the row is
        /// linked to a pantry item and carries a quantity.
        mark_purchased(list_id: String, entry_id: String) -> mealplan_proto::ShoppingList
            = mark_purchased(list_id, entry_id) as "mark purchased";

        /// Put every row back to `needed` — re-run the same list next
        /// week without retyping it. Keeps the rows (unlike `clear`).
        reset_shopping_list(id: String) -> mealplan_proto::ShoppingList
            = reset(id) as "reset shopping list";

        /// Start a fresh run from a template; the template is untouched.
        start_from_template(template_id: String, name: String) -> mealplan_proto::ShoppingList
            = start_from_template(template_id, name) as "start from template";

        /// Keep this list's rows as a reusable template.
        save_as_template(list_id: String, name: String) -> mealplan_proto::ShoppingList
            = save_as_template(list_id, name) as "save as template";

        /// Add everything a recipe needs that the pantry can't cover at
        /// `servings` — the "what do I need to buy for this meal" button.
        add_missing_for_recipe(list_id: String, recipe_path: String, servings: u32)
            -> mealplan_proto::ShoppingList
            = add_missing_for_recipe(list_id, recipe_path, servings) as "add missing for recipe";

        /// Add everything a recipe calls for at `servings`, whatever the
        /// pantry says — the gather checklist, where the kitchen pass is
        /// an actual look at an actual shelf rather than a stock guess.
        add_recipe_ingredients(list_id: String, recipe_path: String, servings: u32)
            -> mealplan_proto::ShoppingList
            = add_recipe_ingredients(list_id, recipe_path, servings) as "add recipe ingredients";

        /// Add every pantry item at or below its reorder minimum.
        add_low_stock(list_id: String) -> mealplan_proto::ShoppingList
            = add_low_stock(list_id) as "add low stock";
    }
}

// ── Timer ─────────────────────────────────────────────────────────

feeds! {
    timer_proto::TimerServiceClient {
        /// The currently-running session for `user_id` in this org, if any.
        fetch_active_timer(user_id: uuid::Uuid) -> Option<timer_proto::WorkSession>
            = active_timer(user_id) as "active timer";
    }
}

/// Recent sessions for `user_id`, newest first.
pub async fn fetch_recent_sessions(
    slug: &str,
    user_id: uuid::Uuid,
) -> Result<Vec<timer_proto::WorkSession>, String> {
    let client = crate::vox_clients::establish_for::<timer_proto::TimerServiceClient>(slug).await?;
    let filter = timer_proto::WorkSessionFilter {
        user_id: Some(user_id),
        ..Default::default()
    };
    let mut sessions = client
        .list_sessions(filter)
        .await
        .map_err(|e| format!("{slug}: list sessions: {e:?}"))?;
    sessions.sort_by(|a, b| b.start_time.cmp(&a.start_time));
    Ok(sessions)
}

/// Sessions across several orgs, each tagged with its org slug, newest
/// first. Powers the multi-org timer / finances / invoices views.
pub async fn fetch_sessions_multi(slugs: &[String]) -> Vec<(String, timer_proto::WorkSession)> {
    let mut out = Vec::new();
    for slug in slugs {
        if let Ok(sessions) = fetch_org_sessions(slug).await {
            out.extend(sessions.into_iter().map(|s| (slug.clone(), s)));
        }
    }
    out.sort_by(|a, b| b.1.start_time.cmp(&a.1.start_time));
    out
}

/// Every session in the org (all members), newest first — the time-log
/// view. Lets the operator see contractors' logged time too, not just
/// their own.
pub async fn fetch_org_sessions(slug: &str) -> Result<Vec<timer_proto::WorkSession>, String> {
    let client = crate::vox_clients::establish_for::<timer_proto::TimerServiceClient>(slug).await?;
    let mut sessions = client
        .list_sessions(timer_proto::WorkSessionFilter::default())
        .await
        .map_err(|e| format!("{slug}: list sessions: {e:?}"))?;
    sessions.sort_by(|a, b| b.start_time.cmp(&a.start_time));
    Ok(sessions)
}

/// Sessions logged against one project (all members), newest first.
/// `open_only = true` narrows to running timers — the project
/// overview's "active now" view; `false` returns the full history for
/// budget aggregation.
pub async fn fetch_project_sessions(
    slug: &str,
    project_id: uuid::Uuid,
    open_only: bool,
) -> Result<Vec<timer_proto::WorkSession>, String> {
    let client = crate::vox_clients::establish_for::<timer_proto::TimerServiceClient>(slug).await?;
    let filter = timer_proto::WorkSessionFilter {
        project_id: Some(project_id),
        open: open_only.then_some(true),
        ..Default::default()
    };
    let mut sessions = client
        .list_sessions(filter)
        .await
        .map_err(|e| format!("{slug}: list sessions: {e:?}"))?;
    sessions.sort_by(|a, b| b.start_time.cmp(&a.start_time));
    Ok(sessions)
}

feeds! {
    timer_proto::TimerServiceClient {
        /// Start a timer; returns the new open session.
        start_timer(req: timer_proto::StartTimerRequest) -> timer_proto::WorkSession
            = start_timer(req) as "start timer";

        /// Stop `user_id`'s running timer; returns the closed session.
        stop_timer(user_id: uuid::Uuid) -> timer_proto::WorkSession
            = stop_timer(user_id) as "stop timer";

        /// Atomically stop the caller's running timer (if any) and start a new
        /// one — "switch the timer to a different task" in one transaction, so
        /// the UI never briefly shows two open sessions (or none). Returns the
        /// newly-started session; the closed one settles on the next refetch.
        switch_timer(req: timer_proto::StartTimerRequest) -> timer_proto::WorkSession
            = switch_timer(req) map |(_closed, started)| started, as "switch timer";

        /// Retro-log a completed session (start + end in the past) — the "I
        /// forgot to start the timer" / manual-entry path. Skips the
        /// active-timer invariant, so it never disturbs a running timer.
        log_session(req: timer_proto::LogSessionRequest) -> timer_proto::WorkSession
            = log_session(req) as "log session";

        /// Edit an existing session — only the `Some(_)` fields change. The
        /// backend re-snapshots the rate afterward.
        update_session(req: timer_proto::service::UpdateSessionRequest) -> timer_proto::WorkSession
            = update_session(req) as "update session";

        /// Permanently delete a session.
        delete_session(id: uuid::Uuid) -> ()
            = delete_session(id) as "delete session";
    }
}

// ── member rates ────────────────────────────────────────────────────

feeds! {
    timer_proto::TimerServiceClient {
        /// Upsert an org-level member rate; returns the stored row.
        set_org_member_rate(org_id: uuid::Uuid, user_id: uuid::Uuid, hourly_cents: i64, currency: String) -> timer_proto::OrgMemberRate
            = set_org_member_rate(org_id, user_id, hourly_cents, currency) as "set org rate";

        /// Upsert a per-project member rate (lets members bill one project at
        /// different rates); returns the stored row.
        set_project_member_rate(project_id: uuid::Uuid, user_id: uuid::Uuid, hourly_cents: i64) -> timer_proto::ProjectMemberRate
            = set_project_member_rate(project_id, user_id, hourly_cents) as "set project rate";

        /// Every org-level member rate configured for `org_id`.
        fetch_org_member_rates(org_id: uuid::Uuid) -> Vec<timer_proto::OrgMemberRate>
            = list_org_member_rates(org_id) as "list org rates";

        /// Every per-member rate set on `project_id`.
        fetch_project_member_rates(project_id: uuid::Uuid) -> Vec<timer_proto::ProjectMemberRate>
            = list_project_member_rates(project_id) as "list project rates";
    }

    auth_proto::AuthServiceClient {
        /// The org's members (name / email / role), for the current session's
        /// org. Requires a valid session `token` — the org is derived from it.
        fetch_org_members(token: String) -> Vec<auth_proto::OrgMember>
            = list_org_members(token) as "list members";
    }
}

// ── finance / invoicing ─────────────────────────────────────────────

async fn invoicing(slug: &str) -> Result<finance_proto::InvoicingClient, String> {
    crate::vox_clients::establish_for::<finance_proto::InvoicingClient>(slug).await
}

/// All invoices in an org, newest first.
pub async fn fetch_invoices(slug: &str) -> Result<Vec<finance_proto::Invoice>, String> {
    invoicing(slug)
        .await?
        .list_invoices()
        .await
        .map_err(|e| format!("{slug}: list invoices: {e:?}"))
}

/// Per-project billable time not yet invoiced, in an org.
pub async fn fetch_uninvoiced(slug: &str) -> Result<Vec<finance_proto::UninvoicedGroup>, String> {
    invoicing(slug)
        .await?
        .uninvoiced()
        .await
        .map_err(|e| format!("{slug}: uninvoiced: {e:?}"))
}

/// Generate + persist a draft invoice from a project's billable time.
pub async fn generate_invoice(
    slug: &str,
    req: finance_proto::GenerateInvoice,
) -> Result<finance_proto::Invoice, String> {
    invoicing(slug)
        .await?
        .generate_invoice(req)
        .await
        .map_err(|e| format!("{slug}: generate invoice: {e:?}"))
}

/// Issue an invoice (assign number, lock).
pub async fn invoice_mark_sent(
    slug: &str,
    id: uuid::Uuid,
) -> Result<finance_proto::Invoice, String> {
    invoicing(slug)
        .await?
        .mark_sent(id)
        .await
        .map_err(|e| format!("{slug}: mark sent: {e:?}"))
}

/// Record a payment against an invoice.
pub async fn invoice_record_payment(
    slug: &str,
    id: uuid::Uuid,
    amount_minor: i64,
    date: String,
) -> Result<finance_proto::Invoice, String> {
    invoicing(slug)
        .await?
        .record_invoice_payment(id, amount_minor, date)
        .await
        .map_err(|e| format!("{slug}: record payment: {e:?}"))
}

/// Delete a draft invoice (un-bills its sessions).
pub async fn invoice_delete(slug: &str, id: uuid::Uuid) -> Result<(), String> {
    invoicing(slug)
        .await?
        .delete_invoice(id)
        .await
        .map_err(|e| format!("{slug}: delete invoice: {e:?}"))
}

// ── finance / ledger ────────────────────────────────────────────────

async fn ledger(slug: &str) -> Result<finance_proto::LedgerClient, String> {
    crate::vox_clients::establish_for::<finance_proto::LedgerClient>(slug).await
}

/// Resolve the org's (single) finance book id, if one exists yet.
async fn ledger_book_id(
    client: &finance_proto::LedgerClient,
    slug: &str,
) -> Result<Option<uuid::Uuid>, String> {
    let books = client
        .books()
        .await
        .map_err(|e| format!("{slug}: books: {e:?}"))?;
    Ok(books.first().map(|b| b.id))
}

/// Every account in an org's (single) book, paired with its current
/// balance. Returns `(account, balance)` rows. Empty when the org has
/// no book / accounts yet.
pub async fn fetch_ledger_accounts(
    slug: &str,
) -> Result<Vec<(finance_proto::Account, finance_proto::AccountBalance)>, String> {
    let client = ledger(slug).await?;
    let Some(book_id) = ledger_book_id(&client, slug).await? else {
        return Ok(Vec::new());
    };
    let accounts = client
        .accounts(book_id)
        .await
        .map_err(|e| format!("{slug}: accounts: {e:?}"))?;
    let balances = client
        .balances(book_id, None)
        .await
        .map_err(|e| format!("{slug}: balances: {e:?}"))?;
    let out = accounts
        .into_iter()
        .map(|a| {
            let bal = balances
                .iter()
                .find(|b| b.account_id == a.id)
                .cloned()
                .unwrap_or_else(|| finance_proto::AccountBalance {
                    account_id: a.id,
                    balance_minor: a.opening_balance_minor,
                    currency: a.currency.clone(),
                });
            (a, bal)
        })
        .collect();
    Ok(out)
}

/// Recent ledger transactions across every account in an org's book,
/// newest first. Pulls each account's history and de-dupes by
/// transaction id (a double-entry txn touches ≥2 accounts).
pub async fn fetch_ledger_transactions(
    slug: &str,
) -> Result<Vec<finance_proto::Transaction>, String> {
    let client = ledger(slug).await?;
    let Some(book_id) = ledger_book_id(&client, slug).await? else {
        return Ok(Vec::new());
    };
    let accounts = client
        .accounts(book_id)
        .await
        .map_err(|e| format!("{slug}: accounts: {e:?}"))?;
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<finance_proto::Transaction> = Vec::new();
    for a in accounts {
        let txns = client
            .account_transactions(a.id, None, None, 100)
            .await
            .map_err(|e| format!("{slug}: account transactions: {e:?}"))?;
        for t in txns {
            if seen.insert(t.id) {
                out.push(t);
            }
        }
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(out)
}

/// Invoices across several orgs, slug-tagged, newest first.
pub async fn fetch_invoices_multi(slugs: &[String]) -> Vec<(String, finance_proto::Invoice)> {
    let mut out = Vec::new();
    for slug in slugs {
        if let Ok(rows) = fetch_invoices(slug).await {
            out.extend(rows.into_iter().map(|i| (slug.clone(), i)));
        }
    }
    out.sort_by(|a, b| b.1.created_at.cmp(&a.1.created_at));
    out
}

/// Uninvoiced groups across several orgs, slug-tagged.
pub async fn fetch_uninvoiced_multi(
    slugs: &[String],
) -> Vec<(String, finance_proto::UninvoicedGroup)> {
    let mut out = Vec::new();
    for slug in slugs {
        if let Ok(rows) = fetch_uninvoiced(slug).await {
            out.extend(rows.into_iter().map(|g| (slug.clone(), g)));
        }
    }
    out
}

/// Fetch one org's vault markdown as `WikiFile`s for the knowledge
/// graph: pull the manifest, then read every `.md` file concurrently
/// over the one socket. Pure graph-building happens caller-side.
/// Every `.base` file in an org's vault (vault-relative paths), sorted.
pub async fn fetch_bases(slug: &str) -> Result<Vec<String>, String> {
    let client = crate::vox_clients::establish_for::<vault_proto::VaultSyncClient>(slug).await?;
    let manifest = client
        .manifest("default".to_owned())
        .await
        .map_err(|e| format!("manifest: {e:?}"))?;
    let mut bases: Vec<String> = manifest
        .files
        .into_iter()
        .map(|f| f.path)
        .filter(|p| {
            std::path::Path::new(p)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("base"))
        })
        .collect();
    bases.sort();
    Ok(bases)
}

/// The executed views of one `.base` file — server-rendered tables.
pub async fn fetch_base_views(
    slug: &str,
    base_path: &str,
) -> Result<Vec<vault_proto::BaseView>, String> {
    let client = crate::vox_clients::establish_for::<vault_proto::VaultSyncClient>(slug).await?;
    client
        .base_views("default".to_owned(), base_path.to_owned())
        .await
        .map_err(|e| format!("{base_path}: {e:?}"))
}

/// Every typed link touching `node_token` (a `kind:id` NodeRef token,
/// e.g. `sermon:god-restores-broken-people`) — the timestamped notes for
/// the watch view.
pub async fn fetch_links_for(
    slug: &str,
    node_token: &str,
) -> Result<Vec<links_proto::TypedLink>, String> {
    let node = links_proto::NodeRef::parse(node_token)
        .ok_or_else(|| format!("bad node token: {node_token}"))?;
    let client = crate::vox_clients::establish_for::<links_proto::LinksServiceClient>(slug).await?;
    client
        .links_for(node)
        .await
        .map_err(|e| format!("{slug}: links_for: {e:?}"))
}

feeds! {
    scripture_proto::ScriptureServiceClient {
        /// One verse's text (`translation` defaults handled by the caller).
        /// `reference` is a human/OSIS-ish ref like `John 3:16` or `1John 3:16`.
        fetch_verse_text(translation: &str, reference: &str) -> String
            = verse(translation.to_owned(), reference.to_owned()) map |v| v.text, as format!("verse {reference}");
    }
}

/// A resource's transcript cues (`resources/<rel_path>`), for the watch
/// view's synced transcript. Empty vec on a missing sidecar.
pub async fn fetch_transcript(
    slug: &str,
    rel_path: &str,
) -> Result<Vec<resources_proto::TranscriptSegment>, String> {
    let client =
        crate::vox_clients::establish_for::<resources_proto::ResourcesServiceClient>(slug).await?;
    // A missing / unreadable transcript just means no cues — never fatal.
    Ok(client
        .transcript(rel_path.to_owned())
        .await
        .map(|doc| doc.segments)
        .unwrap_or_default())
}

/// Save a watched video to the library as a `type: video` vault note
/// (`Videos/<id>.md`) so `[[id]]` resolves and it shows in a Videos base.
/// `CreateOnly`, so re-saving an existing video is a no-op (the title
/// stays whatever you renamed it to).
pub async fn save_video_note(
    slug: &str,
    video_id: &str,
    url: &str,
    title: &str,
) -> Result<(), String> {
    let title = if title.trim().is_empty() {
        video_id
    } else {
        title
    };
    let md = format!(
        "---\ntitle: {title}\ntype: video\nkind: video\nvideo_id: {video_id}\nurl: {url}\ntags: [video]\n---\n\n# {title}\n\nTimestamped notes are typed links on `video:{video_id}`. Watch + annotate at `/watch?v={video_id}&node=video:{video_id}`.\n"
    );
    let client = crate::vox_clients::establish_for::<vault_proto::VaultSyncClient>(slug).await?;
    client
        .put_file(
            "default".to_owned(),
            format!("Videos/{video_id}.md"),
            md.into_bytes(),
            vault_proto::IfMatch::CreateOnly,
        )
        .await
        .map(|_| ())
        .map_err(|e| format!("{slug}: save video: {e:?}"))
}

feeds! {
    links_proto::LinksServiceClient {
        /// Persist one typed link (the watch view's "add note at current time").
        create_link(link: links_proto::TypedLink) -> links_proto::TypedLink
            = create(link) as "create link";

    }
}

pub async fn fetch_wiki_files(slug: &str) -> Result<Vec<view_knowledge_graph::WikiFile>, String> {
    use view_knowledge_graph::WikiFile;

    let client = crate::vox_clients::establish_for::<vault_proto::VaultSyncClient>(slug).await?;
    let manifest = client
        .manifest("default".to_owned())
        .await
        .map_err(|e| format!("manifest: {e:?}"))?;
    let md_paths: Vec<String> = manifest
        .files
        .into_iter()
        .map(|f| f.path)
        .filter(|p| {
            std::path::Path::new(p)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        })
        .collect();

    let futs = md_paths.into_iter().map(|path| {
        let c = client.clone();
        async move {
            let bytes = c.get_file("default".to_owned(), path.clone()).await.ok()?;
            let name = std::path::Path::new(&path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&path)
                .to_string();
            Some(WikiFile {
                name,
                path,
                content: String::from_utf8_lossy(&bytes.0).into_owned(),
            })
        }
    });
    Ok(futures_util::future::join_all(futs)
        .await
        .into_iter()
        .flatten()
        .collect())
}

feeds! {
    wiki_proto::service::pages::PagesClient {
        /// Catalog of the org's curated wiki pages (`<org>/wiki/Knowledge/`)
        /// via the `wiki_proto` Pages service — drives the explorer's Wiki
        /// tree. Path-sorted; carries the `ai_generated` provenance flag.
        fetch_wiki_pages() -> Vec<wiki_proto::pages::PageInfo>
            = list_pages("default".to_owned()) as "wiki pages";
    }
}

/// Fetch the **curated wiki** graph for one org — the server-built
/// 4-signal relevance graph over `<org>/wiki/Knowledge/` (the
/// `wiki_proto` Graph service), adapted to the renderer's
/// [`view_knowledge_graph::WikiGraph`] model. Louvain clusters ride
/// along as the legend's community summaries.
pub async fn fetch_wiki_service_graph(
    slug: &str,
) -> Result<view_knowledge_graph::WikiGraph, String> {
    use std::collections::HashMap;

    use view_knowledge_graph as vkg;

    let client =
        crate::vox_clients::establish_for::<wiki_proto::service::graph::GraphClient>(slug).await?;
    let opts = wiki_proto::graph::GraphOpts {
        query: String::new(),
        node_type: String::new(),
        limit: 0,
        weights: None,
    };
    let graph = client
        .build_graph("default".to_owned(), opts)
        .await
        .map_err(|e| format!("build_graph: {e:?}"))?;
    // Clusters are advisory (legend colors + summaries) — an error
    // here degrades to an unclustered graph, not a failed page.
    let clusters = client
        .clusters("default".to_owned())
        .await
        .unwrap_or_default();

    // Cluster membership → dense u32 community ids (0 = unclustered).
    let mut community_of: HashMap<&str, u32> = HashMap::new();
    for (i, c) in clusters.iter().enumerate() {
        for m in &c.members {
            community_of.insert(m.as_str(), (i + 1) as u32);
        }
    }

    let nodes: Vec<vkg::GraphNode> = graph
        .nodes
        .iter()
        .map(|n| vkg::GraphNode {
            id: n.id.clone(),
            label: n.label.clone(),
            kind: {
                let k = n.node_type.trim().to_ascii_lowercase();
                if k.is_empty() { "other".to_owned() } else { k }
            },
            path: n.id.clone(),
            link_count: n.link_count,
            community: community_of.get(n.id.as_str()).copied().unwrap_or(0),
        })
        .collect();
    let degree_of: HashMap<&str, u32> = graph
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.link_count))
        .collect();
    let label_of: HashMap<&str, &str> = graph
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.label.as_str()))
        .collect();
    let edges: Vec<vkg::GraphEdge> = graph
        .edges
        .iter()
        .map(|e| vkg::GraphEdge::wikilink(e.source.clone(), e.target.clone(), e.weight))
        .collect();
    let communities: Vec<vkg::CommunityInfo> = clusters
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut members: Vec<&str> = c.members.iter().map(String::as_str).collect();
            members.sort_by_key(|m| std::cmp::Reverse(degree_of.get(m).copied().unwrap_or(0)));
            vkg::CommunityInfo {
                id: (i + 1) as u32,
                node_count: c.members.len(),
                cohesion: c.cohesion,
                top_nodes: members
                    .into_iter()
                    .take(5)
                    .map(|m| label_of.get(m).copied().unwrap_or(m).to_owned())
                    .collect(),
            }
        })
        .collect();

    Ok(vkg::WikiGraph {
        nodes,
        edges,
        communities,
    })
}

/// Locate a single project by id across the selected orgs, returning it
/// together with the slug of the org that owns it. Used by the project
/// detail page so it works regardless of which org is in view.
pub async fn find_project(id: &str, slugs: &[String]) -> Result<(ProjectInfo, String), String> {
    let uuid = uuid::Uuid::parse_str(id).map_err(|_| "invalid project id".to_owned())?;
    let mut last_err = None;
    for slug in slugs {
        match crate::vox_clients::establish_for::<project_proto::ProjectServiceClient>(slug).await {
            Ok(client) => match client.get(uuid).await {
                Ok(p) => return Ok((p, slug.clone())),
                Err(e) => last_err = Some(format!("{slug}: {e:?}")),
            },
            Err(e) => last_err = Some(format!("{slug}: {e}")),
        }
    }
    Err(last_err.unwrap_or_else(|| "project not found in any hosted org".to_owned()))
}

// ── Fitness (native stubs) ──────────────────────────────────────────

// ── Mealplan (native stubs) ─────────────────────────────────────────

// ── Agents ────────────────────────────────────────────────────────

/// Agent sessions across the selected orgs (concurrent fan-out).
///
/// Each session carries its owning org slug so the `/agents` page can
/// show provenance in multi-org "All" mode. Archived sessions are
/// included so the listing is a faithful mirror of the backend.
pub async fn fetch_agent_sessions(
    slugs: &[String],
) -> Result<Vec<(String, agent_proto::session::Session)>, String> {
    let futs = slugs.iter().map(|slug| async move {
        let client = crate::vox_clients::establish_for::<
            agent_proto::service::sessions::SessionsClient,
        >(slug)
        .await?;
        let filter = agent_proto::service::sessions::SessionFilter {
            project_id: String::new(),
            backend_id: String::new(),
            profile_id: String::new(),
            include_archived: true,
            only_pinned: false,
            limit: 0,
            cursor: String::new(),
        };
        let page = client
            .list_sessions(filter)
            .await
            .map_err(|e| format!("{slug}: list agent sessions: {e:?}"))?;
        Ok::<_, String>(
            page.sessions
                .into_iter()
                .map(|s| (slug.clone(), s))
                .collect::<Vec<_>>(),
        )
    });
    collect(futures_util::future::join_all(futs).await)
}

/// Agent sessions attached to one project, filtered server-side
/// (`SessionFilter.project_id` is an exact match in the backend).
/// Archived sessions are excluded — this powers "active now" style
/// views, and an archived session can't be active.
pub async fn fetch_project_agent_sessions(
    slug: &str,
    project_id: &str,
) -> Result<Vec<agent_proto::session::Session>, String> {
    let client =
        crate::vox_clients::establish_for::<agent_proto::service::sessions::SessionsClient>(slug)
            .await?;
    let filter = agent_proto::service::sessions::SessionFilter {
        project_id: project_id.to_owned(),
        backend_id: String::new(),
        profile_id: String::new(),
        include_archived: false,
        only_pinned: false,
        limit: 0,
        cursor: String::new(),
    };
    let page = client
        .list_sessions(filter)
        .await
        .map_err(|e| format!("{slug}: list agent sessions: {e:?}"))?;
    Ok(page.sessions)
}

feeds! {
    agent_proto::service::sessions::SessionsClient {
        /// Create a new agent chat session. `backend_id` picks the backend
        /// (`"hermes"`, `"codex"`); empty = the server default (Hermes when
        /// a gateway is configured).
        create_agent_session(backend_id: &str, title: &str) -> agent_proto::session::Session
            = create_session(agent_proto::service::sessions::CreateSession { project_id: String::new(), profile_id: String::new(), backend_id: backend_id.to_owned(), title: title.to_owned(), workspace_path: String::new(), subagent_nickname: String::new(), }) as "create agent session";

        /// Session mutations for the rail/inspector: rename, pin, archive,
        /// delete. Each returns the updated session (delete returns unit).
        rename_agent_session(session_id: &str, title: &str) -> agent_proto::session::Session
            = rename_session(session_id.to_owned(), title.to_owned()) as "rename session";

        pin_agent_session(session_id: &str, pinned: bool) -> agent_proto::session::Session
            = pin_session(session_id.to_owned(), pinned) as "pin session";

        archive_agent_session(session_id: &str, archived: bool) -> agent_proto::session::Session
            = archive_session(session_id.to_owned(), archived) as "archive session";

        delete_agent_session(session_id: &str) -> ()
            = delete_session(session_id.to_owned()) as "delete session";
    }

    agent_proto::service::turn_dispatch::TurnDispatchClient {
        /// Kick off one turn — the user message goes to the session's
        /// backend; the reply arrives on the `Subscriptions` events
        /// stream the chat view holds open.
        dispatch_agent_turn(session_id: &str, text: &str, model_override: &str) -> agent_proto::service::turn_dispatch::DispatchAck
            = dispatch_turn(agent_proto::service::turn_dispatch::DispatchTurn { session_id: session_id.to_owned(), text: text.to_owned(), attachments: Vec::new(), profile_override_id: String::new(), personality_override_id: String::new(), model_override: model_override.to_owned(), }) as "dispatch turn";

        /// Cancel the in-flight turn on a session.
        cancel_agent_turn(session_id: &str) -> ()
            = cancel_turn(session_id.to_owned()) as "cancel turn";
    }

    agent_proto::service::threads::ThreadsClient {
        /// Full transcript for a session (backend returns newest-first;
        /// callers reverse for display).
        fetch_agent_messages(session_id: &str) -> Vec<agent_proto::message::Message>
            = list_messages(session_id.to_owned(), 0, String::new()) as "list messages";
    }

    agent_proto::service::discovery::DiscoveryClient {
        /// Live model list across the org's agent backends (Hermes gateway
        /// models + Codex's static set) — feeds the composer's model chip.
        fetch_agent_models() -> Vec<agent_proto::service::discovery::ModelInfo>
            = list_models(String::new()) as "agent models";

        /// Agent skills (Hermes's self-improving skill library).
        fetch_agent_skills() -> Vec<agent_proto::service::discovery::SkillInfo>
            = list_skills(String::new()) as "agent skills";

        /// Backend capability flags, for the inspector panel.
        fetch_agent_capabilities() -> Vec<agent_proto::service::discovery::CapabilityFlag>
            = list_capabilities(String::new()) as "agent capabilities";

        /// Live per-backend health — gateway state, connected platforms,
        /// in-flight agents, probe latency. Polled by the chat header's
        /// status chip so an unreachable gateway says so instead of
        /// silently swallowing turns.
        fetch_agent_health() -> Vec<agent_proto::backend::BackendHealth>
            = backend_health(String::new()) as "agent health";
    }

    agent_proto::service::routines::RoutinesClient {
        /// Scheduled agent routines (the Hermes gateway's cron jobs).
        /// Includes paused ones — the panel shows them greyed rather than
        /// hiding them, so a paused routine isn't mistaken for a deleted one.
        fetch_agent_routines() -> Vec<agent_proto::service::routines::Routine>
            = list_routines(String::new(), true) as "agent routines";

        create_agent_routine(routine: agent_proto::service::routines::NewRoutine) -> agent_proto::service::routines::Routine
            = create_routine(routine) as "create routine";

        set_agent_routine_paused(id: &str, paused: bool) -> agent_proto::service::routines::Routine
            = set_routine_paused(String::new(), id.to_owned(), paused) as "pause routine";

        run_agent_routine(id: &str) -> agent_proto::service::routines::Routine
            = run_routine(String::new(), id.to_owned()) as "run routine";

        delete_agent_routine(id: &str) -> ()
            = delete_routine(String::new(), id.to_owned()) as "delete routine";
    }
}

// ── Email ───────────────────────────────────────────────────────────

// ── Git / forge ─────────────────────────────────────────────────────

feeds! {
    git_proto::repo::RepoCatalogClient {
        /// Every repo the org's forge backend can address, in the order the
        /// catalog lists them. Backed by `RepoCatalog::list_repos`; when the
        /// forge is unconfigured (no token) the backend returns an
        /// auth/forge error the caller renders as an empty list.
        fetch_repos() -> Vec<git_proto::Repo>
            = list_repos() as "list repos";
    }

    git_proto::issues::IssueTrackerClient {
        /// Issues for one repo (all states), via `IssueTracker::list_issues`
        /// with a default (unfiltered) filter. The `/repos` page calls this
        /// per repo to show each repo's open work inline.
        fetch_issues(repo: git_proto::RepoId) -> Vec<git_proto::issues::Issue>
            = list_issues(repo, git_proto::issues::IssueFilter::default()) as "list issues";
    }
}

/// Everything blocking a human in the agent lane, for one project or
/// the whole fleet.
///
/// One fetch rather than three so the panels always describe the same
/// instant — a surface where "running" and "awaiting review" disagree
/// about a ticket is worse than one that is slightly stale.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentSurface {
    /// Unresolved questions, paired with the ticket each blocks.
    pub questions: Vec<(
        agent_proto::question::QuestionRequest,
        Option<task_proto::TaskInfo>,
    )>,
    /// Runs executing right now, paired with their ticket.
    pub running: Vec<(agent_proto::run::Run, Option<task_proto::TaskInfo>)>,
    /// Tickets whose branch is green and waiting.
    pub review: Vec<task_proto::TaskInfo>,
}

/// Fetch the agent surface. `project` scopes it; `None` is the fleet.
///
/// # Errors
///
/// The first transport failure. The surface is all-or-nothing on
/// purpose: a panel that renders empty because its call failed reads
/// as "nothing is blocking you", which is the opposite of the truth.
pub async fn fetch_agent_surface(
    slug: &str,
    project: Option<uuid::Uuid>,
) -> Result<AgentSurface, String> {
    let tasks = crate::vox_clients::establish_for::<task_proto::TaskServiceClient>(slug).await?;
    let all = tasks.list().await.map_err(|e| format!("{e:?}"))?;
    let in_scope = |t: &task_proto::TaskInfo| project.is_none_or(|p| t.project_id == Some(p));
    let find = |id: uuid::Uuid| all.iter().find(|t| t.id == id).cloned();

    let questions_client =
        crate::vox_clients::establish_for::<agent_proto::service::questions::QuestionsClient>(slug)
            .await?;
    let mut questions = Vec::new();
    for req in questions_client
        .unresolved_questions()
        .await
        .map_err(|e| format!("{e:?}"))?
    {
        let ticket = questions_client
            .question_ticket(req.id.clone())
            .await
            .ok()
            .flatten()
            .and_then(find);
        // A question whose ticket is out of scope belongs to another
        // project's surface. One with no ticket at all is still shown:
        // it is blocked on a human either way, and hiding it loses it.
        if ticket.as_ref().is_none_or(in_scope) {
            questions.push((req, ticket));
        }
    }

    let runs_client =
        crate::vox_clients::establish_for::<agent_proto::service::runs::RunsClient>(slug).await?;
    let running: Vec<_> = runs_client
        .list_runs(agent_proto::run::RunFilter {
            status: Some(agent_proto::run::RunStatus::InProgress),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("{e:?}"))?
        .into_iter()
        .map(|r| {
            let t = find(r.ticket);
            (r, t)
        })
        .filter(|(_, t)| t.as_ref().is_none_or(in_scope))
        .collect();

    let review: Vec<task_proto::TaskInfo> = all
        .iter()
        .filter(|t| task_proto::has_triage_label(t, task_proto::TriageLabel::NeedsReview))
        .filter(|t| in_scope(t))
        .cloned()
        .collect();

    Ok(AgentSurface {
        questions,
        running,
        review,
    })
}
