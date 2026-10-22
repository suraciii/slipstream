import type { NavigationDestination } from "./browser-navigation.js";
import type {
  DestinationEstablishment,
  NavigationSession,
} from "./navigation-session.js";
import type { PhotoAuthority, PhotoOwner } from "./photo-owner.js";
import type { RecoveryGate, RecoveryTransition } from "./async-ownership.js";
import type { SourceGridOwner } from "./source-grid-owner.js";
import type { BrowseWindowController } from "./browse-window-controller.js";
import { randomPosition } from "./random-position.js";

export type GridPhotoAddress = "push" | "replace" | "none";

export type GridPhotoViewPort = Readonly<{
  captureGridRestoration(): Parameters<NavigationSession["captureGrid"]>[0];
  gridVisible(): boolean;
  enterPhoto(): void;
}>;

type GridPhotoDependencies = Readonly<{
  applicationAlive: () => boolean;
  source: Pick<
    SourceGridOwner,
    | "authority"
    | "total"
    | "isCurrent"
    | "isReady"
    | "photoAt"
    | "retryRequired"
    | "stopGridWork"
  >;
  photo: Pick<
    PhotoOwner,
    "authority" | "beginOpen" | "commitOpen" | "cancelOpen" | "isCurrent"
  >;
  view: GridPhotoViewPort;
  navigationSession: Pick<
    NavigationSession,
    "begin" | "isCurrent" | "captureGrid"
  >;
  recoveryGate: Pick<RecoveryGate, "beginTransition" | "succeedTransition">;
  photoRecoveryKey: (authority: PhotoAuthority) => string;
  canOpenGridPhoto: () => boolean;
  loadWindow: BrowseWindowController["load"];
  destinationFor: (photoId?: string) => NavigationDestination;
  recordPhotoAddress: (photoId: string, address: GridPhotoAddress) => void;
  renderPhotoShell: (authority: PhotoAuthority) => boolean;
  showPreview: (
    authority: PhotoAuthority,
    transition: RecoveryTransition,
  ) => Promise<boolean>;
  persistPosition: (authority: PhotoAuthority) => Promise<boolean>;
  syncConnection: () => void;
  updateControls: () => void;
  setGridStatus: (message: string) => void;
}>;

export interface GridPhotoNavigation {
  readonly pending: boolean;
  openPhoto(
    index: number,
    address?: GridPhotoAddress,
    intent?: DestinationEstablishment,
  ): Promise<boolean>;
  openRandomPhoto(): void;
  dispose(): void;
}

export function createGridPhotoNavigation(
  deps: GridPhotoDependencies,
): GridPhotoNavigation {
  let alive = true;
  let pending = false;

  const openPhoto = async (
    index: number,
    address: GridPhotoAddress = "push",
    intent?: DestinationEstablishment,
    ownsPending = false,
  ): Promise<boolean> => {
    // The attempt is this owner's own admission, so it is the one caller that
    // may hand its resolved target to the ordinary Photo path. Every other
    // open waits until the attempt settles.
    if (!alive || !deps.applicationAlive() || !deps.canOpenGridPhoto())
      return false;
    if (pending && !ownsPending) return false;
    const destinationIntent =
      intent ??
      deps.navigationSession.begin(
        deps.destinationFor(deps.source.photoAt(index)?.id),
      );
    const destinationCurrent = () =>
      deps.applicationAlive() &&
      deps.navigationSession.isCurrent(destinationIntent);
    if (!destinationCurrent()) return false;
    deps.navigationSession.captureGrid(deps.view.captureGridRestoration());
    const navigation = deps.photo.beginOpen(index);
    if (!navigation) return false;
    const photoTransition = deps.recoveryGate.beginTransition(
      "photo",
      deps.photoRecoveryKey(navigation.authority),
    );
    deps.syncConnection();
    deps.source.stopGridWork();
    deps.view.enterPhoto();
    let windowReady = true;
    try {
      if (!deps.source.photoAt(index))
        windowReady = await deps.loadWindow(
          index,
          {
            kind: "photo",
            authority: navigation.windowAuthority,
          },
          true,
          "high",
          photoTransition,
          navigation.authority,
        );
      if (
        !destinationCurrent() ||
        !deps.photo.isCurrent(navigation.authority) ||
        !windowReady
      )
        return false;
      const current = deps.photo.commitOpen(navigation);
      if (!current) return false;
      deps.recordPhotoAddress(current.id, address);
      const hasKnownPreview = deps.renderPhotoShell(navigation.authority);
      deps.updateControls();
      const previewRequest = deps.showPreview(
        navigation.authority,
        photoTransition,
      );
      const previewReady = hasKnownPreview || (await previewRequest);
      if (!destinationCurrent() || !deps.photo.isCurrent(navigation.authority))
        return false;
      const positionReady = await deps.persistPosition(navigation.authority);
      if (
        previewReady &&
        positionReady &&
        destinationCurrent() &&
        deps.photo.isCurrent(navigation.authority)
      ) {
        deps.recoveryGate.succeedTransition(photoTransition);
        deps.syncConnection();
      }
    } finally {
      deps.photo.cancelOpen(navigation.authority);
      deps.navigationSession.captureGrid(undefined);
      deps.updateControls();
    }
    return true;
  };

  const openRandomPhoto = (): void => {
    if (
      !alive ||
      !deps.applicationAlive() ||
      pending ||
      !deps.view.gridVisible() ||
      !deps.source.isReady(deps.source.authority) ||
      deps.source.total === 0 ||
      !deps.canOpenGridPhoto()
    )
      return;
    const index = randomPosition(deps.source.total);
    if (index === undefined) return;
    const authority = deps.source.authority;
    pending = true;
    deps.updateControls();
    void (async () => {
      try {
        const ready = await deps.loadWindow(
          index,
          { kind: "grid", authority },
          true,
          "high",
          undefined,
          deps.photo.authority,
          false,
        );
        const current =
          ready &&
          deps.source.isCurrent(authority) &&
          deps.view.gridVisible() &&
          deps.source.photoAt(index) !== undefined;
        if (!current) {
          if (deps.source.isCurrent(authority) && deps.source.retryRequired)
            deps.setGridStatus(
              "Library order expired. Refresh Current Source to continue.",
            );
          return;
        }
        await openPhoto(index, "push", undefined, true);
      } finally {
        pending = false;
        deps.updateControls();
      }
    })();
  };

  return {
    get pending() {
      return pending;
    },
    openPhoto,
    openRandomPhoto,
    dispose() {
      alive = false;
      pending = false;
    },
  };
}
