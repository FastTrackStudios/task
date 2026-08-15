//! Personal day-template CRUD. Only surface the personal-
//! scheduling UI needs.

use crate::error::SchedulingError;
use crate::time_block::{DayTemplate, DayTemplateId};

#[architect::rpc]
pub trait DayTemplates {
    fn list_day_templates(&self) -> Result<Vec<DayTemplate>, SchedulingError>;
    fn get_day_template(&self, id: &DayTemplateId) -> Result<DayTemplate, SchedulingError>;
    fn upsert_day_template(&self, template: &DayTemplate) -> Result<(), SchedulingError>;
    fn delete_day_template(&self, id: &DayTemplateId) -> Result<(), SchedulingError>;
}
