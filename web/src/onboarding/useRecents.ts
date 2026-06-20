// Load the recent-projects list once for the start screen (#66). Reuses the #63
// recents store (`load_recents`, which prunes missing files). Swallows a recents
// hiccup — the start screen simply shows no recents rather than failing.
import { useEffect, useState } from "react";

import type { RecentProject } from "../bindings/RecentProject";
import { loadRecents } from "../session/api";

export function useRecents(): RecentProject[] {
  const [recents, setRecents] = useState<RecentProject[]>([]);
  useEffect(() => {
    let cancelled = false;
    void loadRecents()
      .then((list) => {
        if (!cancelled) {
          setRecents(list);
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);
  return recents;
}
