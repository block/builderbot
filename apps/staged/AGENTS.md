# AGENTS.md

## Commands

rely on git hooks for commit/push checks; don't run `just ci` before every commit.
run `just ci` manually only when you want early pre-push feedback.
generally don't run the dev server unless asked, usually it is run from a UI integration.

## Backend

We are intentionally conservative with our data models. **Before adding fields or new types to
the backend, get human review.**

Generally we want to avoid reconciliation of state, so git is authoritative for anything it tracks.

## Frontend

### Components

Prefer small, composable components over large monolithic ones.
Extract repeated UI blocks into focused subcomponents with clear props.

### Theming

Colors defined in `src/lib/theme.ts`, applied via CSS custom properties in `app.css`.
All components use `var(--*)` for colors—no hardcoded values.

## Rust

Keep `src/lib` code files smaller and organized by responsibility.
Split growing modules into focused submodules before files become hard to navigate.
