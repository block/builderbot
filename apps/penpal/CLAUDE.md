## ANCHORS — REQUIRED FOR ALL CHANGES

This repo uses ANCHORS for requirements-driven development. **You MUST load the anchors skill (`/anchors`) before making any code changes.**

When adding or modifying features, you must update ALL THREE documents before writing code:
1. **PRODUCT.md** — Add a P-* requirement (user-facing behavior only)
2. **ERD.md** — Add an E-* requirement with `←` backlink to the P-* ID
3. **TESTING.md** — Add or update the coverage mapping table so every new/changed requirement has a test-layer assignment. Always verify the table reflects the current scope — even if a row already exists, it may need updating.

Implementation and test code must include inline requirement ID comments (e.g., `// E-PENPAL-FEATURE-NAME`).
