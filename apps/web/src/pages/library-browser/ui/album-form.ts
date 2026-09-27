import type { ModalSurfaces } from "./modal-surface.js";
import type {
  AlbumFormReference,
  LibraryBrowserIntent,
  SourceListViewModel,
} from "./library-browser-view.js";

type AlbumFormIntent = Extract<
  LibraryBrowserIntent,
  | { kind: "album-form-open" }
  | { kind: "album-form-close" }
  | { kind: "album-form-submit" }
  | { kind: "album-resume" }
>;

type AlbumFormState = {
  kind: AlbumFormReference["kind"];
  formId: string;
  albumId?: string;
  name: string;
  returnFocusKey: string;
  pending: boolean;
  message?: string;
};

type AlbumFocusRequest =
  | Readonly<{ kind: "form"; formId: string }>
  | Readonly<{ kind: "return"; focusKey: string }>;

export interface AlbumFormController {
  actionFocusKey(kind: AlbumFormReference["kind"], albumId?: string): string;
  createAlbumTools(album: SourceListViewModel["albums"][number]): HTMLElement;
  open(kind: AlbumFormReference["kind"], albumId?: string, name?: string): void;
  /// Applies a focus request that can only resolve after the source list is
  /// rebuilt. Returns true when it consumed a request.
  restoreSourceFocus(
    focusTarget: (focusKey: string) => HTMLElement | undefined,
  ): boolean;
  /// Discards the draft during a destination change. The return value tells
  /// the caller whether the form owned focus and therefore needs a fallback.
  discard(): boolean;
  setMessage(formId: string, message: string): void;
  setPending(formId: string, pending: boolean, name?: string): void;
  dismiss(formId: string): void;
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

  surfaces.register("album-form", {
    dialog: albumFormDialog,
    modal: () => true,
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
    const title = document.createElement("h2");
    title.id = "album-form-title";
    title.textContent = albumFormTitle(form);
    header.append(title);
    albumFormBody.replaceChildren(header);
    if (form.kind === "delete") {
      const confirmBox = document.createElement("div");
      confirmBox.className = "album-confirm";
      confirmBox.setAttribute("role", "alert");
      confirmBox.append(
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
      save.disabled = form.pending;
      const cancel = document.createElement("button");
      cancel.type = "button";
      cancel.dataset.albumFormId = form.formId;
      cancel.dataset.focusKey = `album:form:${form.formId}:cancel`;
      cancel.textContent = "Cancel";
      cancel.addEventListener("click", () => closeAlbumForm(form));
      input.addEventListener("input", () => {
        if (alive && albumForm === form) {
          form.name = input.value;
          delete form.message;
          // Editing clears the validation message where it is presented, so a
          // background refresh never restores a stale one.
          message.textContent = "";
        }
      });
      element.append(input, save, cancel, message);
      element.addEventListener("submit", (event) => {
        event.preventDefault();
        if (albumForm !== form || form.pending) return;
        send({
          kind: "album-form-submit",
          formId: form.formId,
          name: input.value,
        });
      });
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
    };
    albumFocusRequest = { kind: "form", formId: albumForm.formId };
    send({ kind: "album-form-open", form: { ...albumForm } });
    resetGestures();
    surfaces.open("album-form", albumFormInvoker);
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

  const closeAlbumForm = (form: AlbumFormState): void => {
    if (!alive || albumForm !== form) return;
    albumFocusRequest = {
      kind: "return",
      focusKey: form.returnFocusKey,
    };
    albumForm = undefined;
    send({ kind: "album-form-close", formId: form.formId });
    dismissAlbumFormSurface(form);
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
      if (!albumForm) return false;
      albumForm = undefined;
      albumFormInvoker = undefined;
      albumFormBody.replaceChildren();
      return albumFormDialog.contains(document.activeElement);
    },
    setMessage(formId, message) {
      if (!alive || !albumForm || albumForm.formId !== formId) return;
      albumForm.message = message;
      albumForm.pending = false;
      renderAlbumForm();
    },
    setPending(formId, pending, name) {
      if (!alive || !albumForm || albumForm.formId !== formId) return;
      albumForm.pending = pending;
      if (name !== undefined) albumForm.name = name;
      delete albumForm.message;
      renderAlbumForm();
    },
    dismiss(formId) {
      if (!alive || !albumForm || albumForm.formId !== formId) return;
      const form = albumForm;
      albumFocusRequest = {
        kind: "return",
        focusKey: form.returnFocusKey,
      };
      albumForm = undefined;
      send({ kind: "album-form-close", formId: form.formId });
      dismissAlbumFormSurface(form);
    },
    dispose() {
      alive = false;
    },
  };
}

function paragraph(text: string): HTMLParagraphElement {
  const value = document.createElement("p");
  value.textContent = text;
  return value;
}
