import type { BrowserFetch } from "./access-session.js";
import {
  fetchComposableRecipe,
  parseComposableRecipe,
  saveComposableRecipeBody,
  type ComposableRecipeRead,
} from "../api/composable-recipe.js";
import {
  composableDraftDiffers,
  composableSaveRequest,
  draftFromRecipe,
  type ComposableRecipeDraft,
} from "./composable-recipe-draft.js";
import { randomUuid } from "./browser-crypto.js";
import { isRecord } from "../api/guards.js";

type RecipeRead = ComposableRecipeRead;
type Submission = Readonly<{
  body: string;
  draft: ComposableRecipeDraft;
  rebind: boolean;
}>;
export type ComposableAutosaveState = {
  read: RecipeRead;
  draft: ComposableRecipeDraft;
  saving: boolean;
  uncertain: boolean;
  conflict: boolean;
  failure: boolean;
  recovered: boolean;
  recoveryAvailable: boolean;
  note: string;
  pending: Submission | undefined;
  undo: ComposableRecipeDraft[];
  redo: ComposableRecipeDraft[];
};
const recoveryKey = "slipstream-composable-drafts-v1";
const maximumRecoveryCodeUnits = 1024 * 1024;
const canonicalSettings = (
  value: Pick<ComposableRecipeDraft, "steps" | "currentStepId">,
) => ({
  steps: [...value.steps].sort((left, right) =>
    left.stepId < right.stepId ? -1 : left.stepId > right.stepId ? 1 : 0,
  ),
  currentStepId: value.currentStepId,
});
const sameSettings = (
  a: Pick<ComposableRecipeDraft, "steps" | "currentStepId">,
  b: Pick<ComposableRecipeDraft, "steps" | "currentStepId">,
) =>
  JSON.stringify(canonicalSettings(a)) === JSON.stringify(canonicalSettings(b));
const observedSourceRevision = (read: RecipeRead): string | undefined =>
  (read.currentSourceRevision ?? read.sourceRevision) || undefined;

