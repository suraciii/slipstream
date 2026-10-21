/// The batch tray's Album choices. `pending` covers one Add to Album settling
/// and the outcome is reported beside the control.
export type BatchAlbumsViewModel = Readonly<{
  albums: ReadonlyArray<Readonly<{ id: string; name: string }>>;
  pending: boolean;
}>;
