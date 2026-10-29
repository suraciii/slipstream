/// Album choices for the on-demand batch chooser and the current write state.
export type BatchAlbumsViewModel = Readonly<{
  albums: ReadonlyArray<Readonly<{ id: string; name: string }>>;
  pending: boolean;
}>;
