<!--
  PooledSessionChatPane.svelte — adopts a pooled SessionChatPane.

  Drop-in for `<SessionChatPane>` inside a dialog: the pane itself is leased
  from `sessionPanePool` and attached under this component's slot, so a close
  parks it rather than destroying it and a reopen of the same session finds
  the transcript already built. `onPane` hands the live pane (search, close)
  to the host in place of `bind:this`.
-->
<script lang="ts">
  import { getAllContexts, untrack } from 'svelte';
  import type { Attachment } from 'svelte/attachments';
  import type { HashtagClickInfo } from '../references/referenceHistory.svelte';
  import type { HashtagItem, ProjectRepo, Session } from '../../types';
  import { reuseHashtagItems } from './hashtagItemsIdentity';
  import type { LinkedNoteContext } from './noteFreshness';
  import type { DisplayRootInput } from './pathDisplayRoots';
  import {
    sessionPanePool,
    type PaneLease,
    type PaneProps,
    type SessionPaneExports,
  } from './sessionPanePool.svelte';

  interface Props {
    active: boolean;
    sessionId: string;
    repoDir?: DisplayRootInput;
    branchId?: string | null;
    projectId?: string | null;
    repoLabel?: Pick<ProjectRepo, 'githubRepo' | 'subpath' | 'headRepo'> | null;
    hashtagItems?: HashtagItem[];
    noteInfo?: LinkedNoteContext | null;
    onOpenSession?: (sessionId: string) => void;
    onHashtagClick?: (click: HashtagClickInfo) => void;
    onSessionChange?: (session: Session | null) => void;
    onSearchStateChange?: (state: { matchCount: number; currentIndex: number }) => void;
    compact?: boolean;
    /** The adopted pane once it is attached and awake; null when it is released. */
    onPane?: (pane: SessionPaneExports | null) => void;
  }

  let {
    active,
    sessionId,
    repoDir,
    branchId,
    projectId,
    repoLabel = null,
    hashtagItems,
    noteInfo,
    onOpenSession,
    onHashtagClick,
    onSessionChange,
    onSearchStateChange,
    compact = false,
    onPane,
  }: Props = $props();

  // Carries bits-ui's `defaultPortalTo="#portal-root"` from Root.svelte into
  // the imperatively mounted pane. Without it, pickers and viewers portal to
  // <body> and every open restyles the whole app again.
  const context = getAllContexts();

  let lease = $state.raw<PaneLease | null>(null);

  /** Everything but `active`, which only turns on once the pane is attached and unparked. */
  function syncProps(props: PaneProps) {
    props.sessionId = sessionId;
    props.repoDir = repoDir;
    props.branchId = branchId;
    props.projectId = projectId;
    props.repoLabel = repoLabel;
    props.noteInfo = noteInfo;
    props.compact = compact;
    props.onOpenSession = onOpenSession;
    props.onHashtagClick = onHashtagClick;
    props.onSessionChange = onSessionChange;
    props.onSearchStateChange = onSearchStateChange;
    // The pane resets its markdown cache whenever this identity changes, so a
    // list that reads the same (a hit adopted by the other dialog, which built
    // its own copy) keeps the array the pane already holds.
    const next = hashtagItems;
    const prev = props.hashtagItems;
    if (next && prev) {
      props.hashtagItems = reuseHashtagItems(prev, next);
    } else {
      props.hashtagItems = next;
    }
  }

  // Keyed on the session: navigating a live dialog to another session releases
  // one pane and leases the next. The attachment body runs inside an effect, so
  // everything it reads besides its argument is untracked — a read of `active`
  // here would otherwise re-lease the pane every time the dialog toggled it.
  function adopt(id: string): Attachment<HTMLDivElement> {
    return (slot) => {
      const l = untrack(() => {
        const next = sessionPanePool.acquire(id, context);
        lease = next;
        syncProps(next.props);
        slot.appendChild(next.host);
        // Attach → unpark → active: `unpark` reads and writes the transcript's
        // scroll box, which needs the live layout, and the pane's load effect
        // (which focuses the composer) runs only once `active` flips. On a hit
        // this runs now; on a miss, after the pool's microtask mount.
        next.ready((pane) => {
          if (lease !== next) return;
          pane.unpark();
          next.props.active = active;
          onPane?.(pane);
        });
        return next;
      });
      return () => {
        if (lease === l) lease = null;
        onPane?.(null);
        l.release();
      };
    };
  }

  $effect(() => {
    const l = lease;
    if (!l) return;
    syncProps(l.props);
    l.props.active = active;
  });
</script>

<div class="session-pane-slot" style="display: contents" {@attach adopt(sessionId)}></div>
