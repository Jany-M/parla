import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./index.css";
import App from "./App";
import { getApi } from "./lib/api";
import { Overlay } from "./views/Overlay";

const isOverlay = window.location.hash.startsWith("#/overlay");

const report = (m: string) => void getApi().then((a) => a.logUiError(m)).catch(() => {});
window.addEventListener("error", (e) => report(`${e.message} at ${e.filename}:${e.lineno}`));
window.addEventListener("unhandledrejection", (e) => report(`unhandled rejection: ${String(e.reason)}`));

getApi().then(() => {
  createRoot(document.getElementById("root")!).render(<StrictMode>{isOverlay ? <Overlay /> : <App />}</StrictMode>);
});
