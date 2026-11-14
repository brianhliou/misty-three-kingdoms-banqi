//! Reference search: the first version of the engine, kept frozen. `engine` is
//! this search made faster, and `tests/candidate.rs` holds the two equal on best
//! move, value, depth, nodes, chance cutoffs and TT hits over game positions.
//! Search improvements go in `engine.rs`; correctness fixes apply to both.
//! It shares the rules in `lib.rs`, checked by the oracle replay and
//! `tests/candidate.rs`.

//! Iterative deepening, alpha-beta, Star1 and capture quiescence, adapted from
//! MistyBanqi's engine. No Banqi rank values, weights or repetition draws.
//!
//! Values are seat-to-move utilities in [-1, 1]. Terminal results are exact;
//! heuristics stay strictly inside that interval, the global Star1 bounds.
//! Search consumes the FULL public State (including bag, scores, clock, chased
//! square, position history and last move), never a deal or the lossy public FEN.
//! Every move comes from `State::legal_moves`, so a chase the repetition rule
//! refuses is never searched and the only-legal-move exception is the rules'
//! own. Repetition has no special search value (`cycle_value` returns `None`):
//! play continues until the rules produce an actual terminal result.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::{
    board_dests, code_color, code_role, is_piece, Move, PositionKey, State, Status, NSQ, POINTS,
};

/// Untuned movement potential for this 9x5 geometry: sliders outrank leapers,
/// cannons need a screen, and soldiers have only four adjacent destinations.
/// These are independent of capture POINTS and of Banqi's rank hierarchy.
const MOVEMENT: [f64; 7] = [14.0, 3.0, 4.0, 14.0, 8.0, 10.0, 2.0];
const MIN: f64 = -1.0;
const MAX: f64 = 1.0;

/// Static utility for the seat to move. Score margin dominates the modest
/// remaining movement/mobility terms. Covered potential uses only bag counts.
/// These coefficients are an initial heuristic, not a measured strength claim.
pub fn evaluate(state: &State) -> f64 {
    if let Some(value) = terminal(state) {
        return value;
    }
    heuristic(state)
}

fn terminal(state: &State) -> Option<f64> {
    match state.result() {
        Status::Finished { winner: None, .. } => Some(0.0),
        Status::Finished {
            winner: Some(winner),
            ..
        } => Some(if winner == state.turn() { MAX } else { MIN }),
        Status::Playing(_) => None,
    }
}

/// Repetition supplies no special search value in Three Kingdoms banqi.
/// Play continues until an actual terminal result, including the 40-ply
/// no-progress settlement. Move generation alone enforces the chase limit.
/// Kept as a public hook for callers that check for cycle adjudication.
pub fn cycle_value(_state: &State) -> Option<f64> {
    None
}

fn heuristic(state: &State) -> f64 {
    let seat = state.turn();
    let margin = state.scores[seat as usize] as f64 - state.scores[seat.other() as usize] as f64;
    let Some(ink) = state.ink_for_seat(seat) else {
        return (margin / 135.0).clamp(-0.95, 0.95);
    };
    let mut movement = 0.0;
    let mut mobility = 0.0;
    for (code, &count) in state.bag.iter().enumerate() {
        let sign = if code_color(code as i16) == ink {
            1.0
        } else {
            -1.0
        };
        movement += sign * count as f64 * MOVEMENT[code % 7];
    }
    for (square, &code) in state.board.iter().enumerate() {
        if is_piece(code) {
            let sign = if code_color(code) == ink { 1.0 } else { -1.0 };
            movement += sign * MOVEMENT[code_role(code)];
            mobility += sign * board_dests(&state.board, square).len() as f64;
        }
    }
    ((margin + 0.35 * movement + 0.15 * mobility) / 135.0).clamp(-0.95, 0.95)
}

