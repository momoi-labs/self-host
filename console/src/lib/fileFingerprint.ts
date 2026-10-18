/** How much of each end of a file the fingerprint reads. */
const END = 1024 * 1024;

/** A 53-bit hash of `bytes` (cyrb53), as hex. It tells files apart; it guards nothing. */
function hash(bytes: Uint8Array): string {
  let h1 = 0xdeadbeef;
  let h2 = 0x41c6ce57;
  for (const byte of bytes) {
    h1 = Math.imul(h1 ^ byte, 2654435761);
    h2 = Math.imul(h2 ^ byte, 1597334677);
  }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507) ^ Math.imul(h2 ^ (h2 >>> 13), 3266489909);
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507) ^ Math.imul(h1 ^ (h1 >>> 13), 3266489909);
  return (4294967296 * (2097151 & h2) + (h1 >>> 0)).toString(16);
}

/**
 * A mark for a file from its size, its modification time and the hash of its
 * first and last MiB, so a resumed upload continues the same file and another
 * file starts over.
 */
export function fingerprintOf(size: number, lastModified: number, head: Uint8Array, tail: Uint8Array): string {
  return `${size}-${lastModified}-${hash(head)}-${hash(tail)}`;
}

export async function fileFingerprint(file: File): Promise<string> {
  const head = new Uint8Array(await file.slice(0, END).arrayBuffer());
  const tail = new Uint8Array(await file.slice(Math.max(0, file.size - END)).arrayBuffer());
  return fingerprintOf(file.size, file.lastModified, head, tail);
}
