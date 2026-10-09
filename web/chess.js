// Minimal chess logic in the browser.
// The board enforces the rules (move legality) – the server sends the list of legal moves.
// Here we only parse FEN and replay a move on the board, so we can browse
// the game history and animate moves without extra server requests.
(function () {
  "use strict";

  const START_BOARD = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR";

  // Square "e4" → index 0..63 (a1 = 0, h8 = 63).
  function sq(name) {
    return (name.charCodeAt(1) - 49) * 8 + (name.charCodeAt(0) - 97);
  }

  function sqName(index) {
    return String.fromCharCode(97 + (index % 8)) + String.fromCharCode(49 + Math.floor(index / 8));
  }

  // First FEN field → array of 64 items: "wK", "bp"... or null.
  function parseBoard(fenBoard) {
    const board = new Array(64).fill(null);
    const rows = fenBoard.split(" ")[0].split("/");
    for (let r = 0; r < 8; r++) {
      let file = 0;
      for (const ch of rows[r] || "") {
        if (ch >= "1" && ch <= "8") {
          file += Number(ch);
        } else {
          const color = ch === ch.toUpperCase() ? "w" : "b";
          board[(7 - r) * 8 + file] = color + ch.toUpperCase();
          file++;
        }
      }
    }
    return board;
  }

  // Plays a UCI move on a copy of the board (including castling, en passant, promotion).
  // Returns { board, captured } – captured = the captured piece (for sound and material).
  function applyUci(board, uci) {
    const next = board.slice();
    const from = sq(uci.slice(0, 2));
    const to = sq(uci.slice(2, 4));
    const piece = next[from];
    let captured = next[to];
    if (!piece) return { board: next, captured: null };
    next[from] = null;
    // En passant: a pawn moves diagonally to an empty square → captures the pawn beside it.
    if (piece[1] === "P" && (from % 8) !== (to % 8) && !captured) {
      const victim = to + (piece[0] === "w" ? -8 : 8);
      captured = next[victim];
      next[victim] = null;
    }
    // Castling: the king moves two squares → move the rook too.
    if (piece[1] === "K" && Math.abs((to % 8) - (from % 8)) === 2) {
      const rank = Math.floor(from / 8) * 8;
      if (to % 8 === 6) { next[rank + 5] = next[rank + 7]; next[rank + 7] = null; }
      else { next[rank + 3] = next[rank]; next[rank] = null; }
    }
    next[to] = uci.length > 4 ? piece[0] + uci[4].toUpperCase() : piece;
    return { board: next, captured };
  }

  // Material difference (white − black) in pawns and the captured pieces of both sides.
  const VALUES = { P: 1, N: 3, B: 3, R: 5, Q: 9, K: 0 };
  function material(board) {
    const counts = { w: {}, b: {} };
    let diff = 0;
    for (const p of board) {
      if (!p) continue;
      counts[p[0]][p[1]] = (counts[p[0]][p[1]] || 0) + 1;
      diff += (p[0] === "w" ? 1 : -1) * VALUES[p[1]];
    }
    return { diff, counts };
  }

  // PGN from headers and a list of SAN moves.
  function pgn(headers, san, result) {
    let text = "";
    for (const [key, value] of Object.entries(headers)) {
      text += "[" + key + " \"" + String(value).replace(/"/g, "'") + "\"]\n";
    }
    text += "\n";
    const parts = [];
    san.forEach((move, i) => {
      if (i % 2 === 0) parts.push((i / 2 + 1) + ".");
      parts.push(move);
    });
    parts.push(result);
    // Lines of at most ~80 characters (PGN format recommendation).
    let line = "";
    for (const part of parts) {
      if ((line + " " + part).length > 80) { text += line.trim() + "\n"; line = ""; }
      line += " " + part;
    }
    return text + line.trim() + "\n";
  }

  window.Chess = { START_BOARD, sq, sqName, parseBoard, applyUci, material, pgn };
})();
