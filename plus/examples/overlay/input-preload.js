const { ipcRenderer } = require("electron");
const send = (t, e) => ipcRenderer.send("ov-input", { t, x: e.clientX, y: e.clientY, b: e.button, dx: e.deltaX || 0, dy: e.deltaY || 0 });
window.addEventListener("mousemove", (e) => send("move", e));
window.addEventListener("mousedown", (e) => send("down", e));
window.addEventListener("mouseup", (e) => send("up", e));
window.addEventListener("wheel", (e) => send("wheel", e), { passive: true });
window.addEventListener("contextmenu", (e) => e.preventDefault());
