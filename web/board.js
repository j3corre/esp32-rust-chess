// Interactive chessboard: drawing, move animation, drag and tap input,
// legal-move dots, premoves and the pawn promotion chooser.
(function () {
  "use strict";
  const { sq, sqName } = window.Chess;

  function create(wrap, callbacks) {
    const boardEl = wrap.querySelector(".board");
    const promoEl = wrap.querySelector(".promo");
    const state = {
      orientation: "w",
      board: new Array(64).fill(null),
      legal: null,          // UCI moves the player may play now (null = none)
      premoveColor: null,   // colour that may enter a premove (when it is not the player's turn)
      selected: null,       // selected square (index)
      premove: null,        // { from, to }
      lastMove: null,
      check: null,
      hint: null,
      pieces: new Map(),    // square → piece element
      drag: null,
      promoPending: null
    };

    // --- Board squares (64 divs) and coordinates ---
    const squares = [];
    for (let i = 0; i < 64; i++) {
      const el = document.createElement("div");
      el.className = "sq " + (((i % 8) + Math.floor(i / 8)) % 2 === 0 ? "d" : "l");
      el.dataset.sq = sqName(i);
      boardEl.appendChild(el);
      squares.push(el);
    }

    // Top-left corner of a square in %, by board orientation.
    function coords(index) {
      const file = index % 8, rank = Math.floor(index / 8);
      const x = state.orientation === "w" ? file : 7 - file;
      const y = state.orientation === "w" ? 7 - rank : rank;
      return { x, y };
    }

    function layoutSquares() {
      boardEl.querySelectorAll(".coord").forEach((c) => c.remove());
      for (let i = 0; i < 64; i++) {
        const { x, y } = coords(i);
        squares[i].style.left = x * 12.5 + "%";
        squares[i].style.top = y * 12.5 + "%";
        // File letters on the bottom rank, rank numbers in the left file.
        if (y === 7) addCoord(squares[i], "file", sqName(i)[0], i);
        if (x === 0) addCoord(squares[i], "rank", sqName(i)[1], i);
      }
    }

    function addCoord(squareEl, kind, text, index) {
      const c = document.createElement("span");
      const light = squareEl.classList.contains("l");
      c.className = "coord " + kind + (light ? " on-l" : " on-d");
      c.textContent = text;
      squareEl.appendChild(c);
    }

    function placePiece(el, index) {
      const { x, y } = coords(index);
      el.style.transform = "translate(" + x * 100 + "%," + y * 100 + "%)";
    }

    // --- Rendering a position with animation: pieces move rather than being redrawn ---
    function render(animate) {
      const old = state.pieces;
      const next = new Map();
      const unmatchedNew = [];
      for (let i = 0; i < 64; i++) {
        const code = state.board[i];
        if (!code) continue;
        const el = old.get(i);
        if (el && el.dataset.code === code) {
          next.set(i, el);
          old.delete(i);
        } else {
          unmatchedNew.push(i);
        }
      }
      // Unmatched new pieces: find the same piece that vanished elsewhere (= it moved).
      for (const i of unmatchedNew) {
        const code = state.board[i];
        let best = null, bestDist = 99;
        for (const [from, el] of old) {
          if (el.dataset.code !== code) continue;
          const d = Math.abs((from % 8) - (i % 8)) + Math.abs(Math.floor(from / 8) - Math.floor(i / 8));
          const preferred = state.lastMove && from === sq(state.lastMove.slice(0, 2)) ? -1 : 0;
          if (d + preferred < bestDist) { best = from; bestDist = d + preferred; }
        }
        let el;
        if (best !== null) {
          el = old.get(best);
          old.delete(best);
        } else {
          el = document.createElement("div");
          el.className = "piece " + code;
          el.dataset.code = code;
          el.style.transition = "none";
          boardEl.appendChild(el);
          placePiece(el, i);
          el.getBoundingClientRect(); // forces a layout without animation
          el.style.transition = "";
        }
        if (!animate) el.style.transition = "none";
        placePiece(el, i);
        if (!animate) { el.getBoundingClientRect(); el.style.transition = ""; }
        next.set(i, el);
      }
      for (const el of old.values()) el.remove();
      const dragged = state.drag && state.drag.active ? state.drag.el : null;
      for (const [i, el] of next) {
        // Keep the dragged piece under the finger (redrawing mid-drag would snap it back).
        if (!unmatchedNew.includes(i) && el !== dragged) placePiece(el, i);
      }
      state.pieces = next;
      renderMarks();
    }

    // Highlights: last move, check, selection, dots, premove, hint.
    function renderMarks() {
      const dests = destinations(state.selected);
      const premoveDests = state.selected !== null && state.legal === null;
      for (let i = 0; i < 64; i++) {
        const cl = squares[i].classList;
        const name = sqName(i);
        cl.toggle("last", !!state.lastMove && (state.lastMove.slice(0, 2) === name || state.lastMove.slice(2, 4) === name));
        cl.toggle("check", state.check === i);
        cl.toggle("sel", state.selected === i);
        cl.toggle("dest", !premoveDests && dests.has(i));
        cl.toggle("occ", !!state.board[i]);
        cl.toggle("pre", !!state.premove && (state.premove.from === i || state.premove.to === i));
        cl.toggle("hint-sq", !!state.hint && (state.hint.slice(0, 2) === name || state.hint.slice(2, 4) === name));
      }
    }

    // Where the piece on `from` may move (from the server's legal-move list).
    function destinations(from) {
      const set = new Set();
      if (from === null || !state.legal) return set;
      const name = sqName(from);
      for (const mv of state.legal) if (mv.startsWith(name)) set.add(sq(mv.slice(2, 4)));
      return set;
    }

    function movableColor() {
      if (state.legal) return state.legal.length ? state.board[sq(state.legal[0].slice(0, 2))]?.[0] : null;
      return state.premoveColor;
    }

    function squareAt(clientX, clientY) {
      const rect = boardEl.getBoundingClientRect();
      const x = Math.floor(((clientX - rect.left) / rect.width) * 8);
      const y = Math.floor(((clientY - rect.top) / rect.height) * 8);
      if (x < 0 || x > 7 || y < 0 || y > 7) return null;
      const file = state.orientation === "w" ? x : 7 - x;
      const rank = state.orientation === "w" ? 7 - y : y;
      return rank * 8 + file;
    }

    // --- Attempting a move from `from` to `to` ---
    function tryMove(from, to) {
      if (state.legal) {
        const base = sqName(from) + sqName(to);
        const options = state.legal.filter((m) => m.startsWith(base));
        if (!options.length) return false;
        if (options.length > 1) {
          choosePromotion(from, to, options);
        } else {
          commit(options[0]);
        }
        return true;
      }
      if (state.premoveColor && from !== to) {
        state.premove = { from, to };
        callbacks.onPremove && callbacks.onPremove(sqName(from) + sqName(to));
        return true;
      }
      return false;
    }

    function commit(uci) {
      // Show the move optimistically right away (the server confirms it on the next poll).
      state.board = window.Chess.applyUci(state.board, uci).board;
      state.lastMove = uci;
      state.legal = null;
      state.selected = null;
      state.check = null;
      state.hint = null;
      render(false);
      callbacks.onMove(uci);
    }

    // Promotion chooser: a column of pieces over the target square (like on lichess).
    function choosePromotion(from, to, options) {
      const color = state.board[from][0];
      const { x, y } = coords(to);
      promoEl.innerHTML = "";
      promoEl.style.left = x * 12.5 + "%";
      const down = y === 0;
      promoEl.style.top = down ? "0" : "auto";
      promoEl.style.bottom = down ? "auto" : "0";
      promoEl.style.flexDirection = down ? "column" : "column-reverse";
      const backdrop = document.createElement("div");
      backdrop.className = "promo-backdrop";
      wrap.appendChild(backdrop);
      const close = () => { promoEl.hidden = true; backdrop.remove(); state.selected = null; renderMarks(); };
      backdrop.addEventListener("pointerdown", (e) => { e.stopPropagation(); close(); });
      for (const letter of ["q", "n", "r", "b"]) {
        const uci = sqName(from) + sqName(to) + letter;
        if (!options.includes(uci)) continue;
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "piece-btn " + color + letter.toUpperCase();
        btn.dataset.promo = letter;
        btn.setAttribute("aria-label", letter);
        btn.addEventListener("pointerdown", (e) => { e.stopPropagation(); e.preventDefault(); close(); commit(uci); });
        promoEl.appendChild(btn);
      }
      promoEl.hidden = false;
    }

    // --- Mouse / touch input ---
    boardEl.addEventListener("pointerdown", (e) => {
      if (e.button !== undefined && e.button !== 0) return;
      const index = squareAt(e.clientX, e.clientY);
      if (index === null) return;
      e.preventDefault();
      const piece = state.board[index];
      const color = movableColor();

      // Tapping a target of the selected piece = move.
      if (state.selected !== null && index !== state.selected && !(piece && piece[0] === color)) {
        const from = state.selected;
        state.selected = null;
        if (!tryMove(from, index)) cancelPremove();
        renderMarks();
        return;
      }
      if (piece && color && piece[0] === color) {
        state.selected = index;
        renderMarks();
        const el = state.pieces.get(index);
        state.drag = { from: index, el, startX: e.clientX, startY: e.clientY, active: false, pointerId: e.pointerId };
        boardEl.setPointerCapture(e.pointerId);
      } else {
        state.selected = null;
        cancelPremove();
        renderMarks();
      }
    });

    boardEl.addEventListener("pointermove", (e) => {
      const drag = state.drag;
      if (!drag || !drag.el) return;
      if (!drag.active && Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) < 5) return;
      drag.active = true;
      const rect = boardEl.getBoundingClientRect();
      const size = rect.width / 8;
      drag.el.classList.add("dragging");
      drag.el.style.transform =
        "translate(" + (e.clientX - rect.left - size / 2) + "px," + (e.clientY - rect.top - size / 2) + "px) scale(1.15)";
      const over = squareAt(e.clientX, e.clientY);
      squares.forEach((s, i) => s.classList.toggle("hover", i === over && over !== drag.from));
    });

    function endDrag(e, cancelled) {
      const drag = state.drag;
      if (!drag) return;
      state.drag = null;
      squares.forEach((s) => s.classList.remove("hover"));
      if (!drag.active) return; // plain tap – the selection stays
      drag.el.classList.remove("dragging");
      const to = cancelled ? null : squareAt(e.clientX, e.clientY);
      if (to !== null && to !== drag.from) {
        placePiece(drag.el, drag.from);
        drag.el.style.transition = "none";
        state.selected = null;
        tryMove(drag.from, to);
        requestAnimationFrame(() => { drag.el.style.transition = ""; });
      } else {
        placePiece(drag.el, drag.from);
      }
      renderMarks();
    }

    boardEl.addEventListener("pointerup", (e) => endDrag(e, false));
    boardEl.addEventListener("pointercancel", (e) => endDrag(e, true));
    // Prevents double-tap zoom on iOS.
    boardEl.addEventListener("touchstart", (e) => { if (e.touches.length === 1) e.preventDefault(); }, { passive: false });

    function cancelPremove() {
      if (state.premove) {
        state.premove = null;
        callbacks.onPremove && callbacks.onPremove(null);
      }
    }

    layoutSquares();

    return {
      setOrientation(color) {
        if (state.orientation === color) return;
        state.orientation = color;
        layoutSquares();
        render(false);
      },
      orientation() { return state.orientation; },
      // Sets the position; `opts`: { lastMove, check (square index), animate }
      setPosition(board, opts) {
        state.board = board.slice();
        state.lastMove = opts.lastMove || null;
        state.check = opts.check === undefined ? null : opts.check;
        if (state.selected !== null && !state.board[state.selected]) state.selected = null;
        render(!!opts.animate);
      },
      setLegal(moves) {
        state.legal = moves;
        if (moves) state.premoveColor = null;
        renderMarks();
      },
      setPremoveColor(color) { state.premoveColor = color; },
      premove() { return state.premove ? sqName(state.premove.from) + sqName(state.premove.to) : null; },
      clearPremove() { state.premove = null; renderMarks(); },
      setHint(uci) { state.hint = uci; renderMarks(); },
      clearSelection() { state.selected = null; renderMarks(); },
      // For tests and accessibility: a programmatic move (same path as a click).
      play(from, to) { return tryMove(sq(from), sq(to)); }
    };
  }

  window.Board = { create };
})();
