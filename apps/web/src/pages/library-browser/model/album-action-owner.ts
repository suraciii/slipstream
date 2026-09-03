import {
  addAlbumMember,
  createAlbum,
  deleteAlbum,
  removeAlbumMember,
  renameAlbum,
  type AlbumActionFetch,
  type AlbumCreateResult,
  type AlbumWriteResult,
} from "../api/album-actions.js";
import type { AlbumSummary } from "../api/contracts.js";
import { SettlementFamily, type SettlementHandle } from "./async-ownership.js";
import type { SourceAuthority } from "./source-grid-owner.js";

export type { AlbumActionFetch } from "../api/album-actions.js";

declare const albumFormAuthorityBrand: unique symbol;
export type AlbumFormAuthority = Readonly<{
  [albumFormAuthorityBrand]: true;
}>;

declare const albumMutationBrand: unique symbol;
export type AlbumMutation = Readonly<{
  [albumMutationBrand]: true;
}>;

declare const albumSurfaceAuthorityBrand: unique symbol;
export type AlbumSurfaceAuthority = Readonly<{
  [albumSurfaceAuthorityBrand]: true;
}>;

export type AlbumActionContext = Readonly<{
  sourceAuthority: SourceAuthority;
  surface:
    | Readonly<{ kind: "summary" }>
    | Readonly<{ kind: "photo"; isCurrent(): boolean }>;
  form?: AlbumFormAuthority;
}>;

export type AlbumActionSettlement =
  | AlbumWriteResult
  | AlbumCreateResult
  | Readonly<{ kind: "transport-failed" }>;

type AlbumOutcomeOwner = Readonly<{
  mutation: AlbumMutation;
  surface: AlbumSurfaceAuthority;
  sourceAuthority: SourceAuthority;
  form?: AlbumFormAuthority;
}>;

export type AlbumActionOutcome = AlbumOutcomeOwner &
  (
    | Readonly<{
        kind: "persisted";
        settlement: Readonly<{ kind: "persisted" }>;
        createdAlbum?: AlbumSummary;
        connectivity: "unchanged";
        removedFromCurrentAlbum?: Readonly<{
          albumId: string;
          photoId: string;
          sourceAuthority: SourceAuthority;
        }>;
      }>
    | Readonly<{
        kind: "failed";
        settlement:
          | Readonly<{ kind: "rejected"; status: number }>
          | Readonly<{ kind: "malformed" }>
          | Readonly<{ kind: "transport-failed" }>;
        failureMessage: string;
        connectivity: "unchanged" | "lost-if-latest";
      }>
  );

export type AlbumActionAdmission = Readonly<{
  mutation: AlbumMutation;
  noticeKey: string;
  invalidatesSavedPositionFor?: string;
  settlement: Promise<AlbumActionOutcome>;
}>;

export interface AlbumActionOwner {
  openForm(formId: string): AlbumFormAuthority;
  closeForm(form: AlbumFormAuthority): void;
  isFormCurrent(form: AlbumFormAuthority): boolean;
  create(
    name: string,
    context: AlbumActionContext,
  ): AlbumActionAdmission | undefined;
  rename(
    albumId: string,
    name: string,
    context: AlbumActionContext,
  ): AlbumActionAdmission | undefined;
  delete(
    albumId: string,
    context: AlbumActionContext,
  ): AlbumActionAdmission | undefined;
  addMembership(
    albumId: string,
    photoId: string,
    context: AlbumActionContext,
  ): AlbumActionAdmission | undefined;
  removeMembership(
    albumId: string,
    photoId: string,
    context: AlbumActionContext,
  ): AlbumActionAdmission | undefined;
  isMembershipAdmitted(
    verb: "add" | "remove",
    albumId: string,
    photoId: string,
  ): boolean;
  isLatest(mutation: AlbumMutation): boolean;
  canPresent(surface: AlbumSurfaceAuthority): boolean;
  finish(mutation: AlbumMutation): void;
  dispose(): void;
}

type MutationRecord = Readonly<{
  handle: SettlementHandle;
  surface: AlbumSurfaceAuthority;
  sourceAuthority: SourceAuthority;
  form?: AlbumFormAuthority;
}>;

type FormRecord = Readonly<{
  id: string;
  authority: AlbumFormAuthority;
}>;

type Write = () => Promise<AlbumWriteResult | AlbumCreateResult>;

const membershipKey = (
  verb: "add" | "remove",
  albumId: string,
  photoId: string,
): string => `${verb}:${albumId}:${photoId}`;

