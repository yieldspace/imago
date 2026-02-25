# Codex Review Checklist

Use this label when requesting a general implementation review.

## Focus Areas
- Correctness: Verify behavior matches requirements and edge cases are handled.
- Regression risk: Check for unintended behavior changes in neighboring flows.
- Readability: Confirm code structure and naming remain clear and maintainable.
- Simplicity: Prefer minimal complexity (YAGNI/KISS/DRY) and avoid unnecessary abstractions.
- Tests: Ensure tests cover success and failure paths for changed behavior.
- Documentation sync: Confirm relevant code docs (`//!`/`///`) and user guides are updated when contracts or rules change, and tests reflect the same contract.
- Performance/resource use: Watch for unnecessary allocations, copies, or steady background load.
