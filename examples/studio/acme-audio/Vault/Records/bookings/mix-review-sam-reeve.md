---
type: scheduling-booking
id: 8f3b1c62-4d5a-4e7f-9a10-6b2c8d4e5f71
event_type_id: mix-review-30
start_utc: 2026-08-24T15:00:00Z
end_utc: 2026-08-24T15:30:00Z
attendee_name: Sam Reeve
attendee_email: sam@example.com
status: completed
created_utc: 2026-08-17T09:12:00Z
note: Rough mix of "Washed" — asked about the vocal bus.
---

# Mix review — Sam Reeve

A booking that **happened**, which is the point of it being here.

A completed booking is the one state the bookings app offers to invoice:
billing a cancelled slot, or one that has not occurred yet, would be
offering somebody a mistake. So this is the row that grows an
"Invoice…" link when the finance app is enabled — and simply does not,
when it is off or absent.

Nothing in the bookings app knows what an invoice is. It asks whether
any enabled app offers `finance_contract::Billing`, and if one does, it
renders a link to wherever that app says to go.
