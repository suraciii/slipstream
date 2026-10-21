import type { BrowserFetch } from "../api/browser-fetch.js";
import { releaseBrowse } from "../api/source-grid.js";
import { mountAccessBoundary } from "./access-boundary.js";

export function mountLibraryBrowserAccess(
  root: HTMLElement,
  fetcher: BrowserFetch,
  mount: (
    root: HTMLElement,
    fetcher: BrowserFetch,
    signOut: () => void,
    releaseLease: (token: string) => Promise<void>,
  ) => () => void,
): () => void {
  return mountAccessBoundary(
    root,
    fetcher,
    (privateRoot, privateFetcher, signOut, cleanupFetcher) =>
      mount(privateRoot, privateFetcher, signOut, (token) =>
        releaseBrowse(cleanupFetcher, token),
      ),
  );
}
