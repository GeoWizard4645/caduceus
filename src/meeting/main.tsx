import React from "react";
import ReactDOM from "react-dom/client";

import "@/styles.css";
import { MeetingPopout } from "./MeetingPopout";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <MeetingPopout />
  </React.StrictMode>,
);
