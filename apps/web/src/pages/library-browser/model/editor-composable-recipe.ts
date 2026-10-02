import {
  fetchComposableRecipe,
  type ComposableRecipeRead,
} from "../api/composable-recipe.js";
import { createComposableAutosave } from "./composable-autosave.js";
import {
  fetchProcessingModules,
  type ProcessingModuleDescription,
} from "../api/processing-modules.js";
import {
  addComposableStep,
  composableDraftDiffers,
  composableModuleChoices,
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
} from "./composable-preview.js";
import {
  describeProcessingArtifact,
  parseProcessingArtifactRecord,
  processingArtifactInput,
  type ProcessingArtifactRecord,
} from "./processing-artifact.js";
import { formatByteCount } from "./editor-presentation.js";
import type { BrowserFetch } from "./access-session.js";
import type { EditorControllerDependencies } from "./editor-controller-contract.js";
import type { LibraryBrowserView } from "../ui/library-browser-view.js";
export function createEditorComposableRecipe(
  fetcher: BrowserFetch,
  dependencies: EditorControllerDependencies &
    Readonly<{
      editorOwnsPhoto: (photoId: string) => boolean;
      renderEditor: () => void;
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
    previewSelection,
    clearEditorPreview,
    requestEditorPreview,
    markEditorPreviewStale,
    describeEditRefusal,
  } = dependencies;
  let editorComposable: ComposableRecipeRead | undefined;
  let editorComposableAbort: AbortController | undefined;
  let editorComposableGeneration = 0;
  /// Whether the current Photo's recipe read has not yet settled. Preview
  /// requests wait for the selected step and its confirmed recipe.
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
  let parametersEditing = false;
  /// The input binding choice the step editor offers: the guarded Original
  /// or one explicitly selected retained Processing Artifact.
  let editorComposableInputChoice: "original" | "artifact" = "original";
  let editorComposableArtifactChoice = "";
  let editorComposableNote = "";
  let editorComposableSaving = false;
  /// Retained Processing Artifacts of this session: every artifact this
  /// workspace inspected, offered as the only explicit downstream inputs.
  /// Nothing chains automatically; the caller selects one per step.
  let editorArtifacts: ReadonlyArray<ProcessingArtifactRecord> = [];
  let processingModules: ReadonlyArray<ProcessingModuleDescription> = [];
  let processingModulesGeneration = 0;
  let editorScopeGeneration = 0;
  let activePhotoId: string | undefined;
  const syncEditing = (): void => {
    const steps = editorComposableDraft?.steps;
    const step =
      steps?.find((item) => item.stepId === editorComposableEditingStepId) ??
      steps?.find(
        (item) => item.stepId === editorComposableDraft?.currentStepId,
      );
    editorComposableEditingStepId = step?.stepId;
    editorComposableParametersText = step
      ? JSON.stringify(step.parameters.tree, null, 2)
      : "";
    editorComposableParametersValid = true;
    editorComposableInputChoice =
      step?.input.kind === "artifact" ? "artifact" : "original";
    editorComposableArtifactChoice =
      step?.input.kind === "artifact" ? step.input.artifactId : "";
  };
  const autosave = createComposableAutosave(fetcher, (photoId) => {
    if (photoId !== activePhotoId || !editorOwnsPhoto(photoId)) return;
    const state = autosave.get(photoId);
    if (!state) return;
    editorComposable = state.read;
    if (!parametersEditing) {
      editorComposableDraft = state.draft;
      syncEditing();
    }
    editorComposableSaving = state.saving;
    editorComposableNote =
      state.note +
      (state.recoveryAvailable
        ? ""
        : " Local settings remain in this session; browser reload recovery is unavailable.");
    renderEditor();
    if (
      !state.saving &&
      !state.pending &&
      !state.conflict &&
      !composableDraftDiffers(state.draft, state.read.recipe)
    )
      void requestEditorPreview(photoId);
  });
  const completeAction = (photoId: string): void => {
    if (editorComposableDraft) {
      const state = autosave.get(photoId);
      if (state)
        editorComposableDraft = {
          ...editorComposableDraft,
          baseRevision: state.draft.baseRevision,
        };
      void autosave.change(photoId, editorComposableDraft);
    }
  };
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
      activePhotoId = photoId;
      const state = autosave.open(photoId, read);
      editorComposableDraft = state.draft;
      editorComposableSaving = state.saving;
      editorComposableNote =
        state.note +
        (state.recoveryAvailable
          ? ""
          : " Local settings remain in this session; browser reload recovery is unavailable.");
      const selection = selectedComposableStep(read)?.stepId ?? null;
      if (previewSelection() !== undefined && previewSelection() !== selection)
        clearEditorPreview();
      editorComposable = state.read;
      syncEditing();
    }
    if (currentPhoto()?.id === photoId) renderEditor();
    void requestEditorPreview(photoId);
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
  const composableDirty = (): boolean => {
    const state = activePhotoId ? autosave.get(activePhotoId) : undefined;
    return Boolean(
      state?.pending ||
        state?.conflict ||
        state?.uncertain ||
        (editorComposableDraft &&
          composableDraftDiffers(
            editorComposableDraft,
            editorComposable?.recipe ?? null,
          )),
    );
  };
  /// The workspace owns a composable draft even when no recipe is saved.
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
  /// Adds a step only after the caller chooses its identified input.
  const addEditorComposableStep = (
    photoId: string,
    module: string,
    artifactId?: string,
  ): void => {
    if (!editorOwnsPhoto(photoId)) return;
    const draft = materializeComposableDraft(photoId);
    const choice = composableChoices().find((item) => item.name === module);
    if (!draft || !choice) {
      editorComposableNote =
        "Module discovery is not available right now. Reload to check again.";
      renderEditor();
      return;
    }
    const description = processingModules.find(
      (item) => item.id.name === module,
    );
    const artifact = artifactId
      ? editorArtifacts.find((item) => item.artifactId === artifactId)
      : undefined;
    if (
      artifactId &&
      (!artifact || Date.parse(artifact.expiresAt) <= Date.now())
    ) {
      editorComposableNote =
        "Choose a retained, unexpired export as this step's input.";
      renderEditor();
      return;
    }
    const admitted = artifact
      ? description?.admittedInputs.some(
          (contract) =>
            contract["format"] === artifact.outputContract.format &&
            contract["colorSpace"] === artifact.outputContract.colorSpace &&
            contract["transferFunction"] === artifact.outputContract.transfer &&
            String(contract["precisionBits"]) ===
              artifact.outputContract.precision.replace(/^(?:float|uint)/, ""),
        )
      : description?.admittedInputs.some(
          (contract) => contract["colorSpace"] === "camera-native",
        );
    if (!admitted) {
      editorComposableNote = artifact
        ? "This module does not admit the selected export's image contract. Choose a compatible input."
        : "This module requires a compatible export as input. Choose one explicitly before adding the step.";
      renderEditor();
      return;
    }
    const stepId = nextComposableStepId(draft, module);
    editorComposableDraft = addComposableStep(draft, {
      stepId,
      module,
      input: artifact
        ? processingArtifactInput(artifact)
        : {
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
    editorComposableInputChoice = artifact ? "artifact" : "original";
    editorComposableArtifactChoice = artifact?.artifactId ?? "";
    editorComposableNote = "";
    markEditorPreviewStale();
    completeAction(photoId);
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
    syncEditing();
    markEditorPreviewStale();
    completeAction(photoId);
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
    editorComposableEditingStepId = stepId;
    syncEditing();
    markEditorPreviewStale();
    completeAction(photoId);
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
    parametersEditing = true;
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
    completeAction(photoId);
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
      completeAction(photoId);
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
    if (Date.parse(artifact.expiresAt) <= Date.now()) {
      editorComposableNote =
        "This export has expired. Export again before using it as input.";
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
    completeAction(photoId);
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
  const commitEditorComposableParameters = (photoId: string): void => {
    if (!editorOwnsPhoto(photoId) || !editorComposableParametersValid) return;
    parametersEditing = false;
    completeAction(photoId);
  };
  const saveEditorComposable = async (photoId: string): Promise<void> => {
    if (!editorOwnsPhoto(photoId)) return;
    commitEditorComposableParameters(photoId);
    await autosave.flush(photoId);
  };
  const discardEditorComposable = (photoId: string): void => {
    if (!editorOwnsPhoto(photoId)) return;
    if (autosave.get(photoId)?.pending) return;
    parametersEditing = false;
    autosave.useSaved(photoId);
  };
  /// Reads independent module discovery without changing saved recipe facts.
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
    activePhotoId = undefined;
    parametersEditing = false;
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
        savePending: Boolean(
          activePhotoId && autosave.get(activePhotoId)?.pending,
        ),
        canAddStep: choices.length > 0,
        editing: editingStep
          ? {
              stepId: editingStep.stepId,
              module: editingStep.module,
              schemaVersion: editingStep.parameters.schemaVersion,
              parameterSchema: processingModules.find(
                (module) => module.id.name === editingStep.module,
              )?.parameterSchema,
              schemaVersions: editingModule?.parameterVersions ?? [],
              inputChoice: editorComposableInputChoice,
              artifactChoice: editorComposableArtifactChoice,
              parametersText: editorComposableParametersText,
              parametersValid: editorComposableParametersValid,
            }
          : null,
        artifacts: editorArtifacts.map((artifact) => {
          const expired = Date.parse(artifact.expiresAt) <= Date.now();
          const current = draft?.steps.find(
            (step) => step.stepId === draft.currentStepId,
          );
          const older =
            current !== undefined &&
            (current.module !== artifact.module ||
              JSON.stringify(current.parameters) !==
                JSON.stringify(artifact.parameters) ||
              JSON.stringify(current.input) !==
                JSON.stringify(artifact.input.binding));
          return {
            artifactId: artifact.artifactId,
            note: `${describeProcessingArtifact(artifact, formatByteCount)}${older ? " Based on earlier settings." : ""}${expired ? " This export has expired. Export again." : ""}`,
            canUse: editingStep !== undefined && !expired,
            canDownload: !expired,
          };
        }),
      };
    },
    loadComposableRecipe,
    get state() {
      return activePhotoId ? autosave.get(activePhotoId) : undefined;
    },
    commitEditorComposableParameters,
    undo: autosave.undo,
    redo: autosave.redo,
    useSaved: autosave.useSaved,
    reapplyLocal: autosave.reapply,
    rebind: autosave.rebind,
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
