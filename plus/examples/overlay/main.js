const { app, BrowserWindow, session, screen } = require("electron");
const fs = require("fs");
const { execFileSync } = require("child_process");

const arg = (name, env) => {
  const i = process.argv.indexOf("--" + name);
  return i > 0 ? process.argv[i + 1] : process.env[env];
};
const EVENTS = arg("events", "OVERLAY_EVENTS");
const COMMANDS = arg("commands", "OVERLAY_COMMANDS");
const log = (...a) => console.log("[overlay]", ...a);
const path = require("path");
const { ipcMain, globalShortcut } = require("electron");
const SHM = process.env.AURELIA_OVERLAY_SHM;
const INGAME = process.env.OVERLAY_INGAME === "1" && SHM && fs.existsSync(SHM);
const SHM_MAGIC = 0x4c564f41, SHM_HDR = 64, SHM_BYTES = 16 << 20;
let shmFd = null, shmSeq = 0;
function shmWrite(width, height, bitmap, visible) {
  if (shmFd === null) shmFd = fs.openSync(SHM, "r+");
  if (bitmap && width * height * 4 <= SHM_BYTES - SHM_HDR) fs.writeSync(shmFd, bitmap, 0, width * height * 4, SHM_HDR);
  const hdr = Buffer.alloc(24);
  hdr.writeUInt32LE(SHM_MAGIC, 0); hdr.writeUInt32LE(1, 4); hdr.writeUInt32LE(++shmSeq, 8); hdr.writeUInt32LE(visible ? 1 : 0, 12);
  hdr.writeUInt32LE(width, 16); hdr.writeUInt32LE(height, 20);
  fs.writeSync(shmFd, hdr, 0, 24, 0);
}
function shmFrame() {
  const b = Buffer.alloc(8);
  fs.readSync(shmFd ?? (shmFd = fs.openSync(SHM, "r+")), b, 0, 8, 24);
  return { w: b.readUInt32LE(0), h: b.readUInt32LE(4) };
}

const ALLOWED = [/^https:\/\/([a-z0-9-]+\.)*steampowered\.com(\/|$)/, /^https:\/\/([a-z0-9-]+\.)*steamcommunity\.com(\/|$)/, /^https:\/\/([a-z0-9-]+\.)*steamstatic\.com(\/|$)/, /^https:\/\/([a-z0-9-]+\.)*akamaihd\.net(\/|$)/];
const allowed = (url) => process.env.OVERLAY_SELFTEST === "1" ? /^http:\/\/127\.0\.0\.1[:/]/.test(url) || ALLOWED.some((r) => r.test(url)) : ALLOWED.some((r) => r.test(url));

const send = (cmd) => {
  const line = JSON.stringify(cmd) + "\n";
  try {
    const fd = fs.openSync(COMMANDS, fs.constants.O_WRONLY | fs.constants.O_APPEND | fs.constants.O_NONBLOCK);
    try { fs.writeSync(fd, line); } finally { fs.closeSync(fd); }
  } catch (e) { log("failed to send", line.trim(), e.code || e.message); }
};

const waiters = [];
const handlers = [];
const on = (fn) => handlers.push(fn);
function dispatch(ev) {
  for (const w of [...waiters]) if (w.event === ev.event) { waiters.splice(waiters.indexOf(w), 1); w.resolve(ev); }
  for (const h of handlers) h(ev);
}
const waitFor = (event, ms) => new Promise((resolve, reject) => {
  const w = { event, resolve };
  waiters.push(w);
  setTimeout(() => { const i = waiters.indexOf(w); if (i >= 0) { waiters.splice(i, 1); reject(new Error("timed out waiting for " + event)); } }, ms);
});

function tail() {
  let offset = 0, pending = "";
  setInterval(() => {
    let st;
    try { st = fs.statSync(EVENTS); } catch { return; }
    if (st.size < offset) { offset = 0; pending = ""; }
    if (st.size === offset) return;
    const fd = fs.openSync(EVENTS, "r");
    const buf = Buffer.alloc(st.size - offset);
    fs.readSync(fd, buf, 0, buf.length, offset);
    fs.closeSync(fd);
    offset = st.size;
    const lines = (pending + buf.toString("utf8")).split("\n");
    pending = lines.pop();
    for (const l of lines) { try { dispatch(JSON.parse(l)); } catch {  } }
  }, 150);
}