#[derive(Clone, Debug)]
pub struct SearchConfig {
    /// Completed decision plies; zero requests only the quiescent leaf value.
    pub max_depth: u8,
    /// Hard total node cap across all iterative-deepening iterations.
    pub node_budget: u64,
    pub time_limit: Option<Duration>,
    pub quiescence_depth: u8,
    /// Zero disables the transposition table. Entries never outlive a search.
    pub tt_capacity: usize,
    /// Disable to compare exhaustive chance enumeration against Star1.
    pub star1: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            max_depth: 6,
            node_budget: 100_000,
            time_limit: Some(Duration::from_millis(500)),
            quiescence_depth: 4,
            tt_capacity: 32_768,
            star1: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub best_move: Option<Move>,
    pub value: f64,
    pub completed_depth: u8,
    pub nodes: u64,
    pub chance_cutoffs: u64,
    pub tt_hits: u64,
    /// False when the last attempted iteration exceeded either budget.
    pub completed: bool,
}

/// Equality-checked keys preserve every rule input, including the clock, the
/// chased square and the positions since the last flip or capture (they decide
/// which chases are legal and which nodes are cycles). Capture order and absolute
/// ply do not affect the rules; captured ink counts and the seat to move do. No
/// lossy Zobrist-only matches.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    board: [i16; NSQ],
    bag: [u32; 14],
    first_color: Option<u8>,
    turn: u8,
    scores: [u32; 2],
    clock: u32,
    captured: [u32; 2],
    chased_square: Option<u8>,
    history: Vec<PositionKey>,
    last_move: Option<Move>,
}

impl From<&State> for Key {
    fn from(state: &State) -> Self {
        let mut captured = [0; 2];
        for &code in &state.captures {
            captured[code_color(code) as usize] += 1;
        }
        Self {
            board: state.board,
            bag: state.bag,
            first_color: state.first_color.map(|ink| ink as u8),
            turn: state.turn() as u8,
            scores: state.scores,
            clock: state.no_progress,
            captured,
            chased_square: state.chased_square,
            history: state.history.clone(),
            last_move: state.last_move,
        }
    }
}

#[derive(Clone, Copy)]
enum Bound {
    Exact,
    Lower,
    Upper,
}

#[derive(Clone, Copy)]
struct Entry {
    depth: u8,
    value: f64,
    bound: Bound,
    best: Option<Move>,
}

#[derive(Default)]
pub struct Searcher {
    pub config: SearchConfig,
    tt: HashMap<Key, Entry>,
    nodes: u64,
    chance_cutoffs: u64,
    tt_hits: u64,
    start: Option<Instant>,
    root_ply: u32,
}

type NodeResult = Result<(f64, Option<Move>), ()>;

