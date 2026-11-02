# The Ideal Time Tracking System

_Charter #6 (formerly issue #6)._

Where time actually went, accurate enough to bill from and honest enough to plan from.

## Hard requirements

_To be detailed._

Prompts worth answering: What starts and stops a timer, and how much of that can be automatic without being wrong? What happens to time you forgot to track? How does tracked time attach to a project, a task, or a client? What is the smallest unit that matters? What does an invoice need that a timer does not capture? Can time be edited after the fact, and what is the audit trail?

## Non-goals

_To be detailed._

## Current state

Per-org `timer.sqlite`; a watchOS timer surface over the server HTTP bridge; invoicing exists as a local CLI flow with per-person rates, not yet a server capability.

Depends on [The Temporal Contract](01-temporal-contract.md).
