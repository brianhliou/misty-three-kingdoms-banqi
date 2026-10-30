//! Rules state for Three Kingdoms banqi (三國暗棋).
//!
//! A port of Mistboard's TypeScript rules kernel for the mode. It shares
//! MistyBanqi's piece encoding, flip-as-chance interface, bag accounting and
//! one-screen cannon traversal, adapted to a 9x5 board. Banqi's rank captures,
//! repetition draw and elimination shortcuts do not apply; its perpetual-chase
//! restriction does (see `rules`). Classical search lives in `engine`, the UCI
//! contract in `uci`.
//!
//! Squares are a1=0, b1=1, ..., i5=44. Piece codes are ink*7 + role, with
//! roles general, advisor, elephant, chariot, horse, cannon, soldier (0..6).
//! A covered square is always DOWN: a state never stores its hidden identity.
//! `apply` takes a revealed code only at a flip, from the public bag. A caller
//! can replay a secret deal or enumerate chance outcomes without changing rules.
//!
//! Keep the FULL State for replay/search. The public FEN omits the bag,
//! captures, chased square and position history; it cannot restore a state.
//! Observation matches the kernel's player projection, including at game end.
//!
//! The differential test (`tests/oracle.rs`) replays games recorded from the
//! TypeScript kernel and needs only python3's standard JSON reader, no Node.

pub mod engine;
pub mod reference;
pub mod uci;

pub const WIDTH: usize = 9;
pub const HEIGHT: usize = 5;
pub const NSQ: usize = WIDTH * HEIGHT;
pub const EMPTY: i16 = -1;
pub const DOWN: i16 = -2;
pub const START_COUNTS: [u32; 7] = [1, 2, 2, 2, 2, 2, 5];
pub const POINTS: [u32; 7] = [20, 5, 5, 15, 10, 10, 5];
pub const SCORE_TARGET: u32 = 100;
pub const NO_PROGRESS_LIMIT: u32 = 40;
/// A chase move may not make its resulting position appear this many times.
pub const CHASE_REPETITION_LIMIT: u32 = 3;

/// Mistboard's rules for the mode (decided 2026-10-25), mirroring
/// THREE_KINGDOMS_BANQI_RULES in the TypeScript kernel. The original app
/// publishes no rules text for it, so these are Mistboard's rules.
pub mod rules {
    use super::{Seat, CHASE_REPETITION_LIMIT};

    /// Rule 3: covered tiles block rays, may screen cannons, and cannot be captured.
    pub const CAPTURE_COVERED: bool = false;
    /// Rule 2: a capture and a flip both reset the 40-ply clock.
    pub const CAPTURE_RESETS_CLOCK: bool = true;
    pub const FLIP_RESETS_CLOCK: bool = true;

    /// Rule 1: a chase move is refused when its resulting position would appear
    /// for the third time (`occurrences` includes this move), unless it is the
    /// only kind of move left (see `State::legal_moves`).
    pub fn chase_allowed(occurrences: u32) -> bool {
        occurrences < CHASE_REPETITION_LIMIT
    }

    /// Rule 5: no legal move is a loss for the seat to move.
    pub fn no_legal_move_winner(seat: Seat) -> Option<Seat> {
        Some(seat.other())
    }

    // Rule 4: wipe-out cannot happen. A side's sixteen pieces total 135 and its
    // thirteen cheapest total 85, so the fourteenth capture of one side reaches
    // SCORE_TARGET first. There is no NoPieces result; a board with no pieces for
    // the side to move just reaches the no-legal-move rule.
}