impl Searcher {
    pub fn new(config: SearchConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    /// Return the last COMPLETE iteration, or a deterministic legal fallback if
    /// even depth one is interrupted. No partial chance sum is ever committed.
    /// Requires a valid State (covered count agrees with the bag).
    pub fn search(&mut self, state: &State) -> SearchResult {
        self.tt.clear();
        self.nodes = 0;
        self.chance_cutoffs = 0;
        self.tt_hits = 0;
        self.start = Some(Instant::now());
        self.root_ply = state.ply;
        let value = evaluate(state);
        let finished = terminal(state).is_some();
        let mut result = SearchResult {
            best_move: if finished {
                None
            } else {
                state.legal_moves().first().copied()
            },
            value,
            completed_depth: 0,
            nodes: 0,
            chance_cutoffs: 0,
            tt_hits: 0,
            completed: true,
        };
        if !finished {
            let first = u8::from(self.config.max_depth > 0);
            for depth in first..=self.config.max_depth {
                match self.node(state, depth, self.config.quiescence_depth, MIN, MAX) {
                    Ok((value, best)) => {
                        result.value = value;
                        result.best_move = best.or(result.best_move);
                        result.completed_depth = depth;
                        if value.abs() == MAX {
                            break;
                        }
                    }
                    Err(()) => {
                        result.completed = false;
                        break;
                    }
                }
            }
        }
        result.nodes = self.nodes;
        result.chance_cutoffs = self.chance_cutoffs;
        result.tt_hits = self.tt_hits;
        result
    }

    fn tick(&mut self) -> Result<(), ()> {
        if self.nodes >= self.config.node_budget
            || self
                .config
                .time_limit
                .is_some_and(|limit| self.start.unwrap().elapsed() >= limit)
        {
            return Err(());
        }
        self.nodes += 1;
        Ok(())
    }

    fn node(
        &mut self,
        state: &State,
        depth: u8,
        qleft: u8,
        mut alpha: f64,
        beta: f64,
    ) -> NodeResult {
        self.tick()?;
        if let Some(value) = terminal(state) {
            return Ok((value, None));
        }
        if state.ply > self.root_ply {
            if let Some(value) = cycle_value(state) {
                return Ok((value, None));
            }
        }
        if depth == 0 && qleft == 0 {
            return Ok((heuristic(state), None));
        }
        let original_alpha = alpha;
        let key = (depth > 0 && self.config.tt_capacity > 0).then(|| Key::from(state));
        let mut tt_move = None;
        if let Some(entry) = key.as_ref().and_then(|k| self.tt.get(k)).copied() {
            tt_move = entry.best;
            // Equal depth preserves fixed-horizon values in differential tests.
            if entry.depth == depth {
                self.tt_hits += 1;
                let usable = match entry.bound {
                    Bound::Exact => true,
                    Bound::Lower => entry.value >= beta,
                    Bound::Upper => entry.value <= alpha,
                };
                if usable {
                    return Ok((entry.value, entry.best));
                }
            }
        }
        // Stand pat is only a capture-quiescence approximation. At clock 39 a
        // player MUST act: include quiet moves and flips, with no fictitious pass.
        let stand_pat = depth == 0 && state.no_progress + 1 < crate::NO_PROGRESS_LIMIT;
        let mut best = if stand_pat {
            heuristic(state)
        } else {
            f64::NEG_INFINITY
        };
        let mut best_move = None;
        if best >= beta {
            return Ok((best, None));
        }
        alpha = alpha.max(best);
        let mut moves = state.legal_moves();
        if stand_pat {
            moves.retain(|&(from, to)| from != to && is_piece(state.board[to as usize]));
        }
        moves.sort_by_cached_key(|&mv| {
            let capture = mv.0 != mv.1 && is_piece(state.board[mv.1 as usize]);
            let points = if capture {
                POINTS[code_role(state.board[mv.1 as usize])]
            } else {
                0
            };
            std::cmp::Reverse((Some(mv) == tt_move, capture, points, mv.0 == mv.1))
        });
        for mv in moves {
            let value = self.move_value(state, mv, depth, qleft, alpha, beta)?;
            if value > best {
                best = value;
                best_move = Some(mv);
            }
            alpha = alpha.max(best);
            if alpha >= beta {
                break;
            }
        }
        if let Some(key) = key {
            let bound = if best <= original_alpha {
                Bound::Upper
            } else if best >= beta {
                Bound::Lower
            } else {
                Bound::Exact
            };
            if self.tt.len() >= self.config.tt_capacity {
                self.tt.clear();
            }
            self.tt.insert(
                key,
                Entry {
                    depth,
                    value: best,
                    bound,
                    best: best_move,
                },
            );
        }
        Ok((best, best_move))
    }

    fn move_value(
        &mut self,
        state: &State,
        mv: Move,
        depth: u8,
        qleft: u8,
        alpha: f64,
        beta: f64,
    ) -> Result<f64, ()> {
        let next_depth = depth.saturating_sub(1);
        let next_q = if depth == 0 { qleft - 1 } else { qleft };
        if mv.0 != mv.1 {
            let child = state.apply(mv, None).expect("generated legal move");
            return Ok(-self.node(&child, next_depth, next_q, -beta, -alpha)?.0);
        }
        let total: u32 = state.bag.iter().sum();
        assert!(total > 0, "covered squares require a nonempty public bag");
        let mut remaining = total;
        let mut sum = 0.0;
        for (code, &count) in state.bag.iter().enumerate() {
            if count == 0 {
                continue;
            }
            remaining -= count;
            let p = count as f64 / total as f64;
            let rem = remaining as f64 / total as f64;
            // Star1: unexplored outcomes stay in [MIN, MAX]. Transform the
            // expectation window into this outcome's window before negating.
            let (a, b) = if self.config.star1 {
                ((alpha - sum - rem * MAX) / p, (beta - sum - rem * MIN) / p)
            } else {
                (MIN - 1.0, MAX + 1.0)
            };
            if a >= MAX || b <= MIN {
                self.chance_cutoffs += 1;
                return Ok(if a >= MAX { alpha } else { beta });
            }
            let child = state
                .apply(mv, Some(code as i16))
                .expect("public bag outcome");
            let value = -self
                .node(&child, next_depth, next_q, -b.min(MAX), -a.max(MIN))?
                .0;
            if value <= a {
                self.chance_cutoffs += 1;
                return Ok(alpha);
            }
            if value >= b {
                self.chance_cutoffs += 1;
                return Ok(beta);
            }
            sum += p * value;
        }
        Ok(sum.clamp(MIN, MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ink;

    #[test]
    fn narrow_star1_bounds_do_not_poison_full_window_tt_results() {
        let mut state = State {
            board: [crate::EMPTY; NSQ],
            bag: [0; 14],
            first_color: Some(Ink::Red),
            ..State::initial()
        };
        state.board[0] = 6;
        state.board[44] = 13;
        for square in [12, 22, 32] {
            state.board[square] = crate::DOWN;
        }
        state.bag[3] = 2;
        state.bag[13] = 1;
        let config = SearchConfig {
            max_depth: 3,
            node_budget: u64::MAX,
            time_limit: None,
            quiescence_depth: 0,
            ..SearchConfig::default()
        };
        let exact = Searcher::new(config.clone()).search(&state).value;
        for (alpha, beta) in [
            (-0.9, -0.8),
            (-0.1, 0.1),
            (0.8, 0.9),
            (exact - 0.01, exact + 0.01),
            (exact, exact + 0.01),
            (exact - 0.01, exact),
        ] {
            let mut searcher = Searcher::new(config.clone());
            searcher.start = Some(Instant::now());
            let bound = searcher.node(&state, 3, 0, alpha, beta).unwrap().0;
            if exact <= alpha {
                assert!(bound <= alpha + 1e-10);
            } else if exact >= beta {
                assert!(bound >= beta - 1e-10);
            } else {
                assert!((bound - exact).abs() < 1e-10);
            }
            let full = searcher.node(&state, 3, 0, MIN, MAX).unwrap().0;
            assert!((full - exact).abs() < 1e-10);
            let before = searcher.nodes;
            assert_eq!(searcher.node(&state, 3, 0, MIN, MAX).unwrap().0, full);
            assert_eq!(
                searcher.nodes,
                before + 1,
                "exact TT hit avoids descendants"
            );
            assert!(searcher.tt_hits > 0);
        }
    }

    #[test]
    fn tt_key_retains_all_rule_context() {
        let state = State::initial();
        let key = Key::from(&state);
        let mut variants = Vec::new();
        let mut changed = state.clone();
        changed.bag.swap(0, 6);
        variants.push(changed);
        let mut changed = state.clone();
        changed.first_color = Some(Ink::Black);
        variants.push(changed);
        let mut changed = state.clone();
        changed.ply += 1;
        variants.push(changed);
        let mut changed = state.clone();
        changed.scores[0] = 5;
        variants.push(changed);
        let mut changed = state.clone();
        changed.no_progress = 39;
        variants.push(changed);
        let mut changed = state.clone();
        changed.captures.push(0);
        variants.push(changed);
        let mut changed = state.clone();
        changed.chased_square = Some(4);
        variants.push(changed);
        let mut changed = state.clone();
        changed.history.push(state.position_key());
        variants.push(changed);
        let mut changed = state;
        changed.last_move = Some((0, 1));
        variants.push(changed);
        for variant in variants {
            assert_ne!(key, Key::from(&variant));
        }
    }
}
