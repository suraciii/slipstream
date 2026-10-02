//! The caller-controlled draft of one Photo's composable Processing Recipe:
//! how the workspace adds, removes, selects, binds, and re-parameters zero or
//! more Processing Steps, and which guards a save must pass before it is
//! submitted. The draft owns no network or DOM work, so the whole editing
//! reading can be checked without a browser.
//!
//! [Composable Photo Processing Modules](../../../../../../design/processing-modules.md)
//! makes the caller the composer: each step is one module, one explicit input
//! binding, and one complete module-owned parameter snapshot, and a recipe
//! with steps selects exactly one current step. Nothing here invents a
//! predecessor, merges module schemas, converts a binding, or completes a
//! guard the caller did not observe.

import type {
  ComposableProcessingStep,
  ComposableRecipe,
  ComposableRecipeInput,
} from "../api/composable-recipe.js";
import type { ProcessingModuleDescription } from "../api/processing-modules.js";
import { isRecord } from "../api/editor.js";
import {
  processingArtifactInput,
  type ProcessingArtifactRecord,
} from "./processing-artifact.js";

/// The published bound of one recipe's step list, mirroring the service's
/// own admission bound.
const MAXIMUM_RECIPE_STEPS = 64;

/// One editable Processing Step: a stable opaque identity unique within the
/// recipe, one selected module, one explicit input binding, and one complete
/// module-owned parameter snapshot.
export type ComposableStepDraft = Readonly<{
  stepId: string;
  module: string;
  input: ComposableRecipeInput;
  parameters: Readonly<{ schemaVersion: string; tree: unknown }>;
}>;

/// One Photo's editable composable recipe. `baseRevision` is the saved
/// recipe's committed revision the draft edits, or `null` when no composable
/// recipe is saved yet. `retiredStepIds` keeps the identities this editing
/// session already used, so a removed step's identity is never handed to a
/// new step while the session holds it.
export type ComposableRecipeDraft = Readonly<{
  photoId: string;
  baseRevision: string | null;
  sourceRevision: string;
  steps: ReadonlyArray<ComposableStepDraft>;
  currentStepId: string | null;
  retiredStepIds: ReadonlyArray<string>;
}>;

/// One module's caller-facing editing facts, derived from discovery: the
/// pinned name, its independent availability, the parameter versions the
/// module owns, and the complete initial parameter tree derived from the
/// module's own published schema. Nothing is invented: consts, nested
/// objects, and empty arrays come from the schema document itself.
export type ComposableModuleChoice = Readonly<{
  name: string;
  ready: boolean;
  refusalNote: string;
  parameterVersions: ReadonlyArray<string>;
  defaultTree: Record<string, unknown>;
}>;

/// The module's own default parameter tree, derived from its published
/// schema: each declared property contributes its pinned `const`, its nested
/// object shape, or an empty array. An underdescribed schema yields `{}`,
/// which the owning module's admission still judges.
const defaultTreeFromSchema = (schema: unknown): Record<string, unknown> => {
  if (!isRecord(schema) || schema["type"] !== "object") return {};
  const properties = isRecord(schema["properties"]) ? schema["properties"] : {};
  const tree: Record<string, unknown> = {};
  for (const [name, property] of Object.entries(properties)) {
    if (!isRecord(property)) continue;
    if ("const" in property && property["const"] !== undefined) {
      tree[name] = property["const"];
    } else if (property["type"] === "object") {
      tree[name] = defaultTreeFromSchema(property);
    } else if (property["type"] === "array") {
      tree[name] = [];
    }
  }
  return tree;
};

/// The peer-module choices discovery offers this session. Each entry keeps
/// its own availability reading; one module's state never derives from the
/// other's.
export const composableModuleChoices = (
  modules: ReadonlyArray<ProcessingModuleDescription>,
): ReadonlyArray<ComposableModuleChoice> =>
  Object.freeze(
    modules.map((module) =>
      Object.freeze({
        name: module.id.name,
        ready: module.availability.state === "ready",
        refusalNote: module.availability.refusalReasons.join(", "),
        parameterVersions: module.parameterVersions,
        defaultTree: defaultTreeFromSchema(module.parameterSchema),
      }),
    ),
  );

/// Starts the editable draft of one Photo's composable recipe from the
/// settled read: the saved recipe's steps, selection, and committed revision,
/// or an empty zero-step draft when none is saved.
export const draftFromRecipe = (
  photoId: string,
  sourceRevision: string,
  recipe: ComposableRecipe | null,
): ComposableRecipeDraft => ({
  photoId,
  baseRevision: recipe?.revision ?? null,
  sourceRevision,
  steps: recipe ? [...recipe.steps] : [],
  currentStepId: recipe?.currentStepId ?? null,
  retiredStepIds: [],
});

