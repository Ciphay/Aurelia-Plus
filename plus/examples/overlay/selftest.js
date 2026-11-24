const fs = require("fs");
const http = require("http");
const { app } = require("electron");

const PAGE = `<!doctype html><title>Purchase</title><body style="font:16px sans-serif;background:#16202d;color:#fff">
<h3>500 Coins — $23.95</h3>
<button id="authorize" onclick="location.href='steam://close/authorize?txn=1'">Buy</button>
<button id="cancel" onclick="location.href='steam://close/cancel?txn=1'">Cancel</button>`;

module.exports = async ({ start, open, log, EVENTS, COMMANDS }) => {
  const server = http.createServer((_q, r) => { r.setHeader("content-type", "text/html"); r.end(PAGE); });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const url = `http://127.0.0.1:${server.address().port}/approvetxn/1/`;
  const until = async (fn, ms, what) => { const t = Date.now(); for (;;) { const v = fn(); if (v) return v; if (Date.now() - t > ms) throw new Error("timed out: " + what); await new Promise((r) => setTimeout(r, 100)); } };
  const commands = () => fs.readFileSync(COMMANDS, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l));
  const emit = (ev) => fs.appendFileSync(EVENTS, JSON.stringify(ev) + "\n");
  const cookies = [{ name: "steamLoginSecure", value: "1%7C%7Ctok", domain: ".steampowered.com", path: "/", secure: true, http_only: true }];
  let fails = 0;
  const check = (ok, msg) => { console.log((ok ? "PASS " : "FAIL ") + msg); if (!ok) fails++; };

  fs.writeFileSync(EVENTS, ""); fs.writeFileSync(COMMANDS, "");
  start();
  await until(() => commands().some((c) => c.cmd === "capable"), 5000, "capable");
  check(true, "the overlay declares capable on startup");

  const scenario = async (id, how, expectUrl) => {
    const before = commands().length;
    emit({ event: "microtxn_request", id, app_id: 1, url, window: { width: 600, height: 300 } });
    await until(() => commands().slice(before).some((c) => c.cmd === "web_session"), 5000, "web_session requested");
    emit({ event: "web_session", steam_id: "1", cookies });
    const st = await until(() => open.get(id)?.win && !open.get(id).win.isDestroyed() && open.get(id), 8000, "window opened");
    await until(() => commands().slice(before).some((c) => c.cmd === "shown" && c.id === id), 8000, "shown");
    if (st.win.webContents.isLoading()) await new Promise((r) => st.win.webContents.once("did-finish-load", r));
    if (how === "close") st.win.webContents.sendInputEvent({ type: "keyDown", keyCode: "Escape" });
    else await st.win.webContents.executeJavaScript(`document.getElementById('${how}').click()`);
    const got = await until(() => commands().slice(before).find((c) => c.cmd === "steam_url"), 8000, "steam_url");
    check(got.id === id && got.url.startsWith(expectUrl), `${how}: the redirect becomes steam_url ${expectUrl} (got ${got.url})`);
    await until(() => commands().slice(before).some((c) => c.cmd === "hidden" && c.id === id), 5000, "hidden");
    check(!open.has(id), `${how}: the window closes and the overlay reports hidden`);
  };
  try {
    await scenario("a", "cancel", "steam://close/cancel");
    await scenario("b", "authorize", "steam://close/authorize");
    await scenario("c", "close", "steam://close/cancel");
    const n = commands().length;
    emit({ event: "microtxn_request", id: "d", app_id: 1, url: "https://example.com/", window: {} });
    await new Promise((r) => setTimeout(r, 800));
    check(!commands().slice(n).some((c) => c.cmd === "web_session") && !open.has("d"), "a page that is not from Steam is refused");
  } catch (e) { console.log("FAIL " + e.message); fails++; }
  console.log(fails ? "FAIL overlay selftest" : "PASS overlay selftest");
  server.close();
  app.exit(fails ? 1 : 0);
};