const ORTHO: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const DIAGONAL: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];
const HORSE: [(i32, i32); 8] = [
    (1, 2),
    (2, 1),
    (-1, 2),
    (-2, 1),
    (1, -2),
    (2, -1),
    (-1, -2),
    (-2, -1),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seat {
    Red = 0,
    Black = 1,
}

impl Seat {
    pub fn other(self) -> Self {
        match self {
            Self::Red => Self::Black,
            Self::Black => Self::Red,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    Red = 0,
    Black = 1,
}

impl Ink {
    pub fn other(self) -> Self {
        match self {
            Self::Red => Self::Black,
            Self::Black => Self::Red,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Score,
    NoProgress,
    NoLegalMove,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Playing(Seat),
    Finished {
        winner: Option<Seat>,
        reason: Reason,
    },
}

/// A flip has from == to, as in banqi_rust and the TypeScript oracle.
pub type Move = (u8, u8);

/// One counted position: the board (covered tiles as DOWN, so the deal is
/// excluded) and the parity of the ply, i.e. the seat to move. The TypeScript key
/// also carries the colour binding; it is fixed within one counting window (the
/// first flip binds it and restarts the window), so it is left out here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PositionKey {
    pub board: [i16; NSQ],
    pub parity: u8,
}

/// Complete rules state. Arrays indexed by seat are independent of piece ink.
/// Board entries must be EMPTY, DOWN or a piece code 0..13; bag counts must
/// match the covered multiset. Public fields support full-state adapters and
/// focused rules fixtures; use initial/apply to preserve invariants in play.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub board: [i16; NSQ],
    pub bag: [u32; 14],
    pub first_color: Option<Ink>,
    pub ply: u32,
    pub scores: [u32; 2],
    pub no_progress: u32,
    /// Captured piece codes (ink and role), in capture order.
    pub captures: Vec<i16>,
    /// The square of the piece the previous ply chased, if it was a chase move
    /// (a mutual chase sets it too). Read only to classify the next move as a flee.
    pub chased_square: Option<u8>,
    /// Positions reached since the last flip or capture, oldest first. A capture
    /// removes a piece and a flip removes a covered tile, so no earlier position
    /// can recur after one; within a window the scores are fixed. At most 40
    /// entries (the clock ends the game), so a linear count is cheap.
    pub history: Vec<PositionKey>,
    pub last_move: Option<Move>,
    pub status: Status,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub perspective: Seat,
    pub board: [i16; NSQ],
    pub status: Status,
    pub ply: u32,
    pub move_number: u32,
    pub first_color: Option<Ink>,
    pub scores: [u32; 2],
    pub no_progress: u32,
    pub captured: Vec<i16>,
    pub legal_moves: Vec<Move>,
    pub last_move: Option<Move>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveError {
    IllegalMove,
    /// Missing, invalid or unavailable flip outcome, or an outcome on a non-flip.
    InvalidReveal,
}

fn is_piece(code: i16) -> bool {
    (0..14).contains(&code)
}

fn code_color(code: i16) -> Ink {
    if code < 7 {
        Ink::Red
    } else {
        Ink::Black
    }
}

fn code_role(code: i16) -> usize {
    (code % 7) as usize
}

fn square_at(file: i32, rank: i32) -> Option<usize> {
    ((0..WIDTH as i32).contains(&file) && (0..HEIGHT as i32).contains(&rank))
        .then_some((file + rank * WIDTH as i32) as usize)
}

/// Ordered exactly like boardDests in the oracle, so legal_moves lists moves in
/// the oracle's order (callers that pick by index stay comparable).
fn board_dests(board: &[i16; NSQ], from: usize) -> Vec<u8> {
    let piece = board[from];
    if !is_piece(piece) {
        return Vec::new();
    }
    let role = code_role(piece);
    let (file, rank) = ((from % WIDTH) as i32, (from / WIDTH) as i32);
    let may_land = |target| {
        target == EMPTY
            || if target == DOWN {
                rules::CAPTURE_COVERED
            } else {
                code_color(target) != code_color(piece)
            }
    };
    let mut dests = Vec::new();
    // Generals are ordinary sliders: no check, palace or facing-general rule.
    if role == 0 || role == 3 || role == 5 {
        for (df, dr) in ORTHO {
            let (mut f, mut r) = (file + df, rank + dr);
            let mut screen = false;
            while let Some(to) = square_at(f, r) {
                let target = board[to];
                if role != 5 {
                    if may_land(target) {
                        dests.push(to as u8);
                    }
                    if target != EMPTY {
                        break;
                    }
                } else if !screen {
                    if target == EMPTY {
                        dests.push(to as u8);
                    } else {
                        screen = true;
                    }
                } else if target != EMPTY {
                    if may_land(target) {
                        dests.push(to as u8);
                    }
                    break;
                }
                f += df;
                r += dr;
            }
        }
    } else {
        let steps: &[(i32, i32)] = match role {
            4 => &HORSE,
            6 => &ORTHO,
            _ => &DIAGONAL,
        };
        let scale = if role == 2 { 2 } else { 1 };
        for &(df, dr) in steps {
            if let Some(to) = square_at(file + df * scale, rank + dr * scale) {
                // Horse legs and elephant eyes never block these leapers.
                if may_land(board[to]) {
                    dests.push(to as u8);
                }
            }
        }
    }
    dests
}

impl State {
    /// Thirty-two covered pieces in four 4x2 corner blocks, central cross empty.
    pub fn initial() -> Self {
        let mut board = [EMPTY; NSQ];
        for (i, square) in board.iter_mut().enumerate() {
            if i % WIDTH != 4 && i / WIDTH != 2 {
                *square = DOWN;
            }
        }
        let mut bag = [0; 14];
        bag[..7].copy_from_slice(&START_COUNTS);
        bag[7..].copy_from_slice(&START_COUNTS);
        Self {
            board,
            bag,
            first_color: None,
            ply: 0,
            scores: [0; 2],
            no_progress: 0,
            captures: Vec::new(),
            chased_square: None,
            history: Vec::new(),
            last_move: None,
            status: Status::Playing(Seat::Red),
        }
    }

    pub fn turn(&self) -> Seat {
        if self.ply.is_multiple_of(2) {
            Seat::Red
        } else {
            Seat::Black
        }
    }

    pub fn move_number(&self) -> u32 {
        self.ply / 2 + 1
    }

    pub fn ink_for_seat(&self, seat: Seat) -> Option<Ink> {
        self.first_color
            .map(|ink| if seat == Seat::Red { ink } else { ink.other() })
    }

    pub fn position_key(&self) -> PositionKey {
        PositionKey {
            board: self.board,
            parity: (self.ply % 2) as u8,
        }
    }

    /// How many times `key` has been reached in the current window.
    pub fn occurrences(&self, key: &PositionKey) -> u32 {
        self.history.iter().filter(|k| *k == key).count() as u32
    }

    // ── Perpetual chase (rule 1) ─────────────────────────────────────────────
    //
    // Ported from Mistboard banqi's 長捉 (perpetual chase) rule with this variant's capture test
    // (any face-up piece captures any face-up enemy it reaches, 9x5 geometry).
    // A CHASE MOVE is a quiet move (no flip, no capture) after which the moved
    // piece could capture the enemy piece that moved on the previous ply. A FLEE
    // is a move of the piece the previous ply chased. A move that is both (一捉一)
    // is ordinary. A chase move that would make its resulting position appear a
    // third time is illegal unless the side to move has no other legal move
    // (flips count). Only the chaser is limited.
    //
    // NO repetition draw: banqi's second clause is not imported. An ordinary
    // repetition runs on until the 40-ply clock settles the game on score;
    // positions are counted only to enforce the chase restriction.

    /// If `mv` (pseudo-legal) is a chase move, the square of the piece it chases.
    pub fn chase_target(&self, mv: Move) -> Option<u8> {
        let (from, to) = (mv.0 as usize, mv.1 as usize);
        if from == to || self.board[to] != EMPTY {
            return None;
        }
        let last = self.last_move.filter(|m| m.0 != m.1)?;
        let mut board = self.board;
        board[to] = board[from];
        board[from] = EMPTY;
        // board_dests lands only on empty points and face-up enemies, so a hit
        // on the occupied last.1 is exactly "could capture it".
        board_dests(&board, to).contains(&last.1).then_some(last.1)
    }

    /// True when `mv` is a chase move and not also a flee (a mutual chase is ordinary).
    pub fn is_chase(&self, mv: Move) -> bool {
        let chases = self.chase_target(mv).is_some();
        let flees = self.chased_square == Some(mv.0);
        chases && !flees
    }

    fn chase_forbidden(&self, mv: Move) -> bool {
        if !self.is_chase(mv) {
            return false;
        }
        let mut board = self.board;
        board[mv.1 as usize] = board[mv.0 as usize];
        board[mv.0 as usize] = EMPTY;
        let key = PositionKey {
            board,
            parity: ((self.ply + 1) % 2) as u8,
        };
        !rules::chase_allowed(self.occurrences(&key) + 1)
    }

    /// Every move before the chase restriction, in oracle order.
    fn pseudo_legal_moves(&self) -> Vec<Move> {
        if matches!(self.status, Status::Finished { .. }) {
            return Vec::new();
        }
        let ink = self.ink_for_seat(self.turn());
        let mut moves = Vec::new();
        for (from, &piece) in self.board.iter().enumerate() {
            if piece == DOWN {
                moves.push((from as u8, from as u8));
            } else if is_piece(piece) && Some(code_color(piece)) == ink {
                for to in board_dests(&self.board, from) {
                    moves.push((from as u8, to));
                }
            }
        }
        moves
    }

    /// True when any move exists before the chase restriction.
    fn has_pseudo_legal_move(&self) -> bool {
        if matches!(self.status, Status::Finished { .. }) {
            return false;
        }
        let ink = self.ink_for_seat(self.turn());
        self.board.iter().enumerate().any(|(from, &piece)| {
            piece == DOWN
                || (is_piece(piece)
                    && Some(code_color(piece)) == ink
                    && !board_dests(&self.board, from).is_empty())
        })
    }

    /// The moves rule 1 takes away right now: empty when none, or when they are
    /// the side's only moves and so stay legal.
    pub fn forbidden_chase_moves(&self) -> Vec<Move> {
        let moves = self.pseudo_legal_moves();
        let forbidden: Vec<_> = moves
            .iter()
            .copied()
            .filter(|&m| self.chase_forbidden(m))
            .collect();
        if forbidden.len() < moves.len() {
            forbidden
        } else {
            Vec::new()
        }
    }

    pub fn legal_moves(&self) -> Vec<Move> {
        let moves = self.pseudo_legal_moves();
        let allowed: Vec<_> = moves
            .iter()
            .copied()
            .filter(|&m| !self.chase_forbidden(m))
            .collect();
        // The only-move exception: if repeating the chase is all the side can do,
        // it stays legal.
        if allowed.is_empty() {
            moves
        } else {
            allowed
        }
    }

    pub fn result(&self) -> Status {
        if matches!(self.status, Status::Finished { .. }) {
            return self.status;
        }
        for seat in [Seat::Red, Seat::Black] {
            if self.scores[seat as usize] >= SCORE_TARGET {
                return Status::Finished {
                    winner: Some(seat),
                    reason: Reason::Score,
                };
            }
        }
        if self.no_progress >= NO_PROGRESS_LIMIT {
            let winner = match self.scores[0].cmp(&self.scores[1]) {
                std::cmp::Ordering::Greater => Some(Seat::Red),
                std::cmp::Ordering::Less => Some(Seat::Black),
                std::cmp::Ordering::Equal => None,
            };
            return Status::Finished {
                winner,
                reason: Reason::NoProgress,
            };
        }
        // Rule 4: no elimination check (see `rules`). The only-move exception
        // makes `legal_moves` empty exactly when no pseudo-legal move exists, so
        // the cheaper test skips the chase classification.
        if !self.has_pseudo_legal_move() {
            return Status::Finished {
                winner: rules::no_legal_move_winner(self.turn()),
                reason: Reason::NoLegalMove,
            };
        }
        Status::Playing(self.turn())
    }

    /// Return a new state; invalid actions leave the original untouched.
    /// Reveals are checked against the bag before any mutation.
    pub fn apply(&self, mv: Move, reveal: Option<i16>) -> Result<Self, MoveError> {
        if !self.legal_moves().contains(&mv) {
            return Err(MoveError::IllegalMove);
        }
        self.apply_legal(mv, reveal)
    }

    /// `apply` without the legality check, for a search that takes `mv` from
    /// this state's own `legal_moves()`. The reveal is still validated. An
    /// illegal `mv` here is a caller bug the rules do not catch.
    pub fn apply_legal(&self, mv: Move, reveal: Option<i16>) -> Result<Self, MoveError> {
        let flip = mv.0 == mv.1;
        if flip {
            if !reveal.is_some_and(|c| is_piece(c) && self.bag[c as usize] > 0) {
                return Err(MoveError::InvalidReveal);
            }
        } else if reveal.is_some() {
            return Err(MoveError::InvalidReveal);
        }
        let seat = self.turn();
        let other = seat.other();
        let (from, to) = (mv.0 as usize, mv.1 as usize);
        let target = if flip { EMPTY } else { self.board[to] };
        let irreversible = flip || is_piece(target);
        let mut next = self.clone();
        // Read before the board changes: what this move chases.
        next.chased_square = if irreversible {
            None
        } else {
            self.chase_target(mv)
        };
        if irreversible {
            // A flip or capture can never be undone: a fresh repetition window.
            next.history.clear();
        }
        if flip {
            let piece = reveal.unwrap(); // validated above
            next.board[from] = piece;
            next.bag[piece as usize] -= 1;
            // Bind the FIRST seat's ink even if the first flip occurs later.
            if next.first_color.is_none() {
                next.first_color = Some(code_color(piece));
            }
        } else {
            next.board[to] = next.board[from];
            next.board[from] = EMPTY;
            if is_piece(target) {
                next.scores[seat as usize] += POINTS[code_role(target)];
                next.captures.push(target);
            }
        }
        next.no_progress = if (flip && rules::FLIP_RESETS_CLOCK)
            || (is_piece(target) && rules::CAPTURE_RESETS_CLOCK)
        {
            0
        } else {
            self.no_progress + 1
        };
        next.ply += 1;
        next.last_move = Some(mv);
        next.status = Status::Playing(other);
        next.history.push(next.position_key());
        next.status = next.result();
        Ok(next)
    }

    pub fn observation(&self, perspective: Seat) -> Observation {
        Observation {
            perspective,
            board: self.board,
            status: self.status,
            ply: self.ply,
            move_number: self.move_number(),
            first_color: self.first_color,
            scores: self.scores,
            no_progress: self.no_progress,
            captured: self.captures.clone(),
            last_move: self.last_move,
            legal_moves: if perspective == self.turn() {
                self.legal_moves()
            } else {
                Vec::new()
            },
        }
    }

    /// Exact kernel projection. This deliberately is NOT a full-state save format.
    pub fn public_fen(&self) -> String {
        let mut rows = Vec::new();
        for rank in (0..HEIGHT).rev() {
            let mut row = String::new();
            let mut empty = 0;
            for file in 0..WIDTH {
                let piece = self.board[file + rank * WIDTH];
                if piece == EMPTY {
                    empty += 1;
                    continue;
                }
                if empty > 0 {
                    row.push_str(&empty.to_string());
                    empty = 0;
                }
                row.push(if piece == DOWN {
                    'X'
                } else {
                    let c = b"GAERHCS"[code_role(piece)] as char;
                    if code_color(piece) == Ink::Red {
                        c
                    } else {
                        c.to_ascii_lowercase()
                    }
                });
            }
            if empty > 0 {
                row.push_str(&empty.to_string());
            }
            rows.push(row);
        }
        let first = match self.first_color {
            None => '-',
            Some(Ink::Red) => 'r',
            Some(Ink::Black) => 'b',
        };
        format!(
            "{} {} {} {} {} {} {}",
            rows.join("/"),
            if self.turn() == Seat::Red { 'r' } else { 'b' },
            first,
            self.scores[0],
            self.scores[1],
            self.no_progress,
            self.move_number()
        )
    }
}
