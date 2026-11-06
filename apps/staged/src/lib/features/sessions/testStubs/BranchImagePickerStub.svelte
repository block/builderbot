<!--
  Test stub for BranchImagePicker: exposes its desktop (paths) and web (files)
  callbacks as plain buttons that each pick one PNG, so component tests can
  drive "Add image" without the native open panel or a file input.
-->
<script lang="ts">
  let {
    onPickPaths,
    onPickFiles,
  }: {
    onPickPaths: (paths: string[]) => void;
    onPickFiles: (files: File[]) => void;
  } = $props();
</script>

<button
  type="button"
  data-testid="pick-image-paths"
  onclick={() => onPickPaths(['/tmp/picked.png'])}
>
  Pick image path
</button>
<button
  type="button"
  data-testid="pick-image-files"
  onclick={() => onPickFiles([new File(['png'], 'picked.png', { type: 'image/png' })])}
>
  Pick image file
</button>
