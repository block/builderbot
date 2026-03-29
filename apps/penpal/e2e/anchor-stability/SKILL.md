# Anchor Stability Test Loop

Run the anchor stability test-improve loop for the penpal highlight system. Each iteration generates complex markdown, creates thread highlights via MCP, validates anchors survive edits (before/after/within), and iterates to improve the system.

## Usage

```
/anchor-stability-loop [iterations]
```

- `iterations` — number of iterations to run (default: 10)

## Test Parameters

- Each test scores max **7 points**: initial=2, editBefore=2, editAfter=2, editWithin=1
- 10 tests per iteration = **70 max** per iteration
- Pass threshold: **69/70**
- Dashboard: http://localhost:18950

## Loop Procedure

For EACH iteration (never batch — run ONE at a time with full between-iteration work):

### 1. Determine the next iteration number

```bash
cat e2e/anchor-stability/results/results.json | python3 -c "
import sys, json
d = json.load(sys.stdin)
print(d.get('currentIteration', len(d.get('iterations', []))))
"
```

### 2. Run the iteration

```bash
cd apps/penpal/e2e && \
STABILITY_ITERATION=<N> \
STABILITY_RESULTS_DIR=<absolute-path>/e2e/anchor-stability/results \
npx playwright test --project=anchor-stability
```

### 3. Read and analyze results

```bash
cat e2e/anchor-stability/results/results.json | python3 -c "
import sys,json; d=json.load(sys.stdin); it=d['iterations'][-1]
print(f'Iteration {it[\"iteration\"]}: {it[\"totalScore\"]}/70')
[print(f'  Test {t[\"testIndex\"]}: {t[\"total\"]}/7  init={t[\"scores\"][\"initial\"]}/2 before={t[\"scores\"][\"editBefore\"]}/2 after={t[\"scores\"][\"editAfter\"]}/2 within={t[\"scores\"][\"editWithin\"]}/1  [{t.get(\"sizeClass\",\"?\")}] {t[\"selectedText\"][:60]}') for t in it['tests']]
"
```

If any test scored < 7, examine failure diagnostics in the test details (analysis field). Look for:
- `textInMarkdown` — whether the selectedText exists in the current markdown
- `occurrences` — how many times (affects disambiguation)
- `allHighlights` — total mark elements on page
- `threadHighlight` — marks for the specific thread (0 = highlight never created)

### 4. Make improvements if needed

Improvements should be in **production code**, not by avoiding test cases that expose real issues. Key files:

- **`frontend/src/components/rehypeCommentHighlights.ts`** — rehype plugin that matches highlights via text matching
- **`internal/comments/anchor.go`** — server-side anchor resolution (text matching + Before/After disambiguation)
- **`internal/server/comments.go`** — thread re-resolution endpoint
- **`frontend/src/components/SelectionToolbar.tsx`** — frontend anchor computation

After making improvements:
1. Rebuild: `just build`
2. Record the improvement in results.json (top-level `improvements` array):

```bash
python3 -c "
import json
path = 'e2e/anchor-stability/results/results.json'
with open(path) as f:
    d = json.load(f)
d.setdefault('improvements', []).append({
    'afterIteration': <LAST_COMPLETED_ITERATION>,
    'type': '<production|test|dashboard>',
    'description': 'Description of the improvement',
})
with open(path, 'w') as f:
    json.dump(d, f, indent=2)
    f.write('\n')
"
```

3. Commit the fix

### 5. Repeat from step 1

## Critical Rules

- **NEVER batch iterations.** Run one, do all between-iteration work, then run the next.
- **NEVER avoid test cases that expose real issues.** If a test fails because of a real-world scenario (e.g., user selecting inside inline code), fix the production code, not the test.
- **NEVER make flaky tests.** Tests must pass reliably every time.
- Improvements go in production code (`frontend/src/`, `internal/`), not by constraining test inputs.
- Always record improvements in results.json notes so the dashboard shows them.
- Always check between iterations for new issues from the user.

## Starting the Dashboard

If not already running:

```bash
cd apps/penpal/e2e && npx tsx anchor-stability/serve-dashboard.ts &
```

Dashboard is at http://localhost:18950 with live polling.
