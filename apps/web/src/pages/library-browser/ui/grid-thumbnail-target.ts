import type { GridThumbnailTarget } from "./library-browser-view.js";

export function gridThumbnailTarget(
  image: HTMLImageElement,
  setDeliveryFailed: (failed: boolean) => void,
  setPreviewState: (state: "unavailable" | "failed") => void = () => {},
): GridThumbnailTarget {
  return {
    get complete() {
      return image.complete;
    },
    get isConnected() {
      return image.isConnected;
    },
    get src() {
      return image.src;
    },
    set src(value) {
      image.src = value;
    },
    get onload() {
      return image.onload;
    },
    set onload(value) {
      image.onload = value;
    },
    get onerror() {
      return image.onerror;
    },
    set onerror(value) {
      image.onerror = value;
    },
    removeAttribute(name) {
      image.removeAttribute(name);
    },
    setDeliveryFailed,
    setPreviewState,
  };
}
