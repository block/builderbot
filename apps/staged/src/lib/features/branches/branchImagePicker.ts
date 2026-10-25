import { IMAGE_EXTENSIONS } from './branchCardHelpers';

/** The open panel's filter: the image formats the image store keeps. */
export const IMAGE_PICKER_FILTER = {
  name: 'Images',
  extensions: IMAGE_EXTENSIONS.map((ext) => ext.slice(1)),
};

/**
 * Desktop (Tauri) "Add image": the native open panel, filtered to images.
 * Resolves to the picked paths — empty when the user cancels.
 *
 * The dialog plugin rather than a file `<input>`: wry's open panel ignores
 * the input's `accept`, so it could not be filtered to images.
 */
export async function pickImagePaths(): Promise<string[]> {
  const { open } = await import('@tauri-apps/plugin-dialog');
  const paths = await open({
    title: 'Add image',
    multiple: true,
    directory: false,
    filters: [IMAGE_PICKER_FILTER],
  });
  return paths ?? [];
}
