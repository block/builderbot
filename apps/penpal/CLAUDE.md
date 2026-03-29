<!-- rp1:start -->
## rp1 Knowledge Base

**Use Progressive Disclosure Pattern**

Location: `.rp1/context/`

Files:
- index.md (always load first)
- architecture.md
- modules.md
- patterns.md
- concept_map.md

Loading rules:
1. Always read index.md first.
2. Then load based on task type:
   - Code review: patterns.md
   - Bug investigation: architecture.md, modules.md
   - Feature work: modules.md, patterns.md
   - Strategic or system-wide analysis: all files
<!-- rp1:end -->

## ANCHORS — REQUIRED FOR ALL CHANGES

This repo uses ANCHORS for requirements-driven development. **You MUST load the anchors skill (`/anchors`) before making any code changes.**

When adding or modifying features, you must update ALL THREE documents before writing code:
1. **PRODUCT.md** — Add a P-* requirement (user-facing behavior only)
2. **ERD.md** — Add an E-* requirement with `←` backlink to the P-* ID
3. **TESTING.md** — Add or update the coverage mapping table so every new/changed requirement has a test-layer assignment. Always verify the table reflects the current scope — even if a row already exists, it may need updating.

Implementation and test code must include inline requirement ID comments (e.g., `// E-PENPAL-FEATURE-NAME`).
