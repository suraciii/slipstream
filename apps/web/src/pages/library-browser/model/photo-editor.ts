//! One Photo's editing session in the browser: the confirmed Edit Recipe, the
//! local settings a Photographer has not had confirmed yet, session undo/redo,
//! conflict reconciliation, and the bounded local draft that survives a reload.
//!
//! The model owns no network work and no DOM. A caller feeds it the facts of
//! one serialized read, the acknowledgements and refusals of the writes it
//! sent, and receives the next guarded write to place in that Photo's write
//! stream. That keeps the contract in
//! [Photo Development](../../../../design/photo-development.md#recipe-writes-and-autosave)
//! testable without a browser.

/// The closed white-balance intent of the service's wire contract. `as-shot`
/// carries no other field; `temperature-tint` carries the Kelvin and tint the
/// qualified mapping pins. A stored intent whose mode the deployment does not
/// admit stays readable as retained intent and is never rewritten to as-shot.
export type EditorWhiteBalance =
  | Readonly<{ mode: "as-shot" }>
  | Readonly<{
      mode: "temperature-tint";
      temperatureKelvin: number;
      tintMilli: number;
    }>;

export type EditorSettings = Readonly<{
  exposureEv: number;
  whiteBalance: EditorWhiteBalance;
}>;

/// The two closed intents, so a caller that has already established which one
/// is in force never reads a field the other does not carry.
export type AsShotIntent = Extract<EditorWhiteBalance, { mode: "as-shot" }>;
export type TemperatureTintIntent = Extract<
  EditorWhiteBalance,
  { mode: "temperature-tint" }
>;

/// One closed interval a control may use.
export type WhiteBalanceRange = Readonly<{
  minimum: number;
  maximum: number;
}>;

/// One adjustable white-balance mode the deployment reports as admitted for
/// this Photo's source class, with the ranges its controls may use. The
/// capability report is the only source of these facts, so an editor enables
/// an adjustable control only from here.
export type AdmittedWhiteBalance = Readonly<{
  mode: "temperature-tint";
  temperatureKelvin: WhiteBalanceRange;
  tintMilli: WhiteBalanceRange;
}>;

export type EditorControls = Readonly<{
  minimumEv: number;
  maximumEv: number;
  stepEv: number;
  whiteBalanceModes: ReadonlyArray<string>;
  /// Empty while the deployment admits no adjustable mode for this class.
  adjustableWhiteBalance: ReadonlyArray<AdmittedWhiteBalance>;
}>;

/// The facts of one `GET /api/photos/{id}/edit-recipe` read.
export type EditorFacts = Readonly<{
  photoId: string;
  sourceRevision: string | null;
  recipeVersion: string | null;
  settings: EditorSettings;
  sourceSupport: "supported" | "unsupported" | "unavailable";
  supportReason: string;
  processingAvailable: boolean;
  controls: EditorControls;
}>;

/// One guarded recipe write. The identity is chosen once and reused by a
/// retry, so the service resolves a lost response to the committed revision.
export type SaveRequest = Readonly<{
  id: string;
  photoId: string;
  expectedRecipeVersion: string | null;
  expectedSourceRevision: string;
  settings: EditorSettings;
}>;

/// The refusal of one guarded write, with the facts a conflict discloses.
export type SaveRefusal = Readonly<{
  status: number;
  code: string;
  message: string;
  currentRecipeVersion: string | null;
  currentSourceRevision: string | null;
}>;

/// The bounded local draft store. An unavailable or full store is not a
/// reason to withhold a healthy save; it only changes what recovery promises.
export type DraftStore = Readonly<{
  read(key: string): string | null;
  write(key: string, value: string): boolean;
  remove(key: string): void;
}>;

export type EditorDraftState = Readonly<{
  kind: "none" | "recovered" | "session";
  note: string;
}>;

export type EditorConflict = Readonly<{
  message: string;
  savedSettings: EditorSettings;
  /// The local settings a Photographer may reapply against the newly observed
  /// revision. Reapplying may conflict again.
  localSettings: EditorSettings;
}>;

/// One white-balance control the workspace presents: its current value, the
/// interval it may use, and whether it accepts input.
export type WhiteBalanceControl = Readonly<{
  value: number;
  minimum: number;
  maximum: number;
  enabled: boolean;
}>;

/// What the workspace presents for white balance: the intent in force, the
/// modes a Photographer may select, and why an adjustable mode is not offered.
export type EditorWhiteBalancePresentation = Readonly<{
  intent: EditorWhiteBalance;
  /// The modes the editor may select, in presentation order.
  modes: ReadonlyArray<string>;
  /// True when the adjustable mode is admitted and its controls take input.
  adjustable: boolean;
  /// Why no adjustable mode is offered, when none is.
  note: string;
  temperatureKelvin: WhiteBalanceControl | null;
  tintMilli: WhiteBalanceControl | null;
  /// True when white balance differs from as-shot, so a reset has work to do.
  resettable: boolean;
}>;

