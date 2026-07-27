import React from "react";
import ReactDOM from "react-dom/client";

import "@/styles.css";
import { Recorder } from "./Recorder";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Recorder />
  </React.StrictMode>,
);
