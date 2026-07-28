import React from "react";
import ReactDOM from "react-dom/client";

import "@/styles.css";
import { WidgetApp } from "./WidgetApp";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <WidgetApp />
  </React.StrictMode>,
);
