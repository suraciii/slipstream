import "./ui/access-entry.css";
import {
  createPrivateFetcher,
  createAuthenticatedFetcher,
  exchangeAccessToken,
  readAccessStatus,
  revokeBrowserSession,
  type BrowserFetch,
  type BrowserSession,
} from "./model/access-session.js";

type EntryOptions = Readonly<{
  message?: string;
  configured?: boolean;
  checking?: boolean;
  checkAgain?: boolean;
  logoutRetry?: boolean;
}>;

type MountPrivateLibrary = (
  root: HTMLElement,
  fetcher: BrowserFetch,
  signOut: () => void,
  cleanupFetcher: BrowserFetch,
) => () => void;

const ACCESS_CHANNEL = "slipstream:browser-access";
const DELAY_UNITS = [
  ["day", 86_400],
  ["hour", 3_600],
  ["minute", 60],
  ["second", 1],
] as const;

function formatDelay(totalSeconds: number): string {
  let remaining = Math.max(1, Math.ceil(totalSeconds));
  const parts: string[] = [];
  for (const [unit, unitSeconds] of DELAY_UNITS) {
    const count = Math.floor(remaining / unitSeconds);
    if (count === 0) continue;
    remaining %= unitSeconds;
    parts.push(`${count} ${unit}${count === 1 ? "" : "s"}`);
    if (parts.length === 2) break;
  }
  return parts.join(" ");
}

