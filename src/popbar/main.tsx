import React from "react";
import ReactDOM from "react-dom/client";

import "@/styles.css";
import { PopbarApp } from "./PopbarApp";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <PopbarApp />
  </React.StrictMode>,
);
