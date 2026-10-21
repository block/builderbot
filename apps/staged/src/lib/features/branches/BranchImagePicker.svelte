<!--
  BranchImagePicker.svelte - The branch card's "Add image" picker

  Call `open()` from the user's tap/click. Desktop (Tauri) shows the native
  open panel filtered to images and hands back file paths; the web build (iOS
  Safari) clicks a hidden file input, so iOS presents its own source sheet —
  Photo Library or Files — and hands back `File`s. Cancelling either picker
  reports nothing.

  Props:
    onPickPaths — desktop: the picked file paths
    onPickFiles — web: the picked files
    onError     — the desktop panel could not be opened
-->
<script lang="ts">
  import { isTauri } from '../../transport';
  import { IMAGE_ACCEPT } from '../sessions/attachErrors';
  import { pickImagePaths } from './branchImagePicker';

  interface Props {
    onPickPaths: (paths: string[]) => void;
    onPickFiles: (files: File[]) => void;
    onError: (error: unknown) => void;
  }

  let { onPickPaths, onPickFiles, onError }: Props = $props();

  let fileInput: HTMLInputElement | undefined = $state();

  /**
   * Must run synchronously inside the user's gesture: iOS only opens a file
   * input's picker while the tap is still being handled.
   */
  export function open() {
    if (!isTauri) {
      fileInput?.click();
      return;
    }
    pickImagePaths().then((paths) => {
      if (paths.length > 0) onPickPaths(paths);
    }, onError);
  }

  function handleChange(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const files = Array.from(input.files ?? []);
    // Cleared so picking the same file again still fires `change`.
    input.value = '';
    if (files.length > 0) onPickFiles(files);
  }
</script>

{#if !isTauri}
  <input
    bind:this={fileInput}
    type="file"
    accept={IMAGE_ACCEPT}
    multiple
    class="file-input-hidden"
    onchange={handleChange}
  />
{/if}

<style>
  .file-input-hidden {
    display: none;
  }
</style>
