import type { FolderChild } from "../api/contracts.js";
import type {
  ApplicationOwner,
  FileLocationPresentation,
} from "./application-owner.js";
import {
  createFileLocationOwner,
  type FileLocationAuthority,
  type FileLocationFailure,
  type FileLocationOutcome,
  type FileLocationOwner,
} from "./file-location-owner.js";
import type { RecoveryClaim, RecoveryGate } from "./async-ownership.js";

export type FileLocationTreeNode = Readonly<{
  location: string;
  name: string;
  photoCount: number;
  hasDescendantFolders: boolean;
  expanded: boolean;
  loading: boolean;
  enabled: boolean;
  active: boolean;
  children: ReadonlyArray<FileLocationTreeNode>;
  pager?: Readonly<{
    page: number;
    pages: number;
    hasPrevious: boolean;
    hasNext: boolean;
  }>;
}>;

export type FileLocationTree = Readonly<{
  enabled: boolean;
  failures: ReadonlyArray<
    Readonly<{ key: string; range: string; parent: string; loading: boolean }>
  >;
  rootExpanded: boolean;
  rootLoading: boolean;
  rootChildren: ReadonlyArray<FileLocationTreeNode>;
  rootPager?: FileLocationTreeNode["pager"];
}>;

type Presentation = Readonly<{
  summary: FileLocationPresentation;
  recovery: RecoveryClaim;
}>;

type Dependencies = Readonly<{
  fetcher: Parameters<typeof createFileLocationOwner>[0];
  application: Pick<
    ApplicationOwner,
    | "claimFileLocation"
    | "presentFileLocation"
    | "releaseFileLocation"
    | "notePublicationConflict"
    | "refreshOverview"
  >;
  recoveryGate: RecoveryGate;
  isAlive: () => boolean;
  onChanged: () => void;
  onReachable: () => void;
}>;

export interface FileLocationController {
  readonly owner: FileLocationOwner;
  readonly publication: string | undefined;
  readonly pageSize: number;
  load(parent: string, page: number, expand?: boolean): Promise<void>;
  reset(): FileLocationAuthority;
  rebind(): Promise<FileLocationAuthority>;
  awaitRootBinding(): Promise<boolean>;
  retry(failure: FileLocationFailure): Promise<void>;
  retryKey(key: string): Promise<void>;
  claimPublicationNotice(key: string, message: string): void;
  releasePublicationNotice(): void;
  toggle(location: string, expanded: boolean): Promise<void>;
  page(parent: string, direction: -1 | 1): Promise<void>;
  tree(isActive: (location: string) => boolean): FileLocationTree;
  refreshCounts(): Promise<void>;
  dispose(): void;
}

