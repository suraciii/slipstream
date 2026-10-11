import { fetchEditState, type EditStateRead } from "../api/edit-state.js";
import type { BrowserFetch } from "./access-session.js";

/** Reads the primary projection; the full-recipe owner owns every edit write. */
export function createEditorEditState(
  fetcher: BrowserFetch,
  dependencies: Readonly<{
    owns: (photoId: string) => boolean;
    render: () => void;
  }>,
) {
  let read: EditStateRead | undefined;
  let confirmed = false;
  let note = "Checking current Edit State…";
  let generation = 0;
  let loading: Promise<void> | undefined;
  const load = (photoId: string): Promise<void> => {
    if (loading) return loading;
    const stamp = ++generation;
    confirmed = false;
    dependencies.render();
    const operation = (async () => {
      try {
        const state = await fetchEditState(fetcher, photoId);
        if (stamp !== generation || !dependencies.owns(photoId)) return;
        read = state;
        confirmed = state !== undefined;
        note = state
          ? state.current
            ? "Saved edit."
            : "Choose exposure settings to start editing."
          : "The current Edit State could not be read. Reload to check again.";
      } catch {
        if (stamp !== generation || !dependencies.owns(photoId)) return;
        note =
          "The current Edit State could not be read. Reload to check again.";
      }
      dependencies.render();
    })();
    loading = operation.finally(() => {
      if (stamp === generation) loading = undefined;
    });
    return loading;
  };
  return {
    get read() {
      return read;
    },
    get blocked() {
      return !confirmed;
    },
    get note() {
      return note;
    },
    load,
    invalidate: () => {
      generation += 1;
      loading = undefined;
      confirmed = false;
    },
    reset: () => {
      generation += 1;
      loading = undefined;
      read = undefined;
      confirmed = false;
      note = "Checking current Edit State…";
    },
  };
}
