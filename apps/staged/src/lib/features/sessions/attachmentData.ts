/**
 * A browser `File`'s bytes as base64, the payload `create_image_from_data`
 * takes. Converted in chunks to avoid O(n²) string concatenation and the
 * argument-count limit of a single `String.fromCharCode` call.
 */
export async function fileToBase64(file: Blob): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  const chunks: string[] = [];
  for (let i = 0; i < bytes.length; i += 8192) {
    chunks.push(String.fromCharCode(...bytes.subarray(i, i + 8192)));
  }
  return btoa(chunks.join(''));
}
