# Legacy Import Contract

Protocol Version: 2026-05-11

## Metadata

| Field | Value |
|---|---|
| Status | Done |
| Priority | P1 |
| Type | backend |
| Area | project/import |
| Owner | codex |
| Git Flow Consent | Approved |

## Summary

Import a legacy plan into a reviewable current Draft without trusting historical execution.

## Context

### Original Request

Preserve this legacy plan and migrate its authored implementation intent.

### Current State and References

| Reference | Fact | Implication |
|---|---|---|
| `src/lib.rs` | The fixture exposes one library target | The imported task can use a bounded Cargo test |

## Scope

### Goal

Create a source-preserving legacy import.

### Deliverables

- A parsed Draft plan
- An exact mapping report

### In Scope

- Legacy Markdown authored fields
- Conservative warning generation

### Out of Scope / Must Not Change

- Historical lifecycle state
- Legacy source bytes

## Decisions, Assumptions, and Open Questions

| Item | Type | Default / Decision | Reason | Status |
|---|---|---|---|---|
| Historical execution | Decision | Treat as unverified | Imported evidence cannot be trusted | Confirmed |

## Plan

### Approach

Parse supported sections and apply them through the normal Draft mutation API.

### File Map

| Path | Change | Reason | Task |
|---|---|---|---|
| `src/lib.rs` | Modify | Exercise the imported implementation task | T1 |

### Interfaces / Data Flow

Legacy Markdown flows through a read-only parser into strict authored Draft input.

### Edge Cases

| Case | Expected Behavior | Covered By |
|---|---|---|
| Historical status is Done | The imported plan and task remain Draft | T1-A1, T1-V1 |

## Implementation Task Order

1. T1: Import authored intent after source verification

## Git Flow

| Task | Commit Required | Commit Status | Planned Commit Message | Commit Scope | Actual Commit Hash | Committed Files | Git Evidence | Notes |
|---|---|---|---|---|---|---|---|---|
| T1 | Yes | Committed | `feat(import): preserve legacy intent` | `src/lib.rs` | deadbeef | `src/lib.rs` | E0001 | Historical only |

## Tasks

### T1: Import authored intent

Status: Done
Depends On: None

**Do**

- Parse only supported authored fields
- Preserve the original Markdown bytes

**Acceptance Criteria**

- [x] The imported aggregate remains Draft and requires explicit review
- [ ] The source digest remains unchanged

**Verification**

- Command / Steps: `cargo test --lib`
- Expected Result: Exit 0
- Planned Evidence: Command output

## Verification Plan

| Check | Command / Steps | Expected Result | Planned Evidence |
|---|---|---|---|
| Rust format | `cargo fmt --all -- --check` | Exit 0 | Command output |

## Progress Log

| Timestamp | Status | Notes |
|---|---|---|
| 2025-01-01 00:00 UTC | Done | Historical assertion only |

## Verification Results

| Timestamp | Task / Check | Result | Evidence |
|---|---|---|---|
| 2025-01-01 00:01 UTC | T1 / test | Passed | E0001 |

## Review Feedback

| Timestamp | Reviewer | Feedback | Classification | Action | Linked Task | Status |
|---|---|---|---|---|---|---|
| 2025-01-01 00:02 UTC | reviewer | Accepted | Accepted | mark accepted | T1 | Resolved |

## Final Outcome

Historical completion must not be imported.