export function createFileLocationController(
  deps: Dependencies,
): FileLocationController {
  const owner = createFileLocationOwner(deps.fetcher);
  const presentations = new Map<FileLocationFailure, Presentation>();
  let publicationPresentation: Presentation | undefined;
  const settled = new WeakMap<object, Promise<void>>();
  const expansions = new Map<string, Promise<void>>();
  const retries = new Map<FileLocationFailure, Promise<void>>();
  let closed = false;

  const release = (p: Presentation): void => {
    deps.application.releaseFileLocation(p.summary);
    deps.recoveryGate.recover(p.recovery);
  };
  const claim = (
    key: string,
    message: string,
    transportLost: boolean,
  ): Presentation => {
    const summary = deps.application.claimFileLocation(key, message);
    const recovery = deps.recoveryGate.issue("file-location", key);
    deps.recoveryGate.fail(
      recovery,
      transportLost ? { transportLost: true } : {},
    );
    return { summary, recovery };
  };
  const settle = async (outcome: FileLocationOutcome): Promise<void> => {
    if (closed || !deps.isAlive()) return;
    // Failure identities are committed with the read result. Reconcile them
    // even when a count refresh has since fenced that result's presentation.
    const failures = owner.failures();
    const current = new Set(failures);
    const newest = failures.at(-1);
    const newestWasPresented = newest && presentations.has(newest);
    let changed = false;
    for (const [failure, presentation] of presentations) {
      if (current.has(failure)) continue;
      release(presentation);
      presentations.delete(failure);
      changed = true;
    }
    for (const failure of failures) {
      if (presentations.has(failure)) continue;
      presentations.set(
        failure,
        claim(
          `range:${failure.generation}:${failure.parent}:${failure.page}`,
          failure.message,
          true,
        ),
      );
      changed = true;
    }
    const accepted = owner.accept(outcome);
    if (newest && newestWasPresented && (accepted || changed)) {
      const prior = presentations.get(newest);
      if (prior)
        deps.application.presentFileLocation(prior.summary, newest.message);
    }
    if (!accepted || outcome.kind === "detached" || outcome.kind === "bound") {
      if (changed) deps.onChanged();
      return;
    }
    if (outcome.kind === "publication-conflict") {
      const reboundAuthority = await rebind();
      if (!closed && owner.isCurrent(reboundAuthority) && owner.publication)
        claimPublicationNotice(
          `publication:${owner.publication}`,
          "Library changed. Reloaded folders.",
        );
      if (!closed) deps.onChanged();
      return;
    }
    if (outcome.kind === "loaded" && outcome.markTransportReachable)
      deps.onReachable();
    deps.onChanged();
  };
  const handle = (outcome: FileLocationOutcome): Promise<void> => {
    const prior = settled.get(outcome);
    if (prior) return prior;
    const pending = Promise.resolve().then(() => settle(outcome));
    settled.set(outcome, pending);
    return pending;
  };
  const presentExpansion = async (
    parent: string,
    pending: Promise<void>,
  ): Promise<void> => {
    expansions.set(parent, pending);
    deps.onChanged();
    try {
      await pending;
    } finally {
      if (expansions.get(parent) === pending) {
        expansions.delete(parent);
        if (!closed && deps.isAlive()) deps.onChanged();
      }
    }
  };
  const presentRetry = async (
    failure: FileLocationFailure,
    pending: Promise<void>,
  ): Promise<void> => {
    retries.set(failure, pending);
    try {
      await presentExpansion(failure.parent, pending);
    } finally {
      if (retries.get(failure) === pending) {
        retries.delete(failure);
        if (!closed && deps.isAlive()) deps.onChanged();
      }
    }
  };
  const load = async (
    parent: string,
    page: number,
    expand = true,
  ): Promise<void> => {
    if (closed || !deps.isAlive()) return;
    const joinsRootBinding = parent === "" && !owner.publication;
    const pending = owner.loadWindow(parent, page, expand);
    if (!joinsRootBinding && expansions.delete(parent)) deps.onChanged();
    await handle(await pending);
  };
  const reset = (): FileLocationAuthority => {
    const authority = owner.reset();
    expansions.clear();
    retries.clear();
    if (publicationPresentation) release(publicationPresentation);
    publicationPresentation = undefined;
    for (const p of presentations.values()) release(p);
    presentations.clear();
    deps.onChanged();
    return authority;
  };
  const rebind = async (): Promise<FileLocationAuthority> => {
    if (closed || !deps.isAlive()) return owner.reset();
    deps.application.notePublicationConflict();
    const authority = reset();
    await deps.application.refreshOverview().catch(() => false);
    if (!closed && deps.isAlive()) await load("", 0, true);
    return authority;
  };
  const awaitRootBinding = async (): Promise<boolean> => {
    if (closed || !deps.isAlive()) return false;
    const outcome = await owner.awaitRootBinding();
    if (closed || !deps.isAlive()) return false;
    const bound = outcome.kind === "bound" || outcome.kind === "loaded";
    await handle(outcome);
    return bound && Boolean(owner.publication);
  };
  const retry = async (failure: FileLocationFailure): Promise<void> => {
    if (closed || !deps.isAlive() || !owner.failures().includes(failure))
      return;
    const pending = retries.get(failure);
    if (pending) return pending;
    await presentRetry(failure, owner.retry(failure).then(handle));
  };
  const retryKey = async (key: string): Promise<void> => {
    const failure = owner
      .failures()
      .find((item) => `${item.generation}:${item.parent}:${item.page}` === key);
    if (failure) await retry(failure);
  };
  const claimPublicationNotice = (key: string, message: string): void => {
    if (closed || !deps.isAlive()) return;
    releasePublicationNotice();
    publicationPresentation = claim(key, message, false);
    deps.onChanged();
  };
  const releasePublicationNotice = (): void => {
    if (!publicationPresentation) return;
    release(publicationPresentation);
    publicationPresentation = undefined;
  };
  const toggle = async (location: string, expanded: boolean): Promise<void> => {
    if (closed || !deps.isAlive()) return;
    if (expanded) {
      if (owner.collapse(location)) deps.onChanged();
      return;
    }
    const pending = expansions.get(location);
    if (pending) return pending;
    if (owner.expandRetained(location)) {
      deps.onChanged();
      return;
    }
    await presentExpansion(location, load(location, 0, true));
  };
  const page = async (parent: string, direction: -1 | 1): Promise<void> => {
    const retained = owner.window(parent);
    if (!retained) return;
    const next = retained.page + direction;
    if (next >= 0) await load(parent, next, true);
  };
  const tree = (isActive: (location: string) => boolean): FileLocationTree => {
    const node = (child: FolderChild): FileLocationTreeNode => {
      const retained = owner.isExpanded(child.location)
        ? owner.window(child.location)
        : undefined;
      const pager =
        retained && retained.total > owner.pageSize
          ? {
              page: retained.page,
              pages: Math.max(1, Math.ceil(retained.total / owner.pageSize)),
              hasPrevious: retained.page > 0,
              hasNext: (retained.page + 1) * owner.pageSize < retained.total,
            }
          : undefined;
      return {
        location: child.location,
        name: child.name,
        photoCount: child.photoCount,
        hasDescendantFolders: child.hasDescendantFolders,
        expanded: owner.isExpanded(child.location),
        loading: expansions.has(child.location),
        enabled: Boolean(owner.publication),
        active: isActive(child.location),
        children: retained?.children.map(node) ?? [],
        ...(pager ? { pager } : {}),
      };
    };
    const root = owner.window("");
    return {
      enabled: Boolean(owner.publication),
      failures: owner.failures().map((f) => ({
        key: `${f.generation}:${f.parent}:${f.page}`,
        range: f.range,
        parent: f.parent,
        loading: retries.has(f),
      })),
      rootExpanded: owner.isExpanded(""),
      rootLoading: expansions.has(""),
      rootChildren: owner.isExpanded("") && root ? root.children.map(node) : [],
      ...(root && root.total > owner.pageSize
        ? {
            rootPager: {
              page: root.page,
              pages: Math.max(1, Math.ceil(root.total / owner.pageSize)),
              hasPrevious: root.page > 0,
              hasNext: (root.page + 1) * owner.pageSize < root.total,
            },
          }
        : {}),
    };
  };
  const refreshCounts = async (): Promise<void> => {
    if (closed || !deps.isAlive()) return;
    const presenting = new Set(expansions.keys());
    const retrying = new Map(
      [...retries.keys()].map((failure) => [failure.parent, failure]),
    );
    const plan = owner.prepareCountRefresh();
    expansions.clear();
    retries.clear();
    // Admit the captured visible windows before another interaction can
    // change their pages or disclosure. Old transports no longer own feedback.
    const pending = [...plan].map(([parent, request]) => {
      const loaded = load(parent, request.page, request.expand);
      const failure = retrying.get(parent);
      if (failure?.page === request.page && owner.failures().includes(failure))
        return presentRetry(failure, loaded);
      return presenting.has(parent) ? presentExpansion(parent, loaded) : loaded;
    });
    deps.onChanged();
    await Promise.all(pending);
  };
  return {
    owner,
    get publication() {
      return owner.publication;
    },
    get pageSize() {
      return owner.pageSize;
    },
    load,
    reset,
    rebind,
    awaitRootBinding,
    retry,
    retryKey,
    claimPublicationNotice,
    releasePublicationNotice,
    toggle,
    page,
    tree,
    refreshCounts,
    dispose: () => {
      if (closed) return;
      closed = true;
      owner.dispose();
      expansions.clear();
      retries.clear();
      for (const p of presentations.values()) release(p);
      releasePublicationNotice();
      presentations.clear();
    },
  };
}
