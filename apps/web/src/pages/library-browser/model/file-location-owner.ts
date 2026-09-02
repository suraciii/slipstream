import type { FolderChild } from "../api/contracts.js";
import {
  fetchFileLocationWindow,
  type FileLocationFetch,
} from "../api/file-locations.js";
import { TaskScope } from "./async-ownership.js";

const FOLDER_PAGE_SIZE = 60;
const MAXIMUM_EXPANDED_FOLDERS = 32;

declare const fileLocationAuthorityBrand: unique symbol;
export type FileLocationAuthority = Readonly<{
  [fileLocationAuthorityBrand]: true;
}>;

export type FileLocationWindow = Readonly<{
  page: number;
  children: ReadonlyArray<FolderChild>;
  total: number;
}>;

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

export function createFileLocationOwner(
  fetcher: FileLocationFetch,
): FileLocationOwner {
  let generation = 0;
  let authority = Object.freeze({}) as FileLocationAuthority;
  let closed = false;
  let tasks = new TaskScope();
  let publication: string | undefined;
  let failureOrder = 0;
  const windows = new Map<string, FileLocationWindow>();
  const expanded = new Set<string>();
  const failures = new Map<string, FailureRecord>();

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

  const enforceExpandedCap = (newest: string) => {
    while (expanded.size > MAXIMUM_EXPANDED_FOLDERS) {
      const oldest = [...expanded].find(
        (location) => location !== newest && location !== "",
      );
      if (oldest === undefined) break;
      expanded.delete(oldest);
      windows.delete(oldest);
    }
  };

  async function requestWindow(
    parent: string,
    page: number,
    expand: boolean,
    scope = tasks,
  ): Promise<FileLocationOutcome> {
    const ownerGeneration = generation;
    const ownerAuthority = authority;
    if (scope.halted || closed)
      return detached(ownerAuthority, ownerGeneration, parent, page);
    const task = scope.beginLatest(`folder:${parent}`, {
      abortTransport: false,
    });
    const boundPublication = publication;
    try {
      const result = await fetchFileLocationWindow(fetcher, {
        parent,
        start: page * FOLDER_PAGE_SIZE,
        limit: FOLDER_PAGE_SIZE,
        ...(boundPublication ? { publication: boundPublication } : {}),
      });
      if (!task.isCurrent() || scope !== tasks)
        return detached(ownerAuthority, ownerGeneration, parent, page);
      if (result.kind === "publication-conflict")
        return {
          kind: "publication-conflict",
          authority: ownerAuthority,
          generation: ownerGeneration,
          parent,
          page,
        };
      if (result.kind === "failed") throw new Error("file locations failed");
      if (boundPublication && result.window.publication !== boundPublication)
        return detached(ownerAuthority, ownerGeneration, parent, page);

      publication = result.window.publication;
      windows.set(
        parent,
        Object.freeze({
          page,
          children: Object.freeze([...result.window.children]),
          total: result.window.total,
        }),
      );
      if (expand) {
        expanded.add(parent);
        enforceExpandedCap(parent);
      }
      const key = failureKey(ownerGeneration, parent, page);
      const recovered = failures.get(key);
      if (recovered) failures.delete(key);
      const remainingNewest = [...failures.values()].sort(
        (left, right) => right.order - left.order,
      )[0];
      return {
        kind: "loaded",
        authority: ownerAuthority,
        generation: ownerGeneration,
        parent,
        page,
        ...(recovered ? { recovered } : {}),
        ...(remainingNewest ? { remainingNewest } : {}),
        markTransportReachable: Boolean(recovered) || failures.size > 0,
      };
    } catch {
      if (!task.isCurrent() || scope !== tasks)
        return detached(ownerAuthority, ownerGeneration, parent, page);
      const key = failureKey(ownerGeneration, parent, page);
      const replaced = failures.get(key);
      const failure = Object.freeze({
        generation: ownerGeneration,
        parent,
        page,
        range: describeRange(parent, page),
        message: `Could not load File Locations (${describeRange(parent, page)}). Retry to continue.`,
        order: (failureOrder += 1),
      });
      failures.set(key, failure);
      return {
        kind: "failed",
        authority: ownerAuthority,
        generation: ownerGeneration,
        parent,
        page,
        failure,
        ...(replaced ? { replaced } : {}),
      };
    } finally {
      task.finish();
    }
  }

  async function awaitRootBinding(): Promise<FileLocationOutcome> {
    if (closed || tasks.halted) return detached(authority, generation, "", 0);
    if (publication)
      return { kind: "bound", authority, generation, publication };
    const scope = tasks;
    const ownerGeneration = generation;
    const ownerAuthority = authority;
    const shared = scope.joinOrStart(
      "root-binding",
      {
        abortTransport: false,
        onCancel: () => detached(ownerAuthority, ownerGeneration, "", 0),
      },
      async () => requestWindow("", 0, false, scope),
    );
    try {
      return await shared.promise;
    } catch {
      return detached(ownerAuthority, ownerGeneration, "", 0);
    }
  }

  async function loadWindow(
    parent: string,
    page: number,
    expand = true,
  ): Promise<FileLocationOutcome> {
    if (parent === "" && !publication) {
      const outcome = await awaitRootBinding();
      if (
        (outcome.kind === "bound" || outcome.kind === "loaded") &&
        publication &&
        expand
      ) {
        expanded.add("");
        enforceExpandedCap("");
      }
      return outcome;
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
        outcome.authority === authority
      );
    },
    isCurrent: (candidate) => !closed && candidate === authority,
    window: (parent) => windows.get(parent),
    isExpanded: (parent) => expanded.has(parent),
    failures: () =>
      [...failures.values()].sort((left, right) => left.order - right.order),
    awaitRootBinding,
    loadWindow,
    collapse: (parent) => {
      if (!expanded.delete(parent)) return false;
      if (parent) windows.delete(parent);
      return true;
    },
    retry,
    reset: () => {
      if (closed) return authority;
      generation += 1;
      authority = Object.freeze({}) as FileLocationAuthority;
      tasks.halt();
      tasks = new TaskScope();
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
