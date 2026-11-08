//! The UCI frontend contract: engine FEN and move text.
//!
//! The host and `three-kingdoms-engine` must agree on
//! this. The engine never learns a covered tile's identity: every covered square
//! is `X`, and the `pool` carries only the public per-(ink, role) counts of the
//! covered tiles, which both seats can derive from the start counts, the
//! face-up pieces and the captures.
//!
//! ```text
//! FEN := <board> <turn> <first> <red score> <black score> <clock> <movenum> <pool> <last>
//! ```
//!
//! The first seven fields are the kernel's public FEN
//! (`threeKingdomsBanqiStateToFen`, `State::public_fen`):
//! - board: 5 ranks separated by `/`, top first (rank 5 .. 1), 9 files a..i per
//!   rank. A face-up piece is its role letter (G general, A advisor, E elephant,
//!   R chariot, H horse, C cannon, S soldier), upper case red ink, lower case
//!   black ink; `X` a covered tile; a digit a run of empty points.
//! - turn: `r` or `b`, the SEAT to move (the red seat moves first), not an ink.
//! - first: the ink the red seat flipped (`r`, `b`), `-` before the first flip.
//! - scores by seat, the 40-ply clock, the 1-based move number.
//!
//! Two fields follow that the search needs and the public FEN omits:
//! - pool: covered counts as `<letter><count>` pairs, e.g.
//!   `G1A2E2R2H2C2S5g1a2e2r2h2c2s5`; `-` when nothing is covered. The counts
//!   must sum to the number of `X` squares.
//! - last: the move that produced this position (`e3e4`, a flip `a1a1`), or `-`.
//!   It decides whether the next quiet move is a chase.
//!
//! Faithful form: the FEN is the position right after the last flip or capture
//! (or the initial position), followed by `moves` with every ply since, which
//! the engine replays to rebuild the chased square and the position counts the
//! chase rule reads. A FEN of the current position with no moves is accepted,
//! but its repetition history starts at that position.
//!
//! Moves are `<from><to>` on squares a1..i5 (rank 1 at the bottom of the FEN);
//! a flip has from == to. Replayed moves may not be flips (their outcome is not
//! in the text); a capture is replayed like any other move.

use crate::{
    code_color, code_role, is_piece, Ink, Move, Seat, State, Status, DOWN, EMPTY, HEIGHT,
    NO_PROGRESS_LIMIT, NSQ, POINTS, START_COUNTS, WIDTH,
};
use crate::{engine::Eval, nnue::Net};
use std::sync::Arc;

const ROLE_LETTERS: [u8; 7] = *b"GAERHCS";

fn letter_code(ch: u8) -> Option<i16> {
    let role = ROLE_LETTERS
        .iter()
        .position(|&l| l == ch.to_ascii_uppercase())? as i16;
    Some(if ch.is_ascii_uppercase() {
        role
    } else {
        7 + role
    })
}

fn code_letter(code: i16) -> char {
    let l = ROLE_LETTERS[code_role(code)] as char;
    if code_color(code) == Ink::Red {
        l
    } else {
        l.to_ascii_lowercase()
    }
}

pub fn square_name(square: u8) -> String {
    format!(
        "{}{}",
        (b'a' + square % WIDTH as u8) as char,
        square / WIDTH as u8 + 1
    )
}

pub fn parse_square(text: &str) -> Option<u8> {
    let b = text.as_bytes();
    if b.len() != 2 || !(b'a'..b'a' + WIDTH as u8).contains(&b[0]) {
        return None;
    }
    if !(b'1'..b'1' + HEIGHT as u8).contains(&b[1]) {
        return None;
    }
    Some(b[0] - b'a' + (b[1] - b'1') * WIDTH as u8)
}

pub fn move_to_uci(mv: Move) -> String {
    format!("{}{}", square_name(mv.0), square_name(mv.1))
}

