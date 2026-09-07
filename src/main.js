import "./style.css";
import { Elm } from "./Main.elm";
import { start } from "./bridge.js";

start(Elm, document.getElementById("app"));
