---
scope: Engineering requirements — implementation details derived from product requirements.
see-also: PRODUCT.md — product-level requirements (user-facing behavior, workflows, and experience).
---

# Penpal: Engineering Requirements

This document captures implementation requirements that are derived from the product requirements in PRODUCT.md. Product requirements describe *what* the system does for users; engineering requirements describe *how* it must be built to satisfy those product requirements reliably.

---

## 1. Highlight Rendering

*Derived from: PRODUCT.md § Comment Threads — Highlighting*

### Rendering Strategy

- **ENG-HIGHLIGHT-REHYPE**: Render highlights via a rehype plugin during the markdown-to-HTML pipeline, not via post-render DOM mutation. This ensures highlights survive re-renders and SSE content updates.

- **ENG-HIGHLIGHT-MARK**: Use `<mark>` elements (or equivalent semantic HTML) with data attributes linking each highlight to its thread ID.

### Anchor Resolution

- **ENG-HIGHLIGHT-ANCHOR-MATCH**: Resolve anchors by matching the stored `selectedText` within the scope defined by `headingPath`. Fall back to full-document search if the heading path no longer matches.

- **ENG-HIGHLIGHT-ANCHOR-FUZZY**: When exact text matching fails (e.g., minor edits to the anchored region), attempt fuzzy matching to preserve highlight placement. Mark fuzzy-matched highlights visually so the user knows the anchor may have drifted.

- **ENG-HIGHLIGHT-ANCHOR-LOST**: When an anchor cannot be resolved at all, display the thread in the comments panel with a "lost anchor" indicator rather than silently dropping it.

### Re-anchoring on File Change

- **ENG-HIGHLIGHT-LIVE-RELOAD**: On SSE file-change events, re-run the full highlight pipeline (anchor resolution + rehype rendering) against the new content. Do not cache stale anchor positions across reloads.

- **ENG-HIGHLIGHT-BATCH**: When multiple threads exist on a single file, resolve all anchors in a single pass rather than making multiple passes over the document.

---

## 2. Scroll Synchronization

*Derived from: PRODUCT.md § Comment Threads — Scroll & Focus*

- **ENG-SCROLL-ELEMENT-ID**: Assign stable element IDs to highlight `<mark>` elements so `scrollIntoView` can target them reliably.

- **ENG-SCROLL-PANEL-SYNC**: When scrolling the comments panel to a thread, account for panel header height and thread expansion state to ensure the target thread is fully visible.

- **ENG-SCROLL-DEBOUNCE**: Debounce scroll-triggered focus changes to prevent rapid cycling when the user scrolls through a region with many highlights.
