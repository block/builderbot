---
name: anchors
description: >
  Requirements-driven development framework. Use when setting up product/engineering
  requirements documents, checking traceability between requirements and code,
  verifying document consistency, or working on code in a repo that contains ANCHORS.md
  files (load the skill to understand the requirements context before making changes).
---

# anchors

A skill for managing ANCHORS requirements-driven development documents.

ANCHORS keeps product requirements, engineering requirements, testing strategy,
dependency constraints, and implementation in a consistent, traceable hierarchy.

---

## The ANCHORS Framework

### Documents

A module's ANCHORS document set consists of up to four documents plus a marker file:

| Document | Purpose | Requirement IDs |
|----------|---------|-----------------|
| **ANCHORS.md** | Module marker. YAML frontmatter with `prefix` field. | — |
| **PRODUCT.md** | Product requirements — user-facing behavior, workflows, experience. Source of truth. | `P-*` |
| **ERD.md** | Engineering requirements — technical design, interfaces, constraints. Derived from PRODUCT.md. | `E-*` |
| **TESTING.md** | Testing strategy — pyramid, coverage invariants, requirement-to-test-layer mapping. Defines how requirements are verified. | — |
| **DEPENDENCIES.md** | External dependencies — what the environment must provide because the system cannot supply it. | `D-DEP-*` |

All four documents are part of the framework. TESTING.md and DEPENDENCIES.md participate in the truth hierarchy and have defined disagreement rules.

### Modes

ANCHORS operates in one of two modes:

- **Embedded** (default): Docs live alongside the code they describe. Requirement IDs are tagged inline in source code and tests. Check searches the local codebase for traceability. This is the standard mode.
- **Detached**: Docs live separately from the code they describe. The target codebase is never modified. Traceability uses `→` forward references in ERD.md pointing to code locations in the target codebase. Detached mode works whether the docs are in the same repo (in-repo detached) or in a completely separate repo (external detached).

Mode is set explicitly: if `ANCHORS.md` frontmatter contains `mode: detached`, the module is detached. Otherwise it's embedded.

**ANCHORS.md frontmatter** — embedded mode:
```yaml
---
prefix: AUTH
---
```

**ANCHORS.md frontmatter** — detached mode (in-repo):
```yaml
---
prefix: PENPAL
mode: detached
path: ..
---
```

**ANCHORS.md frontmatter** — detached mode (external repo):
```yaml
---
prefix: AUTH
mode: detached
repo: github.com/org/auth-service
ref: main
path: src/auth
---
```

- `mode` — `detached` or omitted. When `detached`, the target code is never modified and ERD.md uses `→` forward references instead of inline code tags.
- `repo` — target codebase (GitHub URL or local path). Only used in detached mode when the code is in a different repo. Omit for in-repo detached.
- `ref` — branch, tag, or SHA to track (defaults to `main`). Only meaningful with `repo`.
- `path` — subdirectory to scope research and traceability. **In-repo detached** (no `repo`): relative to the ANCHORS.md file, like embedded mode. **External detached** (with `repo`): relative to the target repo root. Defaults to `.` (in-repo) or `/` (external).

### Truth Hierarchy

```
PRODUCT.md (source of truth — defines correct behavior)
    ↓ derives
ERD.md (technical requirements — must fully satisfy the PRD)
    ↓ constrains
DEPENDENCIES.md (external prerequisites — what the environment must provide)

TESTING.md (testing strategy — covers both PRODUCT.md and ERD.md)
    ↓ executed as
Tests (executable specification — truthier than implementation)
    ↓ validated against
Implementation (must satisfy all of the above)
```

### Disagreement Rules

- Implementation vs tests → implementation is probably wrong
- Tests vs PRD/ERD → tests are wrong (fix the tests, or update the documents first if the requirement changed)
- TESTING.md vs PRD/ERD → TESTING.md is wrong (update the testing strategy to match the requirements)
- ERD vs PRD → ERD is wrong (PRD is authoritative)
- DEPENDENCIES.md vs ERD → check which is correct (ERD may include requirements for the system to manage something that DEPENDENCIES.md lists as external — resolve which is current)

### Rules

