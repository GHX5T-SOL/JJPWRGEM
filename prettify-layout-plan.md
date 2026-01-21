# Prettify Layout Refactor Plan

## Goals

- Keep token-stream–based formatting (no AST).
- Preserve performance with minimal intermediate storage.
- Use composable concerns (depth, layout rules, emission).
- Apply fixed layout rules:
  1. Objects always expand if they have 1+ keys.
  2. Objects collapse if 0 keys.
  3. Arrays expand if any child expanded.
  4. Arrays expand if inline width over preferred width.
  5. Empty objects/arrays never expand.
  6. Scalars expand if inline width over preferred width.

## Implementation Stages

### Stage 1: Layout rules & unit tests

- Introduce a `layout` module with `FrameKind`, `FrameStats`, `LayoutDecision`, and `decide_layout`.
- Add `LayoutTracker` to collect per-frame stats during traversal.
- Unit test layout rules (already started in `prettify.rs`).

**Status:** Done. `layout` module added in `crates/parse/src/format/prettify.rs` and unit tests added for layout rules and tracker behavior.

### Stage 2: Capture visitor for layout testing

- Extract a generic `CaptureVisitor` that can wrap any `Visitor` and record decisions.
- Add tests that feed token streams and assert layout decisions without formatting output.

**Status:** Done. Added a generic capture visitor in `crates/parse/src/format/prettify.rs` tests and a unit test to verify event capture.

### Stage 3: Frame stats on the formatter path

- Augment formatter frames with layout stats:
  - `inline_len`, `key_count`, `child_expanded`, `is_empty`.
- Update stats on each event:
  - Increment `key_count` on `ObjectKey`.
  - Update `inline_len` by event token length (and child inline length if compact).
  - Set `child_expanded` when a child frame expands.

**Status:** Done. Formatter frames now track `inline_len`, `key_count`, `child_expanded`, and `is_empty`, with updates on each event.

### Stage 4: Layout decision integration

- Replace legacy `Level`/`root_array_level` logic.
- Use `decide_layout` at container close to set mode.
- Propagate `child_expanded` to parents and update `inline_len` when a child collapses.

**Status:** Done. Legacy `Level`/`root_array_level` logic removed; `decide_layout` now sets mode at container close and propagates child expansion/inline length to parents.

### Stage 5: Emission consistency

- Use a single emission path that consumes frames with a `LayoutDecision`.
- Ensure indentation and line ending decisions remain correct for expanded output.

**Status:** In progress. Emission still flows through the existing compact/expanded emitters but now consumes frame `mode` set by layout; remaining work is to consolidate into a single emission path if desired.

### Stage 6: Full flow validation

- Run unit tests for layout + depth.
- Run prettify integration tests and update/fix formatter until snapshots pass.
- Re-run full `cargo test`.

**Status:** Not started.

### Stage 7: Final prettify review

- Review `crates/parse/src/format/prettify.rs` for simplifications and consistency.
- Consolidate or remove redundant helpers, unused fields, and legacy leftovers.
- Validate layout/emission logic readability and add comments where needed.

**Status:** Not started.

## Notes

- We prefer storing frames over deeper intermediate IR to minimize allocations.
- Layout is decided per frame, but depth remains independent and composable.
- Use `mod x { ... }` to keep orthogonal pieces isolated and testable.
