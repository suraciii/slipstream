import type {
  PhotoListResponse,
  PhotoSummary,
  PreviewResponse,
  PreviewSource,
} from "../../server/src/protocol.js";
import "./style.css";

export function renderApp(
  root: HTMLElement,
  fetcher: typeof fetch = fetch,
): () => void {
  root.innerHTML = `
    <main class="review" aria-live="polite">
      <header><h1>Slipstream</h1><p data-position>Loading Photo Library…</p></header>
      <section class="preview" data-preview><p>Loading…</p></section>
      <dl class="facts"><div><dt>Preview Source</dt><dd data-source>—</dd></div><div data-limited hidden><dt>Detail</dt><dd>Limited by camera Preview resolution</dd></div></dl>
      <p class="status" data-status role="status"></p>
      <nav aria-label="Photo navigation"><button type="button" data-previous>Previous</button><button type="button" data-next>Next</button></nav>
    </main>`;
  const position = root.querySelector<HTMLElement>("[data-position]")!;
  const previewBox = root.querySelector<HTMLElement>("[data-preview]")!;
  const source = root.querySelector<HTMLElement>("[data-source]")!;
  const limited = root.querySelector<HTMLElement>("[data-limited]")!;
  const status = root.querySelector<HTMLElement>("[data-status]")!;
  const previous = root.querySelector<HTMLButtonElement>("[data-previous]")!;
  const next = root.querySelector<HTMLButtonElement>("[data-next]")!;
  let photos: ReadonlyArray<PhotoSummary> = [];
  let index = 0;
  let request = 0;

  const show = async () => {
    const token = ++request;
    const current = photos[index];
    position.textContent = current
      ? `${index + 1} / ${photos.length}`
      : "0 / 0";
    previous.disabled = index <= 0;
    next.disabled = index >= photos.length - 1;
    source.textContent = "—";
    limited.hidden = true;
    status.textContent = "";
    previewBox.replaceChildren(
      paragraph(current ? "Loading review Preview…" : "No Photos found"),
    );
    if (!current) return;
    if (!current.available) {
      status.textContent =
        "Original File is unavailable. Rescan after restoring it.";
      previewBox.replaceChildren(paragraph("Preview unavailable"));
      return;
    }
    try {
      const response = await fetcher(`/api/photos/${current.id}/preview`);
      const result = (await response.json()) as PreviewResponse;
      if (token !== request) return;
      if (result.state !== "ready" || !result.url) {
        status.textContent = result.message ?? "Preview unavailable";
        previewBox.replaceChildren(paragraph("Preview unavailable"));
        return;
      }
      const image = document.createElement("img");
      image.alt = `Photo ${index + 1}`;
      image.src = result.url;
      image.addEventListener(
        "error",
        () => {
          if (token !== request) return;
          status.textContent =
            "Preview could not be loaded. Try reloading or rescanning.";
        },
        { once: true },
      );
      previewBox.replaceChildren(image);
      source.textContent = sourceLabel(result.source);
      limited.hidden = !result.limitedDetail;
      if (result.stale)
        status.textContent =
          result.message ?? "Showing a stale Preview; rescan or retry.";
    } catch {
      if (token !== request) return;
      status.textContent =
        "Slipstream is unavailable. Check the server and retry.";
      previewBox.replaceChildren(paragraph("Preview unavailable"));
    }
  };
  const move = (delta: number) => {
    const target = Math.max(0, Math.min(photos.length - 1, index + delta));
    if (target === index) return;
    index = target;
    void show();
  };
  const keydown = (event: KeyboardEvent) => {
    if (event.key === "ArrowLeft") move(-1);
    if (event.key === "ArrowRight") move(1);
  };
  previous.addEventListener("click", () => move(-1));
  next.addEventListener("click", () => move(1));
  window.addEventListener("keydown", keydown);
  void fetcher("/api/photos")
    .then(async (response) => {
      if (!response.ok) throw new Error("list failed");
      photos = ((await response.json()) as PhotoListResponse).photos;
      return show();
    })
    .catch(() => {
      position.textContent = "Unavailable";
      status.textContent =
        "Photo Library could not be loaded. Check the server configuration.";
    });
  return () => window.removeEventListener("keydown", keydown);
}

function paragraph(text: string): HTMLParagraphElement {
  const value = document.createElement("p");
  value.textContent = text;
  return value;
}

function sourceLabel(source?: PreviewSource): string {
  return source === "matching-jpeg"
    ? "JPEG"
    : source === "embedded-raw-jpeg"
      ? "RAW embedded JPEG"
      : "—";
}

if (typeof document !== "undefined") {
  const root = document.querySelector<HTMLElement>("#app");
  if (root) renderApp(root);
}
