/// Presents the connection state. A wide layout keeps it in the application
/// header. A narrow layout has no dedicated brand or connected-status row, so
/// the normal state shows no permanent connection text at all and the open
/// view's own header carries the state beside its primary actions only while
/// it is a failure. Exactly one indicator holds the text at a time.
export type ConnectionPresenter = Readonly<{
  setConnected(connected: boolean): void;
  present(): void;
}>;

export function createConnectionPresenter({
  connection,
  gridConnection,
  photoConnection,
  compact,
  shortViewport,
  photoVisible,
}: Readonly<{
  connection: HTMLElement;
  gridConnection: HTMLElement;
  photoConnection: HTMLElement;
  compact: MediaQueryList;
  shortViewport: MediaQueryList;
  photoVisible: () => boolean;
}>): ConnectionPresenter {
  let connected = true;

  const present = (): void => {
    const failed = !connected;
    const state = failed ? "Disconnected" : "Connected";
    gridConnection.classList.toggle("offline", failed);
    photoConnection.classList.toggle("offline", failed);
    // A compact layout has no dedicated brand or connected-status row. A short
    // viewport gives its whole height to the open Photo View, so the Photo
    // header carries the state there exactly as a narrow layout does.
    const compactChrome =
      compact.matches || (photoVisible() && shortViewport.matches);
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
    // A narrow normal state presents no connection text: the compact header
    // keeps its row for the source, the count, and the two tool entries.
    const inPhoto = photoVisible();
    gridConnection.textContent = inPhoto || !failed ? "" : state;
    gridConnection.hidden = inPhoto || !failed;
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
