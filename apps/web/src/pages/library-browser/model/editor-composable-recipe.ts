import {
  fetchComposableRecipe,
  parseComposableRecipe,
  saveComposableRecipe,
} from "../api/composable-recipe.js";
import {
  fetchProcessingModules,
  type ProcessingModuleDescription,
} from "../api/processing-modules.js";
import {
  addComposableStep,
  composableDraftDiffers,
  composableModuleChoices,
  composableSaveRequest,
  draftFromRecipe,
  nextComposableStepId,
  removeComposableStep,
  selectComposableStep,
  setComposableStepInput,
  setComposableStepParameters,
  type ComposableModuleChoice,
  type ComposableRecipeDraft,
} from "./composable-recipe-draft.js";
import {
  composablePreviewTarget,
  selectedComposableStep,
  type ComposableRecipeRead,
} from "./composable-preview.js";
import {
  describeProcessingArtifact,
  parseProcessingArtifactRecord,
  processingArtifactInput,
  type ProcessingArtifactRecord,
} from "./processing-artifact.js";
import { isRecord } from "../api/editor.js";
import { formatByteCount } from "./editor-presentation.js";
import { randomUuid } from "./browser-crypto.js";
import type { BrowserFetch } from "./access-session.js";
import type { EditorControllerDependencies } from "./editor-controller-contract.js";
import type { LibraryBrowserView } from "../ui/library-browser-view.js";
type ComposableSaveRequest = Parameters<typeof saveComposableRecipe>[2];
export function createEditorComposableRecipe(
  fetcher: BrowserFetch,
  dependencies: EditorControllerDependencies &
    Readonly<{
      editorOwnsPhoto: (photoId: string) => boolean;
      renderEditor: () => void;
      stage: () => string;
      previewSelection: () => string | null | undefined;
      clearEditorPreview: () => void;
      requestEditorPreview: (photoId: string) => Promise<void>;
      markEditorPreviewStale: () => void;
      describeEditRefusal: (
        response: Response,
        subject: string,
      ) => Promise<string>;
    }>,
) {
  const {
    currentPhoto,
    editorOwnsPhoto,
    renderEditor,
    stage,
    previewSelection,
    clearEditorPreview,
    requestEditorPreview,
    markEditorPreviewStale,
    describeEditRefusal,
  } = dependencies;
  let editorComposable: ComposableRecipeRead | undefined;
  let editorComposableAbort: AbortController | undefined;
  let editorComposableGeneration = 0;
  /// True while this scope's composable recipe read is in flight. The
  /// opening preview request waits for it, so a valid current step can
  /// retarget the preview before any legacy bytes are fetched.
  let editorComposableReadPending = false;
  /// The caller-controlled draft of this Photo's composable Processing
  /// Recipe. Present exactly while the workspace is composing: it starts
  /// from the settled read, edits locally, and saves through the guarded
  /// route. While it differs from the saved recipe, the selected step's
  /// Preview is not requested — the route previews the saved recipe, so the
  /// workspace never presents another caller's intent as this one's result.
  let editorComposableDraft: ComposableRecipeDraft | undefined;
  /// The step whose parameters and input the step editor is editing.
  let editorComposableEditingStepId: string | undefined;
  /// The module parameter text as the caller typed it. The draft only holds
  /// trees that parsed; the text may be invalid JSON, which blocks the save
  /// and is explained in its own note.
  let editorComposableParametersText = "";
  let editorComposableParametersValid = true;
  /// The input binding choice the step editor offers: the guarded Original
  /// or one explicitly selected retained Processing Artifact.
  let editorComposableInputChoice: "original" | "artifact" = "original";
  let editorComposableArtifactChoice = "";
  let editorComposableNote = "";
  let editorComposableSaving = false;
  /// The last submitted save, reusable verbatim until its outcome is
  /// reconciled: a lost response is resolved by resubmitting the identical
  /// body under the same request identity.
  let editorComposableSubmission:
    | Readonly<{
        photoId: string;
        requestId: string;
        body: string;
      }>
    | undefined;
  /// Retained Processing Artifacts of this session: every artifact this
  /// workspace inspected, offered as the only explicit downstream inputs.
  /// Nothing chains automatically; the caller selects one per step.
  let editorArtifacts: ReadonlyArray<ProcessingArtifactRecord> = [];
  let processingModules: ReadonlyArray<ProcessingModuleDescription> = [];
  let processingModulesGeneration = 0;
  let editorScopeGeneration = 0;
  const ownsEditorScope = (photoId: string, generation: number): boolean =>
    generation === editorScopeGeneration && editorOwnsPhoto(photoId);
  const retainArtifact = (artifact: ProcessingArtifactRecord): void => {
    if (
      !editorArtifacts.some((item) => item.artifactId === artifact.artifactId)
    )
      editorArtifacts = [...editorArtifacts, artifact];
  };
  const loadComposableRecipe = async (photoId: string): Promise<void> => {
    const generation = ++editorComposableGeneration;
    editorComposableReadPending = true;
    const controller = new AbortController();
    editorComposableAbort?.abort();
    editorComposableAbort = controller;
    const read = await fetchComposableRecipe(
      fetcher,
      photoId,
      controller.signal,
    );
    if (generation !== editorComposableGeneration || !editorOwnsPhoto(photoId))
      return;
    editorComposableReadPending = false;
    if (read !== undefined) {
      const selection = selectedComposableStep(read)?.stepId ?? null;
      if (previewSelection() !== undefined && previewSelection() !== selection)
        clearEditorPreview();
      editorComposable = read;
    }
    if (currentPhoto()?.id === photoId) renderEditor();
    if (stage() !== "camera") void requestEditorPreview(photoId);
  };
  /// The guarded source revision one already-submitted save body carried, so
  /// a replay's confirmation is checked against exactly what was sent.
  const readSubmittedSourceRevision = (body: string): string => {
    const parsed: unknown = JSON.parse(body);
    return isRecord(parsed) &&
      typeof parsed["expectedSourceRevision"] === "string"
      ? parsed["expectedSourceRevision"]
      : "";
  };
  /// The module choices discovery currently offers this session, as the
  /// draft's own save guards and step editor read them.
  const composableChoices = (): ReadonlyArray<ComposableModuleChoice> =>
    composableModuleChoices(processingModules);
  /// The preview request's binding for the composable read in force.
  const composableTarget = () => composablePreviewTarget(editorComposable);
  /// Whether the local draft differs from the saved recipe it edits. A
  /// dirty draft never previews or exports: the routes serve the saved
  /// recipe, and this workspace does not present one caller's intent as
  /// another's result.
  const composableDirty = (): boolean =>
    editorComposableDraft !== undefined &&
    composableDraftDiffers(
      editorComposableDraft,
      editorComposable?.recipe ?? null,
    );
  /// Composable mode owns the workspace while a composable recipe is saved
  /// or a draft is in progress. Photos with no saved recipe keep the
  /// two-control compatibility path until the caller starts composing.
  const composableMode = (): boolean =>
    (editorComposable !== undefined && editorComposable.recipe !== null) ||
    editorComposableDraft !== undefined;
  /// The draft the composable surface edits: the caller's in-progress draft
  /// when one exists, otherwise the saved recipe's own shape. The first
  /// composing gesture materializes the draft so every later edit is local
  /// until it is saved.
  const composableEffectiveDraft = (): ComposableRecipeDraft | undefined => {
    if (editorComposableDraft) return editorComposableDraft;
    if (editorComposable === undefined) return undefined;
    return draftFromRecipe(
      editorComposable.recipe?.photoId ?? currentPhoto()?.id ?? "",
      editorComposable.sourceRevision,
      editorComposable.recipe,
    );
  };
  const materializeComposableDraft = (
    photoId: string,
  ): ComposableRecipeDraft | undefined => {
    if (editorComposableDraft) return editorComposableDraft;
    const draft = composableEffectiveDraft();
    if (!draft || draft.photoId !== photoId) return undefined;
    editorComposableDraft = draft;
    return draft;
  };
  /// Starts composing. A Photo with no saved recipe begins from its own
  /// zero-step draft; a saved recipe begins from its committed shape.
  const composeEditorSteps = (photoId: string): void => {
    if (!editorOwnsPhoto(photoId)) return;
    if (editorComposable === undefined) {
      editorComposableNote =
        "The Processing Recipe could not be read, so composing waits. Reload to check again.";
      renderEditor();
      return;
    }
    editorComposableNote = "";
    materializeComposableDraft(photoId);
    renderEditor();
  };
  /// Adds one Processing Step for a module discovery reports, with the
  /// module's own schema-derived default parameter tree, and opens it in
  /// the step editor. The input choice starts where the module admits it:
  /// darktable over this Photo's guarded Original, and standalone
  /// SpektraFilm over an artifact the caller still has to select.
  const addEditorComposableStep = (photoId: string, module: string): void => {
    if (!editorOwnsPhoto(photoId)) return;
    const draft = materializeComposableDraft(photoId);
    const choice = composableChoices().find((item) => item.name === module);
    if (!draft || !choice) {
      editorComposableNote =
        "Module discovery is not available right now. Reload to check again.";
      renderEditor();
      return;
    }
    const stepId = nextComposableStepId(draft, module);
    editorComposableDraft = addComposableStep(draft, {
      stepId,
      module,
      input: {
        kind: "original",
        photoId,
        sourceRevision: draft.sourceRevision,
      },
      parameters: {
        schemaVersion: choice.parameterVersions[0] ?? "",
        tree: structuredClone(choice.defaultTree),
      },
    });
    editorComposableEditingStepId = stepId;
    editorComposableParametersText = JSON.stringify(
      structuredClone(choice.defaultTree),
      null,
      2,
    );
    editorComposableParametersValid = true;
    editorComposableInputChoice =
      module === "darktable" ? "original" : "artifact";
    editorComposableArtifactChoice = "";
    editorComposableNote = "";
    markEditorPreviewStale();
    renderEditor();
  };
  const removeEditorComposableStep = (
    photoId: string,
    stepId: string,
  ): void => {
    if (!editorOwnsPhoto(photoId)) return;
    const draft = materializeComposableDraft(photoId);
    if (!draft) return;
    editorComposableDraft = removeComposableStep(draft, stepId);
    if (editorComposableEditingStepId === stepId) {
      editorComposableEditingStepId = editorComposableDraft.steps[0]?.stepId;
      const editing = editorComposableDraft.steps.find(
        (step) => step.stepId === editorComposableEditingStepId,
      );
      editorComposableParametersText = editing
        ? JSON.stringify(editing.parameters.tree, null, 2)
        : "";
      editorComposableParametersValid = true;
    }
    markEditorPreviewStale();
    renderEditor();
  };
  /// Selects the recipe's current Processing Step. The selection is part of
  /// the guarded save, so it edits the draft like any other change.
  const selectEditorComposableStep = (
    photoId: string,
    stepId: string,
  ): void => {
    if (!editorOwnsPhoto(photoId)) return;
    const draft = materializeComposableDraft(photoId);
    if (!draft) return;
    editorComposableDraft = selectComposableStep(draft, stepId);
    markEditorPreviewStale();
    renderEditor();
  };
  /// Opens one step in the step editor: its own parameters text and its own
  /// input binding choice.
  const editEditorComposableStep = (photoId: string, stepId: string): void => {
    if (!editorOwnsPhoto(photoId)) return;
    const draft = materializeComposableDraft(photoId);
    const step = draft?.steps.find((item) => item.stepId === stepId);
    if (!draft || !step) return;
    editorComposableEditingStepId = stepId;
    editorComposableParametersText = JSON.stringify(
      step.parameters.tree,
      null,
      2,
    );
    editorComposableParametersValid = true;
    editorComposableInputChoice =
      step.input.kind === "artifact" ? "artifact" : "original";
    editorComposableArtifactChoice =
      step.input.kind === "artifact" ? step.input.artifactId : "";
    renderEditor();
  };
  /// The parameter text as the caller typed it. Only a tree that parses as
  /// one JSON object reaches the draft; invalid text keeps the last parsed
  /// tree, blocks the save, and is named in its own note.
  const editEditorComposableParameters = (
    photoId: string,
    text: string,
  ): void => {
    if (!editorOwnsPhoto(photoId)) return;
    editorComposableParametersText = text;
    const draft = editorComposableDraft;
    const stepId = editorComposableEditingStepId;
    if (!draft || !stepId) {
      editorComposableParametersValid = true;
      renderEditor();
      return;
    }
    let tree: unknown;
    try {
      tree = JSON.parse(text);
    } catch {
      editorComposableParametersValid = false;
      renderEditor();
      return;
    }
    editorComposableParametersValid =
      typeof tree === "object" && tree !== null && !Array.isArray(tree);
    if (editorComposableParametersValid) {
      editorComposableDraft = setComposableStepParameters(draft, stepId, {
        schemaVersion:
          draft.steps.find((step) => step.stepId === stepId)?.parameters
            .schemaVersion ?? "",
        tree,
      });
      markEditorPreviewStale();
    }
    renderEditor();
  };
  /// Chooses the module parameter version the step's tree is written for.
  /// The version comes from discovery; the tree is the caller's own.
  const editEditorComposableSchema = (
    photoId: string,
    schemaVersion: string,
  ): void => {
    if (!editorOwnsPhoto(photoId)) return;
    const draft = editorComposableDraft;
    const stepId = editorComposableEditingStepId;
    if (!draft || !stepId) return;
    const step = draft.steps.find((item) => item.stepId === stepId);
    if (!step) return;
    editorComposableDraft = setComposableStepParameters(draft, stepId, {
      schemaVersion,
      tree: step.parameters.tree,
    });
    markEditorPreviewStale();
    renderEditor();
  };
  /// Chooses the step's explicit input binding: this Photo's guarded
  /// Original at the observed source revision, or one retained Processing
  /// Artifact the caller selected. No binding is ever implied.
  const editEditorComposableInput = (
    photoId: string,
    choice: "original" | "artifact",
    artifactId = "",
  ): void => {
    if (!editorOwnsPhoto(photoId)) return;
    const draft = editorComposableDraft;
    const stepId = editorComposableEditingStepId;
    if (!draft || !stepId) return;
    editorComposableInputChoice = choice;
    if (choice === "original") {
      editorComposableArtifactChoice = "";
      editorComposableDraft = setComposableStepInput(draft, stepId, {
        kind: "original",
        photoId,
        sourceRevision: draft.sourceRevision,
      });
      markEditorPreviewStale();
      renderEditor();
      return;
    }
    const artifact = editorArtifacts.find(
      (item) => item.artifactId === artifactId,
    );
    if (!artifact) {
      editorComposableNote =
        "Select one retained Processing Artifact, or fetch one by its identity first.";
      renderEditor();
      return;
    }
    editorComposableArtifactChoice = artifactId;
    editorComposableDraft = setComposableStepInput(
      draft,
      stepId,
      processingArtifactInput(artifact),
    );
    editorComposableNote = "";
    markEditorPreviewStale();
    renderEditor();
  };
  /// Fetches one Processing Artifact's provenance by the caller-held
  /// identity, so a retained artifact from an earlier session can still be
  /// selected explicitly. An unknown or malformed record is refused without
  /// touching the draft.
  const fetchEditorArtifact = async (
    photoId: string,
    artifactId: string,
  ): Promise<void> => {
    if (!editorOwnsPhoto(photoId) || !artifactId) return;
    const generation = editorScopeGeneration;
    editorComposableNote = `Reading the Processing Artifact ${artifactId}…`;
    renderEditor();
    let response: Response;
    try {
      response = await fetcher(
        `/api/processing-artifacts/${encodeURIComponent(artifactId)}`,
        { priority: "low" },
      );
    } catch {
      if (ownsEditorScope(photoId, generation)) {
        editorComposableNote =
          "The Processing Artifact could not be read. Check the identity and try again.";
        renderEditor();
      }
      return;
    }
    if (!ownsEditorScope(photoId, generation)) return;
    if (response.status !== 200) {
      const note = await describeEditRefusal(
        response,
        "The Processing Artifact",
      );
      if (!ownsEditorScope(photoId, generation)) return;
      editorComposableNote = note;
      renderEditor();
      return;
    }
    const artifact = parseProcessingArtifactRecord(
      await response.json().catch(() => undefined),
    );
    if (!ownsEditorScope(photoId, generation)) return;
    if (!artifact || artifact.artifactId !== artifactId) {
      editorComposableNote =
        "The Processing Artifact record could not be confirmed.";
      renderEditor();
      return;
    }
    retainArtifact(artifact);
    editorComposableNote = describeProcessingArtifact(
      artifact,
      formatByteCount,
    );
    renderEditor();
  };
  /// Submits the guarded save of the composable draft. The guards run on
  /// this caller first, in the service's own order; the service remains
  /// authoritative. A lost or unusable response keeps the exact submission
  /// reusable under its request identity.
  const saveEditorComposable = async (photoId: string): Promise<void> => {
    if (!editorOwnsPhoto(photoId) || editorComposableSaving) return;
    // An unresolved submission replays its own body; it is never rebuilt.
    if (editorComposableSubmission) {
      if (editorComposableSubmission.photoId !== photoId) return;
      editorComposableNote = "Checking the previous save's result…";
      editorComposableSaving = true;
      renderEditor();
      await submitEditorComposableBody(
        photoId,
        editorComposableSubmission.requestId,
        editorComposableSubmission.body,
        JSON.parse(editorComposableSubmission.body) as ComposableSaveRequest,
      );
      return;
    }
    const draft = materializeComposableDraft(photoId);
    if (!draft) return;
    if (!editorComposableParametersValid) {
      editorComposableNote =
        "The step's parameters are not one JSON object, so the recipe cannot be saved.";
      renderEditor();
      return;
    }
    const guarded = composableSaveRequest(
      draft,
      `web-recipe-${randomUuid().replaceAll("-", "").slice(0, 24)}`,
      composableChoices(),
    );
    if (guarded.kind === "refused") {
      editorComposableNote = guarded.note;
      renderEditor();
      return;
    }
    editorComposableNote = "Saving the Processing Recipe…";
    editorComposableSaving = true;
    renderEditor();
    await submitEditorComposableBody(
      photoId,
      guarded.request.requestId,
      JSON.stringify(guarded.request),
      guarded.request,
    );
  };
  const submitEditorComposableBody = async (
    photoId: string,
    requestId: string,
    body: string,
    request: ComposableSaveRequest,
  ): Promise<void> => {
    const generation = editorScopeGeneration;
    editorComposableSubmission = { photoId, requestId, body };
    let response: Response;
    try {
      response = await saveComposableRecipe(fetcher, photoId, request);
    } catch {
      if (ownsEditorScope(photoId, generation)) {
        editorComposableSaving = false;
        editorComposableNote =
          "The save outcome is unknown. Check its result before editing again.";
        renderEditor();
      }
      return;
    }
    if (!ownsEditorScope(photoId, generation)) return;
    const outcome: unknown = await response.json().catch(() => undefined);
    if (!ownsEditorScope(photoId, generation)) return;
    if (response.status !== 200 && response.status !== 201) {
      editorComposableSaving = false;
      const error =
        isRecord(outcome) && isRecord(outcome["error"])
          ? outcome["error"]
          : undefined;
      const code =
        error && typeof error["code"] === "string" ? error["code"] : "";
      if (response.status >= 500 || code === "outcome_unknown") {
        editorComposableNote =
          "The save outcome is unknown. Check its result before editing again.";
      } else if (code === "recipe_conflict") {
        editorComposableSubmission = undefined;
        editorComposableNote =
          "The saved Processing Recipe changed elsewhere. Reload to read the current recipe, then decide again.";
      } else if (code === "source_changed") {
        editorComposableSubmission = undefined;
        editorComposableNote =
          "This Photo's Original changed elsewhere. Reload to check again.";
      } else if (code === "request_conflict") {
        editorComposableSubmission = undefined;
        editorComposableNote =
          "This request identity was already used with a different recipe. Compose again and save with a new identity.";
      } else {
        editorComposableSubmission = undefined;
        editorComposableNote =
          error && typeof error["message"] === "string"
            ? `The save was refused: ${error["message"]}`
            : "The save was refused. Reload to check again.";
      }
      renderEditor();
      return;
    }
    const responseSource =
      isRecord(outcome) && typeof outcome["sourceRevision"] === "string"
        ? outcome["sourceRevision"]
        : "";
    const parsedSave = responseSource
      ? parseComposableRecipe(
          {
            photoId,
            sourceRevision: responseSource,
            recipe: isRecord(outcome) ? outcome["recipe"] : undefined,
          },
          photoId,
        )
      : undefined;
    const saved = parsedSave?.recipe;
    const recipeVersion =
      isRecord(outcome) && typeof outcome["recipeVersion"] === "string"
        ? outcome["recipeVersion"]
        : "";
    const resultKind =
      isRecord(outcome) && typeof outcome["outcome"] === "string"
        ? outcome["outcome"]
        : "";
    const submitted = readSubmittedSourceRevision(body);
    if (
      !saved ||
      !(
        resultKind === "saved" ||
        resultKind === "replayed" ||
        resultKind === "unchanged"
      ) ||
      recipeVersion !== saved.revision ||
      responseSource !== submitted
    ) {
      editorComposableSaving = false;
      editorComposableNote =
        "The save outcome is unknown. Check its result before editing again.";
      renderEditor();
      return;
    }
    editorComposableSaving = false;
    editorComposableSubmission = undefined;
    clearEditorPreview();
    editorComposable = { sourceRevision: saved.sourceRevision, recipe: saved };
    editorComposableNote =
      resultKind === "saved"
        ? `Saved the Processing Recipe (revision ${saved.revision}).`
        : `The Processing Recipe is unchanged (revision ${saved.revision}).`;
    renderEditor();
    // The saved recipe is now the preview's authority: the current step's
    // own Preview follows the committed selection.
    if (stage() !== "camera") void requestEditorPreview(photoId);
  };
  /// Drops the local draft and returns to the saved recipe's shape. Nothing
  /// is submitted; the service keeps its own committed revision.
  const discardEditorComposable = (photoId: string): void => {
    if (!editorOwnsPhoto(photoId)) return;
    editorComposableDraft = undefined;
    editorComposableEditingStepId = undefined;
    editorComposableParametersText = "";
    editorComposableParametersValid = true;
    editorComposableNote = "";
    editorComposableSubmission = undefined;
    renderEditor();
    if (stage() !== "camera") void requestEditorPreview(photoId);
  };
  /// Reads independent peer-module discovery without changing the legacy
  /// capability state or its controls. A failed discovery read leaves the
  /// capability report and current recipe facts untouched.
  const loadProcessingModules = async (photoId: string): Promise<void> => {
    const generation = ++processingModulesGeneration;
    try {
      const modules = await fetchProcessingModules(
        fetcher,
        new AbortController().signal,
      );
      if (
        generation !== processingModulesGeneration ||
        !editorOwnsPhoto(photoId)
      )
        return;
      if (modules !== undefined) processingModules = modules;
      if (currentPhoto()?.id === photoId) renderEditor();
    } catch {
      /* independent discovery is optional presentation */
    }
  };
  const useEditorArtifactInput = (
    photoId: string,
    artifactId: string,
  ): void => {
    if (!editorArtifacts.some((item) => item.artifactId === artifactId)) return;
    const stepId = editorComposableEditingStepId;
    if (!stepId) {
      editorComposableNote =
        "Open one Processing Step first, then select this artifact as its input.";
      renderEditor();
      return;
    }
    editEditorComposableInput(photoId, "artifact", artifactId);
  };
  const reset = (): void => {
    editorScopeGeneration += 1;
    editorComposable = undefined;
    editorComposableGeneration += 1;
    processingModules = [];
    processingModulesGeneration += 1;
    editorComposableAbort?.abort();
    editorComposableAbort = undefined;
    // A new scope starts from no draft, no step editor, and no retained
    // artifact of another Photo's processing.
    editorComposableDraft = undefined;
    editorComposableEditingStepId = undefined;
    editorComposableParametersText = "";
    editorComposableParametersValid = true;
    editorComposableInputChoice = "original";
    editorComposableArtifactChoice = "";
    editorComposableNote = "";
    editorComposableSaving = false;
    editorComposableSubmission = undefined;
    editorArtifacts = [];
    editorComposableReadPending = false;
  };
  return {
    get read() {
      return editorComposable;
    },
    get readPending() {
      return editorComposableReadPending;
    },
    get modules() {
      return processingModules;
    },
    get artifacts() {
      return editorArtifacts;
    },
    target: composableTarget,
    dirty: composableDirty,
    mode: composableMode,
    retainArtifact,
    reset,
    view: (): NonNullable<
      Parameters<LibraryBrowserView["renderEditor"]>[0]["composable"]
    > => {
      const target = composableTarget();
      const composing = composableMode();
      const dirty = composableDirty();
      const draft = composableEffectiveDraft();
      const choices = composableChoices();
      const editingStep = draft?.steps.find(
        (step) => step.stepId === editorComposableEditingStepId,
      );
      const editingModule = editingStep
        ? choices.find((choice) => choice.name === editingStep.module)
        : undefined;
      return {
        composing,
        legacyOnly: !composing && target.kind !== "unreadable",
        unreadable: target.kind === "unreadable",
        readPending: editorComposableReadPending,
        note: editorComposableNote,
        modules: choices,
        steps: draft
          ? draft.steps.map((step) => ({
              stepId: step.stepId,
              module: step.module,
              inputNote:
                step.input.kind === "artifact"
                  ? `Artifact ${step.input.artifactId}`
                  : "Original",
              current: draft.currentStepId === step.stepId,
              editing: editorComposableEditingStepId === step.stepId,
            }))
          : [],
        currentStepId: draft?.currentStepId ?? null,
        dirty,
        saving: editorComposableSaving,
        savePending: editorComposableSubmission !== undefined,
        canAddStep: choices.length > 0,
        editing: editingStep
          ? {
              stepId: editingStep.stepId,
              module: editingStep.module,
              schemaVersion: editingStep.parameters.schemaVersion,
              schemaVersions: editingModule?.parameterVersions ?? [],
              inputChoice: editorComposableInputChoice,
              artifactChoice: editorComposableArtifactChoice,
              parametersText: editorComposableParametersText,
              parametersValid: editorComposableParametersValid,
            }
          : null,
        artifacts: editorArtifacts.map((artifact) => ({
          artifactId: artifact.artifactId,
          note: describeProcessingArtifact(artifact, formatByteCount),
          canUse: editingStep !== undefined,
        })),
      };
    },
    loadComposableRecipe,
    composeEditorSteps,
    addEditorComposableStep,
    removeEditorComposableStep,
    selectEditorComposableStep,
    editEditorComposableStep,
    editEditorComposableParameters,
    editEditorComposableSchema,
    editEditorComposableInput,
    fetchEditorArtifact,
    saveEditorComposable,
    discardEditorComposable,
    loadProcessingModules,
    useEditorArtifactInput,
  };
}
