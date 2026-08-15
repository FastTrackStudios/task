# Vox Phase 2 — wire DB-only services

Phase 1 (committed) wired `TaskService` + `InboxService` through `apps/server/src/lib.rs::vox_ws_handler` against a `sea_orm::DatabaseConnection` exposed on `AppState::vox`. This plan finishes every service whose `*ServiceDeps` is **just** that same `DatabaseConnection` — no extra repos, no optional providers.

## Services to wire

All take only `pub db: DatabaseConnection` in their `*Deps` (verified by reading `crates/task-core/src/service_impl/*.rs`):

- `PropertyService` (`service_impl/property.rs`)
- `TemplateService` (`service_impl/template.rs`)
- `ProjectTypeService` (`service_impl/project_type.rs`)
- `AudioProductionService` (`service_impl/audio_production.rs`)
- `CookingService` (`service_impl/cooking.rs`)
- `FitnessService` (`service_impl/fitness.rs`)
- `GlossaryService` (`service_impl/glossary.rs`)

Three more take `db` plus an **optional** `OpenFoodFactsClient`. Treat the same way (pass `None` until a provider plan lands):

- `FoodService` (`service_impl/food.rs`)
- `PantryService` (`service_impl/pantry.rs`)
- `NutritionService` (`service_impl/nutrition.rs`)

## Implementation sketch

1. In `apps/server/src/lib.rs::VoxState`, add a field per service:
   ```rust
   pub property_service_impl: PropertyServiceImpl,
   pub template_service_impl: TemplateServiceImpl,
   // … etc.
   ```
2. Construct each inside `VoxState::new(db)` with `XServiceImpl::new(XServiceDeps { db: db.clone(), .. })`. For the OpenFoodFacts trio pass `openfoodfacts: None`.
3. Add the match arms inside `vox_ws_handler`'s `acceptor_fn`:
   ```rust
   "PropertyService" => { connection.handle_with(PropertyServiceDispatcher::new(vox.property_service_impl.clone())); Ok(()) }
   // …
   ```
4. CLI consumers in `crates/task-cli/src/shared.rs` (`property()`, `audio()`, `cooking()`, `fitness()`, `food()`, `glossary()`, `pantry()`, `nutrition()`) start succeeding end-to-end.

## Verification

- `cargo check -p task-server` + `cargo test -p task-server`.
- Boot the server and call one method per service from the CLI (e.g. `task property list`).
- `/vox` route logs should show no "dispatcher not yet wired" lines for any of the listed services.

## Out of scope (→ Phase 3)

Anything requiring extra repos beyond the workspace `task_repo`, or any service whose `*Deps` carries a real (non-`Option`) provider.
