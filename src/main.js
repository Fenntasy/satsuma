import "./style.css";
import { Elm } from "./Main.elm";
import { start } from "./bridge.js";

start(Elm, document.getElementById("app")).catch((error) =>
  console.error("[satsuma] cannot start the app", error),
);
