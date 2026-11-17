import { createRoot } from "react-dom/client";

import { App } from "./app.tsx";
import "./styles/base.css";

// An address from when the screen was named after a `#` leads to the same screen.
if (window.location.hash.startsWith("#/")) window.history.replaceState(null, "", window.location.hash.slice(1));

createRoot(document.getElementById("root")!).render(<App />);
