import React from "react";
import ReactDOM from "react-dom/client";

import "@/styles.css";
import { Orb } from "./Orb";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Orb />
  </React.StrictMode>,
);