1. **PRODUCT.md is authoritative.** If there is a conflict between documents, PRODUCT.md wins.
2. **ERD.md must fully satisfy PRODUCT.md.** Every product requirement should have corresponding engineering requirements that cover it.
3. **All implementation must meet both PRD and ERD requirements.** Do not implement against only one document.
4. **Keep documents consistent.** When changing a requirement, update PRODUCT.md first, then ERD.md to reflect it (and link back).
5. **Do not add requirements to ERD.md that contradict or extend PRODUCT.md without updating PRODUCT.md first.** The PRD drives the ERD, not the other way around.
6. **Tests are truthier than implementation, but documents are truthier than tests.** If the implementation diverges from the tests, the implementation is assumed buggy. If the tests diverge from the PRD or ERD, the tests are wrong. Fix the code to match the tests; fix the tests to match the documents; update the documents first if the requirement has genuinely changed.
7. **Every P-\* and E-\* requirement must have test coverage.** See TESTING.md for coverage invariants and the requirement-to-test-layer mapping. A requirement without a corresponding test is a coverage gap that must be addressed.

### Requirement ID Conventions

**PRODUCT.md** — `P-` prefix with section-scoped slugs:
```markdown
- <a id="P-AUTH-LOGIN"></a>**P-AUTH-LOGIN**: Users can log in with email and password.
```

**ERD.md** — `E-` prefix with backlink to the product requirement it satisfies:
```markdown
- <a id="E-AUTH-SESSION"></a>**E-AUTH-SESSION**: Sessions use signed JWTs with 24-hour expiry.
  ← [P-AUTH-LOGIN](PRODUCT.md#P-AUTH-LOGIN)
```

Every `E-*` requirement must have a `←` backlink. This is how the check tracks coverage.

**ERD.md (detached mode)** — `E-` prefix with backlink and `→` forward references to code locations in the target repo:
```markdown
- <a id="E-AUTH-SESSION"></a>**E-AUTH-SESSION**: Sessions use signed JWTs with 24-hour expiry.
  ← [P-AUTH-LOGIN](PRODUCT.md#P-AUTH-LOGIN)
  → `src/auth/session.go:NewSession`, `src/auth/middleware.go:ValidateToken`
```

The `→` line lists `file:symbol` pairs wrapped in backticks, comma-separated. File paths are relative to the resolved target directory (the `path` directory, however it was resolved — see Modes section). Forward references replace inline code tags — in detached mode, the target code is never modified.

**DEPENDENCIES.md** — `D-DEP-` prefix:
```markdown
### D-DEP-POSTGRES: PostgreSQL
- **Used by:** API server
- **Where it runs:** All environments
- **Why external:** Stateful service, cannot be bundled
```

### Coverage Invariants

TESTING.md defines the project-specific test strategy. These invariants apply universally:

1. **Every requirement has a test.** Every P-* and E-* must be covered by at least one test.
2. **Contract boundaries are tested from both sides.** Parsers (reading) and builders (writing) are both tested.
3. **Trust boundaries are tested end-to-end.** Human-approval vs. automatic actions each need an E2E test.
4. **Every public API surface has a test.** Happy path and error path.
5. **Recovery paths are tested.** At integration layer or above.
6. **Every interface has its production implementation verified.** Test doubles don't mask missing real implementations.

### Code Traceability

Tag functions and tests with requirement IDs. Augment comments, never replace them.

```go
// BuildStartCommand builds the argument list for starting the process.
// E-CLI-START: construct CLI start invocation with config flags.
func BuildStartCommand(req StartRequest) []string {
```

```go
// TestSessionExpiry verifies that expired sessions are rejected.
// E-AUTH-SESSION: sessions expire after the configured TTL.
func TestSessionExpiry(t *testing.T) {
```

Rules:
- One tag per function, not per line
- Keep existing descriptive comments — the tag is an addition
- Natural prose is fine: `// Per E-AUTH-SESSION, tokens expire after 24 hours`
- Skip self-explanatory code, boilerplate, and standard library calls

### Monorepo Support

ANCHORS supports arbitrary module nesting. A directory is an ANCHORS module if it contains an `ANCHORS.md` file with a `prefix` frontmatter field:

```yaml
---
prefix: PAY-CHECKOUT
---
```