export function mountAccessBoundary(
  root: HTMLElement,
  fetcher: BrowserFetch,
  mountPrivateLibrary: MountPrivateLibrary,
): () => void {
  let alive = true;
  let session: BrowserSession | undefined;
  let privateHost: HTMLDivElement | undefined;
  let privateDispose: (() => void) | undefined;
  let expiryTimer: ReturnType<typeof setTimeout> | undefined;
  let validation: Promise<boolean> | undefined;
  let validationBlocksViews = false;
  let logoutPending = false;
  let accessEpoch = 0;
  let currentEntry: EntryOptions = { checking: true };
  let channel: BroadcastChannel | undefined;
  let tokenInput: HTMLInputElement | undefined;
  let restoreFocus: HTMLElement | undefined;
  let historyTraversalPending = false;
  let replayingHistoryTraversal = false;

  try {
    channel = new BroadcastChannel(ACCESS_CHANNEL);
    channel.addEventListener("message", onChannelMessage);
  } catch {
    channel = undefined;
  }

  const detachPrivateHost = (
    message = "Checking access…",
    checkAgain = false,
  ): void => {
    if (!privateHost?.isConnected) return;
    const activeElement = document.activeElement;
    if (
      activeElement instanceof HTMLElement &&
      privateHost.contains(activeElement)
    )
      restoreFocus = activeElement;
    privateHost.inert = true;
    privateHost.setAttribute("aria-hidden", "true");
    privateHost.style.visibility = "hidden";
    root.querySelector(".access-check-overlay")?.remove();
    const checking = document.createElement("main");
    checking.className = "access-screen access-check-overlay";
    checking.setAttribute("aria-busy", String(!checkAgain));
    checking.innerHTML = `<section class="access-panel" aria-labelledby="access-check-title"><h1 id="access-check-title">Slipstream</h1><p class="access-subtitle">Your photo library</p><p class="access-message" role="status" aria-live="polite"></p><button type="button" data-access-check hidden>Check access again</button></section>`;
    const status = checking.querySelector<HTMLElement>("[role=status]");
    if (status) status.textContent = message;
    const retry = checking.querySelector<HTMLButtonElement>(
      "[data-access-check]",
    );
    if (retry) {
      retry.hidden = !checkAgain;
      retry.addEventListener("click", () => {
        detachPrivateHost();
        void checkCurrentAccess();
      });
    }
    root.append(checking);
  };

  const clearExpiryTimer = (): void => {
    if (expiryTimer !== undefined) clearTimeout(expiryTimer);
    expiryTimer = undefined;
  };

  const disposePrivate = (): void => {
    const dispose = privateDispose;
    const host = privateHost;
    privateDispose = undefined;
    privateHost = undefined;
    host?.removeEventListener("error", onPrivateImageError, true);
    dispose?.();
  };

  const renderEntry = (options: EntryOptions = {}): void => {
    if (!alive) return;
    currentEntry = options;
    root.innerHTML = `
      <main class="access-screen">
        <section class="access-panel" aria-labelledby="access-title">
          <h1 id="access-title">Slipstream</h1>
          <p class="access-subtitle">Open your photo library</p>
          <p class="access-message" data-access-message role="alert" aria-live="polite" ${options.message ? "" : "hidden"}>${escapeText(options.message ?? "")}</p>
          <form data-access-form>
            <label for="access-token">Access Token</label>
            <div class="access-token-row">
              <input id="access-token" name="access-token" type="password" autocomplete="current-password" autocapitalize="none" spellcheck="false" inputmode="text" maxlength="64" required aria-describedby="access-help" ${options.checking || options.configured === false ? "disabled" : ""}>
              <button type="button" class="quiet" data-access-visibility aria-pressed="false" aria-label="Show Access Token" ${options.checking || options.configured === false ? "disabled" : ""}>Show</button>
            </div>
            <button class="access-submit" type="submit" ${options.checking || options.configured === false ? "disabled" : ""}>${options.checking ? "Checking access…" : "Open library"}</button>
            <p class="access-help" id="access-help">Need an Access Token? Ask the server operator.</p>
            <p class="access-shared-device">On a shared device, sign out when finished.</p>
          </form>
          <div class="access-recovery" data-access-recovery hidden>
            <button type="button" data-access-check>Check access again</button>
            <button type="button" data-access-logout-retry>Retry sign out</button>
          </div>
        </section>
      </main>`;

    const form = root.querySelector<HTMLFormElement>("[data-access-form]");
    tokenInput =
      root.querySelector<HTMLInputElement>("#access-token") ?? undefined;
    const visibility = root.querySelector<HTMLButtonElement>(
      "[data-access-visibility]",
    );
    const recovery = root.querySelector<HTMLElement>("[data-access-recovery]");
    const checkButton = root.querySelector<HTMLButtonElement>(
      "[data-access-check]",
    );
    const logoutButton = root.querySelector<HTMLButtonElement>(
      "[data-access-logout-retry]",
    );

    if (options.checkAgain || options.logoutRetry)
      recovery?.removeAttribute("hidden");
    if (checkButton) checkButton.hidden = !options.checkAgain;
    if (logoutButton) logoutButton.hidden = !options.logoutRetry;

    visibility?.addEventListener("click", () => {
      if (!tokenInput) return;
      const visible = tokenInput.type === "password";
      tokenInput.type = visible ? "text" : "password";
      visibility.textContent = visible ? "Hide" : "Show";
      visibility.setAttribute(
        "aria-label",
        `${visible ? "Hide" : "Show"} Access Token`,
      );
      visibility.setAttribute("aria-pressed", String(visible));
      tokenInput.focus();
    });

    form?.addEventListener("submit", (event) => {
      event.preventDefault();
      if (tokenInput) void submitToken(tokenInput.value);
    });
    checkButton?.addEventListener("click", () => void checkCurrentAccess());
    logoutButton?.addEventListener("click", () => void signOut());
  };

  const escapeText = (value: string): string =>
    value.replace(/[&<>"']/g, (character) => {
      switch (character) {
        case "&":
          return "&amp;";
        case "<":
          return "&lt;";
        case ">":
          return "&gt;";
        case '"':
          return "&quot;";
        default:
          return "&#39;";
      }
    });

  const scheduleExpiry = (): void => {
    clearExpiryTimer();
    if (!session) return;
    const remaining = session.expiresAt - Date.now();
    if (remaining <= 0) {
      expireLocally();
      return;
    }
    // setTimeout has a signed 32-bit limit; a long-lived session is checked in
    // bounded intervals and still expires against the absolute server time.
    expiryTimer = setTimeout(
      () => {
        if (!session) return;
        if (session.expiresAt <= Date.now()) expireLocally();
        else scheduleExpiry();
      },
      Math.min(remaining, 2_000_000_000),
    );
  };

  const attachPrivateHost = (): void => {
    if (!alive || !privateHost || document.visibilityState === "hidden") return;
    if (!privateHost.isConnected) root.append(privateHost);
    privateHost.inert = false;
    privateHost.removeAttribute("aria-hidden");
    privateHost.style.removeProperty("visibility");
    root.querySelector(".access-check-overlay")?.remove();
    if (restoreFocus?.isConnected && privateHost.contains(restoreFocus))
      restoreFocus.focus({ preventScroll: true });
    restoreFocus = undefined;
  };

  const startPrivateLibrary = (): void => {
    if (!alive || !session) return;
    if (privateDispose && privateHost) {
      attachPrivateHost();
      return;
    }
    clearExpiryTimer();
    scheduleExpiry();
    const mountedEpoch = accessEpoch;
    const host = document.createElement("div");
    host.className = "private-library";
    privateHost = host;
    host.addEventListener("error", onPrivateImageError, true);
    root.replaceChildren(host);
    const privateFetcher = createPrivateFetcher(
      fetcher,
      () => (mountedEpoch === accessEpoch ? session?.csrfToken : undefined),
      () => {
        if (mountedEpoch === accessEpoch)
          loseAccess(
            "Your Browser Session is no longer valid. Enter the Access Token to continue.",
          );
      },
      () =>
        mountedEpoch === accessEpoch
          ? beforePrivateRequest()
          : Promise.resolve(false),
    );
    privateDispose = mountPrivateLibrary(
      host,
      privateFetcher,
      () => void signOut(),
      createAuthenticatedFetcher(
        fetcher,
        () => (mountedEpoch === accessEpoch ? session?.csrfToken : undefined),
        () => {},
      ),
    );
  };

  const broadcastInvalidAccess = (): void => {
    try {
      channel?.postMessage({ type: "access-invalid" });
    } catch {
      // This tab still removes its own private content below.
    }
  };

  const loseAccess = (message: string): void => {
    if (!alive) return;
    accessEpoch++;
    broadcastInvalidAccess();
    clearExpiryTimer();
    session = undefined;
    disposePrivate();
    renderEntry({ message, configured: true });
  };

  const expireLocally = (): void => {
    if (!session) return;
    loseAccess(
      "Your Browser Session expired. Enter the Access Token to continue.",
    );
  };

  async function beforePrivateRequest(): Promise<boolean> {
    if (!alive || logoutPending) return false;
    if (validation && validationBlocksViews) {
      const valid = await validation;
      return (
        valid &&
        alive &&
        !logoutPending &&
        !!session &&
        session.expiresAt > Date.now()
      );
    }
    if (!session) return false;
    if (session.expiresAt <= Date.now()) {
      expireLocally();
      return false;
    }
    return true;
  }

  async function checkCurrentAccess(blockViews = true): Promise<boolean> {
    if (!alive) return false;
    validationBlocksViews ||= blockViews;
    if (validation) return validation;
    const requestedEpoch = accessEpoch;
    const priorSession = session;
    if (priorSession && priorSession.expiresAt <= Date.now()) {
      expireLocally();
      return false;
    }
    const promise = (async () => {
      const status = await readAccessStatus(fetcher);
      if (!alive) return false;
      if (requestedEpoch !== accessEpoch || logoutPending) return false;
      if (
        status.kind === "authenticated" &&
        status.session.expiresAt > Date.now()
      ) {
        const sessionChanged =
          priorSession !== undefined &&
          (priorSession.csrfToken !== status.session.csrfToken ||
            priorSession.expiresAt !== status.session.expiresAt);
        if (!priorSession || sessionChanged) {
          accessEpoch++;
          if (sessionChanged) disposePrivate();
        }
        session = status.session;
        scheduleExpiry();
        if (privateDispose) {
          if (validationBlocksViews) attachPrivateHost();
        } else startPrivateLibrary();
        return true;
      }
      if (status.kind === "anonymous") {
        if (priorSession) broadcastInvalidAccess();
        accessEpoch++;
        clearExpiryTimer();
        session = undefined;
        disposePrivate();
        renderEntry({
          ...(!status.configured
            ? {
                message:
                  "Access is not configured. Ask the server operator to configure it.",
              }
            : priorSession
              ? {
                  message:
                    "Your Browser Session is no longer valid. Enter the Access Token to continue.",
                }
              : {}),
          configured: status.configured,
        });
        return false;
      }
      if (status.kind === "authenticated") {
        if (session) expireLocally();
        else {
          session = undefined;
          disposePrivate();
          renderEntry({
            message:
              "Your Browser Session expired. Enter the Access Token to continue.",
            configured: true,
          });
        }
        return false;
      }
      if (!validationBlocksViews) return false;
      if (priorSession) {
        const message =
          "Could not verify access. Check the server connection and try again.";
        if (privateHost?.isConnected) detachPrivateHost(message, true);
        else renderEntry({ message, configured: true, checkAgain: true });
      } else {
        renderEntry({
          message:
            "Slipstream could not be reached. Check the server connection and try again.",
        });
      }
      return false;
    })();
    validation = promise;
    try {
      return await promise;
    } finally {
      if (validation === promise) {
        validation = undefined;
        validationBlocksViews = false;
      }
    }
  }

  function onPrivateImageError(event: Event): void {
    const target = event.target;
    if (!(target instanceof HTMLImageElement) || !session || logoutPending)
      return;
    try {
      const imageUrl = new URL(
        target.currentSrc || target.src,
        window.location.href,
      );
      if (
        imageUrl.origin !== window.location.origin ||
        !imageUrl.pathname.startsWith("/api/private/derivatives/")
      )
        return;
    } catch {
      return;
    }
    void checkCurrentAccess(false);
  }

  async function submitToken(rawToken: string): Promise<void> {
    if (!alive || logoutPending) return;
    const token = rawToken.trim();
    const configured = currentEntry.configured;
    if (!token) {
      renderEntry({
        message: "Enter an Access Token.",
        ...(configured === undefined ? {} : { configured }),
      });
      tokenInput?.focus();
      return;
    }
    if (!/^[A-Za-z0-9_-]{43}$/.test(token)) {
      renderEntry({
        message:
          "Enter the Access Token exactly as provided by the server operator.",
        ...(configured === undefined ? {} : { configured }),
      });
      tokenInput?.focus();
      return;
    }

    // If an access check could not finish, discard the old private model before
    // creating a fresh session. The new mount fetches current server facts and
    // never repeats a mutation whose earlier result was uncertain.
    disposePrivate();
    clearExpiryTimer();
    session = undefined;
    const exchangeEpoch = ++accessEpoch;
    renderEntry({
      message: "Opening your photo library…",
      ...(configured === undefined ? {} : { configured }),
      checking: true,
    });
    const result = await exchangeAccessToken(fetcher, token);
    if (!alive || exchangeEpoch !== accessEpoch) return;

    if (result.kind === "invalid-token") {
      renderEntry({
        message: "That Access Token is incorrect.",
        configured: true,
      });
      tokenInput?.focus();
      return;
    }
    if (result.kind === "rate-limited") {
      renderEntry({
        message: `Too many attempts. Try again in ${formatDelay(result.retryAfterSeconds)}.`,
        configured: true,
      });
      return;
    }
    if (result.kind === "session-capacity") {
      renderEntry({
        message: `All Browser Session slots are in use. A slot is expected to open in ${formatDelay(result.retryAfterSeconds)}. Sign out an existing session or ask the server operator to rotate the Access Token to free a slot sooner.`,
        configured: true,
      });
      return;
    }
    if (result.kind === "unconfigured") {
      renderEntry({
        message:
          "Access is not configured. Ask the server operator to configure it.",
        configured: false,
      });
      return;
    }
    if (result.kind === "unavailable") {
      renderEntry({
        message:
          "Slipstream is unavailable. Check the server connection and try again.",
        configured: true,
      });
      return;
    }

    // A response can be lost after the server creates a session. Check the
    // cookie once; never resend the token automatically.
    const status = await readAccessStatus(fetcher);
    if (!alive || exchangeEpoch !== accessEpoch) return;
    if (
      status.kind === "authenticated" &&
      status.session.expiresAt > Date.now()
    ) {
      session = status.session;
      accessEpoch++;
      tokenInput = undefined;
      startPrivateLibrary();
      return;
    }
    if (status.kind === "anonymous") {
      renderEntry({
        message:
          result.kind === "uncertain"
            ? "The server did not confirm whether access opened. Check the connection, then try again."
            : "The server did not establish a Browser Session. Try again.",
        configured: status.configured,
      });
      return;
    }
    renderEntry({
      message:
        "Could not confirm the Browser Session. Check the server connection and try again.",
      checkAgain: true,
      configured: true,
    });
  }

  async function signOut(): Promise<void> {
    if (!alive || logoutPending) return;
    const activeSession = session;
    if (!activeSession) {
      renderEntry({
        message: "No active Browser Session to sign out.",
        configured: true,
      });
      return;
    }
    logoutPending = true;
    const hadPrivateHost = privateHost !== undefined;
    detachPrivateHost("Signing out…");
    // Disposal starts its keepalive Browse lease release while the session's
    // CSRF material is still available. Other requests are blocked by the
    // logoutPending flag above.
    disposePrivate();
    accessEpoch++;
    if (!hadPrivateHost)
      renderEntry({
        message: "Signing out…",
        checking: true,
        configured: true,
      });
    try {
      channel?.postMessage({ type: "sign-out-started" });
    } catch {
      // This tab still hides its own private content while the request settles.
    }
    const outcome = await revokeBrowserSession(
      fetcher,
      activeSession.csrfToken,
    );
    logoutPending = false;
    if (!alive) return;
    if (outcome === "confirmed") {
      try {
        channel?.postMessage({ type: "signed-out" });
      } catch {
        // The server session is already revoked; local cleanup remains enough
        // if the browser has disabled cross-tab messaging.
      }
      clearExpiryTimer();
      session = undefined;
      accessEpoch++;
      disposePrivate();
      renderEntry({
        message: "Signed out. Enter the Access Token to return.",
        configured: true,
      });
      return;
    }
    disposePrivate();
    try {
      channel?.postMessage({ type: "sign-out-unconfirmed" });
    } catch {
      // Local content is already hidden; retry remains available in this tab.
    }
    renderEntry({
      message:
        "The server could not confirm sign out. Your Photos are hidden here; retry sign out when connected.",
      configured: true,
      logoutRetry: true,
      checkAgain: true,
    });
  }

  function onChannelMessage(event: MessageEvent<unknown>): void {
    if (!alive) return;
    const data: unknown = event.data;
    const type =
      typeof data === "object" &&
      data !== null &&
      "type" in data &&
      typeof data.type === "string"
        ? data.type
        : undefined;
    if (
      type !== "signed-out" &&
      type !== "sign-out-started" &&
      type !== "sign-out-unconfirmed" &&
      type !== "access-invalid"
    )
      return;
    accessEpoch++;
    clearExpiryTimer();
    session = undefined;
    disposePrivate();
    renderEntry({
      message:
        type === "signed-out"
          ? "This Browser Session was signed out in another tab."
          : type === "access-invalid"
            ? "Your Browser Session expired or was rejected in another tab. Enter the Access Token to continue."
            : type === "sign-out-started"
              ? "Sign out is in progress in another tab. Check access before continuing."
              : "Another tab could not confirm sign out. Check access before continuing.",
      configured: true,
      checkAgain: type !== "signed-out",
    });
  }

  const onResumeBoundary = (): void => {
    if (!session || !alive || document.visibilityState === "hidden") return;
    detachPrivateHost();
    void checkCurrentAccess();
  };
  const onHistoryTraversal = (event: PopStateEvent): void => {
    if (replayingHistoryTraversal) {
      replayingHistoryTraversal = false;
      return;
    }
    if (!session || !privateHost || !alive) return;
    event.stopImmediatePropagation();
    if (historyTraversalPending) return;
    historyTraversalPending = true;
    detachPrivateHost();
    void checkCurrentAccess().then(
      (valid) => {
        historyTraversalPending = false;
        if (!alive || !valid || !session) return;
        attachPrivateHost();
        replayingHistoryTraversal = true;
        window.dispatchEvent(new PopStateEvent("popstate"));
      },
      () => {
        historyTraversalPending = false;
      },
    );
  };
  const onVisibilityChange = (): void => {
    if (document.visibilityState === "hidden") {
      if (session && alive) detachPrivateHost();
    } else onResumeBoundary();
  };
  window.addEventListener("popstate", onHistoryTraversal);
  window.addEventListener("online", onResumeBoundary);
  document.addEventListener("visibilitychange", onVisibilityChange);

  renderEntry({ checking: true });
  void checkCurrentAccess();

  return () => {
    if (!alive) return;
    clearExpiryTimer();
    window.removeEventListener("popstate", onHistoryTraversal);
    window.removeEventListener("online", onResumeBoundary);
    document.removeEventListener("visibilitychange", onVisibilityChange);
    channel?.removeEventListener("message", onChannelMessage);
    channel?.close();
    channel = undefined;
    // The private owner's synchronous teardown may issue its keepalive Browse
    // release. Let that one request capture the still-current CSRF token before
    // closing the boundary epoch; all other new requests are then rejected.
    disposePrivate();
    alive = false;
    accessEpoch++;
    session = undefined;
    root.replaceChildren();
  };
}
