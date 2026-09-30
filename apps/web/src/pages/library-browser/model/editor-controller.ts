import type { PhotoSummary } from "../api/contracts.js";
import {
  createDevelopmentProxy,
  fetchDevelopmentProxy,
  removeDevelopmentProxy,
  type DevelopmentProxyStatus,
} from "../api/development-proxy.js";
import {
  fetchEditRecipe,
  isRecord,
  parseProcessingCapability,
  rebindEditRecipe,
  saveEditRecipe,
  withProcessingCapability,
  type ProcessingCapability,
} from "../api/editor.js";
import {
  BASELINE_SETTINGS,
  CURRENT_SETTINGS,
  comparisonIsCurrent,
  comparisonRefusal,
  currentRenditionRefusal,
  editPreviewUri,
  type Comparison,
} from "./edit-preview.js";
import {
  createWorkspaceOutputController,
  type OutputTarget,
} from "./workspace-output-controller.js";
import {
  asEditorSupportReason,
  createPhotoEditor,
  type EditSourceKind,
  type EditSourceReadiness,
  isRetryableSupportReason,
  supportReasonExplanation,
  type EditorFacts,
  type EditorStep,
  type PhotoEditor,
} from "./photo-editor.js";
import type { BrowserFetch } from "./access-session.js";
import type {
  EditorStage,
  LibraryBrowserView,
} from "../ui/library-browser-view.js";

const browserDraftStore = () => {
  try {
    window.sessionStorage.setItem("slipstream.draft.probe", "1");
    window.sessionStorage.removeItem("slipstream.draft.probe");
  } catch {
    return undefined;
  }
  return {
    read: (key: string) => {
      try {
        return window.sessionStorage.getItem(key);
      } catch {
        return null;
      }
    },
    write: (key: string, value: string) => {
      try {
        window.sessionStorage.setItem(key, value);
        return true;
      } catch {
        return false;
      }
    },
    remove: (key: string) => {
      try {
        window.sessionStorage.removeItem(key);
      } catch {
        /* the store is blocked; the in-memory draft still governs this session */
      }
    },
  };
};

const MAXIMUM_EDITOR_SESSIONS = 4;

const formatByteCount = (bytes: number): string => {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
};

type EditorControllerDependencies = Readonly<{
  isAlive: () => boolean;
  isCurrentPhoto: (photoId: string) => boolean;
  currentPhoto: () => PhotoSummary | undefined;
  /// The Library's current scan phase in the summary's words, or "" while
  /// no scan is running. A `Checking source…` wait names it so the
  /// Photographer can see what the Library is doing.
  libraryPhase?: () => string;
}>;

export type EditorController = Readonly<{
  open: (photoId: string) => void;
  refresh: (photoId: string) => void;
  commitExposure: (photoId: string, exposureEv: number) => void;
  commitWhiteBalance: (
    photoId: string,
    action:
      | Readonly<{ kind: "mode"; mode: string }>
      | Readonly<{ kind: "temperature"; temperatureKelvin: number }>
      | Readonly<{ kind: "tint"; tintMilli: number }>,
  ) => void;
  stepHistory: (
    photoId: string,
    operation:
      | "undo"
      | "redo"
      | "reset"
      | "resetExposure"
      | "resetWhiteBalance",
  ) => void;
  requestPreview: (photoId: string) => void;
  applyStage: (photoId: string, stage: EditorStage) => void;
  setComparison: (photoId: string, pressed: boolean) => void;
  useSaved: (photoId: string) => void;
  reapplyLocal: (photoId: string) => void;
  discardDraft: (photoId: string) => void;
  rebind: (photoId: string) => void;
  createProxy: (photoId: string) => void;
  removeProxy: (photoId: string) => void;
  submitXmp: (photoId: string) => void;
  downloadXmp: (photoId: string) => void;
  submitExport: (photoId: string, target: OutputTarget) => void;
  cancelExport: (photoId: string, target: OutputTarget) => void;
  retryExport: (photoId: string, target: OutputTarget) => void;
  downloadExport: (photoId: string, target: OutputTarget) => void;
  leave: () => void;
}>;

