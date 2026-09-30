import {
  allPhotosDestination,
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

export interface NavigationSession {
  takeStartup(): StartupDestination | undefined;
  begin(destination: NavigationDestination): DestinationEstablishment;
  isCurrent(establishment: DestinationEstablishment): boolean;
  fail(destination: NavigationDestination): void;
  takeRetry(): NavigationDestination | undefined;
  captureGrid(value: NavigationGridRestoration | undefined): void;
  readonly gridRestoration: NavigationGridRestoration | undefined;
  requireCurrentFolder(location: string): void;
  takeCurrentFolder(): string | undefined;
  dispose(): void;
}

export function createNavigationSession(
  startup: NavigationStartup,
): NavigationSession {
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
  let current: DestinationEstablishment | undefined;
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
        current = establishment;
        retry = undefined;
      }
      return establishment;
    },
    isCurrent(establishment: DestinationEstablishment): boolean {
      return alive && current === establishment;
    },
    fail(destination: NavigationDestination): void {
      if (alive) retry = destination;
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
      current = undefined;
      retry = undefined;
      restoration = undefined;
      currentFolder = undefined;
    },
  };
}
