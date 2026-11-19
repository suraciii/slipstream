import type { JsonObject, WorkspaceFile } from "../api/workspace.js";
import { fileBytesUrl } from "../api/workspace.js";
import { isRecord } from "../api/guards.js";
import type { WorkspaceViewModel } from "../model/workspace-controller-contract.js";
import type { WorkspaceIntent } from "../model/workspace-intent-contract.js";
import "./workspace.css";
import { displayScalar, required } from "./presentation-helpers.js";

export interface WorkspaceSurfaceController {
  open(photoId: string): void;
  visible(): boolean;
  render(model: WorkspaceViewModel): void;
  dispose(): void;
}

const filename = (path: string): string => path.split("/").at(-1) ?? path;

const byteSize = (value: number | null): string => {
  if (value === null) return "Unknown size";
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`;
  if (value < 1024 * 1024 * 1024)
    return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
  return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
};

const imageFormat = (file: WorkspaceFile): string | undefined => {
  const contract = file.imageContract;
  if (typeof contract?.format === "string")
    return contract.format.toLowerCase().replace(/^image\//, "");
  const extension = filename(file.relativePath).split(".").at(-1);
  return extension?.toLowerCase();
};

const displayable = (file: WorkspaceFile): boolean =>
  file.state === "ready" &&
  ["png", "jpeg", "jpg", "webp", "gif", "avif"].includes(
    imageFormat(file) ?? "",
  );

const imageFacts = (file: WorkspaceFile): string | undefined => {
  const facts =
    file.imageFacts && typeof file.imageFacts === "object"
      ? file.imageFacts
      : file.imageContract;
  const width = displayScalar(facts?.width);
  const height = displayScalar(facts?.height);
  return width && height ? `${width} × ${height}` : undefined;
};

const roleFor = (file: WorkspaceFile): string =>
  file.kind === "input" ? "Input" : file.kind === "output" ? "Output" : "Other";

const factRow = (label: string, value: string): HTMLDivElement => {
  const row = document.createElement("div");
  const term = document.createElement("dt");
  term.textContent = label;
  const definition = document.createElement("dd");
  definition.textContent = value;
  row.append(term, definition);
  return row;
};

const executionHost = (workspace: WorkspaceViewModel["workspace"]): string => {
  const execution = workspace?.execution;
  return execution && typeof execution.hostId === "string"
    ? execution.hostId
    : "";
};

const scopeText = (file: WorkspaceFile): string =>
  `${roleFor(file)} · ${filename(file.relativePath)}`;

const inputDisclosure = (model: WorkspaceViewModel): string | undefined => {
  const workspace = model.workspace;
  if (!workspace) return undefined;
  if (!workspace.inputArtifactId)
    return "No default input selected. Use the CLI or Agent to prepare input again.";
  const input = model.files.find(
    (file) => file.artifactId === workspace.inputArtifactId,
  );
  const name = input ? filename(input.relativePath) : "the workspace input";
  if (
    workspace.inputAvailability !== undefined &&
    workspace.inputAvailability !== "ready"
  )
    return `Workspace input ${name} is ${workspace.inputAvailability}. Use the CLI or Agent to recover or prepare it again.`;
  if (workspace.inputMatchesObservedSource === false)
    return `Workspace input ${name} differs from the current Library Original. Use the CLI or Agent to refresh the workspace input explicitly.`;
  return undefined;
};

const blockerLabel = (value: unknown): string | undefined => {
  if (!isRecord(value)) return undefined;
  return [
    typeof value.name === "string" ? value.name : undefined,
    typeof value.action === "string" ? value.action : undefined,
    typeof value.reason === "string" ? value.reason : undefined,
  ]
    .filter((part): part is string => Boolean(part))
    .join(" · ");
};

export function createWorkspaceSurfaceController({
  root,
  send,
  isPhotoVisible,
  isWorkspaceSurfaceVisible,
  openWorkspaceSurface,
}: {
  root: ParentNode;
  send: (intent: WorkspaceIntent) => void;
  isPhotoVisible: () => boolean;
  isWorkspaceSurfaceVisible: () => boolean;
  openWorkspaceSurface: () => void;
}): WorkspaceSurfaceController {
  let alive = true;
  let photoId: string | undefined;
  let current: WorkspaceViewModel | undefined;
  let lastInspected: string | undefined;
  let lastError = "";
  let lastDownload: WorkspaceViewModel["download"] | undefined;
  let failedImageUrl: string | undefined;
  const listeners = new AbortController();
  const element = <T extends HTMLElement>(selector: string): T =>
    required<T>(root, selector);
  const status = element<HTMLElement>("[data-workspace-status]");
  const notice = element<HTMLElement>("[data-workspace-notice]");
  const error = element<HTMLElement>("[data-workspace-error]");
  const environment = element<HTMLElement>("[data-workspace-environment]");
  const input = element<HTMLElement>("[data-workspace-input]");
  const inspectedHeading = element<HTMLElement>(
    "[data-workspace-inspected-heading]",
  );
  const image = element<HTMLImageElement>("[data-workspace-image]");
  const imageEmpty = element<HTMLElement>("[data-workspace-image-empty]");
  const inspectedFacts = element<HTMLElement>(
    "[data-workspace-inspected-facts]",
  );
  const facts = element<HTMLElement>("[data-workspace-facts]");
  const filePath = element<HTMLElement>("[data-workspace-file-path]");
  const inspectedDownload = element<HTMLAnchorElement>(
    "[data-workspace-inspected-download]",
  );
  const removeButton = element<HTMLButtonElement>("[data-workspace-remove]");
  const summary = element<HTMLElement>("[data-workspace-summary]");
  const groups = element<HTMLElement>("[data-workspace-file-groups]");
  const empty = element<HTMLElement>("[data-workspace-empty]");
  const more = element<HTMLButtonElement>("[data-workspace-files-more]");
  const refresh = element<HTMLButtonElement>("[data-workspace-refresh]");

  const act = (action: string, args: JsonObject = {}) => {
    if (alive && photoId)
      send({
        kind: "workspace-action",
        photoId,
        action,
        arguments: args,
      });
  };

  const startDownload = (
    download: NonNullable<WorkspaceViewModel["download"]>,
  ): void => {
    const link = document.createElement("a");
    link.href = download.url;
    link.setAttribute("data-workspace-download-clone", "");
    document.body.append(link);
    link.click();
    window.setTimeout(() => link.remove(), 0);
  };

  image.addEventListener(
    "error",
    () => {
      if (image.hidden) return;
      failedImageUrl = image.getAttribute("src") ?? undefined;
      image.hidden = true;
      image.removeAttribute("src");
      imageEmpty.hidden = false;
      imageEmpty.textContent =
        "This file cannot be displayed. Download it to inspect its contents.";
    },
    { signal: listeners.signal },
  );

  root.addEventListener(
    "click",
    (event) => {
      const target = event.target;
      if (!(target instanceof Element)) return;
      if (target.closest("[data-workspace-download-clone]")) return;
      const download = target.closest<HTMLAnchorElement>(
        "[data-workspace-download], [data-workspace-inspected-download]",
      );
      if (download) {
        if (
          event instanceof MouseEvent &&
          (event.button !== 0 ||
            event.metaKey ||
            event.ctrlKey ||
            event.shiftKey ||
            event.altKey)
        )
          return;
        const artifactId = download.dataset.workspaceDownload;
        if (!artifactId) return;
        event.preventDefault();
        act("download", { artifactId });
        return;
      }
      const inspect = target.closest<HTMLButtonElement>(
        "[data-workspace-inspect]",
      );
      if (inspect) {
        const artifactId = inspect.dataset.workspaceInspect;
        if (artifactId) act("inspect", { artifactId });
        return;
      }
      if (target.closest("[data-workspace-refresh]")) {
        act("refresh");
        return;
      }
      if (target.closest("[data-workspace-files-more]")) {
        act("files-more");
        return;
      }
      if (target.closest("[data-workspace-remove]")) {
        const file = current?.inspectedFile;
        if (file)
          act("remove", {
            artifactId: file.artifactId,
            revision: file.revision,
          });
      }
    },
    { signal: listeners.signal },
  );

  const fileCard = (file: WorkspaceFile): HTMLElement => {
    const card = document.createElement("article");
    card.dataset.artifactId = file.artifactId;
    card.dataset.workspaceFileRole = roleFor(file);
    const header = document.createElement("header");
    const name = document.createElement("h5");
    name.textContent = filename(file.relativePath);
    const role = document.createElement("span");
    role.className = "workspace-file-role";
    role.textContent = roleFor(file);
    header.append(name, role);
    const detail = document.createElement("p");
    const dimensions = imageFacts(file);
    const format = imageFormat(file);
    detail.textContent = [
      byteSize(file.byteLength),
      file.state,
      format,
      dimensions,
    ]
      .filter((part): part is string => Boolean(part))
      .join(" · ");
    card.append(header, detail);
    if (displayable(file)) {
      const thumbnail = document.createElement("img");
      thumbnail.className = "workspace-file-thumbnail";
      thumbnail.alt = `Thumbnail of ${filename(file.relativePath)}`;
      thumbnail.loading = "lazy";
      try {
        thumbnail.src = fileBytesUrl(
          current?.workspace?.workspaceId ?? "",
          file,
        );
      } catch {
        // The facts remain; the file is not displayable until it is ready.
      }
      if (thumbnail.src) card.append(thumbnail);
    } else {
      const generic = document.createElement("p");
      generic.className = "workspace-file-generic";
      generic.textContent =
        file.state === "ready"
          ? "No display conversion is generated. Download to inspect this file."
          : `This file is ${file.state}. Download becomes available when it is ready.`;
      card.append(generic);
    }
    const actions = document.createElement("div");
    actions.className = "workspace-file-actions";
    const inspect = document.createElement("button");
    inspect.type = "button";
    inspect.className = "quiet";
    inspect.textContent = "Inspect";
    inspect.dataset.workspaceInspect = file.artifactId;
    inspect.disabled = Boolean(current?.removingArtifactId);
    inspect.setAttribute("aria-label", `Inspect ${scopeText(file)}`);
    if (current?.inspectedFile?.artifactId === file.artifactId)
      inspect.setAttribute("aria-current", "true");
    actions.append(inspect);
    if (file.state === "ready") {
      const download = document.createElement("a");
      download.textContent = "Download";
      download.dataset.workspaceDownload = file.artifactId;
      download.setAttribute("download", filename(file.relativePath));
      download.setAttribute("aria-label", `Download ${scopeText(file)}`);
      try {
        download.href = fileBytesUrl(
          current?.workspace?.workspaceId ?? "",
          file,
        );
      } catch {
        // A ready checked revision is required for a Download link.
      }
      if (download.href) actions.append(download);
    }
    card.append(actions);
    return card;
  };

  const renderFileList = (model: WorkspaceViewModel): void => {
    const sections: [string, WorkspaceFile[]][] = [
      ["Input", model.files.filter((file) => file.kind === "input")],
      ["Outputs", model.files.filter((file) => file.kind === "output")],
      [
        "Other files",
        model.files.filter(
          (file) => file.kind !== "input" && file.kind !== "output",
        ),
      ],
    ];
    groups.replaceChildren();
    for (const [label, files] of sections) {
      if (!files.length) continue;
      const section = document.createElement("section");
      section.className = "workspace-file-group";
      const heading = document.createElement("h4");
      heading.textContent = `${label} (${files.length})`;
      const list = document.createElement("div");
      list.className = "workspace-file-list";
      list.append(...files.map(fileCard));
      section.append(heading, list);
      groups.append(section);
    }
    empty.hidden = model.loading || model.files.length > 0;
    const bytes = model.files.reduce(
      (total, file) => total + (file.byteLength ?? 0),
      0,
    );
    summary.textContent = model.files.length
      ? `${model.files.length} of ${model.fileTotal} files · ${byteSize(bytes)}`
      : "No files loaded.";
    more.hidden = model.fileCursor === null;
    more.disabled = model.loading;
    refresh.disabled = model.loading;
  };

  const renderInspection = (model: WorkspaceViewModel): void => {
    const file = model.inspectedFile;
    inspectedFacts.hidden = !file;
    inspectedHeading.hidden = !file;
    if (!file) {
      image.hidden = true;
      image.removeAttribute("src");
      imageEmpty.hidden = false;
      imageEmpty.textContent = "Inspect a workspace file to see it here.";
      facts.replaceChildren();
      filePath.replaceChildren();
      inspectedDownload.hidden = true;
      inspectedDownload.removeAttribute("href");
      inspectedDownload.removeAttribute("data-workspace-download");
      removeButton.hidden = true;
      return;
    }
    inspectedHeading.textContent = `Inspected file: ${filename(file.relativePath)}`;
    facts.replaceChildren(
      factRow("Filename", filename(file.relativePath)),
      factRow("Role", roleFor(file)),
      factRow("Size", byteSize(file.byteLength)),
      factRow("Readiness", file.state),
      factRow("Revision", file.revision ?? "Unknown revision"),
    );
    const format = imageFormat(file);
    if (format) facts.append(factRow("Format", format));
    const dimensions = imageFacts(file);
    if (dimensions) facts.append(factRow("Image facts", dimensions));
    const path = typeof file.path === "string" ? file.path : "";
    filePath.replaceChildren();
    filePath.hidden = !path;
    if (path) filePath.append(factRow("Execution-environment path", path));
    if (displayable(file)) {
      let url = "";
      try {
        url = fileBytesUrl(model.workspace?.workspaceId ?? "", file);
      } catch {
        // A ready checked revision is required to display the file.
      }
      if (url && url !== failedImageUrl) {
        if (image.getAttribute("src") !== url) image.src = url;
        image.alt = `Inspected workspace file ${filename(file.relativePath)}`;
        image.hidden = false;
        imageEmpty.hidden = true;
      } else {
        image.hidden = true;
        image.removeAttribute("src");
        imageEmpty.hidden = false;
        imageEmpty.textContent =
          "This file cannot be displayed. Download it to inspect its contents.";
      }
    } else {
      image.hidden = true;
      image.removeAttribute("src");
      imageEmpty.hidden = false;
      imageEmpty.textContent =
        file.state === "ready"
          ? "This file cannot be displayed. Download it to inspect its contents."
          : `This file is ${file.state}. Download becomes available when it is ready.`;
    }
    if (file.state === "ready") {
      let href = "";
      try {
        href = fileBytesUrl(model.workspace?.workspaceId ?? "", file);
      } catch {
        // A ready checked revision is required for a Download link.
      }
      inspectedDownload.hidden = !href;
      inspectedDownload.textContent = "Download";
      inspectedDownload.dataset.workspaceDownload = file.artifactId;
      inspectedDownload.setAttribute("download", filename(file.relativePath));
      if (href) inspectedDownload.href = href;
      else inspectedDownload.removeAttribute("href");
    } else {
      inspectedDownload.hidden = true;
      inspectedDownload.removeAttribute("href");
      inspectedDownload.removeAttribute("data-workspace-download");
    }
    removeButton.hidden = file.state !== "ready";
    removeButton.disabled = Boolean(model.removingArtifactId);
    removeButton.textContent = model.removingArtifactId
      ? "Removing…"
      : "Remove this reviewed file";
    removeButton.setAttribute("aria-label", `Remove ${scopeText(file)}`);
  };

  const focusAfterRender = (model: WorkspaceViewModel): void => {
    const errorText = model.error
      ? `${model.error.message} (${model.error.code})`
      : "";
    if (errorText && errorText !== lastError) error.focus();
    else if (
      model.inspectedFile &&
      model.inspectedFile.artifactId !== lastInspected
    )
      inspectedHeading.focus();
    else if (!model.inspectedFile && lastInspected && model.notice)
      status.focus();
    lastInspected = model.inspectedFile?.artifactId;
    lastError = errorText;
  };

  return {
    open(id) {
      if (!alive || !id || !isPhotoVisible()) return;
      photoId = id;
      current = undefined;
      lastInspected = undefined;
      lastError = "";
      lastDownload = undefined;
      failedImageUrl = undefined;
      groups.replaceChildren();
      inspectReset();
      openWorkspaceSurface();
      send({ kind: "workspace-open", photoId: id });
    },
    visible: () =>
      alive &&
      Boolean(photoId) &&
      isPhotoVisible() &&
      isWorkspaceSurfaceVisible(),
    render(model) {
      if (!alive || photoId !== model.photoId) return;
      current = model;
      status.textContent = model.status;
      notice.hidden = !model.notice;
      notice.textContent = model.notice ?? "";
      const errorText = model.error
        ? `${model.error.message} (${model.error.code})`
        : "";
      error.hidden = !errorText;
      error.replaceChildren();
      if (errorText) {
        error.append(document.createTextNode(errorText));
        const blockers = model.error?.details.blockers;
        if (Array.isArray(blockers)) {
          const labels = blockers
            .map(blockerLabel)
            .filter((label): label is string => Boolean(label));
          if (labels.length) {
            const list = document.createElement("ul");
            for (const label of labels) {
              const item = document.createElement("li");
              item.textContent = label;
              list.append(item);
            }
            error.append(list);
          }
        }
      }
      const host = executionHost(model.workspace);
      const path =
        typeof model.workspace?.path === "string" ? model.workspace.path : "";
      environment.hidden = !host && !path;
      environment.textContent = [
        host ? `Execution host: ${host}` : "",
        path ? `Execution-environment path: ${path}` : "",
      ]
        .filter(Boolean)
        .join(" · ");
      const inputText = inputDisclosure(model);
      input.hidden = !inputText;
      input.textContent = inputText ?? "";
      renderInspection(model);
      renderFileList(model);
      if (model.download && model.download !== lastDownload) {
        lastDownload = model.download;
        startDownload(model.download);
      } else if (!model.download) lastDownload = undefined;
      focusAfterRender(model);
    },
    dispose() {
      alive = false;
      listeners.abort();
      groups.replaceChildren();
      image.removeAttribute("src");
      inspectedDownload.removeAttribute("href");
      inspectedDownload.removeAttribute("data-workspace-download");
    },
  };

  function inspectReset(): void {
    inspectedFacts.hidden = true;
    inspectedHeading.hidden = true;
    image.hidden = true;
    image.removeAttribute("src");
    imageEmpty.hidden = false;
    imageEmpty.textContent = "Inspect a workspace file to see it here.";
    facts.replaceChildren();
    filePath.replaceChildren();
    inspectedDownload.hidden = true;
    inspectedDownload.removeAttribute("href");
    inspectedDownload.removeAttribute("data-workspace-download");
    removeButton.hidden = true;
    summary.textContent = "Loading files…";
    empty.hidden = true;
    more.hidden = true;
    error.hidden = true;
    error.textContent = "";
    notice.hidden = true;
    notice.textContent = "";
    environment.hidden = true;
    environment.textContent = "";
    input.hidden = true;
    input.textContent = "";
  }
}
