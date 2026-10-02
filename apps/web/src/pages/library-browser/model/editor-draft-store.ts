export const browserDraftStore = () => {
  try {
    window.sessionStorage.setItem("slipstream.draft.probe", "1");
    window.sessionStorage.removeItem("slipstream.draft.probe");
  } catch {
    return undefined;
  }
  return {
    read: (key: string) => {
      try {
        return window.sessionStorage.getItem(key);
      } catch {
        return null;
      }
    },
    write: (key: string, value: string) => {
      try {
        window.sessionStorage.setItem(key, value);
        return true;
      } catch {
        return false;
      }
    },
    remove: (key: string) => {
      try {
        window.sessionStorage.removeItem(key);
      } catch {
        /* the store is blocked; the in-memory draft still governs this session */
      }
    },
  };
};
