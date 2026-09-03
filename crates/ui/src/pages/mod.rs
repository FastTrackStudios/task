//! Top-level pages mounted by [`crate::routes::Route`].
//!
//! Each page is responsible for its own data wiring. The shell
//! (sidebar + headers + bottom bar) is provided by the route
//! layout; pages only render the content area.

pub mod auth_callback;
pub mod bases;
pub mod contacts;
pub mod files;
pub mod gantt;
pub mod home;
pub mod inbox;
pub mod members;
pub mod milestones;
pub mod missing;
pub mod note_header;
pub mod note_inspector;
pub mod note_properties;
pub mod note_view;
pub mod project_detail;
pub mod projects;
pub mod schedule;
pub mod settings;
pub mod share_panel;
pub mod sync;
pub mod task_detail;
pub mod tasks;
pub mod timer;
pub mod vault;
pub mod wiki;
pub mod wiki_home;
pub mod wiki_index;
pub mod wiki_page;
pub mod wiki_source;
pub mod wiki_subscriptions;
