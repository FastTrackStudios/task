# A Single Trusted System

_Charter #2 (formerly issue #2)._

The meta-charter: Task is meant to be the *one* system you trust with everything, rather than one more place things get lost. This charter defines what "trusted" has to mean concretely.

## Hard requirements

_To be detailed._

Prompts worth answering: What must never be lost, and what proves it? Is the vault always readable without Task running? What happens offline, and what happens when two devices disagree? How does data get out — export, and how complete is it? What is the recovery story when a device dies? What can an agent do without a human, and what must it never do unasked? When the system is wrong, how does a human see that and correct it?

## Non-goals

_To be detailed._

## Current state

Local-first vault of plain markdown; server optional. Per-org SQLite overlays for identity, timers, finance. Permissions enforced in prod via the architect gate. Files layer versions content with jj over a content-addressed store.

## How we evaluate

This is the charter the others are judged against when they conflict.
