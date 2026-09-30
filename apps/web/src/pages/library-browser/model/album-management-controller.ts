import type { AlbumSummary } from "../api/contracts.js";
import type {
  AlbumActionContext,
  AlbumActionOwner,
  AlbumActionAdmission,
  AlbumFormAuthority,
} from "./album-action-owner.js";
import type { SourceAuthority } from "./source-grid-owner.js";
import type { PhotoAuthority } from "./photo-owner.js";

export type AlbumFormReference = Readonly<{
  formId: string;
  kind: "create" | "rename" | "delete";
  albumId?: string;
  name: string;
}>;

export type FolderContext = Readonly<{
  authority: SourceAuthority;
  path: string;
  publication: string;
}>;

type AlbumFormRecord = Readonly<{
  formId: string;
  kind: AlbumFormReference["kind"];
  albumId?: string;
  initialName: string;
  authority: AlbumFormAuthority;
}>;

type FolderAlbumModel = Readonly<{
  visible: boolean;
  folderPath: string;
  albums: ReadonlyArray<Readonly<{ id: string; name: string }>>;
  selectedAlbumId: string;
  pending: boolean;
  status?: string;
}>;

export type AlbumMutationResult = Readonly<{
  ok: boolean;
  latest: boolean;
  createdAlbum?: AlbumSummary;
  folderAdd?: Readonly<{
    matchedCount: number;
    addedCount: number;
    alreadyMemberCount: number;
  }>;
}>;

export interface AlbumManagementController {
  openForm(form: AlbumFormReference): void;
  closeForm(formId: string): boolean;
  submitForm(formId: string, draft?: string): Promise<void>;
  addFolderToAlbum(albumId: string): Promise<void>;
  setAlbums(albums: ReadonlyArray<AlbumSummary>): void;
  dispose(): void;
}

export type AlbumManagementOptions = Readonly<{
  actions: AlbumActionOwner;
  albums: () => ReadonlyArray<AlbumSummary>;
  source: Readonly<{
    readonly authority: SourceAuthority;
    current(): FolderContext | undefined;
    isCurrent(authority: SourceAuthority): boolean;
  }>;
  photo: Readonly<{
    readonly authority: PhotoAuthority;
    isCurrent(authority: PhotoAuthority): boolean;
  }>;
  mutate: (
    start: (context: AlbumActionContext) => AlbumActionAdmission | undefined,
    surface: "summary",
    photoAuthority: PhotoAuthority,
    form?: AlbumFormAuthority,
  ) => Promise<AlbumMutationResult>;
  present: Readonly<{
    setFormPending(formId: string, pending: boolean, name?: string): void;
    setFormMessage(formId: string, message: string): void;
    dismissForm(formId: string): void;
    renderFolder(model: FolderAlbumModel): void;
  }>;
  onCreatedAlbum?: (album: AlbumSummary) => Promise<void>;
  onDeletedAlbum: (albumId: string) => Promise<void>;
  onFolderChanged?: () => void;
}>;

const ALBUM_NAME_MAXIMUM = 120;

const albumNameError = (name: string): string | undefined => {
  const trimmed = name.trim();
  if (!trimmed) return "Enter an Album name.";
  if (Array.from(trimmed).length > ALBUM_NAME_MAXIMUM)
    return `Album names are at most ${ALBUM_NAME_MAXIMUM} characters.`;
  return undefined;
};

