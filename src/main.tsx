import { lazy, StrictMode, Suspense } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";

const App = lazy(() => import("./App"));
const WebApp = lazy(() => import("./WebApp"));
const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Suspense fallback={null}>{isTauri ? <App /> : <WebApp />}</Suspense>
  </StrictMode>,
);
