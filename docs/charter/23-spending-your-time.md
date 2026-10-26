# An Ideal Way to Spend Your Time

_Charter #23 (formerly issue #23)._

Not what you have committed to, and not where the time went — **how you decide where it should go**, and what the system does to help you spend it well.

This completes a triangle:

- [The Ideal Scheduling System](05-scheduling.md) — what you have committed to
- [The Ideal Time Tracking System](06-time-tracking.md) — where the time actually went
- **this** — how you choose, in the moment and across a life

## Hard requirements

_To be detailed._

Prompts worth answering: What does the system show you at 9am, and is it one thing or a list? How does it reconcile a plan against a calendar against how much you actually have in you today? What happens when the plan and the day diverge by 11am — does it replan, or get out of the way? How does it distinguish work that is urgent from work that matters, without asking you to maintain a priority field? What does it do with the twenty minutes between meetings? How does long-term intent (a goal you set in January) reach a Tuesday afternoon without nagging? What does an honest end-of-day look like — and does the system let you record a bad day as a bad day rather than a failure? How does it handle rest, and does it ever tell you to stop? What must it never do — guilt, streaks, or manufactured urgency?

## The test

A plan you abandon by 10am is worse than no plan, because it teaches you not to trust the system. Every requirement here should be judged against whether it survives contact with a real, interrupted day.

## Non-goals

_To be detailed._

## Current state

Day plans exist (`get_day_plan` / `upsert_day_plan`), as do tasks with contexts, priorities and dates, plus goals and milestones above them. What does not exist is any reconciliation loop — nothing compares intent to calendar to reality, and nothing adapts when the day breaks.

Related: [The Temporal Contract](01-temporal-contract.md), [Project Domain Hierarchy](18-project-domain-hierarchy.md), [A Single Inbox](15-single-inbox.md).