pub fn uci_to_move(text: &str) -> Option<Move> {
    if text.len() != 4 || !text.is_ascii() {
        return None;
    }
    Some((parse_square(&text[..2])?, parse_square(&text[2..])?))
}

/// The engine FEN of `state` as it stands (see the module docs). A host builds
/// it for the position after the last flip or capture and sends the plies since
/// as `moves`; `window` does that split for a full move list.
pub fn engine_fen(state: &State) -> String {
    let mut pool = String::new();
    for (code, &count) in state.bag.iter().enumerate() {
        if count > 0 {
            pool.push(code_letter(code as i16));
            pool.push_str(&count.to_string());
        }
    }
    format!(
        "{} {} {}",
        state.public_fen(),
        if pool.is_empty() { "-" } else { &pool },
        state.last_move.map_or("-".to_string(), move_to_uci)
    )
}

fn seat_ink_points(scores_from: &[u32; 14], ink: Ink) -> u32 {
    (0..14)
        .filter(|&c| code_color(c as i16) == ink)
        .map(|c| scores_from[c] * POINTS[c % 7])
        .sum()
}

/// Parse an engine FEN into a full search `State`. Every field is checked
/// against the rules (counts, binding, scores against the implied captures);
/// any inconsistency is an error, never a guess.
pub fn parse_engine_fen(fen: &str) -> Result<State, String> {
    let f: Vec<&str> = fen.split_whitespace().collect();
    if f.len() != 9 {
        return Err(format!("expected 9 FEN fields, got {}", f.len()));
    }
    let mut board = [EMPTY; NSQ];
    let ranks: Vec<&str> = f[0].split('/').collect();
    if ranks.len() != HEIGHT {
        return Err("board needs 5 ranks".into());
    }
    for (i, row) in ranks.iter().enumerate() {
        let rank = HEIGHT - 1 - i;
        let mut file = 0usize;
        for ch in row.bytes() {
            if ch.is_ascii_digit() && ch != b'0' {
                file += (ch - b'0') as usize;
            } else {
                if file >= WIDTH {
                    return Err(format!("rank {} is too long", rank + 1));
                }
                board[file + rank * WIDTH] = if ch == b'X' {
                    DOWN
                } else {
                    letter_code(ch).ok_or_else(|| format!("bad board character {}", ch as char))?
                };
                file += 1;
            }
            if file > WIDTH {
                return Err(format!("rank {} is too long", rank + 1));
            }
        }
        if file != WIDTH {
            return Err(format!("rank {} does not have 9 files", rank + 1));
        }
    }
    let turn = match f[1] {
        "r" => Seat::Red,
        "b" => Seat::Black,
        _ => return Err("turn must be r or b".into()),
    };
    let first_color = match f[2] {
        "-" => None,
        "r" => Some(Ink::Red),
        "b" => Some(Ink::Black),
        _ => return Err("first colour must be r, b or -".into()),
    };
    let number = |s: &str, what: &str| s.parse::<u32>().map_err(|_| format!("bad {what}"));
    let scores = [number(f[3], "red score")?, number(f[4], "black score")?];
    let no_progress = number(f[5], "clock")?;
    let move_number = number(f[6], "move number")?;
    if no_progress > NO_PROGRESS_LIMIT {
        return Err("clock past the 40-ply limit".into());
    }
    if move_number == 0 {
        return Err("move number is 1-based".into());
    }
    let mut bag = [0u32; 14];
    if f[7] != "-" {
        let bytes = f[7].as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let code = letter_code(bytes[i]).ok_or("bad pool letter")? as usize;
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let count: u32 = f[7][start..i].parse().map_err(|_| "bad pool count")?;
            if count == 0 || bag[code] != 0 {
                return Err("pool counts must be positive and listed once".into());
            }
            bag[code] = count;
        }
    }
    let last_move = match f[8] {
        "-" => None,
        text => Some(uci_to_move(text).ok_or("bad last move")?),
    };
    // Conservation: covered tiles match the pool; nothing exceeds its start count.
    let covered = board.iter().filter(|&&p| p == DOWN).count() as u32;
    if bag.iter().sum::<u32>() != covered {
        return Err("pool does not match the covered tiles".into());
    }
    let mut captured = [0u32; 14];
    for code in 0..14 {
        let on_board = board.iter().filter(|&&p| p == code as i16).count() as u32;
        let total = START_COUNTS[code % 7];
        let seen = on_board + bag[code];
        if seen > total {
            return Err(format!("too many {}", code_letter(code as i16)));
        }
        captured[code] = total - seen;
    }
    let face_up = board.iter().any(|&p| is_piece(p));
    let any_captured = captured.iter().any(|&n| n > 0);
    let Some(red_ink) = first_color else {
        if face_up || any_captured || scores != [0, 0] {
            return Err("face-up pieces or captures before the first flip".into());
        }
        return finish(
            board,
            bag,
            None,
            turn,
            move_number,
            scores,
            no_progress,
            Vec::new(),
            last_move,
        );
    };
    // A seat scores the other ink's captured pieces (covered tiles are never captured).
    let expected = [
        seat_ink_points(&captured, red_ink.other()),
        seat_ink_points(&captured, red_ink),
    ];
    if scores != expected {
        return Err(format!(
            "scores {scores:?} disagree with the captures implied by the pool ({expected:?})"
        ));
    }
    let mut captures = Vec::new();
    for (code, &n) in captured.iter().enumerate() {
        captures.extend(std::iter::repeat_n(code as i16, n as usize));
    }
    finish(
        board,
        bag,
        first_color,
        turn,
        move_number,
        scores,
        no_progress,
        captures,
        last_move,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish(
    board: [i16; NSQ],
    bag: [u32; 14],
    first_color: Option<Ink>,
    turn: Seat,
    move_number: u32,
    scores: [u32; 2],
    no_progress: u32,
    captures: Vec<i16>,
    last_move: Option<Move>,
) -> Result<State, String> {
    if let Some((from, to)) = last_move {
        // The last mover's piece stands on `to` face up (a flip turns `from` == `to`).
        let piece = board[to as usize];
        if !is_piece(piece) || (from != to && board[from as usize] != EMPTY) {
            return Err("last move does not fit the board".into());
        }
    }
    let ply = (move_number - 1) * 2 + u32::from(turn == Seat::Black);
    let mut state = State {
        board,
        bag,
        first_color,
        ply,
        scores,
        no_progress,
        captures,
        // A flip or capture clears it; a FEN-only current position loses it.
        chased_square: None,
        history: Vec::new(),
        last_move,
        status: Status::Playing(turn),
    };
    // The position itself is the first of its counting window (apply pushes each
    // reached position; the initial position is never reached again).
    if last_move.is_some() {
        state.history.push(state.position_key());
    }
    state.status = state.result();
    Ok(state)
}

/// Parse `fen` and replay `moves` (UCI text) through the rules.
pub fn parse_position(fen: &str, moves: &[&str]) -> Result<State, String> {
    parse_position_from(parse_engine_fen(fen)?, moves)
}

/// Split a game at its last flip or capture: the state there and the plies
/// since, i.e. what a host sends as `position fen <engine_fen> moves ...`.
/// `states[i]` is the position after ply i (`states[0]` the initial one) and
/// `moves[i]` the move played from `states[i]`.
pub fn window<'a>(states: &'a [State], moves: &[Move]) -> (&'a State, Vec<String>) {
    let last = states.len() - 1;
    let start = (0..last)
        .rev()
        .find(|&i| {
            let (from, to) = moves[i];
            from == to || is_piece(states[i].board[to as usize])
        })
        .map_or(0, |i| i + 1);
    (
        &states[start],
        moves[start..last].iter().map(|&m| move_to_uci(m)).collect(),
    )
}

