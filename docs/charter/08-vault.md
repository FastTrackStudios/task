# The Ideal Vault (PKM)

_Charter #8 (formerly issue #8)._

The raw substrate: plain markdown you own, that outlives the app.

## Hard requirements

_To be detailed._

Prompts worth answering: What must be true of the on-disk tree for it to still be yours — readable, greppable, editable in any editor, syncable by any tool? What does a link mean, and what happens when its target moves? How much structure may frontmatter carry before the file stops being a note? What does the editor owe the file format? How do notes relate to projects, songs, contacts, events without becoming a database?

## Non-goals

_To be detailed._

## Current state

Live FS scan with a debounced watcher; markdown + YAML frontmatter; virtual folders via a `folder:` wikilink rather than physical nesting; wikilink resolution with block refs; per-file CRDT collab; `.base` view files; entity conventions for contacts, events, songs, setlists.

Note the standing constraint: the vault stays a plain portable tree even as the Files layer takes over as its write path.
