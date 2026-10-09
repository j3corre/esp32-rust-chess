// ESP32 Chess – main web app logic.
// Pages: #/welcome (nickname), #/ (lobby), #/game/<id> (game), #/settings.
// We talk to the board through a simple JSON API (/api/...) and short polling.
(function () {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const Chess = window.Chess;

  // ------------------------------------------------------------------
  // Settings storage (localStorage may be missing in a captive portal)
  // ------------------------------------------------------------------
  const memory = {};
  const store = {
    get(key, fallback) {
      try {
        const v = localStorage.getItem("chess." + key);
        return v === null ? (key in memory ? memory[key] : fallback) : JSON.parse(v);
      } catch (e) {
        return key in memory ? memory[key] : fallback;
      }
    },
    set(key, value) {
      memory[key] = value;
      try { localStorage.setItem("chess." + key, JSON.stringify(value)); } catch (e) { /* not important */ }
    }
  };

  // ------------------------------------------------------------------
  // Translations
  // ------------------------------------------------------------------
  const I18N = window.I18N;
  const LANGS = Object.keys(I18N);
  let lang = store.get("lang", null);
  if (!lang || !I18N[lang]) lang = "en"; // the default language is English

  function t(key, vars) {
    let text = (I18N[lang] && I18N[lang][key]) ?? I18N.en[key] ?? key;
    if (vars && typeof text === "string") {
      for (const [k, v] of Object.entries(vars)) text = text.split("{" + k + "}").join(v);
    }
    return text;
  }

  function applyI18n() {
    document.documentElement.lang = lang;
    document.querySelectorAll("[data-i18n]").forEach((el) => { el.textContent = t(el.dataset.i18n); });
    document.querySelectorAll("[data-i18n-html]").forEach((el) => { el.innerHTML = t(el.dataset.i18nHtml); });
    document.querySelectorAll("[data-i18n-title]").forEach((el) => { el.title = t(el.dataset.i18nTitle); });
    document.querySelectorAll("[data-i18n-aria]").forEach((el) => { el.setAttribute("aria-label", t(el.dataset.i18nAria)); });
    renderLangSwitches();
    renderLevelPicker();
    renderTimeControls();
    if (lastLobby) renderLobby(lastLobby);
    if (G.data) renderGame(false);
  }

  function setLang(code) {
    lang = code;
    store.set("lang", code);
    applyI18n();
  }

  function renderLangSwitches() {
    for (const id of ["lang-switch-welcome", "lang-switch-settings"]) {
      const box = $(id);
      box.innerHTML = "";
      for (const code of LANGS) {
        const b = document.createElement("button");
        b.type = "button";
        b.textContent = I18N[code]._name || code;
        b.dataset.lang = code;
        b.className = code === lang ? "selected" : "";
        b.addEventListener("click", () => setLang(code));
        box.appendChild(b);
      }
    }
  }

  // ------------------------------------------------------------------
  // Small things: toast, sound, vibration
  // ------------------------------------------------------------------
  let toastTimer = null;
  function toast(text) {
    const el = $("toast");
    el.textContent = text;
    el.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { el.hidden = true; }, 2600);
  }

  let audio = null;
  function beep(kind) {
    if (!store.get("sound", true)) return;
    try {
      audio = audio || new (window.AudioContext || window.webkitAudioContext)();
      const now = audio.currentTime;
      const tones = { move: [[440, 0.05]], capture: [[330, 0.05], [220, 0.07]], notify: [[660, 0.1], [880, 0.14]] }[kind];
      let at = now;
      for (const [freq, dur] of tones) {
        const osc = audio.createOscillator();
        const gain = audio.createGain();
        osc.type = "triangle";
        osc.frequency.value = freq;
        gain.gain.setValueAtTime(0.18, at);
        gain.gain.exponentialRampToValueAtTime(0.001, at + dur);
        osc.connect(gain).connect(audio.destination);
        osc.start(at);
        osc.stop(at + dur);
        at += dur;
      }
    } catch (e) { /* sound is not essential */ }
  }

  function vibrate(ms) {
    if (navigator.vibrate) try { navigator.vibrate(ms); } catch (e) { /* ignore */ }
  }

  // ------------------------------------------------------------------
  // Communication with the board
  // ------------------------------------------------------------------
  let token = store.get("token", null);
  let failures = 0;

  async function api(path, params, method) {
    const query = new URLSearchParams(params || {});
    if (token) query.set("t", token);
    const url = "/api/" + path + "?" + query.toString();
    // The board handles only a few connections at once – when all are busy it rejects new
    // ones immediately. Hence one quick retry (GET only; a POST could be executed twice).
    const attempts = (method || "GET") === "GET" ? 2 : 1;
    for (let attempt = 0; attempt < attempts; attempt++) {
      const ctrl = new AbortController();
      const timer = setTimeout(() => ctrl.abort(), 6000);
      try {
        const res = await fetch(url, { method: method || "GET", cache: "no-store", signal: ctrl.signal });
        const data = await res.json();
        failures = 0;
        $("offline-banner").hidden = true;
        if (data && data.err === "no_session" && path !== "hello") {
          token = null;
          store.set("token", null);
          go("#/welcome");
        }
        return data;
      } catch (e) {
        if (attempt + 1 < attempts) {
          await new Promise((r) => setTimeout(r, 300));
          continue;
        }
        failures++;
        // Show "offline" only after 3 failures in a row (a single glitch does not bother the user).
        if (failures >= 3) $("offline-banner").hidden = false;
        return { ok: false, err: "network" };
      } finally {
        clearTimeout(timer);
      }
    }
  }

  const post = (path, params) => api(path, params, "POST");

  function showError(data) {
    if (data && data.err) toast(t("err_" + data.err));
  }

  // ------------------------------------------------------------------
  // Navigation between pages
  // ------------------------------------------------------------------
  function go(hash) {
    if (location.hash !== hash) location.hash = hash;
    else route();
  }

  let pollTimer = null;
  function schedule(fn, ms) {
    clearTimeout(pollTimer);
    pollTimer = setTimeout(fn, ms);
  }

  function showView(name) {
    for (const v of ["welcome", "lobby", "game", "settings"]) $("view-" + v).hidden = v !== name;
  }

  function route() {
    const hash = location.hash || "#/";
    if (!token && hash !== "#/welcome") return go("#/welcome");
    const m = hash.match(/^#\/game\/(\d+)/);
    if (hash === "#/welcome") {
      showView("welcome");
      clearTimeout(pollTimer);
      $("nick-input").value = $("nick-input").value || store.get("nick", "") || randomNick();
    } else if (m) {
      showView("game");
      openGame(Number(m[1]));
    } else if (hash === "#/settings") {
      showView("settings");
      renderSettings();
      pollLobby(true);
    } else {
      showView("lobby");
      pollLobby(true);
    }
  }

  // ------------------------------------------------------------------
  // Welcome: nickname
  // ------------------------------------------------------------------
  const NICK_WORDS = {
    en: [["Swift", "Brave", "Clever", "Calm", "Bold", "Lucky", "Sly", "Wise"], ["Knight", "Rook", "Bishop", "Pawn", "King", "Queen"]],
    cs: [["Rychlý", "Statečný", "Chytrý", "Klidný", "Smělý", "Šťastný", "Mazaný", "Moudrý"], ["Jezdec", "Střelec", "Pěšec", "Král"]]
  };
  function randomNick() {
    const [adj, noun] = NICK_WORDS[lang] || NICK_WORDS.en;
    const pick = (a) => a[Math.floor(Math.random() * a.length)];
    return (pick(adj) + " " + pick(noun)).slice(0, 13) + " " + Math.floor(Math.random() * 90 + 10);
  }

  $("nick-dice").addEventListener("click", () => { $("nick-input").value = randomNick(); });

  $("nick-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const nick = $("nick-input").value.trim();
    const data = await post("hello", { nick });
    if (!data.ok) {
      $("nick-error").textContent = t("err_" + data.err);
      $("nick-error").hidden = false;
      return;
    }
    $("nick-error").hidden = true;
    token = data.token;
    store.set("token", token);
    store.set("nick", data.nick);
    setNickChip(data.nick);
    go("#/");
  });

  function setNickChip(nick) {
    const chip = $("nick-chip");
    chip.textContent = nick || "";
    chip.hidden = !nick;
  }
  $("nick-chip").addEventListener("click", () => go("#/settings"));

  // ------------------------------------------------------------------
  // Lobby
  // ------------------------------------------------------------------
  const TIME_CONTROLS = [[1, 0], [3, 2], [5, 0], [5, 3], [10, 0], [15, 10], [30, 0]];
  const LEVEL_TIMES = [0.3, 0.4, 0.5, 0.7, 1, 2.5, 5, 10];
  let engineLevel = store.get("level", 3);
  let lastLobby = null;
  let knownGame; // undefined = lobby not loaded yet

  function tcLabel(min, inc) {
    return min ? min + "+" + inc : "∞";
  }

  function renderLevelPicker() {
    const box = $("level-picker");
    box.innerHTML = "";
    for (let level = 1; level <= 8; level++) {
      const b = document.createElement("button");
      b.type = "button";
      b.textContent = level;
      b.dataset.level = level;
      b.className = level === engineLevel ? "selected" : "";
      b.addEventListener("click", () => { engineLevel = level; store.set("level", level); renderLevelPicker(); });
      box.appendChild(b);
    }
    const names = t("level_names");
    $("level-hint").textContent = (Array.isArray(names) ? names[engineLevel - 1] : "") + " · " +
      t("level_time", { time: LEVEL_TIMES[engineLevel - 1] });
  }

  function renderTimeControls() {
    const select = $("engine-tc");
    const current = select.value || store.get("engineTc", "0+0");
    select.innerHTML = "";
    const opt = (value, label) => {
      const o = document.createElement("option");
      o.value = value; o.textContent = label;
      select.appendChild(o);
    };
    opt("0+0", t("no_clock"));
    for (const [m, i] of TIME_CONTROLS) opt(m + "+" + i, m + "+" + i);
    select.value = current;

    const grid = $("tc-grid");
    grid.innerHTML = "";
    for (const [m, i] of [...TIME_CONTROLS, [0, 0]]) {
      const b = document.createElement("button");
      b.type = "button";
      b.dataset.tc = m + "+" + i;
      const kind = !m ? t("no_clock") : m < 3 ? "Bullet" : m < 10 ? "Blitz" : m < 30 ? "Rapid" : "Classical";
      b.innerHTML = "<span class='tc-main'></span><span class='tc-sub'></span>";
      b.firstChild.textContent = tcLabel(m, i);
      b.lastChild.textContent = kind;
      b.addEventListener("click", () => createSeek(m, i));
      grid.appendChild(b);
    }
  }
  $("engine-tc").addEventListener("change", (e) => store.set("engineTc", e.target.value));

  function colorPicker(id) {
    const box = $(id);
    box.addEventListener("click", (e) => {
      const btn = e.target.closest("button");
      if (!btn) return;
      box.querySelectorAll("button").forEach((b) => b.classList.toggle("selected", b === btn));
    });
    return () => box.querySelector(".selected")?.dataset.color || "r";
  }
  const engineColor = colorPicker("engine-color");
  const seekColor = colorPicker("seek-color");

  $("play-engine-btn").addEventListener("click", async () => {
    const [min, inc] = $("engine-tc").value.split("+");
    const data = await post("engine", { level: engineLevel, color: engineColor(), min, inc });
    if (data.ok) go("#/game/" + data.game);
    else showError(data);
  });

  async function createSeek(min, inc) {
    const data = await post("seek", { min, inc, color: seekColor() });
    if (!data.ok) showError(data);
    pollLobby(true);
  }

  $("cancel-seek-btn").addEventListener("click", async () => {
    await post("cancel");
    pollLobby(true);
  });

  async function pollLobby(immediate) {
    if (!immediate) {
      const data = await api("lobby");
      if (data && data.v !== undefined) {
        lastLobby = data;
        renderLobby(data);
      }
    } else {
      clearTimeout(pollTimer);
      await pollLobby(false);
    }
    const hash = location.hash || "#/";
    if (hash === "#/" || hash === "#/settings" || hash === "") schedule(() => pollLobby(false), 1500);
  }

  function nameOf(p) {
    if (!p) return "?";
    if (p.engine) return p.level === 0 ? t("engine") : t("engine_level", { level: p.level });
    return p.name || "?";
  }

  function miniBoard(boardFen, last) {
    const board = Chess.parseBoard(boardFen);
    const el = document.createElement("div");
    el.className = "mini";
    const lastSquares = last ? [Chess.sq(last.slice(0, 2)), Chess.sq(last.slice(2, 4))] : [];
    for (let row = 7; row >= 0; row--) {
      for (let file = 0; file < 8; file++) {
        const i = row * 8 + file;
        const cell = document.createElement("div");
        cell.className = ((row + file) % 2 === 0 ? "d" : "l") + (lastSquares.includes(i) ? " lm" : "") + (board[i] ? " " + board[i] : "");
        el.appendChild(cell);
      }
    }
    return el;
  }

  function renderLobby(data) {
    $("online-count").textContent = "● " + (data.online || 0);
    const me = data.me;
    if (me) setNickChip(me.nick);

    // A new game since the last poll (someone accepted my challenge) → go straight to it.
    // No redirect on the first lobby load – the player may have come back on purpose.
    const newGame = me && me.game && knownGame !== undefined && me.game !== knownGame;
    knownGame = me ? me.game : null;
    if (newGame && (location.hash === "#/" || location.hash === "")) return go("#/game/" + me.game);

    $("current-game").hidden = !(me && me.game);
    if (me && me.game) $("current-game-link").href = "#/game/" + me.game;
    $("seeking-box").hidden = !(me && me.seeking);

    // Challenges
    const seekList = $("seek-list");
    seekList.innerHTML = "";
    for (const s of data.seeks) {
      const li = document.createElement("li");
      const label = document.createElement("span");
      label.textContent = t("seek_by", { nick: s.nick || "?", tc: tcLabel(s.min, s.inc) });
      li.appendChild(label);
      if (me && s.by === me.id) {
        const tag = document.createElement("span");
        tag.className = "muted small";
        tag.textContent = t("your_seek");
        li.appendChild(tag);
      } else {
        const b = document.createElement("button");
        b.className = "btn primary";
        b.textContent = t("join");
        b.dataset.seek = s.id;
        b.addEventListener("click", async () => {
          const r = await post("accept", { seek: s.id });
          if (r.ok) go("#/game/" + r.game);
          else showError(r);
        });
        li.appendChild(b);
      }
      seekList.appendChild(li);
    }
    $("seek-empty").hidden = data.seeks.length > 0;

    // Games
    const thumbs = $("game-thumbs");
    thumbs.innerHTML = "";
    const games = data.games.filter((g) => g.status === "active").slice(0, 8);
    for (const g of games) {
      const a = document.createElement("a");
      a.className = "thumb";
      a.href = "#/game/" + g.id;
      a.dataset.game = g.id;
      a.appendChild(miniBoard(g.board, g.last));
      const names = document.createElement("div");
      names.className = "names";
      const w = document.createElement("span"); w.textContent = nameOf(g.w);
      const b = document.createElement("span"); b.textContent = nameOf(g.b);
      names.append(w, b);
      a.appendChild(names);
      if (g.demo || g.viewers) {
        const tag = document.createElement("div");
        tag.className = "tag";
        tag.textContent = [g.demo ? t("demo") : "", g.viewers ? t("viewers", { n: g.viewers }) : ""].filter(Boolean).join(" · ");
        a.appendChild(tag);
      }
      thumbs.appendChild(a);
    }
    $("games-empty").hidden = games.length > 0;

    // Players
    const list = $("player-list");
    list.innerHTML = "";
    for (const p of data.players) {
      const li = document.createElement("li");
      if (me && p.id === me.id) li.className = "me";
      const name = document.createElement("span");
      name.innerHTML = "<span class='dot'></span>";
      name.appendChild(document.createTextNode(p.nick));
      const st = document.createElement("span");
      st.className = "status";
      st.textContent = t("status_" + p.status);
      li.append(name, st);
      list.appendChild(li);
    }

    // Engine
    const e = data.engine;
    $("engine-status").textContent = e.thinking
      ? t("engine_busy", { depth: e.depth, knps: (e.nps / 1000).toFixed(1) }) + (e.queue ? " · " + t("queue", { n: e.queue }) : "")
      : e.lastDepth ? t("engine_idle", { depth: e.lastDepth, knps: (e.lastNps / 1000).toFixed(1) }) : "";

    if (!$("view-settings").hidden) {
      renderWifi(data.net);
      renderDevice(data.sys, e);
    }
  }

  // Firmware version and deployed model (from the lobby response).
  function renderDevice(sys, engine) {
    if (!sys) return;
    $("dev-fw").textContent = sys.fw || "–";
    $("dev-chip").textContent = sys.chip || "–";
    $("dev-elo").textContent = sys.elo ? t("dev_elo_value", { elo: sys.elo }) : "–";
    const aboutChip = $("about-chip");
    if (aboutChip && sys.chip) aboutChip.textContent = sys.chip.split(",")[0];
    $("dev-model").textContent = sys.model
      ? t("dev_model_size", { name: sys.model, arch: sys.arch, kb: Math.round(sys.bytes / 1024) })
      : "–";
    const nps = engine && (engine.thinking ? engine.nps : engine.lastNps);
    $("dev-nps").textContent = nps ? t("dev_nps", { knps: (nps / 1000).toFixed(1) }) : "–";
    $("dev-hash").textContent = sys.hashKb
      ? t(sys.hashPsram ? "dev_hash_psram" : "dev_hash_value", { kb: sys.hashKb })
      : "–";
    $("dev-temp").textContent = typeof sys.temp === "number"
      ? t("dev_temp_value", { t: sys.temp.toFixed(1) })
      : "–";
    const s = sys.uptime || 0;
    const d = Math.floor(s / 86400), h = Math.floor(s / 3600) % 24, m = Math.floor(s / 60) % 60;
    $("dev-uptime").textContent = (d ? d + " d " : "") + h + ":" + String(m).padStart(2, "0") + ":" + String(s % 60).padStart(2, "0");
  }

  // ------------------------------------------------------------------
  // Game
  // ------------------------------------------------------------------
  const G = {
    id: null, data: null, moves: [], san: [], boards: [], viewPly: null,
    flipped: false, syncedAt: 0, lastPly: -1, lastStatus: null, requestSeq: 0, sending: false
  };

  const board = window.Board.create(document.querySelector(".board-wrap"), {
    onMove: sendMove,
    onPremove(uci) { if (uci) toast(t("premove_set")); }
  });
  window.__board = board; // for tests (Playwright)

  function openGame(id) {
    if (G.id !== id) {
      Object.assign(G, { id, data: null, moves: [], san: [], boards: [Chess.parseBoard(Chess.START_BOARD)], viewPly: null, flipped: false, lastPly: -1, lastStatus: null });
      board.clearPremove();
      board.setHint(null);
      $("moves").innerHTML = "";
    }
    pollGame();
  }

  async function pollGame() {
    // No polling while a move is being sent – the response could show the position before it.
    if (G.sending) return schedule(pollGame, 150);
    const id = G.id;
    const seq = ++G.requestSeq;
    const data = await api("game", { id, since: G.moves.length });
    if (G.id !== id || seq !== G.requestSeq || $("view-game").hidden) return;
    if (data.ok) {
      applyGameData(data);
      // After the game ends, watch whether the opponent accepted a rematch (new game).
      if (data.status !== "active" && data.you) checkRematchGame();
    } else if (data.err === "no_game") {
      toast(t("err_no_game"));
      return go("#/");
    }
    const active = G.data && G.data.status === "active";
    // "more" = the server sent only part of the moves (long game) → ask for the rest right away.
    schedule(pollGame, data.more ? 50 : active ? 700 : 2000);
  }

  async function checkRematchGame() {
    const lobby = await api("lobby");
    if (lobby.me && lobby.me.game && lobby.me.game !== G.id) go("#/game/" + lobby.me.game);
  }

  function applyGameData(data) {
    const previousPly = G.moves.length;
    // Takeback: the server has fewer moves than we do.
    if (data.from < G.moves.length) {
      G.moves.length = data.from;
      G.san.length = data.from;
      G.boards.length = data.from + 1;
    }
    let captured = false;
    for (let i = 0; i < data.moves.length; i++) {
      G.moves.push(data.moves[i]);
      G.san.push(data.san[i]);
      const r = Chess.applyUci(G.boards[G.boards.length - 1], data.moves[i]);
      captured = captured || !!r.captured;
      G.boards.push(r.board);
    }
    const oldData = G.data;
    G.data = data;
    G.syncedAt = performance.now();
    const newMoves = G.moves.length > previousPly && G.lastPly >= 0;
    const byOpponent = newMoves && (!data.you || data.turn === data.you);
    if (newMoves && byOpponent) {
      beep(captured ? "capture" : "move");
      vibrate(20);
    }
    if (G.lastStatus === "active" && data.status !== "active") beep("notify");
    if (oldData && data.draw && data.you && data.draw !== data.you && oldData.draw !== data.draw) beep("notify");
    G.lastStatus = data.status;
    G.lastPly = G.moves.length;
    renderGame(newMoves || previousPly === 0);
    tryPremove();
  }

  function kingSquare(boardArr, color) {
    return boardArr.findIndex((p) => p === color + "K");
  }

  function renderGame(animate) {
    const d = G.data;
    if (!d) return;
    const bottom = G.flipped ? (d.you === "b" ? "w" : "b") : (d.you === "b" ? "b" : "w");
    board.setOrientation(bottom);

    const live = G.viewPly === null || G.viewPly >= G.moves.length;
    const ply = live ? G.moves.length : G.viewPly;
    const pos = G.boards[ply];
    board.setPosition(pos, {
      lastMove: ply > 0 ? G.moves[ply - 1] : null,
      check: live && d.check ? kingSquare(pos, d.turn) : null,
      animate
    });

    // Who may move.
    const myTurn = live && d.status === "active" && d.you && d.you === d.turn;
    if (myTurn) board.setLegal(d.legal);
    else {
      board.setLegal(null);
      board.setPremoveColor(live && d.status === "active" && d.you ? d.you : null);
    }
    board.setHint(live && d.hint ? d.hint : null);

    // Players, material, clocks.
    const mat = Chess.material(G.boards[G.moves.length]);
    for (const [barId, color] of [["bar-bottom", bottom], ["bar-top", bottom === "w" ? "b" : "w"]]) {
      const bar = $(barId);
      const p = d[color];
      const name = bar.querySelector(".player-name");
      name.textContent = nameOf(p) + (d.you === color ? " (" + t("you") + ")" : "");
      if (p && !p.engine && p.online === false) {
        const off = document.createElement("span");
        off.className = "off";
        off.textContent = t("offline_short");
        name.appendChild(off);
      }
      const adv = color === "w" ? mat.diff : -mat.diff;
      bar.querySelector(".material").textContent = adv > 0 ? "+" + adv : "";
      bar.dataset.color = color;
    }
    renderClocks();

    // Status line.
    let status = "";
    if (d.status === "active") {
      if (!d.you) status = t("spectating") + " · " + t(d.turn === "w" ? "white_to_move" : "black_to_move");
      else status = d.you === d.turn ? t("your_turn") : t("their_turn");
    }
    $("status-line").textContent = status;
    $("thinking").hidden = !d.thinking;
    $("thinking-text").textContent = t("thinking", { depth: d.depth, knps: (d.nps / 1000).toFixed(1) });
    $("viewers").textContent = d.viewers ? t("viewers", { n: d.viewers }) : "";

    // Draw.
    $("draw-banner").hidden = !(d.status === "active" && d.you && d.draw && d.draw !== d.you);

    // Result.
    const over = d.status !== "active";
    $("result-box").hidden = !over;
    if (over) {
      $("result-score").textContent = d.result === "*" ? "–" : d.result.replace("1/2", "½").replace("1/2", "½");
      const who = d.status === "aborted" ? t("result_aborted") : d.winner === "w" ? t("result_white") : d.winner === "b" ? t("result_black") : t("result_draw");
      $("result-reason").textContent = who + " · " + t("reason_" + d.status);
      $("rematch-btn").hidden = !d.you || d.demo;
      $("rematch-btn").textContent = d.rematch && d.rematch !== d.you ? t("accept") + " – " + t("rematch") : t("rematch");
      $("rematch-btn").disabled = d.rematch === d.you;
    }

    // Action buttons.
    const playing = d.you && d.status === "active";
    const vsEngine = (d.w && d.w.engine) || (d.b && d.b.engine);
    $("actions").hidden = !playing;
    $("hint-btn").hidden = !vsEngine;
    $("takeback-btn").hidden = !vsEngine;
    $("draw-btn").hidden = !!vsEngine;
    $("hint-btn").disabled = !myTurn;
    $("takeback-btn").disabled = G.moves.length < 1 || !!d.thinking;
    $("draw-btn").disabled = d.draw === d.you;
    renderResignButton();

    renderMoves();
  }

  function formatClock(ms) {
    ms = Math.max(0, ms);
    const total = ms / 1000;
    const m = Math.floor(total / 60);
    const s = Math.floor(total % 60);
    if (total < 10) return m + ":" + String(s).padStart(2, "0") + "." + Math.floor((ms % 1000) / 100);
    return m + ":" + String(s).padStart(2, "0");
  }

  // Clocks are extrapolated locally between server polls.
  function renderClocks() {
    const d = G.data;
    for (const barId of ["bar-top", "bar-bottom"]) {
      const bar = $(barId);
      const clockEl = bar.querySelector(".clock");
      const color = bar.dataset.color;
      if (!d || !d.clock) { clockEl.textContent = ""; continue; }
      let ms = d.clock[color];
      const running = d.clock.running && d.turn === color && d.status === "active";
      if (running) ms -= performance.now() - G.syncedAt;
      clockEl.textContent = formatClock(ms);
      clockEl.classList.toggle("running", running);
      clockEl.classList.toggle("low", running && ms < 10000);
    }
  }
  setInterval(() => { if (!$("view-game").hidden) renderClocks(); }, 100);

  function renderMoves() {
    const box = $("moves");
    const current = G.viewPly === null ? G.moves.length : G.viewPly;
    box.innerHTML = "";
    G.san.forEach((san, i) => {
      if (i % 2 === 0) {
        const no = document.createElement("span");
        no.className = "no";
        no.textContent = i / 2 + 1;
        box.appendChild(no);
      }
      const mv = document.createElement("span");
      mv.className = "mv" + (i + 1 === current ? " active" : "");
      mv.textContent = san;
      mv.dataset.ply = i + 1;
      mv.addEventListener("click", () => viewPly(i + 1));
      box.appendChild(mv);
    });
    const active = box.querySelector(".active");
    if (active && G.viewPly === null) active.scrollIntoView({ block: "nearest", inline: "nearest" });
  }

  function viewPly(ply) {
    G.viewPly = ply >= G.moves.length ? null : Math.max(0, ply);
    board.clearSelection();
    renderGame(true);
  }

  $("nav-first").addEventListener("click", () => viewPly(0));
  $("nav-prev").addEventListener("click", () => viewPly((G.viewPly === null ? G.moves.length : G.viewPly) - 1));
  $("nav-next").addEventListener("click", () => viewPly((G.viewPly === null ? G.moves.length : G.viewPly) + 1));
  $("nav-last").addEventListener("click", () => viewPly(G.moves.length));
  $("flip-btn").addEventListener("click", () => { G.flipped = !G.flipped; renderGame(false); });
  document.addEventListener("keydown", (e) => {
    if ($("view-game").hidden || e.target.tagName === "INPUT") return;
    if (e.key === "ArrowLeft") $("nav-prev").click();
    if (e.key === "ArrowRight") $("nav-next").click();
    if (e.key === "f") $("flip-btn").click();
  });

  async function sendMove(uci) {
    const id = G.id;
    G.requestSeq++; // discards any in-flight (stale) response
    beep("move");
    G.sending = true;
    const data = await post("move", { id, m: uci });
    G.sending = false;
    if (!data.ok) showError(data);
    if (G.id === id) pollGame();
  }

  // Premove: played automatically once it is our turn (if still legal).
  function tryPremove() {
    const pre = board.premove();
    const d = G.data;
    if (!pre || !d || d.status !== "active" || d.you !== d.turn || G.viewPly !== null) return;
    const legal = d.legal.find((m) => m === pre) || d.legal.find((m) => m === pre + "q");
    board.clearPremove();
    if (legal) {
      board.setPosition(Chess.applyUci(G.boards[G.moves.length], legal).board, { lastMove: legal, animate: true });
      board.setLegal(null);
      sendMove(legal);
    } else {
      toast(t("premove_cancelled"));
    }
  }

  // Resigning with in-page confirmation (like lichess): the first click arms the button,
  // a second click within 3 s resigns. We don't use confirm() – the captive portal browser
  // (the window a phone opens after joining the WiFi) blocks dialogs and returns "no".
  let resignArmedUntil = 0;
  function renderResignButton() {
    const btn = $("resign-btn");
    const armed = performance.now() < resignArmedUntil;
    btn.textContent = armed ? t("confirm_resign") : G.moves.length < 2 ? t("abort") : t("resign");
    btn.classList.toggle("primary", armed);
  }
  $("resign-btn").addEventListener("click", async () => {
    // Aborting before the 2nd ply costs nothing – no confirmation.
    if (G.moves.length >= 2 && performance.now() >= resignArmedUntil) {
      resignArmedUntil = performance.now() + 3000;
      renderResignButton();
      setTimeout(renderResignButton, 3100);
      return;
    }
    resignArmedUntil = 0;
    renderResignButton();
    const r = await post("resign", { id: G.id });
    if (!r.ok) showError(r);
    pollGame();
  });
  $("draw-btn").addEventListener("click", async () => {
    const r = await post("draw", { id: G.id });
    if (r.ok) toast(t("draw_sent")); else showError(r);
    pollGame();
  });
  $("draw-accept").addEventListener("click", async () => { await post("draw", { id: G.id }); pollGame(); });
  $("draw-decline").addEventListener("click", async () => { await post("draw", { id: G.id, decline: 1 }); pollGame(); });
  $("takeback-btn").addEventListener("click", async () => {
    const r = await post("takeback", { id: G.id });
    if (!r.ok) showError(r);
    pollGame();
  });
  $("hint-btn").addEventListener("click", async () => {
    const r = await post("hint", { id: G.id });
    if (!r.ok) showError(r);
    pollGame();
  });
  $("rematch-btn").addEventListener("click", async () => {
    const r = await post("rematch", { id: G.id });
    if (!r.ok) return showError(r);
    if (r.game) go("#/game/" + r.game);
    else { toast(t("rematch_sent")); pollGame(); }
  });

  $("pgn-btn").addEventListener("click", async () => {
    const d = G.data;
    if (!d) return;
    const date = new Date();
    const text = Chess.pgn({
      Event: "ESP32 Chess",
      Site: location.host,
      Date: date.getFullYear() + "." + String(date.getMonth() + 1).padStart(2, "0") + "." + String(date.getDate()).padStart(2, "0"),
      White: nameOf(d.w),
      Black: nameOf(d.b),
      Result: d.result
    }, G.san, d.result);
    toast(copyText(text) ? t("copied") : t("copy_failed"));
  });

  // Copy to clipboard. The modern `navigator.clipboard` only works over HTTPS,
  // but the board serves HTTP – hence the fallback via a hidden text field.
  function copyText(text) {
    if (window.isSecureContext && navigator.clipboard) {
      navigator.clipboard.writeText(text).catch(() => {});
      return true;
    }
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    area.setSelectionRange(0, text.length);
    let ok = false;
    try { ok = document.execCommand("copy"); } catch (e) { ok = false; }
    area.remove();
    return ok;
  }

  // ------------------------------------------------------------------
  // Settings
  // ------------------------------------------------------------------
  function applyAppearance() {
    document.documentElement.dataset.theme = store.get("theme", "dark");
    document.documentElement.dataset.board = store.get("board", "brown");
    document.body.classList.toggle("hide-coords", !store.get("coords", true));
    const meta = document.querySelector("meta[name=theme-color]");
    if (meta) meta.content = store.get("theme", "dark") === "light" ? "#f4f3f1" : "#161512";
  }

  function renderSettings() {
    document.querySelectorAll("#theme-picker button").forEach((b) => b.classList.toggle("selected", b.dataset.themeChoice === store.get("theme", "dark")));
    document.querySelectorAll("#board-picker button").forEach((b) => b.classList.toggle("selected", b.dataset.board === store.get("board", "brown")));
    $("sound-toggle").checked = store.get("sound", true);
    $("coords-toggle").checked = store.get("coords", true);
    $("rename-input").value = store.get("nick", "");
    if (lastLobby) renderWifi(lastLobby.net);
  }

  $("theme-picker").addEventListener("click", (e) => {
    const b = e.target.closest("button"); if (!b) return;
    store.set("theme", b.dataset.themeChoice); applyAppearance(); renderSettings();
  });
  $("board-picker").addEventListener("click", (e) => {
    const b = e.target.closest("button"); if (!b) return;
    store.set("board", b.dataset.board); applyAppearance(); renderSettings();
  });
  $("sound-toggle").addEventListener("change", (e) => store.set("sound", e.target.checked));
  $("coords-toggle").addEventListener("change", (e) => { store.set("coords", e.target.checked); applyAppearance(); });

  $("rename-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const nick = $("rename-input").value.trim();
    const r = await post("hello", { nick });
    if (!r.ok) {
      $("rename-error").textContent = t("err_" + r.err);
      $("rename-error").hidden = false;
      return;
    }
    $("rename-error").hidden = true;
    store.set("nick", r.nick);
    setNickChip(r.nick);
    toast(t("saved"));
  });

  $("logout-btn").addEventListener("click", async () => {
    await post("bye");
    token = null;
    store.set("token", null);
    setNickChip("");
    go("#/welcome");
  });

  function renderWifi(net) {
    if (!net) return;
    let text;
    if (net.connecting) text = t("wifi_connecting", { ssid: net.sta });
    else if (net.mode === "sta" && net.ip) text = t("wifi_sta", { ssid: net.sta, ip: net.ip });
    else if (net.err && net.sta) text = t("wifi_failed", { ssid: net.sta });
    else text = t("wifi_ap", { ap: net.ap });
    $("wifi-status").textContent = text;
    $("wifi-owner").hidden = !net.owner;
    $("wifi-not-owner").hidden = !!net.owner;
  }

  $("wifi-scan-btn").addEventListener("click", async () => {
    const list = $("wifi-list");
    list.innerHTML = "<li class='muted'>" + t("scanning") + "</li>";
    const data = await api("wifi/scan");
    list.innerHTML = "";
    for (const n of data.networks || []) {
      const li = document.createElement("li");
      const name = document.createElement("span");
      name.textContent = (n.secure ? "🔒 " : "") + n.ssid;
      const bars = document.createElement("span");
      bars.className = "bars";
      bars.textContent = n.rssi > -55 ? "▂▄▆█" : n.rssi > -67 ? "▂▄▆" : n.rssi > -78 ? "▂▄" : "▂";
      li.append(name, bars);
      li.addEventListener("click", () => { $("wifi-ssid").value = n.ssid; $("wifi-pass").focus(); });
      list.appendChild(li);
    }
  });

  $("wifi-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const r = await post("wifi", { ssid: $("wifi-ssid").value, pass: $("wifi-pass").value });
    if (r.ok) toast(t("wifi_saved")); else showError(r);
  });
  $("wifi-forget-btn").addEventListener("click", async () => {
    const r = await post("wifi", { forget: 1 });
    if (!r.ok) showError(r);
  });

  // ------------------------------------------------------------------
  // Start
  // ------------------------------------------------------------------
  async function start() {
    applyAppearance();
    applyI18n();
    setNickChip(store.get("nick", ""));
    if (token) {
      // Check that the session still exists on the board (it may have restarted).
      const r = await post("hello", {});
      if (!r.ok) {
        token = null;
        store.set("token", null);
      } else {
        store.set("nick", r.nick);
        setNickChip(r.nick);
      }
    }
    window.addEventListener("hashchange", route);
    route();
  }
  start();
})();