/// Name and version the frontend reports (`id name`).
pub const ENGINE_NAME: &str = "MistyThreeKingdomsBanqi 0.3.0";

/// One frontend session: the command loop's state, kept apart from stdin so
/// tests drive it line by line. A host typically starts one process per move.
#[derive(Default)]
pub struct Session {
    position: Option<State>,
    error: Option<String>,
    eval: Eval,
    chance_mode: crate::engine::ChanceMode,
    play: crate::engine::PlayOptions,
    seed: u64,
}

impl Session {
    /// The reply lines to one command line (possibly none). `quit` is the
    /// caller's to handle.
    pub fn handle(&mut self, line: &str) -> Vec<String> {
        let line = line.trim();
        let mut words = line.split_whitespace();
        match words.next().unwrap_or("") {
            "uci" => vec![
                format!("id name {ENGINE_NAME}"),
                "id author Mistboard".into(),
                "option name EvalFile type string default <empty>".into(),
                "option name ChanceMix type check default false".into(),
                "option name Noise type string default 0".into(),
                "option name NoiseDepth type spin default 1 min 1 max 8".into(),
                "option name Seed type string default 0".into(),
                "uciok".into(),
            ],
            "isready" => vec!["readyok".into()],
            "ucinewgame" => {
                self.position = None;
                self.error = None;
                Vec::new()
            }
            "position" => {
                self.set_position(line["position".len()..].trim());
                match &self.error {
                    Some(error) => vec![format!("info string error {error}")],
                    None => Vec::new(),
                }
            }
            "go" => self.go(words),
            "setoption" => self.set_option(line["setoption".len()..].trim()),
            _ => Vec::new(),
        }
    }