/// The next stable step identity for one module: the module's own name and
/// the first positive suffix neither the recipe's steps nor this session's
/// retired identities already use.
export const nextComposableStepId = (
  draft: ComposableRecipeDraft,
  module: string,
): string => {
  const used = new Set([
    ...draft.steps.map((step) => step.stepId),
    ...draft.retiredStepIds,
  ]);
  for (let index = 1; index <= MAXIMUM_RECIPE_STEPS + 1; index += 1) {
    const candidate = `${module}-${index}`;
    if (!used.has(candidate)) return candidate;
  }
  return `${module}-${Date.now()}`;
};

const withSteps = (
  draft: ComposableRecipeDraft,
  steps: ReadonlyArray<ComposableStepDraft>,
  currentStepId: string | null,
): ComposableRecipeDraft => ({
  ...draft,
  steps,
  currentStepId:
    currentStepId === null && steps.length > 0
      ? steps[0]!.stepId
      : currentStepId,
});

/// Adds one complete Processing Step and selects it as the current step: the
/// caller just chose it, so the recipe's selection names the new work.
export const addComposableStep = (
  draft: ComposableRecipeDraft,
  step: ComposableStepDraft,
): ComposableRecipeDraft => ({
  ...draft,
  steps: [...draft.steps, step],
  currentStepId: step.stepId,
});

export const removeComposableStep = (
  draft: ComposableRecipeDraft,
  stepId: string,
): ComposableRecipeDraft => {
  if (!draft.steps.some((step) => step.stepId === stepId)) return draft;
  const steps = draft.steps.filter((step) => step.stepId !== stepId);
  return {
    ...withSteps(
      draft,
      steps,
      draft.currentStepId === stepId
        ? (steps[0]?.stepId ?? null)
        : draft.currentStepId,
    ),
    retiredStepIds: [...draft.retiredStepIds, stepId],
  };
};

/// Selects one of the recipe's own steps as the current step. A selection
/// that names no step of the recipe changes nothing.
export const selectComposableStep = (
  draft: ComposableRecipeDraft,
  stepId: string,
): ComposableRecipeDraft =>
  draft.steps.some((step) => step.stepId === stepId)
    ? { ...draft, currentStepId: stepId }
    : draft;

/// Rebinds one step's explicit input. The binding is the caller's own
/// choice: the guarded Original of this Photo at the observed source
/// revision, or one published immutable Processing Artifact with the concrete
/// contract its record carries.
export const setComposableStepInput = (
  draft: ComposableRecipeDraft,
  stepId: string,
  input: ComposableRecipeInput,
): ComposableRecipeDraft => ({
  ...draft,
  steps: draft.steps.map((step) =>
    step.stepId === stepId ? { ...step, input } : step,
  ),
});

/// Replaces one step's complete module-owned parameter snapshot. The tree is
/// preserved verbatim for the owning module; it is never flattened, merged,
/// or completed here.
export const setComposableStepParameters = (
  draft: ComposableRecipeDraft,
  stepId: string,
  parameters: Readonly<{ schemaVersion: string; tree: unknown }>,
): ComposableRecipeDraft => ({
  ...draft,
  steps: draft.steps.map((step) =>
    step.stepId === stepId ? { ...step, parameters } : step,
  ),
});

/// Whether the draft differs from the saved recipe it edits. A clean draft
/// may preview and export under the saved recipe's committed identity; a
/// draft that differs must be saved first.
export const composableDraftDiffers = (
  draft: ComposableRecipeDraft,
  recipe: ComposableRecipe | null,
): boolean => {
  if (!recipe) return draft.steps.length > 0 || draft.currentStepId !== null;
  if (
    draft.baseRevision !== recipe.revision ||
    draft.currentStepId !== recipe.currentStepId ||
    draft.steps.length !== recipe.steps.length
  )
    return true;
  return draft.steps.some((step, index) => {
    const saved = recipe.steps[index]!;
    return (
      step.stepId !== saved.stepId ||
      step.module !== saved.module ||
      step.parameters.schemaVersion !== saved.parameters.schemaVersion ||
      JSON.stringify(step.parameters.tree) !==
        JSON.stringify(saved.parameters.tree) ||
      JSON.stringify(step.input) !== JSON.stringify(saved.input)
    );
  });
};