/** Pending writes belong to a Photo, independently of the visible workspace. */
export function createComposableAutosave(
  fetcher: BrowserFetch,
  changed: (photoId: string) => void,
  storage: Storage | undefined = (() => {
    try {
      return globalThis.sessionStorage;
    } catch {
      return undefined;
    }
  })(),
) {
  const photos = new Map<string, ComposableAutosaveState>();
  let recovery: Record<string, unknown> = {};
  let storageAvailable = storage !== undefined;
  try {
    const stored: unknown = JSON.parse(storage?.getItem(recoveryKey) ?? "{}");
    if (
      isRecord(stored) &&
      Object.keys(stored).length <= 64 &&
      JSON.stringify(stored).length <= maximumRecoveryCodeUnits
    )
      recovery = stored;
    else storageAvailable = false;
  } catch {
    storageAvailable = false;
  }
  const persist = (photoId: string, state: ComposableAutosaveState) => {
    const next = { ...recovery };
    if (
      state.pending ||
      composableDraftDiffers(state.draft, state.read.recipe) ||
      state.conflict
    ) {
      next[photoId] = { draft: state.draft, pending: state.pending ?? null };
    } else delete next[photoId];
    const text = JSON.stringify(next);
    state.recoveryAvailable =
      storageAvailable &&
      Object.keys(next).length <= 64 &&
      text.length <= maximumRecoveryCodeUnits;
    if (!state.recoveryAvailable) return;
    try {
      storage!.setItem(recoveryKey, text);
      recovery = next;
    } catch {
      storageAvailable = false;
      state.recoveryAvailable = false;
    }
  };
  const notify = (photoId: string, state: ComposableAutosaveState) => {
    persist(photoId, state);
    changed(photoId);
  };
  const open = (photoId: string, read: RecipeRead) => {
    let state = photos.get(photoId);
    if (state) {
      if (
        !state.pending &&
        (state.read.recipe?.revision !== read.recipe?.revision ||
          state.read.sourceRevision !== read.sourceRevision ||
          state.read.currentSourceRevision !== read.currentSourceRevision ||
          state.read.sourceAvailable !== read.sourceAvailable)
      ) {
        const dirty = composableDraftDiffers(state.draft, state.read.recipe);
        state.read = read;
        if (
          dirty &&
          (state.draft.baseRevision !== (read.recipe?.revision ?? null) ||
            (observedSourceRevision(read) !== undefined &&
              state.draft.sourceRevision !== observedSourceRevision(read)))
        ) {
          state.conflict = true;
          state.note =
            "The saved edit changed elsewhere. Use the saved edit or reapply your local settings.";
        } else if (!dirty) {
          state.draft = draftFromRecipe(
            photoId,
            read.sourceRevision,
            read.recipe,
          );
          state.undo = [];
          state.redo = [];
        }
      }
      if (
        !state.pending &&
        read.recipe &&
        observedSourceRevision(read) !== undefined &&
        read.recipe.sourceRevision !== observedSourceRevision(read)
      ) {
        state.conflict = true;
        state.note =
          "The saved edit belongs to an earlier Original. Explicitly rebind it to continue.";
      }
      return state;
    }
    state = {
      read,
      draft: draftFromRecipe(photoId, read.sourceRevision, read.recipe),
      saving: false,
      uncertain: false,
      conflict: false,
      failure: false,
      recovered: false,
      recoveryAvailable: storageAvailable,
      note: "",
      pending: undefined,
      undo: [],
      redo: [],
    };
    if (
      read.recipe &&
      observedSourceRevision(read) !== undefined &&
      read.recipe.sourceRevision !== observedSourceRevision(read)
    ) {
      state.conflict = true;
      state.note =
        "The saved edit belongs to an earlier Original. Explicitly rebind it to continue.";
    }
    const item = recovery[photoId];
    if (isRecord(item) && isRecord(item["draft"])) {
      const draft = item["draft"];
      const parsed = parseComposableRecipe(
        {
          photoId,
          sourceRevision: draft["sourceRevision"],
          recipe: { ...draft, revision: "local" },
        },
        photoId,
      );
      if (
        parsed?.recipe &&
        draft["photoId"] === photoId &&
        (draft["baseRevision"] === null ||
          typeof draft["baseRevision"] === "string")
      ) {
        state.draft = {
          ...draftFromRecipe(photoId, parsed.sourceRevision, parsed.recipe),
          baseRevision: draft["baseRevision"],
        };
        state.recovered = true;
        state.conflict =
          state.conflict ||
          state.draft.baseRevision !== (read.recipe?.revision ?? null) ||
          (observedSourceRevision(read) !== undefined &&
            state.draft.sourceRevision !== observedSourceRevision(read));
        state.note =
          "Recovered local draft; these settings are not yet confirmed by the service.";
        const pending = item["pending"];
        if (
          isRecord(pending) &&
          typeof pending["body"] === "string" &&
          isRecord(pending["draft"]) &&
          typeof pending["rebind"] === "boolean"
        ) {
          try {
            const request: unknown = JSON.parse(pending["body"]);
            if (
              isRecord(request) &&
              typeof request["requestId"] === "string" &&
              (typeof request["expectedSourceRevision"] === "string" ||
                typeof request["newSourceRevision"] === "string")
            ) {
              const captured = pending["draft"];
              const captureRead = parseComposableRecipe(
                {
                  photoId,
                  sourceRevision: captured["sourceRevision"],
                  recipe: { ...captured, revision: "local" },
                },
                photoId,
              );
              if (captureRead?.recipe) {
                state.pending = {
                  body: pending["body"],
                  draft: draftFromRecipe(
                    photoId,
                    captureRead.sourceRevision,
                    captureRead.recipe,
                  ),
                  rebind: pending["rebind"],
                };
                state.uncertain = true;
                state.conflict = false;
              }
              const recoveredPending = state.pending;
              if (
                state.read.recipe &&
                recoveredPending &&
                sameSettings(state.read.recipe, recoveredPending.draft) &&
                state.read.recipe.sourceRevision ===
                  observedSourceRevision(state.read)
              ) {
                state.draft = {
                  ...state.draft,
                  steps: canonicalSettings(state.draft).steps,
                  baseRevision: state.read.recipe.revision,
                };
                state.pending = undefined;
                state.uncertain = false;
                state.recovered = false;
                state.note = "Recovered the previously saved edit.";
              }
            }
          } catch {
            state.conflict = true;
          }
        }
      }
    }
    photos.set(photoId, state);
    persist(photoId, state);
    if (state.recovered && !state.uncertain && !state.conflict)
      queueMicrotask(() => {
        void flush(photoId);
      });
    return state;
  };
  const submit = async (
    photoId: string,
    state: ComposableAutosaveState,
  ): Promise<void> => {
    const pending = state.pending;
    if (!pending || state.saving) return;
    state.saving = true;
    state.note = state.uncertain
      ? "Checking the previous save…"
      : "Saving edit…";
    notify(photoId, state);
    let response: Response;
    let value: unknown;
    try {
      response = await saveComposableRecipeBody(
        fetcher,
        photoId,
        pending.body,
        pending.rebind,
      );
      value = await response.json();
    } catch {
      state.saving = false;
      state.uncertain = true;
      state.note =
        "The save outcome is unknown. Check its result before further saves or exports.";
      notify(photoId, state);
      return;
    }
    state.saving = false;
    const request = JSON.parse(pending.body) as Record<string, unknown>;
    const parsed = parseComposableRecipe(
      {
        photoId,
        sourceRevision: isRecord(value) ? value["sourceRevision"] : undefined,
        recipe: isRecord(value) ? value["recipe"] : undefined,
      },
      photoId,
    );
    const saved = parsed?.recipe;
    if (
      response.ok &&
      saved &&
      isRecord(value) &&
      ["saved", "replayed", "unchanged", "rebound"].includes(
        String(value["outcome"]),
      ) &&
      value["recipeVersion"] === saved.revision &&
      saved.photoId === photoId &&
      parsed.sourceRevision ===
        (pending.rebind
          ? request["newSourceRevision"]
          : request["expectedSourceRevision"]) &&
      saved.sourceRevision === parsed.sourceRevision &&
      sameSettings(saved, pending.draft)
    ) {
      state.read = { ...state.read, ...parsed };
      state.pending = undefined;
      state.uncertain = false;
      state.failure = false;
      state.recovered = false;
      state.draft = {
        ...state.draft,
        steps: canonicalSettings(state.draft).steps,
        baseRevision: saved.revision,
      };
      state.note = sameSettings(state.draft, saved)
        ? "Saved edit."
        : "Saving newer settings…";
      notify(photoId, state);
      if (composableDraftDiffers(state.draft, saved)) await flush(photoId);
      return;
    }
    const error =
      isRecord(value) && isRecord(value["error"]) ? value["error"] : undefined;
    const code = error?.["code"];
    if (
      response.ok ||
      response.status >= 500 ||
      code === "outcome_unknown" ||
      code === "receipt_expired"
    ) {
      state.uncertain = true;
      state.note =
        code === "receipt_expired"
          ? "The previous save can no longer be reconciled because its receipt expired. Local settings are retained; dependent saves and exports remain blocked."
          : "The save outcome is unknown. Check its result before further saves or exports.";
    } else {
      state.pending = undefined;
      state.uncertain = false;
      state.failure = true;
      state.conflict = [
        "recipe_conflict",
        "source_changed",
        "requires_rebind",
        "request_conflict",
      ].includes(String(code));
      state.note = state.conflict
        ? "The saved edit or Original changed. Use the saved edit or explicitly reapply your local settings."
        : `The edit could not be saved${typeof error?.["message"] === "string" ? `: ${error["message"]}` : "."}`;
      if (state.conflict) {
        const latest = await fetchComposableRecipe(
          fetcher,
          photoId,
          new AbortController().signal,
        );
        if (latest) state.read = latest;
      }
    }
    notify(photoId, state);
  };
  const flush = async (photoId: string): Promise<void> => {
    const state = photos.get(photoId);
    if (!state || state.saving) return;
    if (state.pending) {
      await submit(photoId, state);
      return;
    }
    if (state.conflict) return;
    if (!composableDraftDiffers(state.draft, state.read.recipe)) return;
    const guarded = composableSaveRequest(
      state.draft,
      `web-recipe-${randomUuid()}`,
    );
    if (guarded.kind === "refused") {
      state.failure = true;
      state.note = guarded.note;
      notify(photoId, state);
      return;
    }
    state.pending = {
      body: JSON.stringify(guarded.request),
      draft: structuredClone(state.draft),
      rebind: false,
    };
    await submit(photoId, state);
  };
  const change = async (
    photoId: string,
    draft: ComposableRecipeDraft,
  ): Promise<void> => {
    const state = photos.get(photoId);
    if (!state || sameSettings(state.draft, draft)) return;
    state.undo.push(structuredClone(state.draft));
    if (state.undo.length > 64) state.undo.shift();
    state.redo = [];
    state.draft = structuredClone(draft);
    state.failure = false;
    notify(photoId, state);
    if (!state.uncertain) await flush(photoId);
  };
  const history = async (photoId: string, redo: boolean): Promise<void> => {
    const state = photos.get(photoId);
    if (!state) return;
    const next = (redo ? state.redo : state.undo).pop();
    if (!next) return;
    (redo ? state.undo : state.redo).push(state.draft);
    state.draft = { ...next, baseRevision: state.draft.baseRevision };
    notify(photoId, state);
    if (!state.uncertain) await flush(photoId);
  };
  const useSaved = (photoId: string) => {
    const state = photos.get(photoId);
    if (!state || state.pending) return;
    state.draft = draftFromRecipe(
      photoId,
      state.read.sourceRevision,
      state.read.recipe,
    );
    state.conflict = Boolean(
      state.read.recipe &&
        observedSourceRevision(state.read) !== undefined &&
        state.read.recipe.sourceRevision !== observedSourceRevision(state.read),
    );
    state.failure = false;
    state.recovered = false;
    state.undo = [];
    state.redo = [];
    state.note = state.conflict
      ? "Using saved edit. The Original changed; explicitly rebind it before editing."
      : "Using saved edit.";
    notify(photoId, state);
  };
  const reapply = async (photoId: string) => {
    const state = photos.get(photoId);
    if (!state || state.pending || state.saving) return;
    const latest = await fetchComposableRecipe(
      fetcher,
      photoId,
      new AbortController().signal,
    );
    if (!latest) {
      state.note =
        "The saved edit could not be checked. Local settings are retained.";
      notify(photoId, state);
      return;
    }
    if (state.pending || state.saving) return;
    state.read = latest;
    if (
      observedSourceRevision(latest) !== undefined &&
      state.draft.sourceRevision !== observedSourceRevision(latest)
    ) {
      state.conflict = true;
      state.note =
        "The Original changed. Explicitly rebind the saved edit before reapplying settings.";
      notify(photoId, state);
      return;
    }
    state.draft = {
      ...state.draft,
      baseRevision: latest.recipe?.revision ?? null,
    };
    state.conflict = false;
    state.failure = false;
    await flush(photoId);
  };
  const rebind = async (photoId: string) => {
    const state = photos.get(photoId);
    if (!state || state.pending || state.saving || !state.read.recipe) return;
    const recipe = state.read.recipe;
    const sourceRevision =
      state.read.currentSourceRevision ?? state.read.sourceRevision;
    if (state.read.sourceAvailable === false || !sourceRevision) {
      state.note =
        "The current Original revision is unavailable. The saved edit and local settings are retained.";
      notify(photoId, state);
      return;
    }
    const rebound = {
      ...draftFromRecipe(photoId, sourceRevision, recipe),
      sourceRevision,
      steps: recipe.steps.map((step) =>
        step.input.kind === "original"
          ? { ...step, input: { ...step.input, sourceRevision } }
          : step,
      ),
    };
    state.draft = {
      ...state.draft,
      sourceRevision,
      steps: state.draft.steps.map((step) =>
        step.input.kind === "original"
          ? { ...step, input: { ...step.input, sourceRevision } }
          : step,
      ),
    };
    state.conflict = false;
    state.pending = {
      body: JSON.stringify({
        requestId: `web-rebind-${randomUuid()}`,
        expectedRecipeRevision: recipe.revision,
        newSourceRevision: sourceRevision,
      }),
      draft: rebound,
      rebind: true,
    };
    await submit(photoId, state);
  };
  return {
    open,
    get: (photoId: string) => photos.get(photoId),
    change,
    flush,
    undo: (photoId: string) => history(photoId, false),
    redo: (photoId: string) => history(photoId, true),
    useSaved,
    reapply,
    rebind,
  };
}