export type EditorPresentation = Readonly<{
  photoId: string;
  settings: EditorSettings;
  confirmed: EditorSettings;
  baseline: EditorSettings;
  controls: EditorControls;
  whiteBalance: EditorWhiteBalancePresentation;
  sourceSupport: "supported" | "unsupported" | "unavailable" | "unknown";
  processingAvailable: boolean;
  /// The revision the service confirmed for the settings the Photographer
  /// sees as saved. A guarded write and an Export both start from it.
  recipeVersion: string | null;
  canEdit: boolean;
  saving: boolean;
  dirty: boolean;
  canUndo: boolean;
  canRedo: boolean;
  conflict: EditorConflict | null;
  draft: EditorDraftState;
  status: string;
}>;

export type EditorStep = Readonly<{
  presentation: EditorPresentation;
  request: SaveRequest | null;
}>;

export type PhotoEditor = Readonly<{
  open: (facts: EditorFacts) => EditorStep;
  /// Reconciles a later read of the same Photo. Local intent survives, and a
  /// read that moves the service forward resolves a conflict.
  refresh: (facts: EditorFacts) => EditorStep;
  /// Resolves a save whose outcome is unknown through the identity it already
  /// carries, so the service answers the identical retry from its receipt.
  resolveUnknown: () => EditorStep;
  /// Writes the settings in force now, for the Export ordering barrier that
  /// commits the visible intent before it captures a confirmed revision.
  commitCurrent: () => EditorStep;
  /// One completed edit action: a whole pointer drag, a committed numeric
  /// entry, or a settled keyboard adjustment.
  commitExposure: (exposureEv: number) => EditorStep;
  /// Selects an admitted white-balance mode. A mode the deployment does not
  /// admit is refused here rather than sent as an unexecutable intent.
  selectWhiteBalanceMode: (mode: string) => EditorStep;
  commitTemperature: (temperatureKelvin: number) => EditorStep;
  commitTint: (tintMilli: number) => EditorStep;
  undo: () => EditorStep;
  redo: () => EditorStep;
  /// Restores the processing baseline of exposure alone.
  resetExposure: () => EditorStep;
  /// Restores as-shot white balance alone.
  resetWhiteBalance: () => EditorStep;
  /// Restores both, the baseline of the whole recipe.
  reset: () => EditorStep;
  acknowledge: (
    request: SaveRequest,
    committed: Readonly<{ recipeVersion: string; sourceRevision: string }>,
  ) => EditorStep;
  refuse: (request: SaveRequest, refusal: SaveRefusal) => EditorStep;
  /// Uses the recipe the service holds and drops the local settings.
  useSavedRecipe: () => EditorStep;
  /// Writes the local settings against the newly observed revision.
  reapplyLocal: () => EditorStep;
  discardDraft: () => EditorPresentation;
  setStatus: (status: string) => EditorPresentation;
  presentation: () => EditorPresentation;
  facts: () => EditorFacts | null;
}>;

const BASELINE_MODE = "as-shot";
const ADJUSTABLE_MODE = "temperature-tint";
/// The published payload bounds of the closed `temperature-tint` shape. They
/// are fixed by the service's wire contract, so a conforming client can always
/// construct a valid request even while no mode is admitted for execution.
const TEMPERATURE_KELVIN_BOUNDS = Object.freeze({
  minimum: 1000,
  maximum: 40000,
});
const TINT_MILLI_BOUNDS = Object.freeze({ minimum: -150000, maximum: 150000 });
const MAXIMUM_HISTORY = 64;
const MAXIMUM_DRAFTS = 8;
const DRAFT_KEY_PREFIX = "slipstream.photo-draft.v1.";
const DRAFT_INDEX_KEY = "slipstream.photo-draft.v1.index";

/// The as-shot baseline of the first workload: 0 EV and the as-shot intent.
const baselineSettings = (): EditorSettings =>
  Object.freeze({
    exposureEv: 0,
    whiteBalance: Object.freeze({ mode: "as-shot" }) as EditorWhiteBalance,
  });

/// The nearest value the closed control admits, so a client never sends a
/// setting the service refuses as outside the closed shape.
const admittedExposure = (
  controls: EditorControls,
  exposureEv: number,
): number => {
  if (!Number.isFinite(exposureEv)) return controls.minimumEv;
  const clamped = Math.min(
    controls.maximumEv,
    Math.max(controls.minimumEv, exposureEv),
  );
  if (controls.stepEv <= 0) return clamped;
  const steps = Math.round((clamped - controls.minimumEv) / controls.stepEv);
  const stepped = controls.minimumEv + steps * controls.stepEv;
  // The wire carries a decimal, and a float artefact of the step would be
  // refused as outside the closed values.
  return Number(stepped.toFixed(6));
};

