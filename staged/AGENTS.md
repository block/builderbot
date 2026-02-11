# AGENTS.md

## Commands

use `just check-all` before you finalize any commit.
generally don't run the dev server unless asked, usually it is run from a UI integration.

## Backend

We are intentionally conservative with our data models. **Before adding fields or new types to
the backend, get human review.**

Generally we want to avoid reconciliation of state, so git is authoritative for anything it tracks.

## Frontend

### Theming

Colors defined in `src/lib/theme.ts`, applied via CSS custom properties in `app.css`.
All components use `var(--*)` for colors—no hardcoded values.
