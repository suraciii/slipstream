import type { FolderChild } from "../api/contracts.js";
import {
  fetchFileLocationWindow,
  type FileLocationFetch,
} from "../api/file-locations.js";
import { TaskScope } from "./async-ownership.js";

const FOLDER_PAGE_SIZE = 60;
const MAXIMUM_RETAINED_FOLDERS = 32;

declare const fileLocationAuthorityBrand: unique symbol;
export type FileLocationAuthority = Readonly<{
  [fileLocationAuthorityBrand]: true;
}>;

export type FileLocationWindow = Readonly<{
  page: number;
  children: ReadonlyArray<FolderChild>;
  total: number;
}>;

export type FileLocationRequest = Readonly<{ page: number; expand: boolean }>;

export type FileLocationFailure = Readonly<{
  generation: number;
  parent: string;
  page: number;
  range: string;
  message: string;
}>;

export type FileLocationOutcome =
  | Readonly<{
      kind: "bound";
      authority: FileLocationAuthority;
      generation: number;
      publication: string;
    }>
  | Readonly<{
      kind: "loaded";
      authority: FileLocationAuthority;
      generation: number;
      parent: string;
      page: number;
      recovered?: FileLocationFailure;
      remainingNewest?: FileLocationFailure;
      markTransportReachable: boolean;
    }>
  | Readonly<{
      kind: "failed";
      authority: FileLocationAuthority;
      generation: number;
      parent: string;
      page: number;
      failure: FileLocationFailure;
      replaced?: FileLocationFailure;
    }>
  | Readonly<{
      kind: "publication-conflict";
      authority: FileLocationAuthority;
      generation: number;
      parent: string;
      page: number;
    }>
  | Readonly<{
      kind: "detached";
      authority: FileLocationAuthority;
      generation: number;
      parent: string;
      page: number;
    }>;

export interface FileLocationOwner {
  readonly publication: string | undefined;
  readonly pageSize: number;
  accept(outcome: FileLocationOutcome): boolean;
  isCurrent(authority: FileLocationAuthority): boolean;
  window(parent: string): FileLocationWindow | undefined;
  isExpanded(parent: string): boolean;
  expandRetained(parent: string): boolean;
  prepareCountRefresh(): ReadonlyMap<string, FileLocationRequest>;
  failures(): ReadonlyArray<FileLocationFailure>;
  awaitRootBinding(): Promise<FileLocationOutcome>;
  loadWindow(
    parent: string,
    page: number,
    expand?: boolean,
  ): Promise<FileLocationOutcome>;
  collapse(parent: string): boolean;
  retry(failure: FileLocationFailure): Promise<FileLocationOutcome>;
  reset(): FileLocationAuthority;
  dispose(): void;
}

type FailureRecord = FileLocationFailure & Readonly<{ order: number }>;

type PendingWindow = {
  page: number;
  expand: boolean;
  promise: Promise<FileLocationOutcome>;
};

