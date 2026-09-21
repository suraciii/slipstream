/// The page UI's shared native-modal lifecycle for supporting surfaces.
///
/// Sources, View options, Rating, Photo tools, Album forms, and recovery review
/// are one kind of surface: exactly one is active at a time, opening it moves
/// focus into it,
/// Tab and Shift+Tab stay inside it, the background receives no pointer
/// input, focus, or shortcuts, and closing it returns focus to the invoker
/// that opened it. Native `dialog.showModal()` supplies that lifecycle, so
/// this controller owns only which surface is active, the invoker focus
/// returns to, and the single cleanup path an explicit Close, a native
/// close/cancel request, and a destination change converge on.
///
/// It owns no routes, no server state, and no second state store: each
/// surface keeps its own content and draft values where it already lives.

export type ModalSurfaceKind =
  | "sources"
  | "view-options"
  | "rating"
  | "photo-tools"
  | "album-form"
  | "recovery";

/// The controls one modal surface cycles through. A hidden or disabled
/// control takes no focus, so the cycle never stops on it.
const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

type ModalSurfaceRegistration = Readonly<{
  dialog: HTMLDialogElement;
  /// True while this surface presents as a modal rather than an inline
  /// layout, so focus can only return into it by opening it again.
  modal: () => boolean;
}>;

export interface ModalSurfaces {
  register(
    kind: ModalSurfaceKind,
    registration: ModalSurfaceRegistration,
  ): void;
  /// Makes one surface active. A surface that is already active stays active
  /// and only remembers its new invoker; any other active surface closes
  /// first, because at most one modal surface may be active.
  open(kind: ModalSurfaceKind, invoker?: HTMLElement): void;
  close(kind: ModalSurfaceKind, restoreFocus?: boolean): void;
  /// Closes every active surface without returning focus, for a destination
  /// change that supersedes them all.
  closeAll(): void;
  isActive(kind: ModalSurfaceKind): boolean;
  /// True while a modal surface is active, so background shortcuts stay off.
  blocking(): boolean;
  /// Moves focus to one control, opening the surface that holds it again when
  /// that surface presents as a modal and had to close for another one.
  focus(invoker?: HTMLElement): void;
  dispose(): void;
}

export function createModalSurfaces(): ModalSurfaces {
  const registrations = new Map<ModalSurfaceKind, ModalSurfaceRegistration>();
  const listeners = new Map<ModalSurfaceKind, AbortController>();
  let activeKind: ModalSurfaceKind | undefined;
  let activeInvoker: HTMLElement | undefined;

  const dialogFor = (kind: ModalSurfaceKind): HTMLDialogElement | undefined =>
    registrations.get(kind)?.dialog;

  const isOpen = (kind: ModalSurfaceKind): boolean =>
    dialogFor(kind)?.open === true;

  /// Returns focus to the invoker, opening the surface that holds it again
  /// when that surface presents as a modal and was closed for another one.
  const focusInvoker = (invoker: HTMLElement | undefined): void => {
    if (!invoker || !invoker.isConnected) return;
    for (const [kind, registration] of registrations) {
      if (!registration.dialog.contains(invoker)) continue;
      if (!isOpen(kind) && registration.modal()) open(kind);
      break;
    }
    invoker.focus();
  };

  const cleanup = (
    kind: ModalSurfaceKind,
    restoreFocus: boolean,
    invoker: HTMLElement | undefined,
  ): void => {
    if (activeKind !== kind) return;
    activeKind = undefined;
    activeInvoker = undefined;
    if (restoreFocus) focusInvoker(invoker);
  };

  const close = (kind: ModalSurfaceKind, restoreFocus = true): void => {
    const dialog = dialogFor(kind);
    if (!dialog) return;
    const wasActive = activeKind === kind;
    const invoker = activeInvoker;
    activeKind = undefined;
    activeInvoker = undefined;
    // A dialog that is not open ignores close(), so a repeated or stray
    // request is a no-op rather than a second cleanup.
    dialog.close();
    if (wasActive && restoreFocus) focusInvoker(invoker);
  };

  const open = (kind: ModalSurfaceKind, invoker?: HTMLElement): void => {
    const registration = registrations.get(kind);
    if (!registration) return;
    if (activeKind === kind) {
      activeInvoker = invoker;
      return;
    }
    if (activeKind !== undefined) close(activeKind, false);
    const dialog = registration.dialog;
    activeKind = kind;
    activeInvoker = invoker;
    if (!dialog.open) dialog.showModal();
  };

  const listen = (kind: ModalSurfaceKind): void => {
    const registration = registrations.get(kind);
    if (!registration || listeners.has(kind)) return;
    const controller = new AbortController();
    listeners.set(kind, controller);
    const dialog = registration.dialog;
    // Escape and a platform close request cancel the dialog. Preventing the
    // default and closing through the controller keeps one synchronous
    // cleanup path, so focus returns exactly once.
    dialog.addEventListener(
      "cancel",
      (event) => {
        event.preventDefault();
        close(kind);
      },
      { signal: controller.signal },
    );
    // A close this controller did not start still clears its bookkeeping.
    dialog.addEventListener("close", () => cleanup(kind, true, activeInvoker), {
      signal: controller.signal,
    });
    // A click that lands on the dialog itself rather than its content is a
    // scrim activation for a surface that spans the viewport.
    dialog.addEventListener(
      "click",
      (event) => {
        if (event.target === dialog) close(kind);
      },
      { signal: controller.signal },
    );
    // Explicit edge focus handling keeps Tab and Shift+Tab inside the surface.
    // Native modality already makes the background unreachable, but its wrap
    // can release focus to the browser for one step, so the controller owns
    // the cycle itself.
    dialog.addEventListener(
      "keydown",
      (event) => {
        if (event.key !== "Tab" || activeKind !== kind) return;
        const focusable = Array.from(
          dialog.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
        ).filter((element) => element.offsetParent !== null);
        if (focusable.length === 0) return;
        const active = document.activeElement;
        const first = focusable[0]!;
        const last = focusable[focusable.length - 1]!;
        if (event.shiftKey) {
          if (active === first || active === dialog || active === null) {
            event.preventDefault();
            last.focus();
          }
          return;
        }
        if (active === last || active === dialog || active === null) {
          event.preventDefault();
          first.focus();
        }
      },
      { signal: controller.signal },
    );
  };

  return {
    register(kind, registration) {
      registrations.set(kind, registration);
      listen(kind);
    },
    open,
    close,
    closeAll() {
      for (const kind of Array.from(registrations.keys()))
        if (isOpen(kind)) close(kind, false);
    },
    isActive: (kind) => activeKind === kind,
    blocking: () => activeKind !== undefined,
    focus: focusInvoker,
    dispose() {
      for (const controller of listeners.values()) controller.abort();
      listeners.clear();
      registrations.clear();
      activeKind = undefined;
      activeInvoker = undefined;
    },
  };
}