Prefixes must be unique across the repo. Within a module, PRODUCT.md/ERD.md links are relative. Cross-module references use relative paths:

```markdown
← [P-PAY-CHECKOUT-CART](../checkout/PRODUCT.md#P-PAY-CHECKOUT-CART)
```

Not every module needs all four documents. Pure infrastructure modules might only have ERD.md tracing to another module's PRODUCT.md. TESTING.md can be per-module or shared.

---

## Routing

This skill is invoked as `/anchors` with optional arguments.

**Parse the args to determine the mode:**

| User types | Mode |
|------------|------|
| `/anchors` (no args) | **Interactive** — use `AskUserQuestion` to ask "Setup or check?" (see below) |
| `/anchors setup` | **Setup** — determine target path (see below) |
| `/anchors setup path/to/dir` | **Setup** — use the given path |
| `/anchors check` | **Check** |
| `/anchors embed` | **Embed** — convert detached module to embedded (see below) |
| `/anchors embed path/to/dir` | **Embed** — convert the detached module at the given path |

Any other args → tell the user the available modes (setup, check, embed).

**Interactive mode prompt:** When invoked with no args, first check whether any `ANCHORS.md` files exist in the repo (glob for `**/ANCHORS.md` excluding `node_modules`, `vendor`, `.git`, build output). Then use `AskUserQuestion`:
- Question: "What would you like to do?"
- Header: "Mode"
- If ANCHORS modules already exist, present **Check** first (recommended):
  - **Check** — "Check traceability and consistency across modules (Recommended)"
  - **Setup** — "Scaffold ANCHORS documents in a new directory"
  - **Embed** — "Convert a detached module to embedded mode" (only show if any module has `mode: detached`)
- If no ANCHORS modules exist, present **Setup** first (recommended):
  - **Setup** — "Scaffold ANCHORS documents in a directory (Recommended)"
  - **Check** — "Check traceability and consistency across modules"

---

## Setup Mode

Set up a complete ANCHORS document set for a project or module. Setup produces fully populated documents — real requirements, real engineering specs, a real testing strategy — not empty templates. A full setup (not "Skip existing") should produce a document set that passes an immediate check.

Setup uses the `anchors` CLI for deterministic steps (skill installation, scaffolding) and the LLM for research and content population.

### Determine the target path

1. If a path was given as an argument, use it.
2. If no path was given, check whether the current working directory already has an `ANCHORS.md`.
   - **No** → use the current working directory.
   - **Yes** → use `AskUserQuestion` to ask for a path:
     - Question: "This directory already has ANCHORS.md. Where should the new module be set up?"
     - Header: "Path"
     - Options: suggest 2-3 likely subdirectories based on the repo structure (e.g., `src/`, `services/`, `packages/`). The user can always pick "Other" to type a custom path.

### Steps

1. Check if any ANCHORS documents already exist in the target directory (`ANCHORS.md`, `PRODUCT.md`, `ERD.md`, `TESTING.md`, `DEPENDENCIES.md`). If any exist, use `AskUserQuestion` to confirm:
   - Question: "These ANCHORS documents already exist: [list]. Overwrite them?"
   - Header: "Overwrite"
   - Options: **Skip existing** ("Only create missing documents"), **Overwrite all** ("Replace all documents with fresh content")

2. Use `AskUserQuestion` to ask the user for project details (both in a single call):
   - Question 1: "What is the project/module name?" — Header: "Name" — Options: offer the target directory name as the recommended default, plus 1-2 alternatives derived from parent directories or repo name.
   - Question 2: "What requirement ID prefix should be used?" — Header: "Prefix" — Options: offer an uppercase abbreviation of the name as the recommended default, plus 1-2 alternatives (e.g., shorter/longer forms).