const admittedInteger = (
  range: WhiteBalanceRange,
  value: number,
  bounds: WhiteBalanceRange,
): number => {
  if (!Number.isFinite(value)) return range.minimum;
  const minimum = Math.max(range.minimum, bounds.minimum);
  const maximum = Math.min(range.maximum, bounds.maximum);
  const clamped = Math.min(maximum, Math.max(minimum, Math.round(value)));
  return clamped;
};

/// The admitted adjustable white-balance mode of this deployment, when it
/// admits one. Nothing else may enable the temperature and tint controls.
const admittedAdjustable = (
  controls: EditorControls,
): AdmittedWhiteBalance | undefined =>
  controls.adjustableWhiteBalance.find(
    (entry) => entry.mode === ADJUSTABLE_MODE,
  );

const sameWhiteBalance = (
  left: EditorWhiteBalance,
  right: EditorWhiteBalance,
): boolean => {
  if (left.mode !== right.mode) return false;
  if (left.mode === "as-shot" || right.mode === "as-shot") return true;
  return (
    left.temperatureKelvin === right.temperatureKelvin &&
    left.tintMilli === right.tintMilli
  );
};

const sameSettings = (left: EditorSettings, right: EditorSettings): boolean =>
  left.exposureEv === right.exposureEv &&
  sameWhiteBalance(left.whiteBalance, right.whiteBalance);

const asShot = (): AsShotIntent => Object.freeze({ mode: "as-shot" });

/// Why this Photo has no editing, in the workspace's words. The service
/// reports the closed state and reason; the surface explains them.
const sourceUnavailableStatus = (facts: EditorFacts): string =>
  facts.sourceSupport === "unsupported"
    ? "This Photo's source class has no approved profile in this deployment, so its settings are read-only."
    : facts.supportReason === "original-missing"
      ? "This Photo's Original File is missing from its remembered Location, so its settings are read-only."
      : facts.supportReason === "original-unreadable"
        ? "This Photo's Original File cannot be read right now, so its settings are read-only."
        : "Current source facts are unavailable, so this Photo's settings are read-only.";

const temperatureTint = (
  temperatureKelvin: number,
  tintMilli: number,
): TemperatureTintIntent =>
  Object.freeze({ mode: "temperature-tint", temperatureKelvin, tintMilli });

/// The temperature and tint a Photographer last used in this session, so
/// selecting the adjustable mode again resumes their intent instead of
/// inventing a value.
const defaultTemperatureTint = (
  admitted: AdmittedWhiteBalance,
): TemperatureTintIntent =>
  temperatureTint(
    admittedInteger(
      admitted.temperatureKelvin,
      (admitted.temperatureKelvin.minimum +
        admitted.temperatureKelvin.maximum) /
        2,
      TEMPERATURE_KELVIN_BOUNDS,
    ),
    admittedInteger(
      admitted.tintMilli,
      (admitted.tintMilli.minimum + admitted.tintMilli.maximum) / 2,
      TINT_MILLI_BOUNDS,
    ),
  );

type DraftRecord = Readonly<{
  photoId: string;
  sourceRevision: string;
  recipeVersion: string | null;
  settings: EditorSettings;
  requestId: string;
}>;

/// The closed reading of one stored white-balance intent. A shape outside the
/// contract is not a draft this client may replay.
const parseWhiteBalance = (value: unknown): EditorWhiteBalance | null => {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    return null;
  const record = value as Record<string, unknown>;
  const mode = record["mode"];
  if (mode === "as-shot") return asShot();
  if (mode !== "temperature-tint") return null;
  const temperatureKelvin = record["temperatureKelvin"];
  const tintMilli = record["tintMilli"];
  if (
    typeof temperatureKelvin !== "number" ||
    typeof tintMilli !== "number" ||
    !Number.isInteger(temperatureKelvin) ||
    !Number.isInteger(tintMilli)
  )
    return null;
  return temperatureTint(temperatureKelvin, tintMilli);
};

const parseDraft = (
  value: string | null,
  photoId: string,
): DraftRecord | null => {
  if (!value) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(value);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed))
    return null;
  const record = parsed as Record<string, unknown>;
  const settings = record["settings"];
  if (
    record["photoId"] !== photoId ||
    typeof record["sourceRevision"] !== "string" ||
    typeof record["requestId"] !== "string" ||
    (typeof record["recipeVersion"] !== "string" &&
      record["recipeVersion"] !== null) ||
    typeof settings !== "object" ||
    settings === null ||
    Array.isArray(settings)
  )
    return null;
  const values = settings as Record<string, unknown>;
  const whiteBalance = parseWhiteBalance(values["whiteBalance"]);
  if (
    typeof values["exposureEv"] !== "number" ||
    !Number.isFinite(values["exposureEv"]) ||
    whiteBalance === null
  )
    return null;
  return Object.freeze({
    photoId,
    sourceRevision: record["sourceRevision"],
    recipeVersion: record["recipeVersion"],
    requestId: record["requestId"],
    settings: Object.freeze({
      exposureEv: values["exposureEv"],
      whiteBalance,
    }),
  });
};