const contractShapeRefusal = (contract: Record<string, unknown>): string => {
  const geometry = contract["geometry"];
  if (
    typeof contract["format"] !== "string" ||
    typeof contract["precision"] !== "string" ||
    typeof contract["colorSpace"] !== "string" ||
    typeof contract["transfer"] !== "string" ||
    typeof contract["encoding"] !== "string" ||
    !isRecord(geometry) ||
    !Number.isInteger(geometry["width"]) ||
    !Number.isInteger(geometry["height"]) ||
    (geometry["width"] as number) <= 0 ||
    (geometry["height"] as number) <= 0
  )
    return "The artifact input does not carry a complete image contract.";
  return "";
};

/// The save request the workspace may submit, or the workspace-language
/// refusal that names the guard that failed. Every guard mirrors the
/// service's own admission order, so a refused save never leaves the caller
/// guessing which side refused it.
export type ComposableSaveRequest =
  | Readonly<{
      kind: "ok";
      request: Readonly<{
        requestId: string;
        expectedRecipeRevision: string | null;
        expectedSourceRevision: string;
        currentStepId: string | null;
        steps: ReadonlyArray<ComposableProcessingStep>;
      }>;
    }>
  | Readonly<{ kind: "refused"; note: string }>;

export const composableSaveRequest = (
  draft: ComposableRecipeDraft,
  requestId: string,
  modules: ReadonlyArray<ComposableModuleChoice>,
): ComposableSaveRequest => {
  const refused = (note: string): ComposableSaveRequest => ({
    kind: "refused",
    note,
  });
  if (!draft.sourceRevision)
    return refused(
      "The current source revision is not available, so the Processing Recipe cannot be saved. Reload to check again.",
    );
  if (modules.length === 0)
    return refused(
      "Module discovery is not available right now, so the Processing Recipe cannot be saved. Reload to check again.",
    );
  if (draft.steps.length > MAXIMUM_RECIPE_STEPS)
    return refused(
      `A Processing Recipe carries at most ${MAXIMUM_RECIPE_STEPS} steps.`,
    );
  const byName = new Map(modules.map((module) => [module.name, module]));
  const seen = new Set<string>();
  for (const step of draft.steps) {
    if (seen.has(step.stepId))
      return refused(
        `The step identity ${step.stepId} is used twice; each step needs its own identity.`,
      );
    seen.add(step.stepId);
    const module = byName.get(step.module);
    if (!module)
      return refused(
        `The step ${step.stepId} names the module ${step.module}, which this deployment does not know.`,
      );
    if (!module.parameterVersions.includes(step.parameters.schemaVersion))
      return refused(
        `The module ${step.module} does not admit the parameter version ${step.parameters.schemaVersion}. Choose a version discovery reports.`,
      );
    if (!isRecord(step.parameters.tree))
      return refused(
        `The parameters of the step ${step.stepId} are not one JSON object.`,
      );
    if (step.input.kind === "original") {
      if (
        step.input.photoId !== draft.photoId ||
        step.input.sourceRevision !== draft.sourceRevision
      )
        return refused(
          `The Original input of the step ${step.stepId} is bound to an earlier revision of this Photo. Reload to check again.`,
        );
    } else {
      if (!step.input.artifactId)
        return refused(
          `The step ${step.stepId} needs one explicitly selected Processing Artifact as its input.`,
        );
      const refusal = contractShapeRefusal(step.input.contract);
      if (refusal) return refused(`The step ${step.stepId}: ${refusal}`);
    }
  }
  if (draft.currentStepId === null && draft.steps.length > 0)
    return refused(
      "Select the recipe's current Processing Step before saving.",
    );
  if (
    draft.currentStepId !== null &&
    !draft.steps.some((step) => step.stepId === draft.currentStepId)
  )
    return refused(
      "The selected current Processing Step is not part of this recipe.",
    );
  return {
    kind: "ok",
    request: {
      requestId,
      expectedRecipeRevision: draft.baseRevision,
      expectedSourceRevision: draft.sourceRevision,
      currentStepId: draft.currentStepId,
      steps: draft.steps,
    },
  };
};

/// The explicit artifact input binding of one published immutable Processing
/// Artifact: its identity plus the concrete image contract its record
/// validated. No implicit "latest result" binding exists.
export const artifactStepInput = (
  artifact: ProcessingArtifactRecord,
): ComposableRecipeInput => processingArtifactInput(artifact);
