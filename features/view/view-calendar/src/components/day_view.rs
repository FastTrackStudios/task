//! Day view — 1-column [`TimeGridView`].

use chrono::NaiveDate;
use dioxus::prelude::*;

use crate::store::CalendarMutation;
use crate::types::{CalendarEvent, EventId, TemplateBlock};

use super::time_grid::TimeGridView;

#[derive(Props, Clone, PartialEq)]
pub struct DayViewProps {
    pub anchor: NaiveDate,
    pub events: Vec<CalendarEvent>,
    #[props(default)]
    pub template_blocks: Vec<TemplateBlock>,
    #[props(default)]
    pub on_block_click: Option<EventHandler<(NaiveDate, String)>>,
    #[props(default)]
    pub on_block_edit: Option<EventHandler<crate::types::BlockEdit>>,
    #[props(default = false)]
    pub readonly: bool,
    pub on_event: EventHandler<CalendarMutation>,
    pub on_open_editor: EventHandler<EventId>,
}

#[component]
pub fn DayView(props: DayViewProps) -> Element {
    rsx! {
        TimeGridView {
            days: vec![props.anchor],
            events: props.events,
            template_blocks: props.template_blocks,
            on_block_click: props.on_block_click,
            on_block_edit: props.on_block_edit,
            readonly: props.readonly,
            on_event: props.on_event,
            on_open_editor: props.on_open_editor,
        }
    }
}