3. **Determine the mode.** Check whether the target directory contains code (source files, not just docs). If it does, this is an **embedded** setup — skip to step 4. If the target directory has no code (it's a standalone docs repo or empty directory), use `AskUserQuestion`:
   - Question: "Does this describe an existing codebase?"
   - Header: "Mode"
   - Options:
     - **In-repo detached** ("Yes — code is elsewhere in this repo")
     - **External detached** ("Yes — code is in another repo")
     - **Greenfield** ("No — this is a new project without existing code")

   If **In-repo detached**, use `AskUserQuestion` to collect the code location:
   - Question: "Where is the code relative to this directory?" — Header: "Path" — Suggest likely relative paths based on sibling/parent directories (e.g., `..`, `../../src`). The user can type a custom path.

   If **External detached**, use `AskUserQuestion` to collect target repo details:
   - Question 1: "What is the target repo?" — Header: "Repo" — Free text (GitHub URL or local path)
   - Question 2: "What branch/tag/SHA to track?" — Header: "Ref" — Options: `main` (default), `master`, Other
   - Question 3: "Subdirectory within the repo? (leave blank for root)" — Header: "Path" — Free text, defaults to `/`

4. **Invoke the CLI to scaffold.** First, ensure the skill is installed in the repo by running `anchors install` (idempotent — skips if already installed):

   ```bash
   anchors install
   ```

   Then scaffold the document skeletons with `anchors setup`:

   ```bash
   anchors setup ./payments --prefix PAY --mode embedded
   ```

   For detached mode, also pass `--path`, and optionally `--repo` and `--ref`:

   ```bash
   anchors setup apps/penpal/anchors --prefix PENPAL --mode detached --path ..
   ```

   The CLI creates skeleton files (structure but no populated content). If the user chose "Skip existing" in step 1, pass `--skip-existing` so the CLI only creates missing files.

5. **Research the project.** The goal is to build a complete understanding of what the project does, how it's built, what it depends on, and how it's tested. The approach depends on mode and whether the target directory contains existing code.

   **Embedded mode — target directory contains code:**

   Launch subagents to exhaustively research the codebase. Each subagent explores a different dimension and returns structured findings — not raw source code. The main context receives only the findings.

   Launch these in parallel:

   - **Functional areas agent**: Identify all user-facing functional areas, workflows, and behaviors. For each area, describe what the user sees/experiences, the key scenarios, and edge cases. Return a structured list of functional areas with requirement-level descriptions.

   - **Technical architecture agent**: Map the technical structure — components, interfaces, data flow, protocols, storage, external API contracts. Identify trust boundaries and contract boundaries. Return a structured description of the architecture organized by technical concern.

   - **Dependencies agent**: Find all external dependencies — services, runtimes, tools, and systems that must be present in the environment because the project cannot supply them. Distinguish between managed dependencies (bundled/installed by the project) and true external dependencies. Return a structured list with what each dependency is used by and why it's external.

   - **Testing agent**: Analyze the existing test suite — what frameworks are used, what layers exist (unit/integration/e2e), what's covered, what's not, where fixtures live, what test infrastructure exists. Return a structured summary of the testing approach, coverage patterns, and gaps.

   Each subagent should read broadly — not just top-level files, but trace into implementations, configs, tests, and build files. The findings should be exhaustive in coverage but compact in format: structured lists and descriptions, not code snippets.

   **Tests deserve special attention.** In the ANCHORS truth hierarchy, tests are truthier than implementation. Tests encode real behaviors, edge cases, invariants, and contract boundaries that may not be obvious from implementation alone. The functional areas and technical architecture agents should weight test files heavily — a tested behavior is a stronger signal of a real requirement than an implementation detail that might be incidental. The testing agent's findings directly feed TESTING.md, but they also inform PRODUCT.md (what behaviors matter enough to test) and ERD.md (what technical contracts are explicitly verified).

   **Detached mode — target codebase specified in ANCHORS.md:**

   Resolve the target codebase. **In-repo** (no `repo` field): resolve `path` relative to the ANCHORS.md file to find the local code directory. **External** (with `repo`): access the codebase at `{repo}` (at the `{ref}` revision), scoped to the `path` subdirectory.

   Launch the same four subagents as embedded mode, but point them at the resolved target directory. The subagents research the target codebase exactly as they would for embedded mode. Additionally, the technical architecture agent should note specific file paths and symbol names — these will become `→` forward references in ERD.md.

   **Greenfield — no code, no target repo:**

   The user's description of the project (from the conversation context, or a README, design doc, or similar artifact in the repo) is the source material. If the conversation doesn't contain enough context, use `AskUserQuestion` to ask the user to describe the project — what it does, who it's for, and how it works. This is a single open-ended question, not a multi-step interview.

6. **Populate the documents.** Using the research findings (or user description) and the document format conventions described in the ANCHORS Framework section above, populate the skeleton files created by the CLI with real content:

   - **ANCHORS.md**: Already created by CLI with correct frontmatter. No changes needed.
   - **PRODUCT.md**: Real P-* requirements organized by functional area. Every requirement should describe user-facing behavior, not implementation details. Use the prefix from ANCHORS.md to scope IDs (e.g., prefix `AUTH` → `P-AUTH-LOGIN`).
   - **ERD.md**: Real E-* requirements organized by technical concern, each with a `←` backlink to the P-* requirement it satisfies. Every P-* requirement must have at least one corresponding E-* requirement. **In detached mode**, each E-* requirement should also include `→` forward references to specific file:symbol locations in the target codebase discovered during research.
   - **TESTING.md**: Real testing strategy — actual test layers, actual coverage mapping from requirements to test layers, actual tooling, actual exclusions. Not boilerplate.
   - **DEPENDENCIES.md**: Real external dependencies with D-DEP-* IDs — or delete the file entirely if there are no true external dependencies.

   The populated documents must be internally consistent: every E-* traces to a P-*, the testing strategy covers the actual requirements, and DEPENDENCIES.md only lists things that are genuinely external.

7. **Write the files** to the target directory. If the user chose **Overwrite all** in step 1, and DEPENDENCIES.md was omitted (no true external dependencies), delete any existing `DEPENDENCIES.md` in the target directory to avoid stale content. If the user chose **Skip existing**, never delete existing files.

8. Print a summary of what was created — list the documents and a count of requirements generated (e.g., "12 product requirements, 18 engineering requirements, 3 external dependencies").

---

## Check Mode

Check traceability and consistency across all ANCHORS modules in the repo. Check has two layers: the CLI performs structural validation (deterministic, no LLM), and the skill adds semantic analysis on top.

### Step 1: Invoke the CLI for structural checks

Run `anchors check` to perform all deterministic validation. The CLI outputs a structured report covering:
- Module discovery and prefix collision detection
- Document presence
- Frontmatter validation
- ERD backlink verification
- PRD coverage analysis
- Forward reference validation (detached modules)
- Open question scanning

Review the CLI output. If there are structural errors (missing backlinks, broken refs, prefix collisions), these are definitive — report them directly.

### Step 2: Semantic analysis (skill-only)

After the structural check, the skill performs deeper analysis that requires understanding the code and requirements:

1. **For detached modules, resolve the target codebase.** For each detached module: **In-repo** (no `repo` field): resolve `path` relative to the ANCHORS.md file. **External** (with `repo`): access the codebase at `repo` and `ref`, scoped to `path` (defaulting to `/`). Cache or reuse the resolved target for subsequent steps.

2. **Check TESTING.md.** Verify TESTING.md exists. Check that the coverage mapping table references requirement areas that correspond to actual P-* and E-* requirements.

3. **Check DEPENDENCIES.md boundary.** Verify that nothing listed as an external dependency in DEPENDENCIES.md is contradicted by ERD.md requirements showing the system manages it internally (a stale dependency that was since internalized).

4. **Check code traceability (embedded modules only).** For embedded modules, search the codebase (excluding `node_modules`, `vendor`, `.git`, build output) for references to `P-*`, `E-*`, and `D-DEP-*` requirement IDs. Report:
   - Requirements referenced in code (good)
   - Requirements with no code references (potential coverage gaps)
   - Code references to IDs that don't exist in any document (stale tags)

   Skip this step for detached modules — they use forward references instead of inline code tags.

5. **Check test coverage tags (embedded modules only).** Search test files for requirement ID references. Report requirements that have implementation references but no test references. Skip for detached modules.

6. **Check cross-module references.** Validate that any cross-module `←` backlinks (relative paths to other modules' documents) resolve to actual files and anchors.

### Step 3: Print a combined report

Combine the CLI structural report with the semantic analysis into a single summary:

```
## ANCHORS Check Report

### Modules
- services/auth (prefix: AUTH) — 4/4 documents
- services/payments (prefix: PAY) — 3/4 documents (no DEPENDENCIES.md)
- platform/shared (prefix: SHLIB) — 1/4 documents (ERD.md only)
- apps/penpal/anchors (prefix: PENPAL, detached → ../penpal) — 3/4 documents
- docs/ext-api (prefix: EXTAPI, detached → github.com/org/api@main) — 3/4 documents

### Traceability (all modules)
- ERD → PRD backlinks: 41/43 (2 missing)
- PRD coverage by ERD: 28/28 (100%)
- Requirements in code (embedded): 52/71 (73%)
- Requirements in tests (embedded): 38/71 (54%)
- Forward references (detached): 12/14 (2 broken)

### Gaps

#### Missing ERD Backlinks
- AUTH: E-AUTH-CACHE-TTL (no ← P-* link)
- PAY: E-PAY-LOG-FORMAT (no ← P-* link)

#### Uncovered Product Requirements (P-* with no E-* coverage)
(none)

#### Untraced Requirements (no code references — embedded modules)
- AUTH: P-AUTH-NOTIFY-SOUND
- SHLIB: E-SHLIB-RECOVERY-RESTART

#### Requirements Without Test References (embedded modules)
- PAY: E-PAY-WEBHOOK-RETRY (in code but not in tests)

#### Broken Forward References (detached modules)
- EXTAPI: E-EXTAPI-RATE-LIMIT → `src/middleware/rate.go:RateLimit` (file not found)
- EXTAPI: E-EXTAPI-AUTH-VERIFY → `src/auth/verify.go:VerifyToken` (symbol not found)

#### Stale Code References (embedded modules)
- src/legacy.go: references E-OLD-THING (not in any document)

#### Open Questions
- services/auth/PRODUCT.md: OPEN-MFA-FLOW (unresolved)

#### DEPENDENCIES.md Boundary Issues
(none)
```

---

## Embed Mode

Convert a detached module to embedded mode. This adds inline requirement tags to source files based on `→` forward references, then strips the detached-mode artifacts from the docs. Only available for detached modules.

### Determine the target module

1. If a path was given as an argument, use it.
2. If no path was given, check whether the current working directory has an `ANCHORS.md` with `mode: detached`.
   - **Yes** → use the current working directory.
   - **No `mode: detached`** → error: "This module is already in embedded mode."
   - **No `ANCHORS.md`** → error: "No ANCHORS module found in this directory."

### Steps

1. **Read the module.** Read `ANCHORS.md` frontmatter to get `mode`, `repo`, `ref`, `path`. Read `ERD.md` and extract all `→` forward references (list of `file:symbol` pairs per E-* requirement).

2. **Locate the code locally.** The target code must be accessible on the local filesystem. Resolution:
   - **In-repo** (no `repo`): resolve `path` relative to the ANCHORS.md file. The code is already local.
   - **External with local path**: use `repo` directly, scoped to `path`.
   - **External with remote URL**: use `AskUserQuestion` to ask for the local path:
     - Question: "Where is the code locally? (e.g., you cloned or forked the repo)"
     - Header: "Code path"
     - Suggest the parent directory and common sibling paths.
   - Verify the resolved path exists and contains the expected files (spot-check a few `→` refs).

3. **Add inline tags.** For each `→` reference in ERD.md:
   - Resolve the `file:symbol` against the resolved target directory.
   - Find the function/symbol definition in the file.
   - Add an inline requirement tag comment above or on the function (e.g., `// E-AUTH-SESSION: sessions use signed JWTs`).
   - Follow the standard code traceability rules: one tag per function, augment existing comments, don't replace them.
   - If a `→` ref can't be resolved (file missing, symbol not found), skip it and report it at the end.

4. **Update ERD.md.** Remove all `→` lines. The `←` backlinks are preserved unchanged.

5. **Update ANCHORS.md.** Remove the `mode`, `repo`, `ref`, and `path` fields from the frontmatter, leaving only `prefix`. This switches the module to embedded mode.

6. **Report results.** Print a summary:
   - Number of inline tags added
   - Number of `→` refs that couldn't be resolved (with details)
   - Suggest running `/anchors check` to verify the conversion