export function createEditorController(
  fetcher: BrowserFetch,
  view: LibraryBrowserView,
  dependencies: EditorControllerDependencies,
): EditorController {
  const { isAlive, isCurrentPhoto, currentPhoto, libraryPhase } = dependencies;
  /// One editing session per Photo. Each session keeps its own confirmed
  /// recipe, local intent, session history, and local draft, so navigating to
  /// another Photo never cancels or rebinds a pending save.
  const editorSessions = new Map<string, PhotoEditor>();
  const editorSession = (photoId: string): PhotoEditor => {
    const existing = editorSessions.get(photoId);
    if (existing) {
      editorSessions.delete(photoId);
      editorSessions.set(photoId, existing);
      return existing;
    }
    const created = createPhotoEditor({ store: browserDraftStore() });
    editorSessions.set(photoId, created);
    for (const [key, session] of editorSessions) {
      if (editorSessions.size <= MAXIMUM_EDITOR_SESSIONS) break;
      // A session with a write in flight keeps its identity until the service
      // settles it; the draft in storage already carries every other intent.
      if (key === photoId || session.presentation().saving) continue;
      editorSessions.delete(key);
    }
    return created;
  };
  const currentEditor = (): PhotoEditor | undefined => {
    const photoId = currentPhoto()?.id;
    return photoId ? editorSessions.get(photoId) : undefined;
  };
  /// The view the workspace presents. The current edit (develop) is always
  /// the default; Film and the Original camera reference are explicit
  /// actions the Photographer toggles on and back off.
  let editorStage: EditorStage = "develop";
  let editorComparing = false;
  let editorPreviewUrl: string | undefined;
  let editorPreviewNote = "";
  let editorPreviewOutcome: "pending" | "ready" | "failed" | "unknown" =
    "pending";
  let editorPreviewStale = false;
  let editorPreviewBusy = false;
  let editorPreviewAbort: AbortController | undefined;
  let editorPreviewGeneration = 0;
  /// The identity of the preview request in flight: Photo, stage, source
  /// revision, recipe snapshot, and edit source. A retry of the same
  /// identity joins the in-flight request instead of starting duplicate
  /// physical work.
  let editorPreviewIdentity: string | undefined;
  /// True when the last preview outcome was a refusal and no rendition is
  /// presented, so the Edit Preview axis reports a failure rather than a
  /// wait.
  let editorPreviewRefused = false;
  /// How many follow-up requests one admitted preview has already made.
  let editorPreviewAttempts = 0;
  let editorPreviewTimer: number | undefined;
  /// One recipe read per Photo at a time. A Refresh-source action or an
  /// automatic publication refresh joins the read already under way.
  const editorRecipeReads = new Map<string, Promise<void>>();
  const editorRecipeGenerations = new Map<string, number>();
  /// The retained as-shot/baseline comparison of the chosen stage, the image
  /// it was served, and its own progress. A comparison is defined by the Photo,
  /// the stage, and the source revision, so it is retained across saved-settings
  /// changes and dropped when one of those moves.
  let editorComparison: Comparison | undefined;
  let editorComparisonUrl: string | undefined;
  let editorComparisonNote = "";
  let editorComparisonBusy = false;
  let editorComparisonAbort: AbortController | undefined;
  let editorComparisonGeneration = 0;
  /// How many follow-up requests one admitted comparison has already made.
  let editorComparisonAttempts = 0;
  let editorComparisonTimer: number | undefined;
  let editorWriteAbort: AbortController | undefined;
  /// Resolves the writers waiting for this Photo's write stream to settle.
  let editorWriteWaiters: Array<() => void> = [];
  /// Each Photo has its own Export barrier. An uncertain Export keeps only
  /// that Photo's later writes behind its idempotency reconciliation.
  const editorExportBarriers = new Map<
    string,
    Readonly<{ promise: Promise<void>; resolve: () => void }>
  >();
  /// The automatic resolution of a save whose outcome is unknown: at most one
  /// identical retry per lost response, so a lost receipt cannot spin.
  let editorUnknownResolution: string | undefined;
  let filmUnavailableReason = "Film is temporarily unavailable.";
  /// The deployment's processing capability report, once one Edit session has
  /// read it. It is the only source of the admitted adjustable controls.
  let processingCapability: ProcessingCapability | undefined;
  let editorProxy: DevelopmentProxyStatus = {
    photoId: "",
    state: "absent",
    proxy: null,
    failure: null,
  };
  let editorProxyFailure = "";
  let editorProxyTimer: number | undefined;
  let editorProxyGeneration = 0;
  const settleEditorWriters = (): void => {
    const waiters = editorWriteWaiters;
    editorWriteWaiters = [];
    for (const waiter of waiters) waiter();
  };
  const editorOwnsPhoto = (photoId: string): boolean =>
    isAlive() &&
    view.editorVisible() &&
    isCurrentPhoto(photoId) &&
    currentPhoto()?.id === photoId;
  const releaseEditorExportBarrier = (photoId: string): void => {
    const barrier = editorExportBarriers.get(photoId);
    if (!barrier) return;
    editorExportBarriers.delete(photoId);
    barrier.resolve();
  };
  const outputs = createWorkspaceOutputController(fetcher, {
    owns: editorOwnsPhoto,
    render: () => renderEditor(),
    facts: (photoId) => {
      const session = editorSessions.get(photoId);
      if (!session) return undefined;
      const presented = session.presentation();
      const facts = session.facts();
      return {
        recipeVersion: presented.recipeVersion,
        sourceRevision: facts?.sourceRevision ?? null,
        recipeSourceRevision:
          facts?.recipeSourceRevision ?? facts?.sourceRevision ?? null,
        saving: presented.saving,
        dirty: presented.dirty,
        conflict: Boolean(presented.conflict),
        canRender:
          presented.canEdit &&
          presented.processingAvailable &&
          presented.editSourceKind === "original",
        canRenderFilm:
          presented.canEdit &&
          presented.processingAvailable &&
          presented.editSourceKind === "original" &&
          !filmUnavailableReason,
      };
    },
    settle: async (photoId) => {
      for (;;) {
        if (!editorOwnsPhoto(photoId)) return false;
        const presented = editorSessions.get(photoId)?.presentation();
        if (!presented || presented.conflict) return false;
        if (!presented.saving) return !presented.dirty;
        const { promise, resolve } = Promise.withResolvers<void>();
        editorWriteWaiters.push(resolve);
        await promise;
      }
    },
    acquire: (photoId) => {
      editorExportBarriers.set(photoId, Promise.withResolvers<void>());
    },
    release: releaseEditorExportBarrier,
  });
  const clearEditorComparison = (): void => {
    editorComparisonAbort?.abort();
    editorComparisonAbort = undefined;
    editorComparisonGeneration += 1;
    if (editorComparisonUrl) URL.revokeObjectURL(editorComparisonUrl);
    editorComparison = undefined;
    editorComparisonUrl = undefined;
    editorComparisonNote = "";
    editorComparisonBusy = false;
    editorComparisonAttempts = 0;
    if (editorComparisonTimer !== undefined) {
      clearTimeout(editorComparisonTimer);
      editorComparisonTimer = undefined;
    }
  };
  const clearEditorPreview = (): void => {
    editorPreviewAbort?.abort();
    editorPreviewAbort = undefined;
    editorPreviewGeneration += 1;
    editorPreviewBusy = false;
    editorPreviewIdentity = undefined;
    editorPreviewAttempts = 0;
    if (editorPreviewTimer !== undefined) {
      clearTimeout(editorPreviewTimer);
      editorPreviewTimer = undefined;
    }
    if (editorPreviewUrl) URL.revokeObjectURL(editorPreviewUrl);
    editorPreviewUrl = undefined;
    editorPreviewNote = "";
    editorPreviewOutcome = "pending";
    editorPreviewStale = false;
    editorPreviewRefused = false;
    clearEditorComparison();
    view.clearEditorPreview();
  };
  /// The Processing axis. The axis is the deployment's engine capability,
  /// not the Photo's source: a source wait belongs to the Edit source axis,
  /// so a Photo whose facts have not settled never reclassifies the engines.
  const processingReadiness = ():
    | "checking"
    | "ready"
    | "waiting"
    | "unavailable" =>
    !processingCapability
      ? "checking"
      : processingCapability.state === "ready"
        ? "ready"
        : processingCapability.state === "resource-unavailable"
          ? "waiting"
          : "unavailable";
  /// The Edit Preview axis for the chosen stage: what the presented
  /// rendition is — pending, current, older than the settings, or failed.
  /// `null` when the Camera stage is presented or no rendition can be
  /// requested; the reason is on the other axes then, so the axis never
  /// claims a state it cannot name.
  const previewReadiness = ():
    | "pending"
    | "ready"
    | "stale"
    | "failed"
    | null => {
    if (editorStage === "camera") return null;
    if (editorPreviewBusy || editorPreviewTimer !== undefined) return "pending";
    if (editorPreviewOutcome === "failed") return "failed";
    if (editorPreviewUrl) return editorPreviewStale ? "stale" : "ready";
    return editorPreviewRefused ? "failed" : null;
  };
  /// The Source support fact line. The readiness word names the axis's own
  /// state, the Library's scan phase rides along while the source is being
  /// checked, and a proxy edit source is named as provenance.
  const sourceFactNote = (
    readiness: EditSourceReadiness,
    editSource: EditSourceKind,
  ): string => {
    const word =
      readiness === "checking"
        ? "Checking source…"
        : readiness === "ready"
          ? "Ready"
          : readiness === "missing"
            ? "Original missing"
            : readiness === "unreadable"
              ? "Original unreadable"
              : "Unsupported source class";
    const phase = readiness === "checking" ? (libraryPhase?.() ?? "") : "";
    const provenance =
      editSource === "development-proxy"
        ? " (Development Proxy edit source)"
        : "";
    return phase ? `${word} — ${phase}${provenance}` : `${word}${provenance}`;
  };
  /// The session's internal wording in the workspace's plain language. The
  /// compact status line never names recipes, revisions, or deployments;
  /// the original wording stays available under the Details affordance.
  const plainEditorMessage = (message: string): string =>
    message
      .replace(
        "This Photo's source class has no approved profile in this deployment",
        "This Photo is not supported for editing",
      )
      .replace(
        "Reading this Photo's Original File failed for its current source revision",
        "Reading this Photo's Original File failed",
      )
      .replace(
        "Reload the recipe to check for current source facts.",
        "Reload to check again.",
      )
      .replace(
        "Reload the recipe to retry once the current Library work settles.",
        "Reload to retry.",
      )
      .replaceAll("the saved recipe", "the saved edit")
      .replaceAll("The saved recipe", "The saved edit");
  /// The conflict resolutions are preserved exactly; only their wording moves
  /// from service concepts to the Photographer's edit.
  const plainConflictMessage = (message: string): string => {
    if (
      message ===
      "A local draft from an earlier revision was recovered. Use the saved recipe or reapply the draft."
    )
      return "A local draft from an earlier version of this Photo was recovered. Use the saved edit or reapply the draft.";
    if (
      message ===
      "The saved recipe is bound to a different source. Rebind it or use the saved recipe."
    )
      return "The saved edit belongs to a different file. Keep the edit for the current file, or use the saved edit.";
    if (
      message ===
      "The Original File changed elsewhere. Autosave stopped; use the saved recipe or reapply the local settings."
    )
      return "The Original File changed elsewhere. Autosave stopped; use the saved edit or reapply your changes.";
    if (
      message ===
      "The saved recipe changed elsewhere. Autosave stopped; use the saved recipe or reapply the local settings."
    )
      return "The saved edit changed elsewhere. Autosave stopped; use the saved edit or reapply your changes.";
    return plainEditorMessage(message);
  };
  const plainWhiteBalanceNote = (note: string): string =>
    note.startsWith("This deployment admits temperature and tint")
      ? "Temperature and tint are not available for this Photo right now, so the controls stay read-only."
      : "Only as-shot white balance is available for this Photo.";
  /// The one compact status line. Uncertain effects read as checking, never
  /// as failure; a refused save offers its retry; everything else the session
  /// reports is either transient progress or a read-only explanation.
  const compactEditorStatus = (
    presented: Readonly<{
      photoId: string;
      saving: boolean;
      dirty: boolean;
      canEdit: boolean;
      processingAvailable: boolean;
      conflict: unknown;
      status: string;
      recipeVersion: string | null;
    }>,
  ): string => {
    if (presented.photoId === "") return "Loading edit…";
    if (presented.conflict)
      return "Could not update — choose how to resolve it below.";
    const status = presented.status;
    if (status.startsWith("The save outcome is unknown"))
      return "Checking result…";
    if (
      status.startsWith("The save was refused") ||
      (presented.dirty && !presented.saving)
    )
      return "Could not update — Retry";
    if (presented.saving) return "Saving…";
    if (status.startsWith("This deployment does not admit the white-balance"))
      return "This white-balance mode is not available for this Photo.";
    if (status.startsWith("This deployment does not admit temperature"))
      return "Temperature and tint are not available for this Photo.";
    if (
      status === "" ||
      status === "Saved." ||
      status === "Using the saved recipe."
    )
      return presented.recipeVersion ? "Edit state saved" : "No saved edit yet";
    return plainEditorMessage(status);
  };
  const renderEditor = (): void => {
    const photoId = currentPhoto()?.id;
    const session = currentEditor();
    if (!photoId || !session) return;
    const presented = session.presentation();
    view.renderEditor({
      photoId,
      loading: presented.photoId === "",
      stage: editorStage,
      stageNote: editorStageNote(),
      filmReason:
        filmUnavailableReason ||
        (!presented.canEdit && presented.photoId !== ""
          ? "Film is not available for this Photo."
          : !presented.processingAvailable && presented.photoId !== ""
            ? "Film is temporarily unavailable for this Photo."
            : ""),
      editSourceReadiness: presented.editSourceReadiness,
      editSourceKind: presented.editSourceKind,
      sourceFactNote: sourceFactNote(
        presented.editSourceReadiness,
        presented.editSourceKind,
      ),
      processingReadiness: processingReadiness(),
      previewState: previewReadiness(),
      processingAvailable: presented.processingAvailable,
      capabilityNote: processingCapability
        ? capabilityNote(processingCapability.state)
        : "",
      exposureEv: presented.settings.exposureEv,
      savedExposureEv: presented.confirmed.exposureEv,
      baselineExposureEv: presented.baseline.exposureEv,
      exposureMinimumEv: presented.controls.minimumEv,
      exposureMaximumEv: presented.controls.maximumEv,
      exposureStepEv: presented.controls.stepEv,
      whiteBalance: presented.whiteBalance.note
        ? {
            ...presented.whiteBalance,
            note: plainWhiteBalanceNote(presented.whiteBalance.note),
          }
        : presented.whiteBalance,
      proxy: {
        state: editorProxy.state,
        note:
          editorProxyFailure ||
          (editorProxy.state === "building"
            ? "Building Development Proxy…"
            : editorProxy.state === "current"
              ? `Current proxy ${editorProxy.proxy?.width ?? "?"}×${editorProxy.proxy?.height ?? "?"}, ${formatByteCount(editorProxy.proxy?.byteLength ?? 0)}.`
              : editorProxy.state === "stale"
                ? "Stale Development Proxy; rebuild it for this Original."
                : "No Development Proxy."),
        proxy: editorProxy.proxy
          ? {
              width: editorProxy.proxy.width,
              height: editorProxy.proxy.height,
              longEdge: editorProxy.proxy.longEdge,
              qualityLimit: editorProxy.proxy.qualityLimit,
              byteLength: editorProxy.proxy.byteLength,
              sourceRevision: editorProxy.proxy.sourceRevision,
              sourceProfileId: editorProxy.proxy.sourceProfileId,
              pipelineVersion: editorProxy.proxy.pipelineVersion,
            }
          : null,
        canCreate:
          presented.editSourceReadiness === "ready" &&
          Boolean(session.facts()?.sourceRevision) &&
          editorProxy.state !== "building" &&
          presented.editSourceKind !== "development-proxy",
        canRemove: editorProxy.state === "current",
      },
      canEdit: presented.canEdit,
      canPreview: presented.canEdit && presented.processingAvailable,
      previewing: editorPreviewBusy,
      // While the comparison is pressed, the presented image is the
      // unadjusted rendering, so its own note and its own freshness describe
      // what a Photographer sees.
      previewNote: editorComparing ? editorComparisonNote : editorPreviewNote,
      previewStale: !editorComparing && editorPreviewStale,
      saving: presented.saving,
      dirty: presented.dirty,
      canUndo: presented.canUndo,
      canRedo: presented.canRedo,
      comparing: editorComparing,
      conflict: presented.conflict
        ? { message: plainConflictMessage(presented.conflict.message) }
        : null,
      draftNote: presented.draft.note,
      outputs: outputs.view(photoId),
      status: compactEditorStatus(presented),
      statusDetail: presented.status,
    });
  };
  /// What the presented image actually is, in plain language. A proxy-backed
  /// rendition retains its provenance; it is not a full-resolution result.
  const editorStageNote = (): string => {
    if (editorStage === "camera")
      return "Original reference: the camera preview of this Photo, before your edit.";
    const stageName = editorStage === "film" ? "Film" : "Edit";
    const proxy =
      currentEditor()?.facts()?.editSource === "development-proxy"
        ? " The current edit source is a Development Proxy, so this rendition's detail is the proxy's, not a full-resolution result."
        : "";
    if (editorComparing && editorComparisonUrl) {
      // A comparison is only a comparison while both images describe the same
      // development: a current rendition that is absent or older than the
      // current settings is named instead of being compared as if it were
      // current.
      const current = !editorPreviewUrl
        ? " No current preview is shown, so the comparison is shown alone."
        : editorPreviewStale
          ? " The current preview is older than the current settings."
          : "";
      return `${stageName} comparison: the unadjusted rendering, compared with the current settings.${current}${proxy}`;
    }
    if (!editorPreviewUrl)
      return editorStage === "film"
        ? "Film: the Film preview is not ready yet, so no edited image is shown."
        : "Edit: the preview is not ready yet; the camera preview is shown.";
    if (editorStage === "film")
      return `Film: the fixed film look applied to your edit.${proxy}`;
    const preview = currentPhoto()?.preview;
    const jpegOriginal =
      preview?.state === "ready" && preview.source === "jpeg-original"
        ? " This Photo's camera preview is a JPEG Original."
        : "";
    return `Edit: a preview of your edited result at reduced resolution.${proxy}${jpegOriginal}`;
  };
  /// Places the session's next guarded write in its Photo's stream. One write
  /// is in flight per Photo, and the model coalesces later actions behind it.
  const placeEditorWrite = async (
    photoId: string,
    session: PhotoEditor,
    step: EditorStep,
  ): Promise<void> => {
    // The Export ordering barrier: an edit placed while a submission is
    // settling waits behind it, so the accepted Export can never be retargeted
    // by a later write.
    const barrier = editorExportBarriers.get(photoId)?.promise;
    if (barrier) await barrier;
    const request = step.request;
    if (!request) {
      settleEditorWriters();
      if (currentPhoto()?.id === photoId) renderEditor();
      return;
    }
    const controller = new AbortController();
    editorWriteAbort = controller;
    const result = await saveEditRecipe(fetcher, request, controller.signal);
    if (editorWriteAbort === controller) editorWriteAbort = undefined;
    const next =
      result.kind === "saved"
        ? session.acknowledge(request, {
            recipeVersion: result.recipeVersion,
            sourceRevision: result.sourceRevision,
          })
        : session.refuse(request, result.refusal);
    settleEditorWriters();
    if (currentPhoto()?.id === photoId) renderEditor();
    if (result.kind === "saved") {
      // A save that lost its response is resolved through its own identity
      // before further writes advance: one identical retry, then the explicit
      // user action. A resolved receipt confirms or refuses that operation.
      if (next.request === null) editorUnknownResolution = undefined;
      // The preview follows the settings the service confirmed, so the
      // rendition that arrives belongs to the recipe now in force.
      if (next.request === null) void requestEditorPreview(photoId);
    } else if (
      result.refusal.code === "outcome_unknown" &&
      editorUnknownResolution !== request.id
    ) {
      editorUnknownResolution = request.id;
      window.setTimeout(() => {
        if (!editorOwnsPhoto(photoId)) return;
        const resolution = session.resolveUnknown();
        renderEditor();
        void placeEditorWrite(photoId, session, resolution);
      }, 750);
    }
    await placeEditorWrite(photoId, session, next);
  };
  const editorFactsFromWire = (
    photoId: string,
    response:
      | Readonly<{ kind: "ok"; facts: EditorFacts }>
      | Readonly<{ kind: "failed"; message: string }>,
    mode: "open" | "refresh",
  ): void => {
    const session = editorSession(photoId);
    if (response.kind === "failed") {
      if (mode === "open") {
        // An unanswered read is not a confirmed read failure: the session
        // opens on a retryable wait — no reason claimed, the transport's own
        // message on the status line — instead of presenting the Photo as
        // though its Original File had failed to read.
        session.open({
          photoId,
          sourceRevision: null,
          recipeVersion: null,
          settings: {
            exposureEv: 0,
            whiteBalance: Object.freeze({ mode: "as-shot" }),
          },
          sourceSupport: "unavailable",
          supportReason: "",
          editSource: "original",
          editSourceProxyId: null,
          processingAvailable: false,
          controls: {
            minimumEv: 0,
            maximumEv: 1,
            stepEv: 0.001,
            whiteBalanceModes: ["as-shot"],
            adjustableWhiteBalance: [],
          },
        });
        session.setStatus(response.message);
      } else {
        session.setStatus(response.message);
      }
      if (currentPhoto()?.id === photoId) renderEditor();
      return;
    }
    const facts = processingCapability
      ? withProcessingCapability(response.facts, processingCapability)
      : response.facts;
    const step = mode === "open" ? session.open(facts) : session.refresh(facts);
    // A comparison is of one development: a source revision that moved makes
    // the retained baseline rendition a comparison of an earlier source, so it
    // is dropped rather than presented as this Photo's.
    if (
      editorComparison &&
      !comparisonIsCurrent(editorComparison, {
        photoId,
        stage: editorStage,
        sourceRevision: session.facts()?.sourceRevision ?? null,
      })
    )
      clearEditorComparison();
    if (currentPhoto()?.id === photoId) renderEditor();
    void placeEditorWrite(photoId, session, step);
    // The surface opens on the stage's rendition, so the Edit Preview is read
    // as soon as the facts are known instead of leaving the camera Preview
    // presented under a Develop provenance.
    void requestEditorPreview(photoId);
  };
  /// Reads the current facts of one Photo. The read is stamped against the
  /// revisions in force when it starts and discarded when they moved while it
  /// was in flight, so a slow read can neither overwrite facts a save
  /// acknowledgement already confirmed nor read as another client's change.
  const loadEditorFacts = (
    photoId: string,
    mode: "open" | "refresh",
  ): Promise<void> => {
    const generation = editorRecipeGenerations.get(photoId) ?? 0;
    const inFlight = editorRecipeReads.get(photoId);
    if (inFlight) {
      if (mode === "refresh") {
        return inFlight.then(() => {
          if (editorRecipeGenerations.get(photoId) !== generation) {
            return loadEditorFacts(photoId, "refresh");
          }
        });
      }
      return inFlight;
    }
    const read = (async () => {
      const session = editorSession(photoId);
      const stampedRecipe = session.presentation().recipeVersion;
      const stampedSource = session.facts()?.sourceRevision ?? null;
      const controller = new AbortController();
      const result = await fetchEditRecipe(fetcher, photoId, controller.signal);
      if (!isAlive()) return;
      const current = editorSessions.get(photoId);
      if (
        current &&
        (current.presentation().recipeVersion !== stampedRecipe ||
          (current.facts()?.sourceRevision ?? null) !== stampedSource)
      )
        return;
      editorFactsFromWire(photoId, result, mode);
    })();
    editorRecipeReads.set(photoId, read);
    return read.finally(() => {
      if (editorRecipeReads.get(photoId) === read)
        editorRecipeReads.delete(photoId);
    });
  };
  /// Reads the deployment's processing capability once per Edit session. The
  /// report is the only source of the adjustable white-balance ranges and of
  /// the state of each stage, so a stage the deployment cannot execute is
  /// explained instead of attempted.
  const loadProcessingCapability = async (photoId: string): Promise<void> => {
    try {
      const response = await fetcher("/api/processing/capability", {
        priority: "low",
      });
      if (!response.ok) return;
      const capability = parseProcessingCapability(await response.json());
      if (!capability) return;
      processingCapability = capability;
      filmUnavailableReason = filmStageReason(capability.stages.film);
      if (
        filmUnavailableReason &&
        editorStage === "film" &&
        editorOwnsPhoto(photoId)
      )
        applyEditorStage(photoId, "develop");
      applyProcessingCapability(photoId);
      if (currentPhoto()?.id === photoId) renderEditor();
    } catch {
      /* the deployment's capability report is optional presentation */
    }
  };
  /// The deployment's capability state in the Photographer's words. Internal
  /// causes (launcher, bundle, allowance) stay service concepts: the
  /// workspace says what the Photographer can do and what stays safe.
  const capabilityNote = (state: string): string => {
    switch (state) {
      case "disabled":
        return "Editing previews and Export are not enabled in this deployment. Saved edits and downloads stay available.";
      case "launcher-unavailable":
      case "bundle-unavailable":
        return "Processing is temporarily unavailable, so previews and Export cannot run. Saved edits and downloads stay available.";
      case "source-unsupported":
        return "This Photo is not supported for editing in this deployment.";
      case "resource-unavailable":
        return "Processing is temporarily unavailable. Try again later.";
      default:
        return "";
    }
  };
  /// The deployment's answer for Film, in the workspace's words. A Photo the
  /// deployment cannot process for Film is not the same failure as a
  /// capability it has not enabled.
  const filmStageReason = (state: string): string =>
    state === "unsupported"
      ? "Film is not available for this Photo."
      : state === "unavailable"
        ? "Film is temporarily unavailable."
        : "";
  /// Applies the capability report to one Photo's editing facts, so the
  /// adjustable controls follow the report instead of assuming it.
  const applyProcessingCapability = (photoId: string): void => {
    const session = editorSessions.get(photoId);
    const current = session?.facts();
    if (!session || !current || !processingCapability) return;
    const step = session.refresh(
      withProcessingCapability(current, processingCapability),
    );
    void placeEditorWrite(photoId, session, step);
  };
  const openEditor = (photoId: string): void => {
    if (!photoId) return;
    // The Edit workspace always opens on the current edit; Film is an
    // explicit optional action, never the default view.
    editorStage = "develop";
    editorSession(photoId);
    renderEditor();
    void loadEditorFacts(photoId, "open");
    void readProxy(photoId);
    void loadProcessingCapability(photoId);
    outputs.open(photoId);
  };
  const refreshEditor = (photoId: string): void => {
    editorRecipeGenerations.set(
      photoId,
      (editorRecipeGenerations.get(photoId) ?? 0) + 1,
    );
    void loadEditorFacts(photoId, "refresh");
    void outputs.refresh(photoId);
  };
  /// The presented rendition is older than the settings in force as soon as
  /// an edit action lands. The matching rendition clears the mark when it
  /// arrives, so the workspace never presents an image as current after the
  /// settings it shows have moved.
  const markEditorPreviewStale = (): void => {
    if (editorPreviewUrl) editorPreviewStale = true;
  };
  const commitEditorExposure = (photoId: string, exposureEv: number): void => {
    const session = editorSessions.get(photoId);
    if (!session) return;
    const step = session.commitExposure(exposureEv);
    markEditorPreviewStale();
    renderEditor();
    void placeEditorWrite(photoId, session, step);
  };
  /// One white-balance action: a selected mode, a settled temperature, or a
  /// settled tint. Each is one edit action with its own guarded write.
  const commitEditorWhiteBalance = (
    photoId: string,
    action:
      | Readonly<{ kind: "mode"; mode: string }>
      | Readonly<{ kind: "temperature"; temperatureKelvin: number }>
      | Readonly<{ kind: "tint"; tintMilli: number }>,
  ): void => {
    const session = editorSessions.get(photoId);
    if (!session) return;
    const step =
      action.kind === "mode"
        ? session.selectWhiteBalanceMode(action.mode)
        : action.kind === "temperature"
          ? session.commitTemperature(action.temperatureKelvin)
          : session.commitTint(action.tintMilli);
    markEditorPreviewStale();
    renderEditor();
    void placeEditorWrite(photoId, session, step);
  };
  const stepEditorHistory = (
    photoId: string,
    operation:
      | "undo"
      | "redo"
      | "reset"
      | "resetExposure"
      | "resetWhiteBalance",
  ): void => {
    const session = editorSessions.get(photoId);
    if (!session) return;
    const step =
      operation === "undo"
        ? session.undo()
        : operation === "redo"
          ? session.redo()
          : operation === "resetExposure"
            ? session.resetExposure()
            : operation === "resetWhiteBalance"
              ? session.resetWhiteBalance()
              : session.reset();
    markEditorPreviewStale();
    renderEditor();
    void placeEditorWrite(photoId, session, step);
  };
  /// Uses the recipe the service holds now. The read is the authoritative
  /// source of the saved settings, so the conflict resolves to the service's
  /// recipe instead of this client's older confirmed copy; a read that fails
  /// leaves the conflict standing for another attempt.
  const useSavedRecipe = async (photoId: string): Promise<void> => {
    const session = editorSessions.get(photoId);
    if (!session) return;
    await loadEditorFacts(photoId, "refresh");
    const current = editorSessions.get(photoId);
    if (!current) return;
    const step = current.useSavedRecipe();
    renderEditor();
    void placeEditorWrite(photoId, current, step);
    void requestEditorPreview(photoId);
  };
  const reapplyLocalSettings = (photoId: string): void => {
    const session = editorSessions.get(photoId);
    if (!session) return;
    const step = session.reapplyLocal();
    markEditorPreviewStale();
    renderEditor();
    void placeEditorWrite(photoId, session, step);
  };
  const discardEditorDraft = (photoId: string): void => {
    const session = editorSessions.get(photoId);
    if (!session) return;
    session.discardDraft();
    renderEditor();
  };
  const applyEditorStage = (photoId: string, stage: EditorStage): void => {
    if (
      stage === "film" &&
      (filmUnavailableReason ||
        !editorSessions.get(photoId)?.presentation().canEdit ||
        !editorSessions.get(photoId)?.presentation().processingAvailable)
    )
      return;
    if (stage === editorStage) return;
    editorStage = stage;
    editorComparing = false;
    // A comparison is of one stage: the rendition of another stage, or of the
    // other settings selector, is not this stage's comparison.
    clearEditorComparison();
    // A rendition of another view is never presented as this view's result:
    // the surface shows no edited image until this view's own preview
    // arrives, so Film can never fall back to a camera or Develop image.
    clearEditorPreview();
    renderEditor();
    if (stage !== "camera") void requestEditorPreview(photoId);
  };
  /// The as-shot/baseline development comparison of the chosen stage. Pressing
  /// the control presents the baseline development of the same stage beside the
  /// current settings; releasing it presents the current rendition again. The
  /// comparison never replaces the current rendition, the chosen stage, or the
  /// saved recipe, and a comparison whose rendition is still being prepared
  /// says so instead of presenting an unrelated image.
  const setEditorComparison = (photoId: string, pressed: boolean): void => {
    if (!isCurrentPhoto(photoId) || currentPhoto()?.id !== photoId) return;
    editorComparing = pressed;
    if (pressed) {
      if (editorComparisonUrl) view.presentEditorPreview(editorComparisonUrl);
      else if (!editorComparisonBusy) {
        if (!editorComparison)
          editorComparisonNote = "Preparing the comparison…";
        void requestEditorComparisonPreview(photoId);
      }
    } else if (editorPreviewUrl) view.presentEditorPreview(editorPreviewUrl);
    else view.clearEditorPreview();
    renderEditor();
  };
  /// A full RAW development can take over a minute. Continue polling an
  /// admitted comparison for up to five minutes, then offer a fresh request.
  const scheduleEditorComparisonFollowUp = (photoId: string): void => {
    if (editorComparisonAttempts >= PREVIEW_POLL_LIMIT) {
      editorComparisonNote =
        "The comparison is taking too long. Compare again to check its result.";
      renderEditor();
      return;
    }
    editorComparisonAttempts += 1;
    if (editorComparisonTimer !== undefined)
      clearTimeout(editorComparisonTimer);
    editorComparisonTimer = window.setTimeout(() => {
      editorComparisonTimer = undefined;
      if (!editorOwnsPhoto(photoId)) return;
      void requestEditorComparisonPreview(photoId, true);
    }, PREVIEW_POLL_MS);
  };
  /// One comparison request for the chosen stage. The comparison is its own
  /// rendition: it is requested under the closed `baseline` selector, it never
  /// replaces the current rendition's image or note, and a rendition served for
  /// another Photo, stage, settings selector, or source revision is refused
  /// rather than presented as the comparison.
  const requestEditorComparisonPreview = async (
    photoId: string,
    followUp = false,
  ): Promise<void> => {
    const session = editorSessions.get(photoId);
    const presented = session?.presentation();
    if (
      !session ||
      !presented ||
      !presented.canEdit ||
      !presented.processingAvailable ||
      // The camera stage presents the camera Preview: the baseline of a stage
      // the deployment does not execute is not a comparison it can render.
      editorStage === "camera" ||
      !editorOwnsPhoto(photoId)
    )
      return;
    const stage = editorStage;
    const expected: Comparison = {
      photoId,
      stage,
      sourceRevision: session.facts()?.sourceRevision ?? null,
    };
    if (!followUp) editorComparisonAttempts = 0;
    const generation = ++editorComparisonGeneration;
    editorComparisonAbort?.abort();
    const controller = new AbortController();
    editorComparisonAbort = controller;
    editorComparisonBusy = true;
    editorComparisonNote = "Preparing the comparison…";
    renderEditor();
    let response: Response;
    try {
      response = await fetcher(
        editPreviewUri(photoId, stage, BASELINE_SETTINGS),
        {
          signal: controller.signal,
          priority: "high",
        },
      );
    } catch {
      if (generation === editorComparisonGeneration) {
        editorComparisonBusy = false;
        editorComparisonNote =
          "The comparison request did not reach the service.";
        renderEditor();
      }
      return;
    }
    if (generation !== editorComparisonGeneration || !editorOwnsPhoto(photoId))
      return;
    editorComparisonBusy = false;
    if (response.status === 202) {
      const body: unknown = await response.json().catch(() => undefined);
      const state =
        isRecord(body) && typeof body["state"] === "string"
          ? body["state"]
          : "queued";
      editorComparisonNote =
        state === "running"
          ? "Rendering the comparison…"
          : "Preparing the comparison…";
      renderEditor();
      scheduleEditorComparisonFollowUp(photoId);
      return;
    }
    if (!response.ok) {
      editorComparisonNote = await describePreviewRefusal(response);
      renderEditor();
      return;
    }
    let image: Blob;
    try {
      image = await response.blob();
    } catch {
      editorComparisonNote = "The comparison could not be read. Compare again.";
      renderEditor();
      return;
    }
    if (generation !== editorComparisonGeneration || !editorOwnsPhoto(photoId))
      return;
    const refusal = comparisonRefusal(response.headers, expected);
    if (refusal) {
      editorComparisonNote = refusal;
      renderEditor();
      return;
    }
    if (editorComparisonUrl) URL.revokeObjectURL(editorComparisonUrl);
    editorComparisonUrl = URL.createObjectURL(image);
    editorComparison = expected;
    const width = response.headers.get("slipstream-edit-preview-width") ?? "?";
    const height =
      response.headers.get("slipstream-edit-preview-height") ?? "?";
    editorComparisonNote = `Comparison ${width}×${height}: the unadjusted rendering${stage === "film" ? " with the film look" : ""}. The current settings are unchanged.`;
    if (editorComparing) view.presentEditorPreview(editorComparisonUrl);
    renderEditor();
  };
  /// Poll admitted work long enough for a full RAW render. Stop after five
  /// minutes so a lost or stuck operation does not poll indefinitely.
  const PREVIEW_POLL_MS = 750;
  const PREVIEW_POLL_LIMIT = 400;
  const scheduleEditorPreviewFollowUp = (photoId: string): void => {
    if (editorPreviewAttempts >= PREVIEW_POLL_LIMIT) {
      editorPreviewNote =
        "The preview is taking too long. Refresh the preview to check its result.";
      editorPreviewOutcome = "unknown";
      renderEditor();
      return;
    }
    editorPreviewAttempts += 1;
    if (editorPreviewTimer !== undefined) clearTimeout(editorPreviewTimer);
    editorPreviewTimer = window.setTimeout(() => {
      editorPreviewTimer = undefined;
      void requestEditorPreview(photoId, true);
    }, PREVIEW_POLL_MS);
  };

  /// One preview request follows each completed edit action and each settled
  /// save. A retained image stays presented and is marked out of date until
  /// the matching rendition arrives, and an image for another source, stage,
  /// settings snapshot, or edit source is refused.
  const requestEditorPreview = async (
    photoId: string,
    followUp = false,
  ): Promise<void> => {
    const session = editorSessions.get(photoId);
    const presented = session?.presentation();
    if (
      !session ||
      !presented ||
      !presented.canEdit ||
      !presented.processingAvailable ||
      // The camera stage presents the camera Preview: it has no rendition of
      // its own to request, and the closed route admits only Develop and Film.
      editorStage === "camera" ||
      !editorOwnsPhoto(photoId)
    )
      return;
    const facts = session.facts();
    const expectedSource = facts?.sourceRevision ?? null;
    const expectedRecipe = presented.recipeVersion;
    const expectedEditSource = facts?.editSource ?? "original";
    const expectedProxyId = facts?.editSourceProxyId ?? null;
    const stage = editorStage;
    const identity = [
      photoId,
      stage,
      expectedSource ?? "",
      expectedRecipe ?? "",
      expectedEditSource,
      expectedProxyId ?? "",
    ].join("|");
    // A retry of the identity already in flight joins that request: the
    // in-flight attempt settles for this owner, and no duplicate physical
    // render is admitted behind it.
    if (editorPreviewBusy && editorPreviewIdentity === identity) return;
    if (!followUp) editorPreviewAttempts = 0;
    const generation = ++editorPreviewGeneration;
    editorPreviewAbort?.abort();
    const controller = new AbortController();
    editorPreviewAbort = controller;
    editorPreviewBusy = true;
    editorPreviewIdentity = identity;
    editorPreviewRefused = false;
    editorPreviewOutcome = "pending";
    if (editorPreviewUrl) {
      editorPreviewStale = true;
      editorPreviewNote = "This preview is older than the current settings.";
    } else {
      editorPreviewNote = "Updating preview…";
    }
    renderEditor();
    let response: Response;
    try {
      response = await fetcher(
        editPreviewUri(photoId, stage, CURRENT_SETTINGS),
        { signal: controller.signal, priority: "high" },
      );
    } catch {
      if (generation === editorPreviewGeneration) {
        editorPreviewBusy = false;
        editorPreviewNote = "The preview request did not reach the service.";
        editorPreviewRefused = !editorPreviewUrl;
        editorPreviewOutcome = "failed";
        renderEditor();
      }
      return;
    }
    if (generation !== editorPreviewGeneration || !editorOwnsPhoto(photoId))
      return;
    editorPreviewBusy = false;
    if (response.status === 202) {
      const body: unknown = await response.json().catch(() => undefined);
      const state =
        isRecord(body) && typeof body["state"] === "string"
          ? body["state"]
          : "queued";
      // Queued work is waiting for admission, not computing: the note names
      // the capacity wait, and a retained image stays presented as older than
      // the current settings instead of being replaced or hidden.
      editorPreviewNote =
        state === "running"
          ? "Rendering the preview. The image shown is older than the current settings."
          : "Updating preview…";
      if (editorPreviewUrl) editorPreviewStale = true;
      renderEditor();
      scheduleEditorPreviewFollowUp(photoId);
      return;
    }
    if (!response.ok) {
      editorPreviewRefused = !editorPreviewUrl;
      editorPreviewNote = await describePreviewRefusal(response);
      editorPreviewOutcome = "failed";
      renderEditor();
      return;
    }
    let image: Blob;
    try {
      image = await response.blob();
    } catch {
      editorPreviewRefused = !editorPreviewUrl;
      editorPreviewNote =
        "The preview could not be read. Refresh the preview to try again.";
      editorPreviewOutcome = "failed";
      renderEditor();
      return;
    }
    if (generation !== editorPreviewGeneration || !editorOwnsPhoto(photoId))
      return;
    const refusal = currentRenditionRefusal(response.headers, image.size, {
      photoId,
      stage,
      sourceRevision: expectedSource,
      recipeVersion: expectedRecipe ?? "",
      editSource: expectedEditSource,
      editSourceProxyId: expectedProxyId,
    });
    if (refusal) {
      editorPreviewRefused = !editorPreviewUrl;
      editorPreviewNote = refusal;
      editorPreviewOutcome = "failed";
      renderEditor();
      return;
    }
    if (editorPreviewUrl) URL.revokeObjectURL(editorPreviewUrl);
    editorPreviewUrl = URL.createObjectURL(image);
    editorPreviewStale = false;
    editorPreviewRefused = false;
    editorPreviewOutcome = "ready";
    const width = response.headers.get("slipstream-edit-preview-width") ?? "?";
    const height =
      response.headers.get("slipstream-edit-preview-height") ?? "?";
    const transform =
      response.headers.get("slipstream-edit-preview-display-transform") ?? "";
    const provenance =
      expectedEditSource === "development-proxy"
        ? ", from the Development Proxy edit source"
        : "";
    editorPreviewNote = `${stage === "film" ? "Film" : "Edit"} preview ${width}×${height} at the current settings${provenance}${transform ? `, display transform ${transform}` : ""}.`;
    if (!editorComparing) view.presentEditorPreview(editorPreviewUrl);
    renderEditor();
  };
  /// The closed refusal set of a preview in the workspace's words. Internal
  /// causes stay service concepts; a code this client does not know is
  /// disclosed as the service's own rather than explained away.
  const describePreviewRefusal = async (
    response: Response,
  ): Promise<string> => {
    const body: unknown = await response.json().catch(() => undefined);
    const error = isRecord(body) ? body["error"] : undefined;
    const code =
      isRecord(error) && typeof error["code"] === "string" ? error["code"] : "";
    const reason =
      isRecord(error) &&
      isRecord(error["details"]) &&
      typeof error["details"]["reason"] === "string"
        ? error["details"]["reason"]
        : "";
    if (code === "processing_unavailable") {
      if (reason === "preview-render-admission-unavailable")
        return "Previews are not available yet in this deployment, so no preview is shown. Editing, Export, and download still work.";
      return "Processing is not available for this Photo right now, so no preview is shown.";
    }
    if (code === "resource_unavailable") {
      // A refusal that follows from the Photo's source state carries the
      // same closed reason the edit read reports, so the preview surface
      // agrees with the read: a retryable wait names its retry, a confirmed
      // outcome explains the permanent read failure.
      const sourceReason = asEditorSupportReason(reason);
      if (sourceReason !== "") {
        const note = `${plainEditorMessage(
          supportReasonExplanation(sourceReason),
        )}, so no preview is shown.`;
        return isRetryableSupportReason(sourceReason)
          ? `${note} Refresh the preview once the current Library work settles.`
          : note;
      }
      return "Could not create the preview right now. Refresh the preview to try again.";
    }
    if (code === "unsupported_photo")
      return "This Photo is not supported for editing, so no preview is shown.";
    if (code === "unknown_photo")
      return "This Photo is no longer in the Library, so no preview is shown.";
    return code
      ? `The service refused the preview: ${code}${reason ? ` (${reason})` : ""}.`
      : `The preview request failed with HTTP ${response.status}.`;
  };
  /// Rebinds the stored recipe to the currently observed source. It is the one
  /// explicit reconciliation the workspace offers when the service reports the
  /// saved recipe is bound to different content.
  const rebindEditor = async (photoId: string): Promise<void> => {
    const session = editorSessions.get(photoId);
    const facts = session?.facts();
    const presented = session?.presentation();
    if (!session || !facts || !presented) return;
    if (!presented.recipeVersion || !facts.sourceRevision) {
      session.setStatus("There is no saved edit for this Photo.");
      renderEditor();
      return;
    }
    const result = await rebindEditRecipe(
      fetcher,
      photoId,
      presented.recipeVersion,
      facts.sourceRevision,
    );
    if (!isAlive()) return;
    if (result.kind !== "saved") {
      session.setStatus(
        result.refusal.message ||
          "The update was refused. Reload the edit to read the current state.",
      );
      renderEditor();
      return;
    }
    await loadEditorFacts(photoId, "refresh");
  };
  /// Leaving Photo View releases the presented image but keeps each Photo's
  /// session: a pending save still settles under its own identity, and the
  /// local draft remains for a later visit.
  const readProxy = async (photoId: string): Promise<void> => {
    const generation = ++editorProxyGeneration;
    const result = await fetchDevelopmentProxy(fetcher, photoId);
    if (generation !== editorProxyGeneration || !editorOwnsPhoto(photoId))
      return;
    if (result.kind === "failed") {
      editorProxyFailure = result.message;
      renderEditor();
      return;
    }
    editorProxyFailure = "";
    editorProxy = result.status;
    renderEditor();
    if (result.status.state === "building") {
      if (editorProxyTimer !== undefined) clearTimeout(editorProxyTimer);
      editorProxyTimer = window.setTimeout(() => {
        editorProxyTimer = undefined;
        void readProxy(photoId);
      }, 500);
    } else if (result.status.state === "current") {
      void loadEditorFacts(photoId, "refresh");
    }
  };
  const createProxy = async (photoId: string): Promise<void> => {
    const session = editorSessions.get(photoId);
    const sourceRevision = session?.facts()?.sourceRevision;
    if (!session || !sourceRevision) return;
    editorProxyFailure = "";
    editorProxy = { photoId, state: "building", proxy: null, failure: null };
    renderEditor();
    const result = await createDevelopmentProxy(
      fetcher,
      photoId,
      sourceRevision,
    );
    if (!editorOwnsPhoto(photoId)) return;
    if (result.kind === "failed") editorProxyFailure = result.message;
    else editorProxy = result.status;
    renderEditor();
    void readProxy(photoId);
  };
  const removeProxy = async (photoId: string): Promise<void> => {
    const result = await removeDevelopmentProxy(fetcher, photoId);
    if (!editorOwnsPhoto(photoId)) return;
    if (result.kind === "failed") editorProxyFailure = result.message;
    else editorProxy = result.status;
    renderEditor();
    if (result.kind === "ok") void loadEditorFacts(photoId, "refresh");
  };
  const leaveEditor = (): void => {
    editorPreviewAbort?.abort();
    editorPreviewAbort = undefined;
    editorPreviewGeneration += 1;
    editorPreviewBusy = false;
    editorPreviewIdentity = undefined;
    outputs.leave();
    clearEditorPreview();
    editorComparing = false;
    editorStage = "develop";
    settleEditorWriters();
  };
  return {
    open: openEditor,
    refresh: refreshEditor,
    commitExposure: commitEditorExposure,
    commitWhiteBalance: commitEditorWhiteBalance,
    stepHistory: stepEditorHistory,
    requestPreview: (photoId) => {
      void requestEditorPreview(photoId);
    },
    applyStage: applyEditorStage,
    setComparison: setEditorComparison,
    useSaved: (photoId) => {
      void useSavedRecipe(photoId);
    },
    reapplyLocal: reapplyLocalSettings,
    discardDraft: discardEditorDraft,
    rebind: (photoId) => {
      void rebindEditor(photoId);
    },
    createProxy: (photoId) => {
      void createProxy(photoId);
    },
    removeProxy: (photoId) => {
      void removeProxy(photoId);
    },
    submitXmp: (photoId) => {
      void outputs.submit(photoId);
    },
    downloadXmp: (photoId) => {
      void outputs.download(photoId);
    },
    submitExport: (photoId, target) => {
      void outputs.submit(photoId, target);
    },
    cancelExport: (photoId, target) => {
      void outputs.cancel(photoId, target);
    },
    retryExport: (photoId, target) => {
      void outputs.retry(photoId, target);
    },
    downloadExport: (photoId, target) => {
      void outputs.download(photoId, target);
    },
    leave: leaveEditor,
  };
}
