import React from "react";
import ReactDOM from "react-dom/client";

import "@/styles.css";
import { CommandCenter } from "./CommandCenter";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <CommandCenter />
  </React.StrictMode>,
);
