import type {
  PhotoListResponse,
  PhotoSetResponse,
  PhotoSummary,
  PreviewResponse,
  PreviewSource,
  SelectionState,
  UndoDescription,
} from "./protocol.js";
import "./style.css";

type SessionUndo = UndoDescription & Readonly<{ advanced: boolean }>;
type PhotoSetList = Readonly<{ photoSets: ReadonlyArray<PhotoSetResponse> }>;
type MutationResponse = Readonly<{ undo: UndoDescription }>;

const swipePendingPixels = 24;
const swipeCommitPixels = 72;
const swipeCommitVelocity = 0.5;

export function renderApp(
  root: HTMLElement,
  fetcher: typeof fetch = fetch,
): () => void {
  root.innerHTML = `
    <main class="app-shell">
      <header class="app-header"><h1>Slipstream</h1><p data-connection role="status">Connecting…</p></header>
      <section class="set-screen" data-set-screen aria-labelledby="sets-title">
        <h2 id="sets-title">Photo Sets</h2>
        <p data-set-status role="status">Loading Photo Sets…</p>
        <div class="set-list" data-set-list></div>
        <button type="button" data-retry hidden>Retry connection</button>
      </section>
      <section class="review" data-review hidden tabindex="-1" aria-labelledby="review-title">
        <header class="review-header">
          <button type="button" class="quiet" data-leave>Photo Sets</button>
          <div><h2 id="review-title" data-set-name>Review</h2><p data-position>0 / 0</p></div>
          <button type="button" class="quiet" data-retry-review hidden>Retry</button>
        </header>
        <section class="preview" data-preview aria-label="Photo Preview">
          <div class="swipe-feedback reject" data-reject-feedback>Reject</div>
          <div class="image-stage" data-stage><p>Loading…</p></div>
          <div class="swipe-feedback select" data-select-feedback>Select</div>
        </section>
        <dl class="facts">
          <div><dt>Selection</dt><dd data-selection>Undecided</dd></div>
          <div><dt>Rating</dt><dd data-rating>0 stars</dd></div>
          <div><dt>Preview Source</dt><dd data-source>—</dd></div>
          <div data-limited hidden><dt>Detail</dt><dd>Limited by camera Preview resolution</dd></div>
        </dl>
        <p class="status" data-status role="status" aria-live="polite"></p>
        <div class="decision-controls" aria-label="Selection controls">
          <button type="button" class="reject-button" data-reject>Reject <span aria-hidden="true">X</span></button>
          <button type="button" data-clear>Clear <span aria-hidden="true">U</span></button>
          <button type="button" class="select-button" data-select>Select <span aria-hidden="true">P</span></button>
        </div>
        <fieldset class="rating-controls"><legend>Rating</legend><div data-ratings></div></fieldset>
        <div class="session-controls">
          <button type="button" data-previous>Previous</button>
          <button type="button" data-detail aria-pressed="false">Detail Review</button>
          <button type="button" data-undo disabled>Undo</button>
          <button type="button" data-next>Next</button>
        </div>
      </section>
    </main>`;

  const setScreen = required<HTMLElement>(root, "[data-set-screen]");
  const review = required<HTMLElement>(root, "[data-review]");
  const setList = required<HTMLElement>(root, "[data-set-list]");
  const setStatus = required<HTMLElement>(root, "[data-set-status]");
  const connection = required<HTMLElement>(root, "[data-connection]");
  const retry = required<HTMLButtonElement>(root, "[data-retry]");
  const retryReview = required<HTMLButtonElement>(root, "[data-retry-review]");
  const leave = required<HTMLButtonElement>(root, "[data-leave]");
  const setName = required<HTMLElement>(root, "[data-set-name]");
  const position = required<HTMLElement>(root, "[data-position]");
  const stage = required<HTMLElement>(root, "[data-stage]");
  const preview = required<HTMLElement>(root, "[data-preview]");
  const selection = required<HTMLElement>(root, "[data-selection]");
  const rating = required<HTMLElement>(root, "[data-rating]");
  const source = required<HTMLElement>(root, "[data-source]");
  const limited = required<HTMLElement>(root, "[data-limited]");
  const status = required<HTMLElement>(root, "[data-status]");
  const previous = required<HTMLButtonElement>(root, "[data-previous]");
  const next = required<HTMLButtonElement>(root, "[data-next]");
  const select = required<HTMLButtonElement>(root, "[data-select]");
  const reject = required<HTMLButtonElement>(root, "[data-reject]");
  const clear = required<HTMLButtonElement>(root, "[data-clear]");
  const undoButton = required<HTMLButtonElement>(root, "[data-undo]");
  const detail = required<HTMLButtonElement>(root, "[data-detail]");
  const ratings = required<HTMLElement>(root, "[data-ratings]");
  const selectFeedback = required<HTMLElement>(root, "[data-select-feedback]");
  const rejectFeedback = required<HTMLElement>(root, "[data-reject-feedback]");

  for (let value = 0; value <= 5; value += 1) {
    const button = document.createElement("button");
    button.type = "button";
    button.dataset.ratingValue = String(value);
    button.setAttribute(
      "aria-label",
      value === 0 ? "Clear Rating" : `Rate ${value} stars`,
    );
    button.textContent = value === 0 ? "0" : "★".repeat(value);
    ratings.append(button);
  }

  let sets: ReadonlyArray<PhotoSetResponse> = [];
  let photoFacts = new Map<string, PhotoSummary>();
  let currentSet: PhotoSetResponse | undefined;
  let index = 0;
  let connected = false;
  let busy = false;
  let undo: SessionUndo | undefined;
  let requestGeneration = 0;
  let sessionGeneration = 0;
  let progressQueue: Promise<void> = Promise.resolve();
  let zoomed = false;
  let panX = 0;
  let panY = 0;
  let pointer:
    | {
        id: number;
        startX: number;
        startY: number;
        lastX: number;
        lastY: number;
        startedAt: number;
        vertical: boolean;
        sessionGeneration: number;
        photoId: string;
      }
    | undefined;

  const currentMember = () => currentSet?.members[index];
  const clearPointer = () => {
    const pointerId = pointer?.id;
    pointer = undefined;
    if (pointerId !== undefined && preview.hasPointerCapture(pointerId))
      preview.releasePointerCapture(pointerId);
    stage.style.transform = "";
    selectFeedback.classList.remove("pending");
    rejectFeedback.classList.remove("pending");
  };
  const currentPhoto = () => {
    const member = currentMember();
    return member ? photoFacts.get(member.photoId) : undefined;
  };
  const setConnected = (value: boolean, message?: string) => {
    if (!value) clearPointer();
    connected = value;
    connection.textContent = value ? "Connected" : "Disconnected";
    connection.classList.toggle("offline", !value);
    retry.hidden = value;
    retryReview.hidden = value;
    if (message) status.textContent = message;
    updateControls();
  };
  const updateControls = () => {
    const member = currentMember();
    const decisionsEnabled = Boolean(member) && connected && !busy;
    for (const button of [
      select,
      reject,
      clear,
      ...Array.from(ratings.querySelectorAll<HTMLButtonElement>("button")),
    ])
      button.disabled = !decisionsEnabled;
    leave.disabled = busy;
    for (const button of Array.from(
      setList.querySelectorAll<HTMLButtonElement>("button"),
    ))
      button.disabled = busy || button.dataset.empty === "true";
    previous.disabled = busy || index <= 0;
    next.disabled =
      busy || !currentSet || index >= currentSet.members.length - 1;
    undoButton.disabled = !connected || busy || !undo;
    detail.disabled = !stage.querySelector("img");
  };
  const resetTransform = () => {
    zoomed = false;
    panX = 0;
    panY = 0;
    detail.setAttribute("aria-pressed", "false");
    detail.textContent = "Detail Review";
    applyTransform();
  };
  const applyTransform = () => {
    const image = stage.querySelector<HTMLImageElement>("img");
    if (!image) return;
    image.style.transform = zoomed
      ? `translate(${panX}px, ${panY}px) scale(2)`
      : "translate(0, 0) scale(1)";
    preview.classList.toggle("detail", zoomed);
  };
  const refreshFacts = async (preservePosition = true) => {
    const [setResponse, photosResponse] = await Promise.all([
      fetcher("/api/photo-sets"),
      fetcher("/api/photos"),
    ]);
    if (!setResponse.ok || !photosResponse.ok)
      throw new Error("refresh failed");
    sets = ((await setResponse.json()) as PhotoSetList).photoSets;
    const photos = ((await photosResponse.json()) as PhotoListResponse).photos;
    photoFacts = new Map(photos.map((photo) => [photo.id, photo]));
    if (currentSet) {
      const priorId = preservePosition ? currentMember()?.photoId : undefined;
      currentSet = sets.find((item) => item.id === currentSet!.id);
      if (!currentSet) leaveSession();
      else if (priorId) {
        const preserved = currentSet.members.findIndex(
          (item) => item.photoId === priorId,
        );
        if (preserved >= 0) index = preserved;
      }
    }
    setConnected(true);
  };
  const loadSets = async () => {
    setStatus.textContent = "Loading Photo Sets…";
    try {
      await refreshFacts();
      renderSets();
    } catch {
      setStatus.textContent =
        "Could not reach Slipstream. Check the server and retry.";
      setConnected(false);
    }
  };
  const renderSets = () => {
    setList.replaceChildren();
    if (sets.length === 0) {
      setStatus.textContent =
        "No Photo Sets yet. Create a Photo Set through the server API, then retry.";
      return;
    }
    setStatus.textContent = "Choose a Photo Set to start or resume review.";
    for (const set of sets) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "set-card";
      button.disabled = busy || set.members.length === 0;
      button.dataset.empty = String(set.members.length === 0);
      button.innerHTML = `<strong></strong><span></span>`;
      button.querySelector("strong")!.textContent = set.name;
      button.querySelector("span")!.textContent = set.members.length
        ? `${set.members.length} Photos${set.lastReviewedPhotoId ? " · Resume" : ""}`
        : "Empty Photo Set";
      button.addEventListener("click", () => startSession(set.id));
      setList.append(button);
    }
  };
  const startSession = (id: string) => {
    if (busy) return;
    sessionGeneration += 1;
    clearPointer();
    const set = sets.find((item) => item.id === id);
    if (!set || set.members.length === 0) return;
    currentSet = set;
    undo = undefined;
    const saved = set.lastReviewedPhotoId
      ? set.members.findIndex(
          (item) => item.photoId === set.lastReviewedPhotoId,
        )
      : -1;
    const resumed =
      saved >= 0 && set.members[saved]?.available
        ? saved
        : saved >= 0
          ? set.members.findIndex(
              (item, memberIndex) => memberIndex > saved && item.available,
            )
          : -1;
    index =
      resumed >= 0
        ? resumed
        : Math.max(
            0,
            set.members.findIndex((item) => item.available),
          );
    setScreen.hidden = true;
    review.hidden = false;
    setName.textContent = set.name;
    review.focus();
    void show();
  };
  const leaveSession = () => {
    if (busy) return;
    sessionGeneration += 1;
    requestGeneration += 1;
    clearPointer();
    currentSet = undefined;
    undo = undefined;
    review.hidden = true;
    setScreen.hidden = false;
    resetTransform();
    renderSets();
  };
  const show = async () => {
    const token = ++requestGeneration;
    clearPointer();
    resetTransform();
    const member = currentMember();
    const photo = currentPhoto();
    position.textContent =
      member && currentSet
        ? `${index + 1} / ${currentSet.members.length}`
        : "0 / 0";
    selection.textContent = selectionLabel(member?.selectionState);
    rating.textContent = `${member?.rating ?? 0} ${member?.rating === 1 ? "star" : "stars"}`;
    source.textContent = "—";
    limited.hidden = true;
    status.textContent = "";
    stage.replaceChildren(
      paragraph(member ? "Loading review Preview…" : "No Photos found"),
    );
    updateControls();
    if (!member || !photo) return;
    if (!member.available) {
      status.textContent =
        "Original File is unavailable. Decisions remain available; restore it and rescan for a Preview.";
      stage.replaceChildren(paragraph("Preview unavailable"));
      updateControls();
      return;
    }
    try {
      const response = await fetcher(`/api/photos/${member.photoId}/preview`);
      const result = (await response.json()) as PreviewResponse;
      if (token !== requestGeneration) return;
      if (result.state !== "ready" || !result.url) {
        status.textContent = result.message ?? "Preview unavailable";
        stage.replaceChildren(paragraph("Preview unavailable"));
        return;
      }
      const image = document.createElement("img");
      image.alt = `Photo ${index + 1} of ${currentSet!.members.length}`;
      image.draggable = false;
      image.src = result.url;
      image.addEventListener(
        "error",
        () => {
          if (token !== requestGeneration) return;
          status.textContent =
            "Preview could not be loaded. Retry the connection or continue reviewing.";
        },
        { once: true },
      );
      stage.replaceChildren(image);
      source.textContent = sourceLabel(result.source);
      limited.hidden = !result.limitedDetail;
      if (result.stale)
        status.textContent =
          result.message ??
          "Showing a stale Preview; retry to generate the current Preview.";
      updateControls();
    } catch {
      if (token !== requestGeneration) return;
      setConnected(
        false,
        "Connection lost. Decisions are disabled until Retry succeeds.",
      );
      stage.replaceChildren(paragraph("Preview unavailable"));
    }
  };
  const persistProgress = (photoId: string) => {
    const set = currentSet;
    if (!set) return;
    const generation = sessionGeneration;
    const setId = set.id;
    progressQueue = progressQueue.then(async () => {
      try {
        const response = await fetcher(`/api/photo-sets/${setId}/progress`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ photoId }),
        });
        if (!response.ok) throw new Error("progress rejected");
        sets = sets.map((item) =>
          item.id === setId ? { ...item, lastReviewedPhotoId: photoId } : item,
        );
        if (generation === sessionGeneration && currentSet?.id === setId) {
          currentSet = { ...currentSet, lastReviewedPhotoId: photoId };
        }
      } catch {
        if (
          generation === sessionGeneration &&
          currentSet?.id === setId &&
          currentMember()?.photoId === photoId
        )
          setConnected(
            false,
            "Review position could not be saved. Retry before making decisions.",
          );
      }
    });
  };
  const moveTo = (target: number) => {
    if (!currentSet || busy) return;
    const bounded = Math.max(
      0,
      Math.min(currentSet.members.length - 1, target),
    );
    if (bounded === index) return;
    clearPointer();
    index = bounded;
    const photoId = currentMember()!.photoId;
    void show();
    persistProgress(photoId);
  };
  const mutate = async (
    field: "selectionState" | "rating",
    value: SelectionState | number,
    advance: boolean,
  ) => {
    const member = currentMember();
    const set = currentSet;
    if (!member || !set || !connected || busy) return;
    const generation = sessionGeneration;
    const setId = set.id;
    const photoId = member.photoId;
    const priorIndex = index;
    const priorUndo = undo;
    undo = undefined;
    clearPointer();
    busy = true;
    status.textContent = `Saving ${field === "rating" ? "Rating" : "Selection State"}…`;
    updateControls();
    try {
      const response = await fetcher(`/api/photos/${photoId}/state`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ field, value, photoSetId: setId }),
      });
      const current =
        generation === sessionGeneration && currentSet?.id === setId;
      if (!response.ok) {
        if (current) {
          undo = priorUndo;
          if (response.status === 409) {
            setConnected(
              false,
              "The Photo changed elsewhere. Retry to refresh its current state.",
            );
          } else {
            status.textContent =
              "The decision could not be saved. Try the action again without losing your place.";
          }
        }
        return;
      }
      const result = (await response.json()) as MutationResponse;
      if (!current) return;
      const updatedMembers = currentSet!.members.map((item) =>
        item.photoId === photoId
          ? {
              ...item,
              ...(field === "selectionState"
                ? { selectionState: value as SelectionState }
                : { rating: value as number }),
            }
          : item,
      );
      currentSet = {
        ...currentSet!,
        members: updatedMembers,
        lastReviewedPhotoId: photoId,
      };
      sets = sets.map((item) => (item.id === setId ? currentSet! : item));
      undo = {
        ...result.undo,
        advanced: advance && priorIndex < updatedMembers.length - 1,
      };
      status.textContent = `${field === "rating" ? "Rating" : "Selection"} saved.`;
      if (undo.advanced) {
        index = priorIndex + 1;
        const nextPhotoId = currentMember()!.photoId;
        await show();
        persistProgress(nextPhotoId);
      } else await show();
    } catch {
      if (generation === sessionGeneration && currentSet?.id === setId) {
        undo = undefined;
        setConnected(
          false,
          "Connection lost before the decision was confirmed. Retry to refresh current state.",
        );
      }
    } finally {
      if (generation === sessionGeneration && currentSet?.id === setId) {
        busy = false;
        updateControls();
      }
    }
  };
  const performUndo = async () => {
    const set = currentSet;
    if (!undo || !connected || busy || !set) return;
    const action = undo;
    const generation = sessionGeneration;
    const setId = set.id;
    const affectedIndex = set.members.findIndex(
      (item) => item.photoId === action.photoId,
    );
    if (affectedIndex < 0) {
      undo = undefined;
      updateControls();
      return;
    }
    undo = undefined;
    clearPointer();
    busy = true;
    updateControls();
    try {
      const response = await fetcher(`/api/photos/${action.photoId}/state`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          field: action.field,
          value: action.priorValue,
          expectedCurrent: action.expectedCurrent,
          photoSetId: setId,
        }),
      });
      const current =
        generation === sessionGeneration && currentSet?.id === setId;
      if (!response.ok) {
        if (current) {
          if (response.status === 409) {
            setConnected(
              false,
              "Undo is no longer available because the Photo changed elsewhere. Retry to refresh its current state.",
            );
          } else {
            undo = action;
            status.textContent =
              "Undo could not be saved. Try Undo again or Retry the connection.";
          }
        }
        return;
      }
      if (!current) return;
      currentSet = {
        ...currentSet!,
        members: currentSet!.members.map((item) =>
          item.photoId === action.photoId
            ? {
                ...item,
                ...(action.field === "selectionState"
                  ? { selectionState: action.priorValue as SelectionState }
                  : { rating: action.priorValue as number }),
              }
            : item,
        ),
        lastReviewedPhotoId: action.photoId,
      };
      sets = sets.map((item) => (item.id === setId ? currentSet! : item));
      index = affectedIndex;
      await show();
      status.textContent = "Last change undone.";
    } catch {
      if (generation === sessionGeneration && currentSet?.id === setId) {
        undo = undefined;
        setConnected(
          false,
          "Connection lost before Undo was confirmed. Retry to refresh the current state.",
        );
      }
    } finally {
      if (generation === sessionGeneration && currentSet?.id === setId) {
        busy = false;
        updateControls();
      }
    }
  };
  const reconnect = async () => {
    sessionGeneration += 1;
    clearPointer();
    undo = undefined;
    status.textContent = "Reconnecting…";
    try {
      await refreshFacts(true);
      if (currentSet) {
        await show();
        const photoId = currentMember()?.photoId;
        if (photoId) persistProgress(photoId);
      } else renderSets();
      status.textContent = "Connected. Current state refreshed.";
    } catch {
      setConnected(false, "Still disconnected. Check the server and retry.");
    }
  };
  const toggleDetail = () => {
    if (!stage.querySelector("img")) return;
    zoomed = !zoomed;
    panX = 0;
    panY = 0;
    detail.setAttribute("aria-pressed", String(zoomed));
    detail.textContent = zoomed ? "Exit Detail" : "Detail Review";
    applyTransform();
  };
  const pointerDown = (event: PointerEvent) => {
    const member = currentMember();
    if (pointer || !event.isPrimary || busy || !connected || !member) return;
    pointer = {
      id: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      lastX: event.clientX,
      lastY: event.clientY,
      startedAt: event.timeStamp,
      vertical: false,
      sessionGeneration,
      photoId: member.photoId,
    };
    preview.setPointerCapture(event.pointerId);
  };
  const pointerMove = (event: PointerEvent) => {
    if (!pointer || pointer.id !== event.pointerId) return;
    const dx = event.clientX - pointer.startX;
    const dy = event.clientY - pointer.startY;
    const stepX = event.clientX - pointer.lastX;
    const stepY = event.clientY - pointer.lastY;
    pointer.lastX = event.clientX;
    pointer.lastY = event.clientY;
    if (zoomed) {
      panX = clamp(panX + stepX, -stage.clientWidth / 2, stage.clientWidth / 2);
      panY = clamp(
        panY + stepY,
        -stage.clientHeight / 2,
        stage.clientHeight / 2,
      );
      applyTransform();
      return;
    }
    if (Math.abs(dy) > Math.abs(dx) && Math.abs(dy) > 12)
      pointer.vertical = true;
    if (pointer.vertical) return;
    stage.style.transform = `translateX(${clamp(dx, -140, 140)}px)`;
    selectFeedback.classList.toggle("pending", dx > swipePendingPixels);
    rejectFeedback.classList.toggle("pending", dx < -swipePendingPixels);
  };
  const finishPointer = (event: PointerEvent, cancelled = false) => {
    if (!pointer || pointer.id !== event.pointerId) return;
    const active = pointer;
    clearPointer();
    if (
      zoomed ||
      cancelled ||
      active.vertical ||
      !connected ||
      active.sessionGeneration !== sessionGeneration ||
      currentMember()?.photoId !== active.photoId
    )
      return;
    const dx = event.clientX - active.startX;
    const elapsed = Math.max(1, event.timeStamp - active.startedAt);
    const velocity = Math.abs(dx) / elapsed;
    const commit =
      Math.abs(dx) >= swipeCommitPixels ||
      (Math.abs(dx) >= 48 && velocity >= swipeCommitVelocity);
    if (commit)
      void mutate("selectionState", dx > 0 ? "selected" : "rejected", true);
  };
  const keydown = (event: KeyboardEvent) => {
    const target = event.target as HTMLElement | null;
    if (
      event.isComposing ||
      target?.matches("input, textarea, select, [contenteditable=true]") ||
      target?.isContentEditable ||
      event.altKey
    )
      return;
    const modifier = event.ctrlKey || event.metaKey;
    if (modifier && !event.shiftKey && event.key.toLowerCase() === "z") {
      event.preventDefault();
      void performUndo();
      return;
    }
    if (modifier || event.shiftKey) return;
    if (event.key === "ArrowLeft") moveTo(index - 1);
    else if (event.key === "ArrowRight") moveTo(index + 1);
    else if (event.key.toLowerCase() === "p")
      void mutate("selectionState", "selected", true);
    else if (event.key.toLowerCase() === "x")
      void mutate("selectionState", "rejected", true);
    else if (event.key.toLowerCase() === "u")
      void mutate("selectionState", "undecided", false);
    else if (/^[0-5]$/.test(event.key))
      void mutate("rating", Number(event.key), false);
  };

  select.addEventListener(
    "click",
    () => void mutate("selectionState", "selected", true),
  );
  reject.addEventListener(
    "click",
    () => void mutate("selectionState", "rejected", true),
  );
  clear.addEventListener(
    "click",
    () => void mutate("selectionState", "undecided", false),
  );
  ratings.addEventListener("click", (event) => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
      "[data-rating-value]",
    );
    if (button)
      void mutate("rating", Number(button.dataset.ratingValue), false);
  });
  previous.addEventListener("click", () => moveTo(index - 1));
  next.addEventListener("click", () => moveTo(index + 1));
  undoButton.addEventListener("click", () => void performUndo());
  detail.addEventListener("click", toggleDetail);
  stage.addEventListener("dblclick", toggleDetail);
  leave.addEventListener("click", leaveSession);
  retry.addEventListener("click", () => void loadSets());
  retryReview.addEventListener("click", () => void reconnect());
  preview.addEventListener("pointerdown", pointerDown);
  preview.addEventListener("pointermove", pointerMove);
  preview.addEventListener("pointerup", (event) => finishPointer(event));
  preview.addEventListener("pointercancel", (event) =>
    finishPointer(event, true),
  );
  preview.addEventListener("lostpointercapture", (event) =>
    finishPointer(event, true),
  );
  window.addEventListener("keydown", keydown);
  void loadSets();
  return () => window.removeEventListener("keydown", keydown);
}

function required<T extends Element>(root: ParentNode, selector: string): T {
  const value = root.querySelector<T>(selector);
  if (!value) throw new Error(`Missing ${selector}`);
  return value;
}
function paragraph(text: string): HTMLParagraphElement {
  const value = document.createElement("p");
  value.textContent = text;
  return value;
}
function selectionLabel(value?: SelectionState): string {
  return value === "selected"
    ? "Selected"
    : value === "rejected"
      ? "Rejected"
      : "Undecided";
}
function sourceLabel(source?: PreviewSource): string {
  return source === "matching-jpeg"
    ? "JPEG"
    : source === "embedded-raw-jpeg"
      ? "RAW embedded JPEG"
      : "—";
}
function clamp(value: number, low: number, high: number): number {
  return Math.max(low, Math.min(high, value));
}

if (typeof document !== "undefined") {
  const root = document.querySelector<HTMLElement>("#app");
  if (root) renderApp(root);
}
