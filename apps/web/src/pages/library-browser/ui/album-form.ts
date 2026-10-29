import type { ModalSurfaces } from "./modal-surface.js";
import type { BatchAlbumsViewModel } from "../model/grid-batch-albums-contract.js";
import { paragraph } from "./presentation-helpers.js";
import type {
  AlbumFormReference,
  LibraryBrowserIntent,
  SourceListViewModel,
} from "./library-browser-view.js";

type AlbumChoice = Readonly<{ id: string; name: string }>;

type AlbumFormIntent = Extract<
  LibraryBrowserIntent,
  | { kind: "album-form-open" }
  | { kind: "album-form-close" }
  | { kind: "album-form-submit" }
  | { kind: "grid-batch-album-add" }
  | { kind: "album-overview-retry" }
  | { kind: "album-resume" }
>;

type AlbumFormState = {
  kind: AlbumFormReference["kind"];
  formId: string;
  albumId?: string;
  name: string;
  contextual?: boolean;
  returnFocusKey: string;
  pending: boolean;
  message?: string;
  unknown?: boolean;
};

type AlbumFocusRequest =
  | Readonly<{ kind: "form"; formId: string }>
  | Readonly<{ kind: "return"; focusKey: string }>;

export interface AlbumFormController {
  actionFocusKey(kind: AlbumFormReference["kind"], albumId?: string): string;
  createAlbumTools(album: SourceListViewModel["albums"][number]): HTMLElement;
  open(kind: AlbumFormReference["kind"], albumId?: string, name?: string): void;
  openContextual(
    invoker: HTMLElement,
    onReturn: (album?: AlbumChoice) => void,
  ): void;
  openChooser(invoker: HTMLElement, count: number, source: string): void;
  setAlbums(model: BatchAlbumsViewModel): void;
  /// Applies a focus request that can only resolve after the source list is
  /// rebuilt. Returns true when it consumed a request.
  restoreSourceFocus(
    focusTarget: (focusKey: string) => HTMLElement | undefined,
  ): boolean;
  /// Discards the draft during a destination change. The return value tells
  /// the caller whether the form owned focus and therefore needs a fallback.
  discard(): boolean;
  setMessage(formId: string, message: string, unknown?: boolean): void;
  setPending(formId: string, pending: boolean, name?: string): void;
  dismiss(formId: string, album?: AlbumChoice, suppressReturn?: boolean): void;
  dispose(): void;
}

