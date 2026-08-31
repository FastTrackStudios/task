//! Top-level pages mounted by [`crate::routes::Route`].
//!
//! Each page is responsible for its own data wiring. The shell
//! (sidebar + headers + bottom bar) is provided by the route
//! layout; pages only render the content area.

pub mod agent_surface;
pub mod agents;
pub mod bases;
pub mod bookings;
pub mod contacts;
pub mod cook_mode;
pub mod files;
pub mod finances;
pub mod gantt;
pub mod home;
pub mod inbox;
pub mod invoices;
pub mod ledger;
pub mod mealplan;
pub mod mealplan_week;
pub mod members;
pub mod milestones;
pub mod missing;
pub mod note_header;
pub mod note_properties;
pub mod note_view;
pub mod project_detail;
pub mod projects;
pub mod recall;
pub mod recipe_edit;
pub mod recipe_read;
pub mod repos;
pub mod schedule;
pub mod settings;
pub mod share_panel;
pub mod shopping;
pub mod sync;
pub mod task_detail;
pub mod tasks;
pub mod timer;
pub mod vault;
pub mod watch;
pub mod wiki;
pub mod wiki_page;
pub mod wiki_source;
