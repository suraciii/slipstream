const rawExtensions = Object.freeze([
  "3fr",
  "ari",
  "arw",
  "bay",
  "bmq",
  "cap",
  "cine",
  "cr2",
  "cr3",
  "cs1",
  "dc2",
  "dcr",
  "dng",
  "drf",
  "eip",
  "erf",
  "fff",
  "gpr",
  "iiq",
  "k25",
  "kc2",
  "kdc",
  "mdc",
  "mef",
  "mos",
  "mrw",
  "nef",
  "nrw",
  "obm",
  "orf",
  "pef",
  "ptx",
  "pxn",
  "qtk",
  "r3d",
  "raf",
  "raw",
  "rdc",
  "rw2",
  "rwl",
  "rwz",
  "sr2",
  "srf",
  "srw",
  "x3f",
]);
const jpegExtensions = Object.freeze(["jpg", "jpeg"]);
const rawSet = new Set(rawExtensions);
const jpegSet = new Set(jpegExtensions);
function classifyOriginalFile(name) {
  const dot = name.lastIndexOf(".");
  if (dot <= 0 || dot === name.length - 1) return undefined;
  const extension = name.slice(dot + 1).toLowerCase();
  if (rawSet.has(extension)) return "raw";
  if (jpegSet.has(extension)) return "jpeg";
  return undefined;
}
function pairingBaseName(name) {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}
module.exports = {
  rawExtensions,
  jpegExtensions,
  classifyOriginalFile,
  pairingBaseName,
};
