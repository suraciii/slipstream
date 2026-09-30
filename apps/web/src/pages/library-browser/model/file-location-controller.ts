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
  failures: ReadonlyArray<Readonly<{ key: string; range: string }>>;
  rootExpanded: boolean;
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
  refreshCounts(parent?: string): Promise<void>;
  dispose(): void;
}

export function createFileLocationController(
  deps: Dependencies,
): FileLocationController {
  const owner = createFileLocationOwner(deps.fetcher);
  const presentations = new Map<FileLocationFailure, Presentation>();
  let publicationPresentation: Presentation | undefined;
  const settled = new WeakMap<object, Promise<void>>();
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
    if (closed || !deps.isAlive() || !owner.accept(outcome)) return;
    if (outcome.kind === "detached" || outcome.kind === "bound") return;
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
    if (outcome.kind === "failed") {
      if (outcome.replaced) {
        const prior = presentations.get(outcome.replaced);
        if (prior) release(prior);
        presentations.delete(outcome.replaced);
      }
      presentations.set(
        outcome.failure,
        claim(
          `range:${outcome.generation}:${outcome.parent}:${outcome.page}`,
          outcome.failure.message,
          true,
        ),
      );
      deps.onChanged();
      return;
    }
    if (outcome.recovered) {
      const prior = presentations.get(outcome.recovered);
      if (prior) release(prior);
      presentations.delete(outcome.recovered);
    }
    if (outcome.remainingNewest) {
      const prior = presentations.get(outcome.remainingNewest);
      if (prior)
        deps.application.presentFileLocation(
          prior.summary,
          outcome.remainingNewest.message,
        );
    }
    if (outcome.markTransportReachable) deps.onReachable();
    deps.onChanged();
  };
  const handle = (outcome: FileLocationOutcome): Promise<void> => {
    const prior = settled.get(outcome);
    if (prior) return prior;
    const pending = Promise.resolve().then(() => settle(outcome));
    settled.set(outcome, pending);
    return pending;
  };
  const load = async (
    parent: string,
    page: number,
    expand = true,
  ): Promise<void> => handle(await owner.loadWindow(parent, page, expand));
  const reset = (): FileLocationAuthority => {
    const authority = owner.reset();
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
    if (!closed && deps.isAlive()) await handle(await owner.retry(failure));
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
    if (expanded) {
      if (owner.collapse(location)) deps.onChanged();
      return;
    }
    await load(location, 0, true);
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
      })),
      rootExpanded: owner.isExpanded(""),
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
  const refreshCounts = async (parent = ""): Promise<void> => {
    const retained = owner.window(parent);
    if (!retained) return;
    const children = [...retained.children];
    await load(parent, retained.page, false);
    for (const child of children)
      if (owner.isExpanded(child.location)) await refreshCounts(child.location);
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
      for (const p of presentations.values()) release(p);
      releasePublicationNotice();
      presentations.clear();
    },
  };
}