export function createFileLocationOwner(
  fetcher: FileLocationFetch,
): FileLocationOwner {
  let generation = 0;
  let authority = Object.freeze({}) as FileLocationAuthority;
  let closed = false;
  let tasks = new TaskScope();
  const acceptedOutcomes = new Map<string, FileLocationOutcome>();
  let publication: string | undefined;
  let failureOrder = 0;
  const windows = new Map<string, FileLocationWindow>();
  const expanded = new Set<string>();
  const failures = new Map<string, FailureRecord>();
  const pendingWindows = new Map<string, PendingWindow>();

  const describeRange = (parent: string, page: number) =>
    `${parent || "Library Folder"} items ${(page * FOLDER_PAGE_SIZE + 1).toLocaleString()}–${((page + 1) * FOLDER_PAGE_SIZE).toLocaleString()}`;

  const failureKey = (ownerGeneration: number, parent: string, page: number) =>
    `${ownerGeneration}:${parent}:${page}`;

  const detached = (
    ownerAuthority: FileLocationAuthority,
    ownerGeneration: number,
    parent: string,
    page: number,
  ): FileLocationOutcome => ({
    kind: "detached",
    authority: ownerAuthority,
    generation: ownerGeneration,
    parent,
    page,
  });

  const enforceWindowCap = (newest: string) => {
    while (windows.size > MAXIMUM_RETAINED_FOLDERS) {
      const oldest = [...windows.keys()].find(
        (location) => location !== newest && location !== "",
      );
      if (oldest === undefined) break;
      expanded.delete(oldest);
      windows.delete(oldest);
      if (acceptedOutcomes.get(oldest)?.kind === "loaded")
        acceptedOutcomes.delete(oldest);
    }
  };

  function requestWindow(
    parent: string,
    page: number,
    expand: boolean,
  ): Promise<FileLocationOutcome> {
    const scope = tasks;
    const ownerGeneration = generation;
    const ownerAuthority = authority;
    if (scope.halted || closed)
      return Promise.resolve(
        detached(ownerAuthority, ownerGeneration, parent, page),
      );
    acceptedOutcomes.delete(parent);
    const task = scope.beginLatest(
      parent === "" && !publication ? "root-binding" : `folder:${parent}`,
      {
        abortTransport: false,
      },
    );
    const boundPublication = publication;
    let resolve!: (outcome: FileLocationOutcome) => void;
    const promise = new Promise<FileLocationOutcome>((accept) => {
      resolve = accept;
    });
    const request: PendingWindow = { page, expand, promise };
    pendingWindows.set(parent, request);
    task.onCleanup(() => {
      if (pendingWindows.get(parent) === request) pendingWindows.delete(parent);
      resolve(detached(ownerAuthority, ownerGeneration, parent, page));
    });
    const finish = (outcome: FileLocationOutcome): void => {
      if (scope === tasks && task.isCurrent() && outcome.kind !== "detached")
        acceptedOutcomes.set(parent, outcome);
      resolve(outcome);
      task.finish();
    };
    const read = async (): Promise<void> => {
      try {
        const result = await fetchFileLocationWindow(fetcher, {
          parent,
          start: page * FOLDER_PAGE_SIZE,
          limit: FOLDER_PAGE_SIZE,
          ...(boundPublication ? { publication: boundPublication } : {}),
        });
        if (!task.isCurrent() || scope !== tasks)
          return finish(
            detached(ownerAuthority, ownerGeneration, parent, page),
          );
        if (result.kind === "publication-conflict")
          return finish({
            kind: "publication-conflict",
            authority: ownerAuthority,
            generation: ownerGeneration,
            parent,
            page,
          });
        if (result.kind === "failed") throw new Error("file locations failed");
        if (boundPublication && result.window.publication !== boundPublication)
          return finish(
            detached(ownerAuthority, ownerGeneration, parent, page),
          );

        publication = result.window.publication;
        windows.set(
          parent,
          Object.freeze({
            page,
            children: Object.freeze([...result.window.children]),
            total: result.window.total,
          }),
        );
        if (request.expand) expanded.add(parent);
        enforceWindowCap(parent);
        const key = failureKey(ownerGeneration, parent, page);
        const recovered = failures.get(key);
        if (recovered) failures.delete(key);
        const remainingNewest = [...failures.values()].sort(
          (left, right) => right.order - left.order,
        )[0];
        return finish({
          kind: "loaded",
          authority: ownerAuthority,
          generation: ownerGeneration,
          parent,
          page,
          ...(recovered ? { recovered } : {}),
          ...(remainingNewest ? { remainingNewest } : {}),
          markTransportReachable: Boolean(recovered) || failures.size > 0,
        });
      } catch {
        if (!task.isCurrent() || scope !== tasks)
          return finish(
            detached(ownerAuthority, ownerGeneration, parent, page),
          );
        const key = failureKey(ownerGeneration, parent, page);
        const replaced = failures.get(key);
        const failure = Object.freeze({
          generation: ownerGeneration,
          parent,
          page,
          range: describeRange(parent, page),
          message: `Could not load folders (${describeRange(parent, page)}). Retry to continue.`,
          order: (failureOrder += 1),
        });
        failures.set(key, failure);
        return finish({
          kind: "failed",
          authority: ownerAuthority,
          generation: ownerGeneration,
          parent,
          page,
          failure,
          ...(replaced ? { replaced } : {}),
        });
      }
    };
    void read();
    return promise;
  }

  async function awaitRootBinding(): Promise<FileLocationOutcome> {
    if (closed || tasks.halted) return detached(authority, generation, "", 0);
    if (publication)
      return { kind: "bound", authority, generation, publication };
    return loadWindow("", 0, false);
  }

  async function loadWindow(
    parent: string,
    page: number,
    expand = true,
  ): Promise<FileLocationOutcome> {
    if (closed || tasks.halted)
      return detached(authority, generation, parent, page);
    if (parent === "" && !publication) {
      const current = pendingWindows.get("");
      if (current) {
        current.expand ||= expand;
        return current.promise;
      }
      return requestWindow("", 0, expand);
    }
    return requestWindow(parent, page, expand);
  }

  const retry = async (
    failure: FileLocationFailure,
  ): Promise<FileLocationOutcome> => {
    const current = failures.get(
      failureKey(generation, failure.parent, failure.page),
    );
    if (current !== failure)
      return detached(authority, generation, failure.parent, failure.page);
    return loadWindow(failure.parent, failure.page);
  };

  return {
    get publication() {
      return publication;
    },
    pageSize: FOLDER_PAGE_SIZE,
    accept: (outcome) => {
      return (
        !closed &&
        outcome.kind !== "detached" &&
        outcome.authority === authority &&
        (outcome.kind === "bound" ||
          acceptedOutcomes.get(outcome.parent) === outcome)
      );
    },
    isCurrent: (candidate) => !closed && candidate === authority,
    window: (parent) => windows.get(parent),
    isExpanded: (parent) => expanded.has(parent),
    expandRetained: (parent) => {
      if (
        closed ||
        !publication ||
        [...failures.values()].some((failure) => failure.parent === parent)
      )
        return false;
      const retained = windows.get(parent);
      if (!retained) return false;
      windows.delete(parent);
      windows.set(parent, retained);
      expanded.add(parent);
      return true;
    },
    failures: () =>
      [...failures.values()].sort((left, right) => left.order - right.order),
    awaitRootBinding,
    loadWindow,
    collapse: (parent) => {
      const pending = pendingWindows.get(parent);
      if (pending) pending.expand = false;
      return expanded.delete(parent);
    },
    prepareCountRefresh: () => {
      const plan = new Map<string, FileLocationRequest>();
      if (closed) return plan;
      const visible = new Set<string>([""]);
      const parents = [""];
      for (let index = 0; index < parents.length; index += 1) {
        const parent = parents[index]!;
        const retained = windows.get(parent);
        const pending = pendingWindows.get(parent);
        if (!retained && !pending) continue;
        plan.set(parent, {
          page: pending?.page ?? retained?.page ?? 0,
          expand: pending?.expand ?? false,
        });
        if (!expanded.has(parent) && !pending?.expand) continue;
        for (const child of retained?.children ?? []) {
          if (
            visible.has(child.location) ||
            (!expanded.has(child.location) &&
              !pendingWindows.get(child.location)?.expand)
          )
            continue;
          visible.add(child.location);
          parents.push(child.location);
        }
      }
      for (const parent of windows.keys()) {
        if (visible.has(parent)) continue;
        windows.delete(parent);
        expanded.delete(parent);
      }
      tasks.halt();
      tasks = new TaskScope();
      acceptedOutcomes.clear();
      return plan;
    },
    retry,
    reset: () => {
      if (closed) return authority;
      generation += 1;
      authority = Object.freeze({}) as FileLocationAuthority;
      tasks.halt();
      tasks = new TaskScope();
      acceptedOutcomes.clear();
      publication = undefined;
      windows.clear();
      expanded.clear();
      failures.clear();
      return authority;
    },
    dispose: () => {
      if (closed) return;
      closed = true;
      tasks.halt();
      authority = Object.freeze({}) as FileLocationAuthority;
    },
  };
}