const readIndex = (store: DraftStore): ReadonlyArray<string> => {
  const value = store.read(DRAFT_INDEX_KEY);
  if (!value) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(value);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  return parsed.filter((entry): entry is string => typeof entry === "string");
};

/// The bounded draft index. A store that cannot hold another unconfirmed draft
/// keeps the new one in memory only: an unconfirmed draft is never evicted to
/// make room for another.
const writeDraft = (
  store: DraftStore,
  key: string,
  record: DraftRecord,
): boolean => {
  const index = readIndex(store);
  if (!index.includes(record.photoId)) {
    if (index.length >= MAXIMUM_DRAFTS) return false;
    const next = [...index, record.photoId];
    if (!store.write(DRAFT_INDEX_KEY, JSON.stringify(next))) return false;
  }
  return store.write(key, JSON.stringify(record));
};

const clearDraft = (store: DraftStore, key: string, photoId: string): void => {
  store.remove(key);
  const index = readIndex(store);
  if (!index.includes(photoId)) return;
  store.write(
    DRAFT_INDEX_KEY,
    JSON.stringify(index.filter((entry) => entry !== photoId)),
  );
};

export const createPhotoEditor = (options: {
  store?: DraftStore | undefined;
  nextRequestId?: (() => string) | undefined;
  maximumHistory?: number | undefined;
}): PhotoEditor => {
  const store = options.store;
  const maximumHistory = options.maximumHistory ?? MAXIMUM_HISTORY;
  const nextRequestId =
    options.nextRequestId ??
    (() => `web-edit-${crypto.randomUUID().replaceAll("-", "").slice(0, 24)}`);

  let facts: EditorFacts | null = null;
  let confirmed: EditorSettings = baselineSettings();
  let settings: EditorSettings = baselineSettings();
  /// The temperature and tint this session last used, so selecting the
  /// adjustable mode again resumes the Photographer's intent.
  let lastTemperatureTint: TemperatureTintIntent | null = null;
  let recipeVersion: string | null = null;
  let sourceRevision: string | null = null;
  let history: EditorSettings[] = [baselineSettings()];
  let historyIndex = 0;
  let historyInvalidated = false;
  let inFlight: SaveRequest | null = null;
  let conflict: EditorConflict | null = null;
  let draft: EditorDraftState = Object.freeze({ kind: "none", note: "" });
  let draftRequestId: string | null = null;
  /// The payload one retained identity was sent with, while its outcome is
  /// unknown. The identical retry carries it, so the receipt resolves that
  /// operation instead of refusing a different payload under the identity.
  let draftRequestSettings: EditorSettings | null = null;
  let status = "";

  const draftKey = (photoId: string): string => `${DRAFT_KEY_PREFIX}${photoId}`;

  const controls = (): EditorControls =>
    facts?.controls ?? {
      minimumEv: 0,
      maximumEv: 1,
      stepEv: 0.001,
      whiteBalanceModes: [BASELINE_MODE],
      adjustableWhiteBalance: [],
    };

  const editable = (): boolean =>
    Boolean(
      facts &&
        facts.sourceSupport === "supported" &&
        sourceRevision &&
        !conflict,
    );

  /// What the workspace presents for white balance. An admitted adjustable
  /// mode enables the temperature and tint controls; otherwise the intent in
  /// force stays readable and the workspace explains why it cannot change.
  const whiteBalancePresentation = (): EditorWhiteBalancePresentation => {
    const admitted = admittedAdjustable(controls());
    const modes = controls().whiteBalanceModes;
    const intent = settings.whiteBalance;
    const adjustable = admitted !== undefined && editable();
    const active =
      adjustable && intent.mode === ADJUSTABLE_MODE ? intent : undefined;
    const temperatureKelvin =
      admitted === undefined
        ? null
        : Object.freeze({
            value:
              active?.temperatureKelvin ??
              admittedInteger(
                admitted.temperatureKelvin,
                admitted.temperatureKelvin.minimum,
                TEMPERATURE_KELVIN_BOUNDS,
              ),
            minimum: Math.max(
              admitted.temperatureKelvin.minimum,
              TEMPERATURE_KELVIN_BOUNDS.minimum,
            ),
            maximum: Math.min(
              admitted.temperatureKelvin.maximum,
              TEMPERATURE_KELVIN_BOUNDS.maximum,
            ),
            enabled: adjustable,
          });
    const tintMilli =
      admitted === undefined
        ? null
        : Object.freeze({
            value:
              active?.tintMilli ??
              admittedInteger(
                admitted.tintMilli,
                admitted.tintMilli.minimum,
                TINT_MILLI_BOUNDS,
              ),
            minimum: Math.max(
              admitted.tintMilli.minimum,
              TINT_MILLI_BOUNDS.minimum,
            ),
            maximum: Math.min(
              admitted.tintMilli.maximum,
              TINT_MILLI_BOUNDS.maximum,
            ),
            enabled: adjustable,
          });
    const note =
      admitted !== undefined
        ? ""
        : modes.includes(ADJUSTABLE_MODE)
          ? "This deployment admits temperature and tint for this source class but does not report the range it qualifies, so the controls stay read-only."
          : "This deployment admits as-shot white balance only, so temperature and tint are unavailable. Only an operator can qualify them for this source class.";
    return Object.freeze({
      intent,
      modes,
      adjustable,
      note,
      temperatureKelvin,
      tintMilli,
      resettable: intent.mode !== "as-shot",
    });
  };

  const presentation = (): EditorPresentation =>
    Object.freeze({
      photoId: facts?.photoId ?? "",
      settings,
      confirmed,
      baseline: baselineSettings(),
      controls: controls(),
      whiteBalance: whiteBalancePresentation(),
      sourceSupport: facts?.sourceSupport ?? "unknown",
      processingAvailable: facts?.processingAvailable ?? false,
      recipeVersion,
      canEdit: editable(),
      saving: inFlight !== null,
      dirty: !sameSettings(settings, confirmed),
      canUndo: !historyInvalidated && historyIndex > 0,
      canRedo: !historyInvalidated && historyIndex < history.length - 1,
      conflict,
      draft,
      status,
    });

  /// Places the current local settings in the Photo's write stream. One write
  /// is in flight at a time; a later edit action coalesces into the pending
  /// settings the next step carries.
  const step = (): EditorStep => {
    if (inFlight || conflict || !editable()) {
      return Object.freeze({ presentation: presentation(), request: null });
    }
    // An identity whose outcome is unknown is resolved before any later
    // intent advances: the identical retry answers from its receipt, and the
    // coalesced settings follow once that operation has settled.
    const unresolved = draftRequestSettings;
    const intent = unresolved ?? settings;
    if (unresolved === null && sameSettings(settings, confirmed)) {
      return Object.freeze({ presentation: presentation(), request: null });
    }
    const request = Object.freeze({
      id: draftRequestId ?? nextRequestId(),
      photoId: facts?.photoId ?? "",
      expectedRecipeVersion: recipeVersion,
      expectedSourceRevision: sourceRevision ?? "",
      settings: Object.freeze({
        exposureEv: intent.exposureEv,
        whiteBalance: intent.whiteBalance,
      }),
    });
    draftRequestId = request.id;
    draftRequestSettings = null;
    inFlight = request;
    status = "Saving…";
    persistDraft(request);
    return Object.freeze({ presentation: presentation(), request });
  };

  /// The bounded local pending draft, persisted before the write leaves. A
  /// store that is blocked or full leaves the draft in memory for this
  /// session and says so.
  const persistDraft = (request: SaveRequest): void => {
    const record: DraftRecord = Object.freeze({
      photoId: request.photoId,
      sourceRevision: request.expectedSourceRevision,
      recipeVersion: request.expectedRecipeVersion,
      settings: request.settings,
      requestId: request.id,
    });
    if (!store) {
      draft = Object.freeze({
        kind: "session",
        note: "This draft is kept for this session only.",
      });
      return;
    }
    const written = writeDraft(store, draftKey(request.photoId), record);
    draft = !written
      ? Object.freeze({
          kind: "session",
          note: "This draft is kept for this session only; browser storage is unavailable or full.",
        })
      : draft.kind === "recovered"
        ? Object.freeze({
            kind: "recovered",
            note: "A recovered local draft is still unconfirmed.",
          })
        : Object.freeze({
            kind: "none",
            note: "The draft survives a reload until the service confirms it.",
          });
  };

  const pushHistory = (next: EditorSettings): void => {
    history = history.slice(0, historyIndex + 1);
    history.push(next);
    if (history.length > maximumHistory)
      history = history.slice(-maximumHistory);
    historyIndex = history.length - 1;
  };

  /// The temperature and tint in force for an adjustable action: the current
  /// intent when it is adjustable, else the intent this session last used,
  /// else the admitted range's midpoint.
  const adjustableIntent = (
    admitted: AdmittedWhiteBalance,
  ): TemperatureTintIntent => {
    const current = settings.whiteBalance;
    if (current.mode === ADJUSTABLE_MODE) return current;
    if (lastTemperatureTint) return lastTemperatureTint;
    return defaultTemperatureTint(admitted);
  };

  /// The nearest settings the closed controls admit. A retained intent the
  /// deployment does not admit keeps its values: it stays readable and is
  /// written back unchanged, so an unrelated edit never rewrites it.
  const admitSettings = (next: EditorSettings): EditorSettings => {
    const admitted = admittedAdjustable(controls());
    const exposureEv = admittedExposure(controls(), next.exposureEv);
    const intent = next.whiteBalance;
    if (intent.mode !== ADJUSTABLE_MODE)
      return Object.freeze({ exposureEv, whiteBalance: intent });
    const temperatureKelvin = admittedInteger(
      admitted?.temperatureKelvin ?? TEMPERATURE_KELVIN_BOUNDS,
      intent.temperatureKelvin,
      TEMPERATURE_KELVIN_BOUNDS,
    );
    const tintMilli = admittedInteger(
      admitted?.tintMilli ?? TINT_MILLI_BOUNDS,
      intent.tintMilli,
      TINT_MILLI_BOUNDS,
    );
    return Object.freeze({
      exposureEv,
      whiteBalance: temperatureTint(temperatureKelvin, tintMilli),
    });
  };

  /// Applies local settings as one edit action and places the resulting write.
  const applySettings = (next: EditorSettings, record: boolean): EditorStep => {
    const admitted = admitSettings(next);
    if (record && !sameSettings(admitted, settings)) pushHistory(admitted);
    settings = admitted;
    if (admitted.whiteBalance.mode === ADJUSTABLE_MODE)
      lastTemperatureTint = admitted.whiteBalance;
    return step();
  };

  const open = (next: EditorFacts): EditorStep => {
    facts = next;
    sourceRevision = next.sourceRevision;
    recipeVersion = next.recipeVersion;
    confirmed = next.settings;
    settings = confirmed;
    lastTemperatureTint =
      confirmed.whiteBalance.mode === ADJUSTABLE_MODE
        ? confirmed.whiteBalance
        : null;
    history = [confirmed];
    historyIndex = 0;
    historyInvalidated = false;
    inFlight = null;
    conflict = null;
    draft = Object.freeze({ kind: "none", note: "" });
    draftRequestId = null;
    draftRequestSettings = null;
    status = "";
    if (next.sourceSupport !== "supported" || !next.sourceRevision) {
      status = sourceUnavailableStatus(next);
      return Object.freeze({ presentation: presentation(), request: null });
    }
    // A draft left by an earlier session is local intent the service never
    // confirmed. It is replayed only through its own guarded write, which the
    // service refuses when a newer revision exists.
    const recovered = store
      ? parseDraft(store.read(draftKey(next.photoId)), next.photoId)
      : null;
    if (!recovered)
      return Object.freeze({ presentation: presentation(), request: null });
    settings = admitSettings(
      Object.freeze({
        exposureEv: recovered.settings.exposureEv,
        whiteBalance: recovered.settings.whiteBalance,
      }),
    );
    if (settings.whiteBalance.mode === ADJUSTABLE_MODE)
      lastTemperatureTint = settings.whiteBalance;
    draftRequestId = recovered.requestId;
    if (
      recovered.sourceRevision !== next.sourceRevision ||
      recovered.recipeVersion !== next.recipeVersion
    ) {
      draft = Object.freeze({
        kind: "recovered",
        note: "A local draft from an earlier revision was recovered. It is not saved; reapply it to write against the current revision.",
      });
      conflict = Object.freeze({
        message:
          "A local draft from an earlier revision was recovered. Use the saved recipe or reapply the draft.",
        savedSettings: confirmed,
        localSettings: settings,
      });
      history = [confirmed, settings];
      historyIndex = 1;
      historyInvalidated = true;
      status = conflict.message;
      return Object.freeze({ presentation: presentation(), request: null });
    }
    draft = Object.freeze({
      kind: "recovered",
      note: "A local draft was recovered and is being saved.",
    });
    history = [confirmed, settings];
    historyIndex = 1;
    status = "Saving the recovered local draft…";
    return step();
  };

  const refresh = (next: EditorFacts): EditorStep => {
    const photoId = facts?.photoId;
    if (photoId !== next.photoId) return open(next);
    const movedVersion = next.recipeVersion !== recipeVersion;
    const movedSource = next.sourceRevision !== sourceRevision;
    facts = next;
    sourceRevision = next.sourceRevision;
    recipeVersion = next.recipeVersion;
    if (next.sourceSupport !== "supported" || !next.sourceRevision) {
      conflict = null;
      historyInvalidated = true;
      status = sourceUnavailableStatus(next);
      return Object.freeze({ presentation: presentation(), request: null });
    }
    if (conflict !== null) {
      // A read is the authoritative current recipe. The conflict stays open
      // with the local intent retained, and its saved side becomes the recipe
      // the service holds now instead of this client's last confirmed copy.
      conflict = Object.freeze({
        message: conflict.message,
        savedSettings: Object.freeze({ ...next.settings }),
        localSettings: settings,
      });
      status = conflict.message;
      return Object.freeze({ presentation: presentation(), request: null });
    }
    if (inFlight || sameSettings(settings, confirmed)) {
      confirmed = Object.freeze({ ...next.settings });
      if (!inFlight) settings = confirmed;
      if (!sameSettings(settings, confirmed) && conflict === null) {
        // Local intent stays; it is still unconfirmed and still guarded.
        status = "Local settings are still not saved.";
      }
      return step();
    }
    if (movedVersion || movedSource) {
      // Another client moved the recipe. Autosave stops and the local
      // settings are retained for explicit reconciliation.
      conflict = Object.freeze({
        message:
          "The saved recipe changed elsewhere. Autosave stopped; use the saved recipe or reapply the local settings.",
        savedSettings: Object.freeze({ ...next.settings }),
        localSettings: settings,
      });
      historyInvalidated = true;
      status = conflict.message;
      return Object.freeze({ presentation: presentation(), request: null });
    }
    return step();
  };

  const acknowledge = (
    request: SaveRequest,
    committed: Readonly<{ recipeVersion: string; sourceRevision: string }>,
  ): EditorStep => {
    // An acknowledgement updates the confirmed baseline only for its own
    // operation, and never marks later pending intent saved.
    if (!inFlight || inFlight.id !== request.id)
      return Object.freeze({ presentation: presentation(), request: null });
    inFlight = null;
    recipeVersion = committed.recipeVersion;
    sourceRevision = committed.sourceRevision;
    confirmed = request.settings;
    if (store && facts)
      clearDraft(store, draftKey(facts.photoId), facts.photoId);
    draftRequestId = null;
    draftRequestSettings = null;
    draft = Object.freeze({ kind: "none", note: "" });
    status = sameSettings(settings, confirmed)
      ? "Saved."
      : "Saved earlier settings; the latest settings are still saving.";
    if (sameSettings(settings, confirmed)) status = "Saved.";
    return step();
  };

  const refuse = (request: SaveRequest, refusal: SaveRefusal): EditorStep => {
    if (!inFlight || inFlight.id !== request.id)
      return Object.freeze({ presentation: presentation(), request: null });
    inFlight = null;
    if (
      refusal.code === "recipe_conflict" ||
      refusal.code === "source_changed"
    ) {
      conflict = Object.freeze({
        message:
          refusal.code === "source_changed"
            ? "The Original File changed elsewhere. Autosave stopped; use the saved recipe or reapply the local settings."
            : "The saved recipe changed elsewhere. Autosave stopped; use the saved recipe or reapply the local settings.",
        savedSettings: confirmed,
        localSettings: settings,
      });
      historyInvalidated = true;
      status = conflict.message;
      if (refusal.currentRecipeVersion !== null)
        recipeVersion = refusal.currentRecipeVersion;
      if (refusal.currentSourceRevision !== null)
        sourceRevision = refusal.currentSourceRevision;
      return Object.freeze({ presentation: presentation(), request: null });
    }
    if (refusal.code === "requires_rebind") {
      conflict = Object.freeze({
        message:
          "The saved recipe is bound to a different source. Rebind it or use the saved recipe.",
        savedSettings: confirmed,
        localSettings: settings,
      });
      historyInvalidated = true;
      status = conflict.message;
      return Object.freeze({ presentation: presentation(), request: null });
    }
    // A failure keeps the local settings. An expired receipt never frees its
    // identity, and a reused identity with a different payload is refused, so
    // both need a new identity: the intent is written again under a fresh one
    // rather than replayed forever against a receipt that cannot resolve it.
    if (
      refusal.code === "receipt_expired" ||
      refusal.code === "request_conflict"
    ) {
      draftRequestId = null;
      draftRequestSettings = null;
      const next = step();
      status =
        refusal.code === "receipt_expired"
          ? "The earlier save's receipt expired; saving again under a new request."
          : "The service already recorded a different save under this request identity; saving again under a new request.";
      return Object.freeze({ ...next, presentation: presentation() });
    }
    if (refusal.code === "outcome_unknown" || refusal.status === 500) {
      // The service may have committed this payload. The identity stays, and
      // it stays pinned to what it carried, so the next write resolves that
      // operation before any later intent advances.
      draftRequestSettings = request.settings;
      status =
        "The save outcome is unknown. Retry to resolve the same request.";
      return Object.freeze({ presentation: presentation(), request: null });
    }
    draftRequestSettings = null;
    status =
      refusal.message ||
      "The save was refused. Retry when the service is ready.";
    return Object.freeze({ presentation: presentation(), request: null });
  };

  const useSavedRecipe = (): EditorStep => {
    if (!conflict)
      return Object.freeze({ presentation: presentation(), request: null });
    // The saved recipe is the service's recipe: the conflict's saved side is
    // what the service held when this client last read or was refused, so
    // adopting it never presents this client's older confirmed copy as saved.
    const saved = conflict.savedSettings;
    conflict = null;
    historyInvalidated = false;
    confirmed = Object.freeze({ ...saved });
    settings = confirmed;
    if (store && facts) {
      clearDraft(store, draftKey(facts.photoId), facts.photoId);
      draftRequestId = null;
      draftRequestSettings = null;
      draft = Object.freeze({ kind: "none", note: "" });
    }
    history = [confirmed];
    historyIndex = 0;
    status = "Using the saved recipe.";
    return Object.freeze({ presentation: presentation(), request: null });
  };

  const reapplyLocal = (): EditorStep => {
    if (!conflict)
      return Object.freeze({ presentation: presentation(), request: null });
    const local = conflict.localSettings;
    conflict = null;
    historyInvalidated = false;
    settings = local;
    status = "Reapplying the local settings.";
    // A new write needs a new identity: the earlier one may be settled with
    // the settings the service already refused.
    draftRequestId = null;
    draftRequestSettings = null;
    return step();
  };

  const discardDraft = (): EditorPresentation => {
    if (store && facts)
      clearDraft(store, draftKey(facts.photoId), facts.photoId);
    draftRequestId = null;
    draftRequestSettings = null;
    draft = Object.freeze({ kind: "none", note: "" });
    return presentation();
  };

  return Object.freeze({
    open,
    refresh,
    /// Resolves a save whose outcome is unknown through the identity it
    /// already carries: the service answers the identical retry from its
    /// receipt, so the same operation settles instead of a new one starting.
    resolveUnknown: (): EditorStep => step(),
    /// Writes the settings in force now. The Export ordering barrier commits
    /// the visible intent before it captures the confirmed revision.
    commitCurrent: (): EditorStep => step(),
    commitExposure: (exposureEv: number): EditorStep =>
      applySettings(
        Object.freeze({ exposureEv, whiteBalance: settings.whiteBalance }),
        true,
      ),
    selectWhiteBalanceMode: (mode: string): EditorStep => {
      if (!controls().whiteBalanceModes.includes(mode)) {
        status = `This deployment does not admit the white-balance mode ${mode}.`;
        return Object.freeze({ presentation: presentation(), request: null });
      }
      if (mode === BASELINE_MODE)
        return applySettings(
          Object.freeze({
            exposureEv: settings.exposureEv,
            whiteBalance: asShot(),
          }),
          true,
        );
      const admitted = admittedAdjustable(controls());
      if (!admitted) {
        status =
          "This deployment does not admit temperature and tint for this source class.";
        return Object.freeze({ presentation: presentation(), request: null });
      }
      const resumed = lastTemperatureTint ?? defaultTemperatureTint(admitted);
      return applySettings(
        Object.freeze({
          exposureEv: settings.exposureEv,
          whiteBalance: resumed,
        }),
        true,
      );
    },
    commitTemperature: (temperatureKelvin: number): EditorStep => {
      const admitted = admittedAdjustable(controls());
      if (!admitted)
        return Object.freeze({ presentation: presentation(), request: null });
      const current = adjustableIntent(admitted);
      return applySettings(
        Object.freeze({
          exposureEv: settings.exposureEv,
          whiteBalance: temperatureTint(temperatureKelvin, current.tintMilli),
        }),
        true,
      );
    },
    commitTint: (tintMilli: number): EditorStep => {
      const admitted = admittedAdjustable(controls());
      if (!admitted)
        return Object.freeze({ presentation: presentation(), request: null });
      const current = adjustableIntent(admitted);
      return applySettings(
        Object.freeze({
          exposureEv: settings.exposureEv,
          whiteBalance: temperatureTint(current.temperatureKelvin, tintMilli),
        }),
        true,
      );
    },
    undo: (): EditorStep => {
      if (historyInvalidated || historyIndex === 0)
        return Object.freeze({ presentation: presentation(), request: null });
      historyIndex -= 1;
      settings = history[historyIndex] ?? settings;
      draftRequestId = null;
      draftRequestSettings = null;
      return step();
    },
    redo: (): EditorStep => {
      if (historyInvalidated || historyIndex >= history.length - 1)
        return Object.freeze({ presentation: presentation(), request: null });
      historyIndex += 1;
      settings = history[historyIndex] ?? settings;
      draftRequestId = null;
      draftRequestSettings = null;
      return step();
    },
    resetExposure: (): EditorStep => {
      if (historyInvalidated)
        return Object.freeze({ presentation: presentation(), request: null });
      return applySettings(
        Object.freeze({
          exposureEv: baselineSettings().exposureEv,
          whiteBalance: settings.whiteBalance,
        }),
        true,
      );
    },
    resetWhiteBalance: (): EditorStep => {
      if (historyInvalidated)
        return Object.freeze({ presentation: presentation(), request: null });
      return applySettings(
        Object.freeze({
          exposureEv: settings.exposureEv,
          whiteBalance: asShot(),
        }),
        true,
      );
    },
    reset: (): EditorStep => {
      if (historyInvalidated)
        return Object.freeze({ presentation: presentation(), request: null });
      return applySettings(baselineSettings(), true);
    },
    acknowledge,
    refuse,
    useSavedRecipe,
    reapplyLocal,
    discardDraft,
    setStatus: (next: string): EditorPresentation => {
      status = next;
      return presentation();
    },
    presentation,
    facts: () => facts,
  });
};
