import { fetchEditState, type EditStateRead } from "../api/edit-state.js";
import { isRecord } from "../api/guards.js";
import type { BrowserFetch } from "./access-session.js";
import { randomUuid } from "./browser-crypto.js";

export function createEditorEditState(
  fetcher: BrowserFetch,
  dependencies: Readonly<{
    owns: (photoId: string) => boolean;
    render: () => void;
    markStale: () => void;
    refreshCompatibility: (photoId: string) => Promise<void>;
    requestPreview: (photoId: string) => Promise<void>;
    describeRefusal: (response: Response, subject: string) => Promise<string>;
  }>,
) {
  let read: EditStateRead | undefined;
  let pending = false;
  let confirmed = false;
  let note = "Checking current Edit State…";
  let generation = 0;
  let scopeGeneration = 0;
  let loading: Promise<void> | undefined;
  const load = (photoId: string, confirmingWrite = false): Promise<void> => {
    if (pending && !confirmingWrite) return Promise.resolve();
    if (loading && !confirmingWrite) return loading;
    if (loading && confirmingWrite) {
      generation += 1;
      loading = undefined;
    }
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
          ? "Current Edit State checked."
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
  const change = async (photoId: string, value?: number): Promise<void> => {
    if (
      !dependencies.owns(photoId) ||
      pending ||
      !confirmed ||
      !read ||
      read.requiresRebind
    )
      return;
    if (value !== undefined && !Number.isFinite(value)) return;
    if (value === undefined && read.current?.engine !== "darktable") return;
    const stamp = scopeGeneration;
    const current = read.current;
    pending = true;
    generation += 1;
    loading = undefined;
    confirmed = false;
    note = value === undefined ? "Resetting exposure…" : "Saving exposure…";
    dependencies.markStale();
    dependencies.render();
    try {
      const response = await fetcher(
        `/api/photos/${encodeURIComponent(photoId)}/edit/${value === undefined ? "reset" : "set"}`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            requestId: randomUuid(),
            expectedEditRevision: read.editRevision,
            target: "darktable.exposure",
            control: "ev",
            ...(value === undefined
              ? {}
              : {
                  value,
                  ...(current?.engine === "darktable"
                    ? {}
                    : {
                        from:
                          current?.input.kind === "artifact"
                            ? `artifact:${current.input.artifactId}`
                            : "original",
                      }),
                }),
          }),
        },
      );
      if (stamp !== scopeGeneration || !dependencies.owns(photoId)) return;
      if (!response.ok) {
        const refusal = await dependencies.describeRefusal(
          response,
          "Exposure change",
        );
        if (stamp === scopeGeneration && dependencies.owns(photoId))
          note = refusal;
        return;
      }
      const outcome: unknown = await response.json();
      if (
        !isRecord(outcome) ||
        !["saved", "replayed", "unchanged"].includes(String(outcome["outcome"]))
      ) {
        note =
          "The exposure change could not be confirmed. Reload the current Edit State.";
        return;
      }
      await load(photoId, true);
      if (stamp !== scopeGeneration || !dependencies.owns(photoId)) return;
      await dependencies.refreshCompatibility(photoId);
    } catch {
      if (stamp === scopeGeneration && dependencies.owns(photoId))
        note =
          "The exposure change could not be confirmed. Reload the current Edit State.";
    } finally {
      if (stamp === scopeGeneration && dependencies.owns(photoId)) {
        pending = false;
        dependencies.render();
        if (confirmed) void dependencies.requestPreview(photoId);
      }
    }
  };
  return {
    get read() {
      return read;
    },
    get pending() {
      return pending;
    },
    get blocked() {
      return pending || !confirmed;
    },
    get note() {
      return note;
    },
    load,
    setExposure: (photoId: string, value: number) => change(photoId, value),
    resetExposure: (photoId: string) => change(photoId),
    reset: () => {
      scopeGeneration += 1;
      generation += 1;
      loading = undefined;
      read = undefined;
      pending = false;
      confirmed = false;
      note = "Checking current Edit State…";
    },
  };
}
