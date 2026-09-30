import {
  allPhotosDestination,
  sameSourceView,
  type NavigationDestination,
  type NavigationGridRestoration,
  type NavigationStartup,
} from "./browser-navigation.js";

export type DestinationEstablishment = Readonly<{
  destination: NavigationDestination;
}>;

type StartupDestination = Readonly<{
  destination: NavigationDestination;
  explanation?: string;
  restoration?: NavigationGridRestoration;
}>;

export function createNavigationSession(startup: NavigationStartup) {
  let alive = true;
  let initial: StartupDestination | undefined =
    startup.kind === "invalid"
      ? {
          destination: allPhotosDestination,
          explanation:
            "That link is not a valid Library Browser address. Showing All Photos.",
        }
      : {
          destination: startup.destination,
          ...(startup.entry.anchor && startup.entry.focus
            ? {
                restoration: {
                  anchor: startup.entry.anchor,
                  focus: startup.entry.focus,
                },
              }
            : {}),
        };
  let pending: DestinationEstablishment | undefined;
  let retry: NavigationDestination | undefined;
  let restoration: NavigationGridRestoration | undefined;
  let currentFolder: string | undefined;

  return {
    takeStartup(): StartupDestination | undefined {
      const destination = alive ? initial : undefined;
      initial = undefined;
      return destination;
    },
    begin(destination: NavigationDestination): DestinationEstablishment {
      const establishment = { destination };
      if (alive) {
        pending = establishment;
        retry = undefined;
      }
      return establishment;
    },
    finish(establishment: DestinationEstablishment): void {
      if (pending === establishment) pending = undefined;
    },
    allows(destination: NavigationDestination): boolean {
      return (
        alive && (!pending || sameSourceView(pending.destination, destination))
      );
    },
    fail(destination: NavigationDestination): void {
      if (alive) retry = destination;
    },
    clearRetry(): void {
      retry = undefined;
    },
    takeRetry(): NavigationDestination | undefined {
      const destination = alive ? retry : undefined;
      retry = undefined;
      return destination;
    },
    captureGrid(value: NavigationGridRestoration | undefined): void {
      if (alive) restoration = value;
    },
    get gridRestoration(): NavigationGridRestoration | undefined {
      return alive ? restoration : undefined;
    },
    requireCurrentFolder(location: string): void {
      if (alive) currentFolder = location;
    },
    takeCurrentFolder(): string | undefined {
      const location = alive ? currentFolder : undefined;
      currentFolder = undefined;
      return location;
    },
    dispose(): void {
      if (!alive) return;
      alive = false;
      initial = undefined;
      pending = undefined;
      retry = undefined;
      restoration = undefined;
      currentFolder = undefined;
    },
  };
}
