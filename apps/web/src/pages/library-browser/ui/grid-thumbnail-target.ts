import type { GridThumbnailTarget } from "./library-browser-view.js";

export function gridThumbnailTarget(
  image: HTMLImageElement,
  setDeliveryFailed: (failed: boolean) => void,
  setThumbnailState: (state: "unavailable" | "failed") => void = () => {},
): GridThumbnailTarget {
  image.hidden = true;
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
      image.hidden = true;
      setDeliveryFailed(false);
      image.src = value;
    },
    get onload() {
      return image.onload;
    },
    set onload(value) {
      if (!value) {
        image.onload = null;
        return;
      }
      const loaded: GlobalEventHandlers["onload"] = (event) => {
        if (image.onload !== loaded) return;
        image.hidden = false;
        setDeliveryFailed(false);
        value.call(image, event);
      };
      image.onload = loaded;
    },
    get onerror() {
      return image.onerror;
    },
    set onerror(value) {
      if (!value) {
        image.onerror = null;
        return;
      }
      const failed: GlobalEventHandlers["onerror"] = (event) => {
        if (image.onerror !== failed) return;
        image.hidden = true;
        value.call(image, event);
      };
      image.onerror = failed;
    },
    removeAttribute(name) {
      image.removeAttribute(name);
      if (name === "src") image.hidden = true;
    },
    setDeliveryFailed(failed) {
      if (failed) image.hidden = true;
      setDeliveryFailed(failed);
    },
    setThumbnailState(state) {
      image.hidden = true;
      setThumbnailState(state);
    },
  };
}