    fn set_option(&mut self, rest: &str) -> Vec<String> {
        // Split at tokens rather than spaces: paths may contain spaces, while
        // UCI permits arbitrary whitespace between the protocol words.
        let Some(rest) = rest.strip_prefix("name").map(str::trim_start) else {
            return Vec::new();
        };
        let mut end = 0;
        let mut name = Vec::new();
        let mut value = "";
        for word in rest.split_whitespace() {
            let start = end + rest[end..].find(word).unwrap();
            end = start + word.len();
            if word == "value" {
                value = rest[end..].trim();
                break;
            }
            name.push(word);
        }
        let name = name.join(" ");
        if name.eq_ignore_ascii_case("Noise") {
            if let Ok(noise) = value.parse::<f64>() {
                let play = crate::engine::PlayOptions { noise, ..self.play };
                if play.validate().is_ok() {
                    self.play = play;
                    return Vec::new();
                }
            }
            return vec!["info string invalid or unknown option".into()];
        }
        if name.eq_ignore_ascii_case("NoiseDepth") {
            if let Ok(noise_depth) = value.parse::<i32>() {
                let play = crate::engine::PlayOptions {
                    noise_depth,
                    ..self.play
                };
                if noise_depth >= 1 && play.validate().is_ok() {
                    self.play = play;
                    return Vec::new();
                }
            }
            return vec!["info string invalid or unknown option".into()];
        }
        if name.eq_ignore_ascii_case("Seed") {
            if let Ok(seed) = value.parse::<u64>() {
                self.seed = seed;
                return Vec::new();
            }
            return vec!["info string invalid or unknown option".into()];
        }
        if name.eq_ignore_ascii_case("ChanceMix") {
            use crate::engine::ChanceMode;
            self.chance_mode = match value {
                "true" => ChanceMode::mix(),
                "false" => ChanceMode::Star1,
                _ => {
                    return vec!["info string error ChanceMix: expected true or false".into()];
                }
            };
            return Vec::new();
        }
        if !name.eq_ignore_ascii_case("EvalFile") {
            return Vec::new();
        }
        // A failed replacement must never leave the previous net active.
        self.eval = Eval::Classical;
        if value.is_empty() || value == "<empty>" {
            return Vec::new();
        }
        match Net::load(value).and_then(|net| {
            if net.can_quantize() {
                Ok(net)
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "network exceeds safe int16 accumulator range",
                ))
            }
        }) {
            Ok(net) => {
                self.eval = Eval::Nnue(Arc::new(net));
                Vec::new()
            }
            Err(error) => vec![format!(
                "info string error EvalFile: {error}; using classical"
            )],
        }
    }

    fn set_position(&mut self, rest: &str) {
        let (base, moves) = match rest.split_once("moves") {
            Some((base, moves)) => (base.trim(), moves.split_whitespace().collect()),
            None => (rest.trim(), Vec::new()),
        };
        let parsed = if base == "startpos" {
            parse_position_from(State::initial(), &moves)
        } else if let Some(fen) = base.strip_prefix("fen") {
            parse_position(fen.trim(), &moves)
        } else {
            Err("position needs startpos or fen".into())
        };
        match parsed {
            Ok(state) => {
                self.position = Some(state);
                self.error = None;
            }
            Err(error) => {
                self.position = None;
                self.error = Some(error);
            }
        }
    }

    fn go<'a>(&mut self, mut words: impl Iterator<Item = &'a str>) -> Vec<String> {
        use crate::engine::{SearchConfig, Searcher, LEVEL_MOVETIME_MS};
        use std::time::{Duration, Instant};
        let mut config = SearchConfig::level(None);
        config.chance_mode = self.chance_mode;
        let mut movetime = LEVEL_MOVETIME_MS;
        let mut allowed = Vec::new();
        while let Some(word) = words.next() {
            let value = |v: Option<&str>| v.and_then(|v| v.parse::<u64>().ok());
            match word {
                "nodes" => {
                    if let Some(n) = value(words.next()).filter(|&n| n > 0) {
                        config.node_budget = n;
                    }
                }
                "movetime" => {
                    if let Some(ms) = value(words.next()) {
                        movetime = ms;
                    }
                }
                "depth" => {
                    if let Some(d) = value(words.next()).filter(|&d| d > 0) {
                        config.max_depth = d.min(u8::MAX as u64) as u8;
                    }
                }
                "searchmoves" => allowed.extend(words.by_ref().filter_map(uci_to_move)),
                _ => {}
            }
        }
        // movetime 0 = no time cap (node budget only, reproducible).
        config.time_limit = (movetime > 0).then(|| Duration::from_millis(movetime));
        let Some(state) = &self.position else {
            let why = self.error.as_deref().unwrap_or("no position");
            return vec![format!("info string error {why}"), "bestmove (none)".into()];
        };
        let started = Instant::now();
        let mut searcher = Searcher::new(config.with_eval(self.eval.clone()));
        let result = if self.play.noise == 0.0 {
            searcher.search_moves(state, &allowed)
        } else {
            // Seed=0 requests fresh noise on each search. Sample entropy only
            // when noise is enabled; nonzero seeds replay the public position.
            let seed = if self.seed == 0 {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(1)
                    .max(1)
            } else {
                self.seed
            };
            searcher.select_move_with_options(
                state,
                &allowed,
                &self.play,
                &mut crate::engine::PlayRng::for_position(seed, state),
            )
        };
        let ms = started.elapsed().as_millis();
        let cp = (result.value * 1000.0).round() as i64;
        let mut info = format!(
            "info depth {} nodes {} time {ms} score cp {cp}",
            result.completed_depth, result.nodes
        );
        match result.best_move {
            Some(mv) => {
                let text = move_to_uci(mv);
                info.push_str(&format!(" pv {text}"));
                vec![info, format!("bestmove {text}")]
            }
            None => vec![info, "bestmove (none)".into()],
        }
    }
}

fn parse_position_from(mut state: State, moves: &[&str]) -> Result<State, String> {
    for text in moves {
        let mv = uci_to_move(text).ok_or_else(|| format!("bad move {text}"))?;
        if mv.0 == mv.1 {
            return Err(format!(
                "flip {text} in the move list (its outcome is unknown)"
            ));
        }
        state = state
            .apply(mv, None)
            .map_err(|_| format!("illegal move {text}"))?;
    }
    Ok(state)
}
