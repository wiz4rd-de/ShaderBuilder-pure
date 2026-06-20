// Tiny path helpers (#63) for display labels in the File menu / title bar. We
// only ever show the final path component, so a pure string split (handling both
// POSIX `/` and Windows `\` separators) is enough — no Node `path` dependency in
// the browser bundle.

/** The final path component of `path` (its filename), or `path` itself if none. */
export function basename(path: string): string {
  const parts = path.split(/[\\/]/);
  // Drop any trailing empty segment (a path ending in a separator).
  for (let i = parts.length - 1; i >= 0; i--) {
    if (parts[i]) {
      return parts[i]!;
    }
  }
  return path;
}