export function createAlbumForm({
  elements,
  surfaces,
  send,
  resetGestures,
  findFocusTarget,
}: Readonly<{
  elements: Readonly<{
    albumFormDialog: HTMLDialogElement;
    albumFormBody: HTMLElement;
  }>;
  surfaces: ModalSurfaces;
  send: (intent: AlbumFormIntent) => void;
  resetGestures: () => void;
  findFocusTarget: (focusKey: string) => HTMLElement | undefined;
}>): AlbumFormController {
  const { albumFormDialog, albumFormBody } = elements;
  let alive = true;
  let albumFormCounter = 0;
  let albumForm: AlbumFormState | undefined;
  let albumFocusRequest: AlbumFocusRequest | undefined;
  let albumFormInvoker: HTMLElement | undefined;
  let albums: BatchAlbumsViewModel["albums"] = [];
  let chooser:
    | {
        invoker: HTMLElement;
        count: number;
        source: string;
        selected: string;
        confirmed?: AlbumChoice;
      }
    | undefined;
  let contextual:
    | {
        invoker: HTMLElement;
        onReturn: (album?: AlbumChoice) => void;
        dialog: HTMLDialogElement;
        hidden: HTMLElement[];
        title?: HTMLElement;
        previousTitle?: string;
        listeners: AbortController;
      }
    | undefined;

  surfaces.register("album-form", {
    dialog: albumFormDialog,
    modal: () => true,
  });
  albumFormDialog.addEventListener("close", () => {
    if (!albumFormDialog.open && (albumForm || chooser)) abandonContext();
  });

  const albumActionFocusKey = (
    kind: AlbumFormReference["kind"],
    albumId = "",
  ): string =>
    kind === "create" ? "album:create" : `album:${kind}:${albumId}`;

  const albumFormTitle = (form: AlbumFormState): string =>
    form.kind === "create"
      ? "Create Album"
      : form.kind === "rename"
        ? "Rename Album"
        : "Delete Album";

  const albumFormMessage = (): HTMLParagraphElement => {
    const message = document.createElement("p");
    message.className = "album-form-message";
    message.setAttribute("role", "alert");
    return message;
  };

  const albumNameInput = (form: AlbumFormState): HTMLInputElement => {
    const input = document.createElement("input");
    input.type = "text";
    input.name = "name";
    input.dataset.albumFormId = form.formId;
    input.dataset.focusKey = `album:form:${form.formId}:name`;
    input.setAttribute("aria-label", "Album name");
    input.value = form.name;
    return input;
  };

  /// The control the Album form owns: the name field, the delete confirmation,
  /// or Cancel while the write settles and the committing control is disabled.
  const albumFormControl = (
    form: AlbumFormState,
  ): HTMLInputElement | HTMLButtonElement | undefined => {
    const selector =
      form.kind === "delete"
        ? `[data-album-form-id="${form.formId}"][data-focus-key$=":confirm"]`
        : `input[data-album-form-id="${form.formId}"]`;
    const target = albumFormBody.querySelector<HTMLElement>(selector);
    if (target && !target.matches(":disabled")) {
      return target as HTMLInputElement | HTMLButtonElement;
    }
    return albumFormBody.querySelector<HTMLElement>(
      `[data-album-form-id="${form.formId}"][data-focus-key$=":cancel"]`,
    ) as HTMLButtonElement | undefined;
  };

  /// Rebuilds the Album form surface from its own state. Only a change to the
  /// form reaches it, so a background source-list re-render never touches the
  /// draft, and the caret and validation message survive their own rebuilds.
  const renderAlbumForm = (): void => {
    const form = albumForm;
    if (!form) {
      albumFormBody.replaceChildren();
      return;
    }
    const active = document.activeElement;
    const heldForm =
      active instanceof HTMLElement &&
      active.dataset.albumFormId === form.formId;
    const heldSelection =
      active instanceof HTMLInputElement
        ? [active.selectionStart, active.selectionEnd]
        : undefined;
    const header = document.createElement("header");
    header.className = "album-dialog-header";
    if (!contextual?.title) {
      const title = document.createElement("h2");
      title.id = "album-form-title";
      title.textContent = albumFormTitle(form);
      header.append(title);
      const close = document.createElement("button");
      close.type = "button";
      close.className = "quiet";
      close.dataset.albumWorkflowClose = "true";
      close.textContent = "Close";
      close.addEventListener("click", () => {
        abandonContext();
        surfaces.close("album-form");
      });
      header.append(close);
    }
    albumFormBody.replaceChildren(header);
    if (form.kind === "delete") {
      const confirmBox = document.createElement("div");
      confirmBox.className = "album-confirm";
      confirmBox.setAttribute("role", "alert");
      confirmBox.append(
        paragraph(`Delete “${form.name}”?`),
        paragraph("Photos and Original Files remain unchanged."),
      );
      const confirm = document.createElement("button");
      confirm.type = "button";
      confirm.dataset.albumFormId = form.formId;
      confirm.dataset.focusKey = `album:form:${form.formId}:confirm`;
      confirm.textContent = "Delete Album";
      confirm.disabled = form.pending;
      confirm.addEventListener("click", () => {
        if (albumForm === form && !form.pending)
          send({ kind: "album-form-submit", formId: form.formId });
      });
      const cancel = document.createElement("button");
      cancel.type = "button";
      cancel.dataset.albumFormId = form.formId;
      cancel.dataset.focusKey = `album:form:${form.formId}:cancel`;
      cancel.textContent = "Cancel";
      cancel.addEventListener("click", () => closeAlbumForm(form));
      confirmBox.append(confirm, cancel);
      albumFormBody.append(confirmBox);
    } else {
      const element = document.createElement("form");
      element.className = "album-form";
      element.setAttribute("aria-label", albumFormTitle(form));
      const input = albumNameInput(form);
      const message = albumFormMessage();
      message.textContent = form.message ?? "";
      const save = document.createElement("button");
      save.type = "submit";
      save.dataset.albumFormId = form.formId;
      save.dataset.focusKey = `album:form:${form.formId}:submit`;
      save.textContent = form.kind === "create" ? "Create Album" : "Save Name";
      save.disabled = form.pending || form.unknown === true;
      const cancel = document.createElement("button");
      cancel.type = "button";
      cancel.dataset.albumFormId = form.formId;
      cancel.dataset.focusKey = `album:form:${form.formId}:cancel`;
      cancel.textContent = "Cancel";
      cancel.addEventListener("click", () => closeAlbumForm(form));
      input.addEventListener("input", () => {
        if (alive && albumForm === form) {
          form.name = input.value;
          if (!form.unknown) {
            delete form.message;
            message.textContent = "";
          }
        }
      });
      element.append(input, save, cancel, message);
      element.addEventListener("submit", (event) => {
        event.preventDefault();
        if (albumForm !== form || form.pending || form.unknown) return;
        send({
          kind: "album-form-submit",
          formId: form.formId,
          name: input.value,
        });
      });
      if (form.unknown) {
        const inspect = document.createElement("button");
        inspect.type = "button";
        inspect.textContent = "Inspect Albums";
        inspect.addEventListener("click", () => {
          closeAlbumForm(form);
          send({ kind: "album-overview-retry" });
        });
        element.append(inspect);
      }
      albumFormBody.append(element);
    }
    // A rebuild keeps the control the Photographer was using, including the
    // caret, so a validation message or a pending write never moves focus.
    const request = albumFocusRequest;
    const control = albumFormControl(form);
    if (!control) return;
    if (request?.kind === "form" && request.formId === form.formId) {
      albumFocusRequest = undefined;
      control.focus();
      if (control instanceof HTMLInputElement) control.select();
      return;
    }
    if (heldForm) {
      control.focus();
      if (control instanceof HTMLInputElement && heldSelection) {
        const end = control.value.length;
        control.setSelectionRange(
          Math.min(heldSelection[0] ?? end, end),
          Math.min(heldSelection[1] ?? end, end),
        );
      }
      return;
    }
    // Opening a modal moves focus into the surface.
    control.focus();
    if (control instanceof HTMLInputElement && form.name === "")
      control.select();
  };
  const restoreContext = () => {
    const current = contextual;
    if (!current) return;
    contextual = undefined;
    albumFocusRequest = undefined;
    current.listeners.abort();
    for (const element of current.hidden) element.hidden = false;
    if (current.title && current.previousTitle !== undefined)
      current.title.textContent = current.previousTitle;
    if (current.dialog !== albumFormDialog)
      albumFormDialog.append(albumFormBody);
  };

  const abandonContext = () => {
    const form = albumForm;
    albumForm = undefined;
    albumFocusRequest = undefined;
    restoreContext();
    chooser = undefined;
    albumFormBody.replaceChildren();
    if (form) send({ kind: "album-form-close", formId: form.formId });
  };

  const openContextual = (
    invoker: HTMLElement,
    onReturn: (album?: AlbumChoice) => void,
  ) => {
    if (!alive || albumForm) return;
    const dialog = invoker.closest("dialog");
    if (!dialog || !dialog.open) return;
    const parent =
      dialog === albumFormDialog
        ? albumFormBody
        : invoker.closest<HTMLElement>(".photo-tools-body");
    if (!parent) return;
    const hidden =
      dialog === albumFormDialog
        ? []
        : Array.from(parent.children).filter(
            (child): child is HTMLElement =>
              child instanceof HTMLElement && !child.hidden,
          );
    const title =
      dialog === albumFormDialog
        ? undefined
        : (dialog.querySelector<HTMLElement>("[data-photo-tools-title]") ??
          undefined);
    contextual = {
      invoker,
      onReturn,
      dialog,
      hidden,
      ...(title ? { title, previousTitle: title.textContent ?? "" } : {}),
      listeners: new AbortController(),
    };
    for (const element of hidden) element.hidden = true;
    if (title) title.textContent = "Create Album";
    if (dialog !== albumFormDialog) parent.append(albumFormBody);
    const signal = contextual.listeners.signal;
    dialog.addEventListener(
      "cancel",
      (event) => {
        if (!albumForm || !contextual) return;
        event.preventDefault();
        event.stopImmediatePropagation();
        closeAlbumForm(albumForm);
      },
      { capture: true, signal },
    );
    dialog.addEventListener(
      "click",
      (event) => {
        if (
          event.target === dialog ||
          (event.target instanceof Element &&
            event.target.closest(
              "[data-photo-tools-close], [data-album-workflow-close]",
            ))
        )
          abandonContext();
      },
      { capture: true, signal },
    );
    dialog.addEventListener(
      "close",
      () => {
        if (!dialog.open && contextual) abandonContext();
      },
      { signal },
    );
    openAlbumForm("create");
  };

  const openAlbumForm = (
    kind: AlbumFormReference["kind"],
    albumId = "",
    name = "",
  ): void => {
    if (!alive) return;
    albumFormInvoker =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : undefined;
    albumForm = {
      kind,
      formId: `album-form-${++albumFormCounter}`,
      ...(albumId ? { albumId } : {}),
      name,
      returnFocusKey: albumActionFocusKey(kind, albumId),
      pending: false,
      ...(contextual ? { contextual: true } : {}),
    };
    albumFocusRequest = { kind: "form", formId: albumForm.formId };
    send({ kind: "album-form-open", form: { ...albumForm } });
    resetGestures();
    if (!contextual) surfaces.open("album-form", albumFormInvoker);
    renderAlbumForm();
  };

  /// Closes the Album form and returns focus to the action that opened it, or
  /// to the nearest valid Album action when that row is gone.
  const dismissAlbumFormSurface = (form: AlbumFormState): void => {
    const invoker = albumFormInvoker;
    albumFormInvoker = undefined;
    surfaces.close("album-form", false);
    if (invoker?.isConnected && !("disabled" in invoker && invoker.disabled)) {
      surfaces.focus(invoker);
      return;
    }
    const target =
      findFocusTarget(form.returnFocusKey) ?? findFocusTarget("album:create");
    if (target) surfaces.focus(target);
  };
  const renderChooser = () => {
    const current = chooser;
    if (!current || albumForm) return;
    const active = document.activeElement;
    const heldSelect =
      active instanceof HTMLSelectElement && albumFormBody.contains(active);
    const heldCreate =
      active instanceof HTMLElement &&
      active.hasAttribute("data-chooser-new-album");
    const heldRefresh =
      active instanceof HTMLElement &&
      active.hasAttribute("data-chooser-refresh");
    const heldAction =
      active instanceof HTMLElement && albumFormBody.contains(active)
        ? active.dataset.chooserAction
        : undefined;
    const header = document.createElement("header");
    header.className = "album-dialog-header";
    const title = document.createElement("h2");
    title.id = "album-form-title";
    title.textContent = "Add to Album";
    const close = document.createElement("button");
    close.type = "button";
    close.className = "quiet";
    close.textContent = "Close";
    close.dataset.chooserAction = "close";
    close.addEventListener("click", () => {
      chooser = undefined;
      surfaces.close("album-form");
    });
    header.append(title, close);
    const details = paragraph(
      `${current.count.toLocaleString()} selected from ${current.source}`,
    );
    const label = document.createElement("label");
    label.textContent = "Choose an Album";
    const select = document.createElement("select");
    const placeholder = document.createElement("option");
    placeholder.value = "";
    placeholder.textContent = "Choose an Album";
    select.append(placeholder);
    const choices =
      current.confirmed &&
      !albums.some((album) => album.id === current.confirmed!.id)
        ? [...albums, current.confirmed]
        : albums;
    for (const album of choices) {
      const option = document.createElement("option");
      option.value = album.id;
      option.textContent = album.name;
      select.append(option);
    }
    if (!choices.some((album) => album.id === current.selected))
      current.selected = "";
    select.value = current.selected;
    label.append(select);
    const create = document.createElement("button");
    create.type = "button";
    create.textContent = "New Album";
    create.addEventListener("click", () =>
      openContextual(create, (album) => {
        if (!chooser) return;
        if (album) {
          chooser.confirmed = album;
          chooser.selected = album.id;
        }
        renderChooser();
        const target = album
          ? albumFormBody.querySelector<HTMLSelectElement>("select")
          : albumFormBody.querySelector<HTMLButtonElement>(
              "[data-chooser-new-album]",
            );
        target?.focus();
      }),
    );
    create.dataset.chooserNewAlbum = "true";
    const confirm = document.createElement("button");
    confirm.type = "button";
    confirm.dataset.chooserAction = "confirm";
    const refreshMessage = paragraph(
      "Album created. Refresh the Album list before adding Photos.",
    );
    const refresh = document.createElement("button");
    refresh.type = "button";
    refresh.textContent = "Refresh Album list";
    refresh.dataset.chooserRefresh = "true";
    refresh.addEventListener("click", () =>
      send({ kind: "album-overview-retry" }),
    );
    const update = () => {
      const chosen = choices.find((album) => album.id === current.selected);
      const authoritative = albums.some(
        (album) => album.id === current.selected,
      );
      confirm.disabled = !chosen || !authoritative;
      confirm.textContent = chosen
        ? `Add to Album: ${chosen.name}`
        : "Add to Album";
      refresh.hidden = refreshMessage.hidden = !chosen || authoritative;
    };
    select.addEventListener("change", () => {
      current.selected = select.value;
      update();
    });
    confirm.addEventListener("click", () => {
      if (
        !chooser ||
        !current.selected ||
        !albums.some((album) => album.id === current.selected)
      )
        return;
      const albumId = current.selected;
      chooser = undefined;
      surfaces.close("album-form");
      send({ kind: "grid-batch-album-add", albumId });
    });
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.textContent = "Cancel";
    cancel.dataset.chooserAction = "cancel";
    cancel.addEventListener("click", () => {
      chooser = undefined;
      surfaces.close("album-form");
    });
    albumFormBody.replaceChildren(
      header,
      details,
      label,
      refreshMessage,
      refresh,
      create,
      confirm,
      cancel,
    );
    update();
    if (heldSelect || heldRefresh) select.focus();
    else if (heldCreate) create.focus();
    else if (heldAction === "close") close.focus();
    else if (heldAction === "cancel") cancel.focus();
    else if (heldAction === "confirm")
      (confirm.disabled ? select : confirm).focus();
  };

  const openChooser = (invoker: HTMLElement, count: number, source: string) => {
    if (!alive || count === 0) return;
    chooser = { invoker, count, source, selected: "" };
    resetGestures();
    surfaces.open("album-form", invoker);
    renderChooser();
    albumFormBody.querySelector<HTMLElement>("select")?.focus();
  };

  const closeAlbumForm = (form: AlbumFormState): void => {
    if (!alive || albumForm !== form) return;
    const parent = contextual;
    albumForm = undefined;
    send({ kind: "album-form-close", formId: form.formId });
    if (parent) {
      restoreContext();
      parent.onReturn();
      if (parent.dialog !== albumFormDialog && parent.invoker.isConnected)
        parent.invoker.focus();
    } else {
      albumFocusRequest = { kind: "return", focusKey: form.returnFocusKey };
      dismissAlbumFormSurface(form);
    }
  };

  const createAlbumTools = (
    album: SourceListViewModel["albums"][number],
  ): HTMLElement => {
    const tools = document.createElement("div");
    tools.className = "album-tools";
    const rename = document.createElement("button");
    rename.type = "button";
    rename.className = "album-tool";
    rename.textContent = "Rename";
    rename.dataset.focusKey = albumActionFocusKey("rename", album.id);
    rename.setAttribute("aria-label", `Rename ${album.name}`);
    rename.addEventListener("click", () =>
      openAlbumForm("rename", album.id, album.name),
    );
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "album-tool";
    remove.textContent = "Delete";
    remove.dataset.focusKey = albumActionFocusKey("delete", album.id);
    remove.setAttribute("aria-label", `Delete ${album.name}`);
    remove.addEventListener("click", () =>
      openAlbumForm("delete", album.id, album.name),
    );
    tools.append(rename, remove);
    // Resume resolves the saved position under the existing saved-position
    // rules and opens Photo View, so its destination is not an address: it
    // stays an explicit button beside the Album's Grid destination, reachable
    // from any source. View options carries the same action for the open
    // Album; both emit the one album-resume intent.
    if (album.hasSavedPosition) {
      const resume = document.createElement("button");
      resume.type = "button";
      resume.className = "album-tool album-resume";
      resume.textContent = "Resume";
      resume.setAttribute("aria-label", `Resume ${album.name}`);
      resume.addEventListener("click", () =>
        send({ kind: "album-resume", albumId: album.id }),
      );
      tools.append(resume);
    }
    return tools;
  };

  return {
    actionFocusKey: albumActionFocusKey,
    createAlbumTools,
    open: openAlbumForm,
    openContextual,
    openChooser,
    setAlbums(model) {
      albums = model.albums;
      if (chooser && !albumForm) renderChooser();
    },
    restoreSourceFocus(focusTarget) {
      const request = albumFocusRequest;
      if (request?.kind === "form" && albumForm?.formId === request.formId) {
        albumFocusRequest = undefined;
        return true;
      }
      if (request?.kind === "return") {
        albumFocusRequest = undefined;
        const target =
          focusTarget(request.focusKey) ??
          focusTarget(albumActionFocusKey("create"));
        target?.focus();
        return true;
      }
      return false;
    },
    discard() {
      if (!albumForm && !chooser) return false;
      const hadFocus = Boolean(
        document.activeElement &&
          (albumFormBody.contains(document.activeElement) ||
            contextual?.dialog.contains(document.activeElement)),
      );
      abandonContext();
      albumFormInvoker = undefined;
      return hadFocus;
    },
    setMessage(formId, message, unknown) {
      if (!alive || !albumForm || albumForm.formId !== formId) return;
      albumForm.message = message;
      albumForm.unknown = unknown === true;
      albumForm.pending = false;
      renderAlbumForm();
    },
    setPending(formId, pending, name) {
      if (!alive || !albumForm || albumForm.formId !== formId) return;
      albumForm.pending = pending;
      delete albumForm.unknown;
      if (name !== undefined) albumForm.name = name;
      delete albumForm.message;
      renderAlbumForm();
    },
    dismiss(formId, album, suppressReturn) {
      if (!alive || !albumForm || albumForm.formId !== formId) return;
      const form = albumForm;
      const parent = contextual;
      albumForm = undefined;
      send({ kind: "album-form-close", formId: form.formId });
      if (parent) {
        restoreContext();
        if (suppressReturn) {
          chooser = undefined;
          surfaces.close(
            parent.dialog === albumFormDialog ? "album-form" : "photo-tools",
          );
        } else {
          parent.onReturn(album);
          if (
            !album &&
            parent.dialog !== albumFormDialog &&
            parent.invoker.isConnected
          )
            parent.invoker.focus();
        }
      } else {
        albumFocusRequest = { kind: "return", focusKey: form.returnFocusKey };
        dismissAlbumFormSurface(form);
      }
    },
    dispose() {
      abandonContext();
      alive = false;
    },
  };
}