export function createAlbumManagementController(
  options: AlbumManagementOptions,
): AlbumManagementController {
  let closed = false;
  let form: AlbumFormRecord | undefined;
  let albumList = [...options.albums()];
  let selectedAlbumId = "";
  let folderOperation:
    | Readonly<{
        authority: SourceAuthority;
        albumId: string;
        path: string;
        publication: string;
        pending: boolean;
        status?: string;
      }>
    | undefined;

  const currentFolder = () => options.source.current();
  const renderFolder = () => {
    const context = currentFolder();
    const operation =
      context &&
      folderOperation?.authority === context.authority &&
      folderOperation.path === context.path &&
      folderOperation.publication === context.publication
        ? folderOperation
        : undefined;
    const visible = Boolean(context && albumList.length > 0);
    if (visible && !albumList.some((album) => album.id === selectedAlbumId))
      selectedAlbumId = albumList[0]?.id ?? "";
    options.present.renderFolder(
      Object.freeze({
        visible,
        folderPath: context?.path ?? "",
        albums: albumList.map(({ id, name }) => ({ id, name })),
        selectedAlbumId,
        pending: operation?.pending ?? false,
        ...(operation?.status ? { status: operation.status } : {}),
      }),
    );
  };
  const dismiss = (record: AlbumFormRecord): boolean => {
    if (!options.actions.isFormCurrent(record.authority)) return false;
    options.actions.closeForm(record.authority);
    if (form === record) form = undefined;
    options.present.dismissForm(record.formId);
    return true;
  };

  return {
    openForm(reference) {
      if (closed) return;
      form = Object.freeze({
        formId: reference.formId,
        kind: reference.kind,
        authority: options.actions.openForm(reference.formId),
        ...(reference.albumId ? { albumId: reference.albumId } : {}),
        initialName: reference.name,
      });
    },
    closeForm(formId) {
      if (closed || !form || form.formId !== formId) return false;
      return dismiss(form);
    },
    async submitForm(formId, draft) {
      const record = form;
      if (
        closed ||
        !record ||
        record.formId !== formId ||
        !options.actions.isFormCurrent(record.authority)
      )
        return;
      if (record.kind === "delete") {
        options.present.setFormPending(formId, true);
        const photoAuthority = options.photo.authority;
        const result = await options.mutate(
          (context) => options.actions.delete(record.albumId!, context),
          "summary",
          photoAuthority,
          record.authority,
        );
        if (closed) return;
        if (options.actions.isFormCurrent(record.authority)) dismiss(record);
        if (result.ok) await options.onDeletedAlbum(record.albumId!);
        if (!closed) options.onFolderChanged?.();
        return;
      }
      const name = (draft ?? "").trim();
      if (record.kind === "rename" && (!name || name === record.initialName)) {
        dismiss(record);
        renderFolder();
        return;
      }
      const invalid = albumNameError(name);
      if (invalid) {
        options.present.setFormMessage(formId, invalid);
        return;
      }
      options.present.setFormPending(formId, true, name);
      const sourceAuthority = options.source.authority;
      const photoAuthority = options.photo.authority;
      const result = await options.mutate(
        (context) =>
          record.kind === "create"
            ? options.actions.create(name, context)
            : options.actions.rename(record.albumId!, name, context),
        "summary",
        photoAuthority,
        record.authority,
      );
      if (closed) return;
      const current = options.actions.isFormCurrent(record.authority);
      if (current) {
        if (result.ok && (record.kind === "rename" || result.createdAlbum))
          dismiss(record);
        else options.present.setFormPending(formId, false);
      }
      renderFolder();
      if (
        current &&
        result.ok &&
        result.createdAlbum &&
        options.source.isCurrent(sourceAuthority) &&
        options.photo.isCurrent(photoAuthority)
      )
        await options.onCreatedAlbum?.(result.createdAlbum);
    },
    async addFolderToAlbum(albumId) {
      if (closed) return;
      const context = currentFolder();
      if (
        !context ||
        !albumList.some((album) => album.id === albumId) ||
        options.actions.isFolderMembersAdmitted(
          albumId,
          context.path,
          context.publication,
        )
      )
        return;
      selectedAlbumId = albumId;
      folderOperation = Object.freeze({
        authority: context.authority,
        albumId,
        path: context.path,
        publication: context.publication,
        pending: true,
      });
      renderFolder();
      const result = await options.mutate(
        (actionContext) =>
          options.actions.addFolderMembers(
            albumId,
            context.path,
            context.publication,
            actionContext,
          ),
        "summary",
        options.photo.authority,
      );
      const current = currentFolder();
      if (
        closed ||
        !current ||
        !options.source.isCurrent(context.authority) ||
        current.path !== context.path ||
        current.publication !== context.publication
      )
        return;
      const status = result.ok
        ? result.folderAdd
          ? `Added ${result.folderAdd.addedCount.toLocaleString()} Photos. ${result.folderAdd.alreadyMemberCount.toLocaleString()} already in the Album.`
          : "Folder added to the Album."
        : "The Folder could not be added to the Album. Try again.";
      folderOperation = Object.freeze({
        authority: context.authority,
        albumId,
        path: context.path,
        publication: context.publication,
        pending: false,
        status,
      });
      renderFolder();
      options.onFolderChanged?.();
    },
    setAlbums(albums) {
      albumList = [...albums];
      renderFolder();
    },
    dispose() {
      if (closed) return;
      closed = true;
      form = undefined;
      folderOperation = undefined;
    },
  };
}
