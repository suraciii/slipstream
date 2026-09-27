import type {
  LibraryBrowserIntent,
  MembershipViewModel,
} from "./library-browser-view.js";

type MembershipPanelIntentKind = "membership-toggle" | "membership-retry";

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
}>;

export interface MembershipPanel {
  render(model: MembershipViewModel): void;
  dispose(): void;
}

export function createMembershipPanel({
  elements,
  send,
}: Readonly<{
  elements: MembershipPanelElements;
  send: (intent: MembershipPanelIntent) => void;
}>): MembershipPanel {
  const {
    membershipStatus,
    membershipList,
    membershipMessage,
    membershipManage,
    membershipRetry,
    membershipPanel,
    membershipOptions,
  } = elements;
  let alive = true;
  let membershipManageOpen = false;
  let membershipFocusAlbumId: string | undefined;

  const render = (model: MembershipViewModel): void => {
    if (!alive) return;
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
        empty.textContent = model.photoPresent
          ? "No Albums yet."
          : "No current Photo.";
        empty.className = "membership-empty";
        membershipOptions.append(empty);
        return;
      }
      for (const album of model.options) {
        const option = document.createElement("label");
        option.className = "membership-option";
        const input = document.createElement("input");
        input.type = "checkbox";
        input.dataset.membershipAlbumId = album.id;
        input.checked = album.member;
        input.disabled = !model.photoPresent || pending.has(album.id);
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
      const targetId = focusedAlbumId ?? membershipFocusAlbumId;
      if (targetId !== undefined) {
        const restored = Array.from(
          membershipOptions.querySelectorAll("input"),
        ).find((input) => input.dataset.membershipAlbumId === targetId);
        if (!restored) membershipFocusAlbumId = undefined;
        else if (!restored.disabled) {
          if (document.activeElement === document.body) restored.focus();
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

  let currentModel: MembershipViewModel | undefined;
  return {
    render(model) {
      if (!alive) return;
      currentModel = model;
      render(model);
    },
    dispose() {
      alive = false;
      currentModel = undefined;
      membershipFocusAlbumId = undefined;
    },
  };
}