const xprop = (...a) => execFileSync("xprop", a, { encoding: "utf8", timeout: 3000 });
function gameWindow(appId) {
  try {
    const ids = xprop("-root", "_NET_CLIENT_LIST").replace(/.*# /, "").split(",").map((x) => x.trim()).filter(Boolean);
    return ids.find((id) => xprop("-id", id, "WM_CLASS").includes("steam_app_" + appId)) || null;
  } catch { return null; }
}
function makeChildOfGame(win, appId) {
  const game = gameWindow(appId);
  if (!game) { log("game window not found; the overlay stays detached"); return false; }
  try {
    const own = "0x" + win.getNativeWindowHandle().readUInt32LE(0).toString(16);
    xprop("-id", own, "-f", "WM_TRANSIENT_FOR", "32x", "-set", "WM_TRANSIENT_FOR", game);
    log("overlay is now a child of the game window", game);
    return true;
  } catch (e) { log("could not attach to the game:", e.message); return false; }
}

const open = new Map();

async function sessionFor(id) {
  const ses = session.fromPartition("aurelia-overlay-" + id + "-" + Date.now());
  ses.setUserAgent(ses.getUserAgent().replace(/ Electron\/\S+/, "") + " Valve Steam GameOverlay/1");
  send({ cmd: "web_session" });
  const ws = await waitFor("web_session", 15000);
  for (const c of ws.cookies) {
    const host = c.domain.replace(/^\./, "");
    await ses.cookies.set({ url: "https://" + host + "/", name: c.name, value: c.value, domain: c.domain, path: c.path, secure: c.secure, httpOnly: c.http_only, sameSite: "no_restriction" });
  }
  return ses;
}

function makeInputWindow(page, w, h) {
  const d = screen.getPrimaryDisplay();
  const win = new BrowserWindow({
    x: d.bounds.x, y: d.bounds.y, width: d.bounds.width, height: d.bounds.height, transparent: true, frame: false, hasShadow: false, focusable: false, skipTaskbar: true,
    alwaysOnTop: true, resizable: false, webPreferences: { preload: path.join(__dirname, "input-preload.js"), sandbox: false, contextIsolation: true, nodeIntegration: false },
  });
  win.setAlwaysOnTop(true, "screen-saver");
  win.loadURL("data:text/html,<body style='margin:0;background:rgba(0,0,0,0.004);cursor:default;overflow:hidden'></body>");
  const map = (e) => {
    const f = shmFrame(), fw = f.w || d.bounds.width, fh = f.h || d.bounds.height;
    return { x: Math.round(e.x * fw / d.bounds.width - (fw - w) / 2), y: Math.round(e.y * fh / d.bounds.height - (fh - h) / 2) };
  };
  const onInput = (_ev, e) => {
    if (page.isDestroyed()) return;
    const p = map(e);
    if (p.x < 0 || p.y < 0 || p.x >= w || p.y >= h) return;
    const wc = page.webContents, button = e.b === 2 ? "right" : e.b === 1 ? "middle" : "left";
    if (e.t === "move") wc.sendInputEvent({ type: "mouseMove", x: p.x, y: p.y });
    else if (e.t === "down") { wc.focus(); wc.sendInputEvent({ type: "mouseDown", x: p.x, y: p.y, button, clickCount: 1 }); }
    else if (e.t === "up") wc.sendInputEvent({ type: "mouseUp", x: p.x, y: p.y, button, clickCount: 1 });
    else if (e.t === "wheel") wc.sendInputEvent({ type: "mouseWheel", x: p.x, y: p.y, deltaX: -e.dx, deltaY: -e.dy });
  };
  ipcMain.on("ov-input", onInput);
  win.on("closed", () => ipcMain.removeListener("ov-input", onInput));
  return win;
}

async function showPage(id, url, size, decides, appId) {
  if (!allowed(url)) { log("page refused (outside Steam):", url); return; }
  const display = screen.getPrimaryDisplay();
  const [w, h] = [Math.min(size?.width || 1020, display.workAreaSize.width), Math.min(size?.height || 440, display.workAreaSize.height)];
  const state = { done: false, win: null };
  open.set(id, state);
  let ses;
  try { ses = await sessionFor(id); } catch (e) { log("no web session:", e.message); open.delete(id); if (decides) send({ cmd: "steam_url", id, url: "steam://close/cancel" }); return; }
  const win = new BrowserWindow(INGAME ? {
    width: w, height: h, show: false, frame: false, webPreferences: { session: ses, sandbox: true, contextIsolation: true, nodeIntegration: false, offscreen: true },
  } : {
    width: w, height: h, frame: false, show: false, alwaysOnTop: true, skipTaskbar: true, resizable: false, minimizable: false, fullscreenable: false,
    backgroundColor: "#16202d", webPreferences: { session: ses, sandbox: true, contextIsolation: true, nodeIntegration: false, webSecurity: true },
  });
  state.win = win;
  if (!INGAME) { win.setAlwaysOnTop(true, "screen-saver"); win.center(); if (appId) makeChildOfGame(win, appId); }

  const finish = (decision) => {
    if (state.done) return;
    state.done = true;
    if (decides) send({ cmd: "steam_url", id, url: decision || "steam://close/cancel" });
    send({ cmd: "hidden", id });
    open.delete(id);
    if (INGAME) { shmWrite(w, h, null, false); state.input?.destroy(); globalShortcut.unregister("Escape"); }
    if (!win.isDestroyed()) win.destroy();
    dispatch({ event: "overlay:window-closed", id, decision });
  };
  const intercept = (event, target) => {
    if (target.startsWith("steam://")) { event?.preventDefault(); log("steam://", target.slice(0, 60)); if (decides && /^steam:\/\/close\//.test(target)) finish(target); return true; }
    if (!allowed(target)) { event?.preventDefault(); log("navigation refused:", target.slice(0, 80)); return true; }
    return false;
  };
  const dbg = process.env.OVERLAY_DEBUG_DIR;
  win.webContents.on("did-finish-load", async () => {
    win.webContents.insertCSS("#global_header, .responsive_header, #store_header { display: none !important; }").catch(() => {});
    try {
      const text = await win.webContents.executeJavaScript("document.body ? document.body.innerText.slice(0, 400) : ''");
      log("page loaded:", win.webContents.getURL().slice(0, 120), "| title:", win.webContents.getTitle(), "| text:", JSON.stringify(text));
      if (dbg) { await new Promise((r) => setTimeout(r, 1500)); fs.writeFileSync(dbg + "/overlay-page-" + id + ".png", (await win.webContents.capturePage()).toPNG()); }
    } catch (e) { log("page diagnostics failed:", e.message); }
  });
  win.webContents.on("did-redirect-navigation", (_e, u) => log("redirected to", u.slice(0, 120)));
  win.webContents.on("will-navigate", (e, u) => intercept(e, u));
  win.webContents.on("will-redirect", (e, u) => intercept(e, u));
  win.webContents.setWindowOpenHandler(({ url: u }) => { if (!intercept(null, u) && allowed(u)) win.loadURL(u); return { action: "deny" }; });
  win.webContents.on("did-fail-load", (_e, _code, _desc, u) => { if (u.startsWith("steam://")) intercept(null, u); });
  win.webContents.on("before-input-event", (e, input) => { if (input.type === "keyDown" && input.key === "Escape") { e.preventDefault(); finish(null); } });
  win.on("closed", () => finish(null));
  if (INGAME) {
    win.webContents.setFrameRate(30);
    win.webContents.on("paint", (_e, _dirty, image) => { if (!state.done) { const sz = image.getSize(); shmWrite(sz.width, sz.height, image.getBitmap(), true); } });
    state.input = makeInputWindow(win, w, h);
    globalShortcut.register("Escape", () => finish(null));
    send({ cmd: "shown", id });
    log("overlay drawn inside the game (Vulkan layer) for", id);
  } else {
    win.once("ready-to-show", () => { win.show(); win.focus(); send({ cmd: "shown", id }); log("window opened for", id); });
  }
  if (INGAME) {
    const limit = setTimeout(() => { log("overlay time limit reached: cancelling"); finish(null); }, 180000);
    const guard = setInterval(() => {
      if (state.done) { clearInterval(guard); clearTimeout(limit); return; }
      if (appId && !gameWindow(appId)) { log("the game closed: shutting down the overlay"); clearInterval(guard); clearTimeout(limit); finish(null); }
    }, 2000);
  }
  win.loadURL(url).catch((e) => log("failed to load:", e.message));
  on((ev) => { if (ev.event === "overlay_closed" && ev.id === id && !state.done) { state.done = true; open.delete(id); if (INGAME) { shmWrite(w, h, null, false); state.input?.destroy(); globalShortcut.unregister("Escape"); } if (!win.isDestroyed()) win.destroy(); } });
}

function start() {
  tail();
  on((ev) => {
    if (ev.event === "microtxn_request") showPage(ev.id, ev.url, ev.window, true, ev.app_id);
    else if (ev.event === "overlay_request" && ev.kind === "web_page" && ev.url) showPage(ev.id, ev.url, null, false, ev.app_id);
    else if (ev.event === "overlay_request" && ev.kind === "store" && ev.target) showPage(ev.id, "https://store.steampowered.com/app/" + ev.target.app_id, { width: 1100, height: 700 }, false, ev.app_id);
  });
  send({ cmd: "capable", on: true });
  log("ready; events:", EVENTS, "commands:", COMMANDS);
}

app.whenReady().then(() => {
  if (process.env.OVERLAY_SELFTEST === "1") return require("./selftest.js")({ start, open, log, EVENTS, COMMANDS });
  start();
});
app.on("before-quit", () => { try { if (INGAME && shmFd !== null) shmWrite(0, 0, null, false); } catch {} });
app.on("window-all-closed", () => {  });
