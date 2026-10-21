/// The Destination controller's view port: exactly the view controls a
/// destination establishment drives. The page composition root passes the
/// full view; the controller names only these members, so the model never
/// depends on the whole browser view.

export interface DestinationViewPort {
  /// Clears the open source's presentation before a new source opens.
  prepareSourceOpen(name: string): void;
  /// Presents an explained state that is not a source Grid: the Photographer
  /// is told why the requested destination is not shown and given the one
  /// explicit action that establishes it. It never claims a source loaded.
  setGridExplanation(
    message: string,
    action?: Readonly<{ label: string }>,
  ): void;
  /// Shows the source Grid, optionally at one Photo.
  showGrid(index?: number): void;
  /// Restores one captured Grid anchor after the current size step has
  /// determined the column count, then returns cell focus to `focusIndex`, or
  /// focuses the Grid when no cell is named. The offset is applied once, so
  /// the restore never fights later user scrolling.
  restoreGridAnchor(
    model: Readonly<{
      index: number;
      offset: number;
      focusIndex?: number;
    }>,
  ): void;
  /// Closes the transient surfaces a destination change supersedes: the
  /// supporting sheets, the Sources surface, the Album form, and the recovery
  /// review. It adds no history entry.
  closeTransientSurfaces(): void;
}