export function createAlbumActionOwner(
  fetcher: AlbumActionFetch,
): AlbumActionOwner {
  const settlements = new SettlementFamily();
  const records = new Map<AlbumMutation, MutationRecord>();
  const surfaceRecords = new Map<AlbumSurfaceAuthority, MutationRecord>();
  let currentForm: FormRecord | undefined;
  let closed = false;

  const start = (
    noticeKey: string,
    context: AlbumActionContext,
    write: Write,
    failureMessage: (status?: number) => string,
    options: {
      admissionKey?: string;
      removedFromCurrentAlbum?: Readonly<{
        albumId: string;
        photoId: string;
      }>;
      invalidatesSavedPositionFor?: string;
    } = {},
  ): AlbumActionAdmission | undefined => {
    if (closed) return undefined;
    const handle = settlements.begin({
      ...(options.admissionKey ? { admissionKey: options.admissionKey } : {}),
      ownsSurface:
        context.surface.kind === "photo"
          ? context.surface.isCurrent
          : () => false,
    });
    if (!handle) return undefined;

    const mutation = Object.freeze({}) as AlbumMutation;
    const surface = Object.freeze({}) as AlbumSurfaceAuthority;
    const record: MutationRecord = Object.freeze({
      handle,
      surface,
      sourceAuthority: context.sourceAuthority,
      ...(context.form ? { form: context.form } : {}),
    });
    records.set(mutation, record);
    surfaceRecords.set(surface, record);

    const settlement = (async (): Promise<AlbumActionOutcome> => {
      let result: AlbumActionSettlement;
      try {
        result = await write();
      } catch {
        result = Object.freeze({ kind: "transport-failed" });
      }
      const losesTransport =
        result.kind === "transport-failed" ||
        (result.kind === "rejected" && result.status >= 500);
      const removed =
        result.kind === "persisted" && options.removedFromCurrentAlbum
          ? Object.freeze({
              ...options.removedFromCurrentAlbum,
              sourceAuthority: context.sourceAuthority,
            })
          : undefined;
      const createdAlbum =
        result.kind === "persisted" && "createdAlbum" in result
          ? result.createdAlbum
          : undefined;
      const owner = {
        mutation,
        surface,
        sourceAuthority: context.sourceAuthority,
        ...(context.form ? { form: context.form } : {}),
      };
      return result.kind === "persisted"
        ? Object.freeze({
            ...owner,
            kind: "persisted",
            settlement: result,
            connectivity: "unchanged",
            ...(createdAlbum ? { createdAlbum } : {}),
            ...(removed ? { removedFromCurrentAlbum: removed } : {}),
          })
        : Object.freeze({
            ...owner,
            kind: "failed",
            settlement: result,
            failureMessage: failureMessage(
              result.kind === "rejected" ? result.status : undefined,
            ),
            connectivity: losesTransport ? "lost-if-latest" : "unchanged",
          });
    })();

    return Object.freeze({
      mutation,
      noticeKey,
      ...(options.invalidatesSavedPositionFor
        ? {
            invalidatesSavedPositionFor: options.invalidatesSavedPositionFor,
          }
        : {}),
      settlement,
    });
  };

  return {
    openForm: (formId) => {
      if (!closed && currentForm?.id === formId) return currentForm.authority;
      const authority = Object.freeze({}) as AlbumFormAuthority;
      if (!closed) currentForm = Object.freeze({ id: formId, authority });
      return authority;
    },
    closeForm: (form) => {
      if (currentForm?.authority === form) currentForm = undefined;
    },
    isFormCurrent: (form) => !closed && currentForm?.authority === form,
    create: (name, context) =>
      start(
        "/api/albums",
        context,
        () => createAlbum(fetcher, name),
        (status) =>
          status === 409
            ? "An Album with this name already exists."
            : "The Album could not be created.",
      ),
    rename: (albumId, name, context) =>
      start(
        `/api/albums/${albumId}/rename`,
        context,
        () => renameAlbum(fetcher, albumId, name),
        (status) =>
          status === 409
            ? "An Album with this name already exists."
            : "The Album could not be renamed.",
      ),
    delete: (albumId, context) =>
      start(
        `/api/albums/${albumId}/delete`,
        context,
        () => deleteAlbum(fetcher, albumId),
        () => "The Album could not be deleted.",
        { invalidatesSavedPositionFor: albumId },
      ),
    addMembership: (albumId, photoId, context) =>
      start(
        membershipKey("add", albumId, photoId),
        context,
        () => addAlbumMember(fetcher, albumId, photoId),
        () => "The Photo could not be added to the Album.",
        { admissionKey: membershipKey("add", albumId, photoId) },
      ),
    removeMembership: (albumId, photoId, context) =>
      start(
        membershipKey("remove", albumId, photoId),
        context,
        () => removeAlbumMember(fetcher, albumId, photoId),
        () => "The Photo could not be removed from the Album.",
        {
          admissionKey: membershipKey("remove", albumId, photoId),
          removedFromCurrentAlbum: { albumId, photoId },
          invalidatesSavedPositionFor: albumId,
        },
      ),
    isMembershipAdmitted: (verb, albumId, photoId) =>
      settlements.isAdmitted(membershipKey(verb, albumId, photoId)),
    isLatest: (mutation) =>
      !closed && Boolean(records.get(mutation)?.handle.isNewest()),
    canPresent: (surface) =>
      !closed && Boolean(surfaceRecords.get(surface)?.handle.canPresent()),
    finish: (mutation) => {
      const record = records.get(mutation);
      if (!record) return;
      records.delete(mutation);
      surfaceRecords.delete(record.surface);
      record.handle.finish();
    },
    dispose: () => {
      if (closed) return;
      closed = true;
      currentForm = undefined;
      settlements.closePresentation();
    },
  };
}
