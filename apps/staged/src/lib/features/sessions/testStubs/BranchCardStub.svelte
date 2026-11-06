<!--
  Test stub for BranchCard: records the delete state a project section hands
  each card (deleting, dropsDisabled) as data attributes, so component tests
  can assert on it without the real card.
-->
<script lang="ts">
  import type { Branch } from '../../../types';

  let {
    branch,
    deleting = false,
    dropsDisabled = false,
  }: { branch: Branch; deleting?: boolean; dropsDisabled?: boolean } = $props();
</script>

<div
  data-testid="branch-card"
  data-branch-id={branch.id}
  data-deleting={deleting}
  data-drops-disabled={dropsDisabled}
></div>
