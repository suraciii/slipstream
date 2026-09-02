import "./styles/global.css";
import { mountLibraryBrowser } from "../pages/library-browser/index.js";

const root = document.querySelector<HTMLElement>("#app");
if (root) {
  const dispose = mountLibraryBrowser(root);
  window.addEventListener("pagehide", dispose);
}
