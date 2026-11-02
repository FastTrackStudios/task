# Does AI Belong in Daily Workflow?

_Charter #16 (formerly issue #16)._

Deliberately a question, not a plan. Task already has an agent surface — an MCP server, per-org agent accounts, capability-routed runners, chat. This charter exists to decide whether that earns its place in daily use, or whether it is sophistication that makes the system worse.

The honest failure mode: an assistant that produces plausible output you then have to check, converting work you understood into work you must audit. If that is what we have, the answer is no, and the code should shrink.

## Hard requirements

_To be detailed._

Prompts worth answering: What must an agent do unasked before it is worth having, and what must it never do? When an agent is wrong, how fast do you find out — and does the system make that faster or slower than doing it yourself? Is agent output attributed and reversible? What is the trust ladder — can an agent earn more autonomy, and can it lose it? Which surfaces are agent-first (triage, capture, summarisation) and which must stay human-only (sending mail, spending money, deleting)? Does an agent ever answer *for* you to another human? What does the system look like with the AI turned off — is it still complete?

## The test

Every capability here should be judged against: **does this reduce the total work, or relocate it?** A charter item that only relocates work is a no.

## Non-goals

_To be detailed._

## Current state

MCP server exposing 15 tools; agents hold real per-org accounts; Routines run scheduled agent work; a forked agent-skills lane with capability-routed runners; Hermes as an agent chat surface. Standing rule already in force: agents never answer for the human.

Related: [A Single Trusted System](02-single-trusted-system.md), [A Single Inbox](15-single-inbox.md).
