import "./styles/global.css";
import { mountLibraryBrowser } from "../pages/library-browser/index.js";

const root = document.querySelector<HTMLElement>("#app");
if (root) {
  let dispose = mountLibraryBrowser(root);
  window.addEventListener("pagehide", dispose);
  window.addEventListener("pageshow", (event) => {
    // A back-forward cache restore resumes this document with its script
    // frozen where pagehide left it, so the browser that was mounted has been
    // disposed and the restored document would be inert. It mounts exactly one
    // new browser, which revalidates the address it was restored with, and
    // leaves no duplicate subscription behind.
    if (!event.persisted) return;
    window.removeEventListener("pagehide", dispose);
    dispose();
    root.replaceChildren();
    dispose = mountLibraryBrowser(root);
    window.addEventListener("pagehide", dispose);
  });
}
