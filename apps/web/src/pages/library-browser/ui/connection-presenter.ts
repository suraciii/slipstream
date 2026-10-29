/// Presents the connection state. A wide layout keeps it in the application
/// header. A narrow Grid carries a compact readiness status in its source
/// row; a narrow Photo View gives that row to the filename and state, so only
/// a failure appears there beside the affected actions. Exactly one indicator
/// holds the text at a time.
export type ConnectionPresenter = Readonly<{
  setConnected(connected: boolean): void;
  present(): void;
}>;

export function createConnectionPresenter({
  connection,
  gridConnection,
  photoConnection,
  compact,
  photoVisible,
}: Readonly<{
  connection: HTMLElement;
  gridConnection: HTMLElement;
  photoConnection: HTMLElement;
  compact: MediaQueryList;
  photoVisible: () => boolean;
}>): ConnectionPresenter {
  let connected = true;

  const present = (): void => {
    const failed = !connected;
    // One compact word carries the reachable state on every layout; a lost
    // connection names the failure explicitly.
    const state = failed ? "Disconnected" : "Ready";
    gridConnection.classList.toggle("offline", failed);
    photoConnection.classList.toggle("offline", failed);
    // The application bar is not part of Photo View at any width, so the
    // Photo header carries a failure there. A compact Grid carries readiness
    // in its source row; a failure is the only text for an open Photo.
    const compactChrome = compact.matches || photoVisible();
    if (!compactChrome) {
      connection.textContent = state;
      connection.classList.toggle("offline", failed);
      gridConnection.textContent = "";
      gridConnection.hidden = true;
      photoConnection.textContent = "";
      photoConnection.hidden = true;
      return;
    }
    connection.textContent = "";
    // The compact Grid keeps the source, count, and compact readiness status
    // in its first row. A compact Photo View gives that row to the filename
    // and state, so only a failure is presented beside its actions.
    const inPhoto = photoVisible();
    gridConnection.textContent = inPhoto ? "" : state;
    gridConnection.hidden = inPhoto;
    gridConnection.classList.toggle("offline", failed);
    photoConnection.textContent = !inPhoto || !failed ? "" : state;
    photoConnection.hidden = !inPhoto || !failed;
  };

  return {
    setConnected(value) {
      connected = value;
      present();
    },
    present,
  };
}
