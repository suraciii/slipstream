import type {
  LibraryBrowserIntent,
  MembershipViewModel,
} from "./library-browser-view.js";

type MembershipPanelIntentKind =
  | "membership-toggle"
  | "membership-retry"
  | "album-overview-retry";

export type MembershipPanelIntent = Extract<
  LibraryBrowserIntent,
  { kind: MembershipPanelIntentKind }
>;

export type MembershipPanelElements = Readonly<{
  membershipStatus: HTMLElement;
  membershipList: HTMLElement;
  membershipMessage: HTMLElement;
  membershipManage: HTMLButtonElement;
  membershipRetry: HTMLButtonElement;
  membershipPanel: HTMLElement;
  membershipOptions: HTMLElement;
  membershipNewAlbum: HTMLButtonElement;
}>;

export interface MembershipPanel {
  render(model: MembershipViewModel): void;
  focusAlbum(album: Readonly<{ id: string; name: string }>): void;
  setAlbums(albums: ReadonlyArray<Readonly<{ id: string }>>): void;
  dispose(): void;
}

export function createMembershipPanel({
  elements,
  send,
  openNewAlbum,
}: Readonly<{
  elements: MembershipPanelElements;
  send: (intent: MembershipPanelIntent) => void;
  openNewAlbum: (invoker: HTMLElement) => void;
}>): MembershipPanel {
  const {
    membershipStatus,
    membershipList,
    membershipMessage,
    membershipManage,
    membershipRetry,
    membershipPanel,
    membershipOptions,
    membershipNewAlbum,
  } = elements;
  let alive = true;
  let membershipManageOpen = false;
  let membershipFocusAlbumId: string | undefined;
  let createdFocusAlbumId: string | undefined;
  let createdAlbum:
    | Readonly<{ id: string; name: string; photoId: string }>
    | undefined;
  let albums: ReadonlyArray<Readonly<{ id: string }>> = [];

  const render = (model: MembershipViewModel): void => {
    if (!alive) return;
    if (createdAlbum && createdAlbum.photoId !== model.photoId) {
      createdAlbum = undefined;
      createdFocusAlbumId = undefined;
    }
    if (createdAlbum && albums.some((album) => album.id === createdAlbum?.id))
      createdAlbum = undefined;
    const pending = new Set(model.pendingAlbumIds);
    const renderFacts = () => {
      membershipList.replaceChildren();
      membershipMessage.hidden = true;
      membershipMessage.textContent = "";
      membershipRetry.hidden = true;
      if (!model.photoPresent) {
        membershipStatus.hidden = false;
        membershipStatus.textContent = "No current Photo.";
        membershipList.hidden = true;
        return;
      }
      if (model.loading) {
        membershipStatus.hidden = false;
        membershipStatus.textContent = "Loading Albums…";
        membershipList.hidden = true;
      } else if (model.failed) {
        membershipStatus.hidden = false;
        membershipStatus.textContent = "Albums could not be loaded.";
        membershipList.hidden = true;
        membershipRetry.hidden = false;
      } else if (model.containing.length === 0) {
        membershipStatus.hidden = false;
        membershipStatus.textContent = "Not in any Album yet";
        membershipList.hidden = true;
      } else {
        membershipStatus.hidden = true;
        membershipStatus.textContent = "";
        membershipList.hidden = false;
        for (const album of model.containing) {
          const item = document.createElement("li");
          item.className = "membership-item";
          item.textContent = album.name;
          membershipList.append(item);
        }
      }
      if (model.message) {
        membershipMessage.hidden = false;
        membershipMessage.textContent = model.message;
      }
    };
    const renderOptions = () => {
      // Rebuilding the options must not drop keyboard focus from a checkbox
      // the visitor is operating, even while that checkbox is disabled for
      // its in-flight toggle.
      const focused = document.activeElement;
      const focusedAlbumId =
        focused instanceof HTMLInputElement &&
        membershipOptions.contains(focused)
          ? focused.dataset.membershipAlbumId
          : undefined;
      if (focusedAlbumId !== undefined && pending.has(focusedAlbumId))
        membershipFocusAlbumId = focusedAlbumId;
      membershipOptions.replaceChildren();
      if (!model.options.length) {
        const empty = document.createElement("p");
        empty.textContent = !model.photoPresent
          ? "No current Photo."
          : model.loading
            ? "Loading Album membership…"
            : model.failed
              ? "Retry loading Album membership to manage Albums."
              : createdAlbum
                ? "The new Album is not yet in the Album list."
                : "No Albums yet.";
        empty.className = "membership-empty";
        membershipOptions.append(empty);
      }
      for (const album of model.options) {
        const option = document.createElement("label");
        option.className = "membership-option";
        const input = document.createElement("input");
        input.type = "checkbox";
        input.dataset.membershipAlbumId = album.id;
        input.checked = album.member;
        input.disabled =
          !model.photoPresent ||
          model.loading ||
          model.failed ||
          pending.has(album.id);
        input.addEventListener("change", () => {
          if (!alive) return;
          send({
            kind: "membership-toggle",
            albumId: album.id,
            member: input.checked,
          });
        });
        const name = document.createElement("span");
        name.textContent = album.name;
        option.append(input, name);
        membershipOptions.append(option);
      }
      if (createdAlbum) {
        const message = document.createElement("p");
        message.textContent = `Album created: ${createdAlbum.name}. Refresh the Album list before adding Photos.`;
        const refresh = document.createElement("button");
        refresh.type = "button";
        refresh.textContent = "Refresh Album list";
        refresh.addEventListener("click", () =>
          send({ kind: "album-overview-retry" }),
        );
        membershipOptions.append(message, refresh);
      }
      const targetId =
        createdFocusAlbumId ?? focusedAlbumId ?? membershipFocusAlbumId;
      if (targetId !== undefined) {
        const restored = Array.from(
          membershipOptions.querySelectorAll("input"),
        ).find((input) => input.dataset.membershipAlbumId === targetId);
        if (!restored && !createdFocusAlbumId)
          membershipFocusAlbumId = undefined;
        else if (restored && !restored.disabled) {
          if (createdFocusAlbumId || document.activeElement === document.body)
            restored.focus();
          createdFocusAlbumId = undefined;
          membershipFocusAlbumId = undefined;
        }
      }
    };
    renderFacts();
    membershipManage.disabled = !model.photoPresent;
    membershipManage.setAttribute(
      "aria-expanded",
      String(membershipManageOpen),
    );
    membershipPanel.hidden = !membershipManageOpen;
    if (membershipManageOpen) renderOptions();
    else {
      membershipFocusAlbumId = undefined;
      membershipOptions.replaceChildren();
    }
  };

  membershipManage.addEventListener("click", () => {
    if (!alive) return;
    membershipManageOpen = !membershipManageOpen;
    // The page model owns the latest projection; the panel only re-presents
    // it when disclosure changes.
    if (currentModel) render(currentModel);
  });
  membershipRetry.addEventListener("click", () => {
    if (!alive) return;
    send({ kind: "membership-retry" });
  });
  membershipNewAlbum.addEventListener("click", () => {
    if (alive) openNewAlbum(membershipNewAlbum);
  });

  let currentModel: MembershipViewModel | undefined;
  return {
    render(model) {
      if (!alive) return;
      currentModel = model;
      render(model);
    },
    focusAlbum(album) {
      if (!alive || !currentModel?.photoId) return;
      membershipManageOpen = true;
      createdFocusAlbumId = album.id;
      createdAlbum = { ...album, photoId: currentModel.photoId };
      render(currentModel);
    },
    setAlbums(value) {
      albums = value;
      if (currentModel) render(currentModel);
    },
    dispose() {
      alive = false;
      currentModel = undefined;
      membershipFocusAlbumId = undefined;
      createdAlbum = undefined;
    },
  };
}
