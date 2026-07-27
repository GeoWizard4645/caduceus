import React from "react";
import ReactDOM from "react-dom/client";

import "@/styles.css";
import { Manage } from "./Manage";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Manage />
  </React.StrictMode>,
);
