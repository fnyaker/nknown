// Charge l'interface (WebAssembly) ; fichier séparé : la CSP du serveur interdit les scripts en ligne.
import init from "./nknown.js";
init({ module_or_path: "./nknown_bg.wasm" });
