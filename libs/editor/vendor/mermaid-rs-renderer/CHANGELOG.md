# Changelog

## v0.2.2 (2026-04-23)

### Visual and Layout Fixes
- Fixed sequence diagram `alt` frame geometry and prevented wide section labels from panicking layout.
- Fixed compact flowchart label decorations.
- Made dotted edges visually distinct from solid edges.
- Fixed class diagram stereotypes being rendered as members.
- Fixed class diagram arrowheads being hidden under node boxes.
- Fixed state diagram description lines so titles are preserved and descriptions accumulate.
- Fixed empty-subgraph layout panic by keeping graph-level and local subgraph indexes mapped correctly.

### Rendering and Theme Fixes
- Fixed invalid non-ASCII hex color values causing panics.
- Preserved quoted font-family normalization for SVG text output.

### Gantt
- Added compact Gantt display mode via YAML frontmatter (`displayMode: compact`).

### Dependencies and Release
- Updated `anyhow`, `clap`, `criterion`, `regex`, and release action dependencies.
- Added release workflow automation for Homebrew and AUR package updates.

## v0.2.0 (2026-02-07)

### Layout Engine Overhaul
- Rewrote flowchart layout with improved routing, subgraph compaction, and tighter node spacing
- Auto-place edge labels with collision-aware search grid
- Added edge label relaxation for Flowchart, State, ER, and Requirement diagrams
- Node overlap resolver now runs for all diagram types when overlaps are detected
- Finer-grained label placement search for closer label-to-edge proximity

### Visual Quality Improvements
- Redesigned ER diagram tables with cleaner styling
- Redesigned pie charts with improved label readability
- Redesigned journey diagram layout
- Improved state diagram composite labels and marker sizing
- Improved gantt chart rendering: section bands, color coding, in-bar labels
- Improved mindmap, class, and flowchart rendering polish
- Compact subgraph sizing across diagram types

### Parser Fixes
- Parse `-- "text" -->` quoted edge label syntax (fixes #27)

### Performance
- Added font cache for text metrics — avoids redundant font lookups
- Added `--fastText` option for approximate text width metrics

### Benchmarking & Quality
- Layout quality scoring vs mermaid-cli
- 16 new stress fixtures for benchmarks
- Expanded comparison examples across all diagram types
- Sankey link path detection in quality checks

## v0.1.3 (2026-02-02)

Initial public release with 13 diagram types and 100-1400x performance vs mermaid-cli.
