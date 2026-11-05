//! Iterative deepening, alpha-beta, Star1 and capture quiescence, adapted from
//! MistyBanqi's engine. No Banqi rank values, weights or repetition draws.
//!
//! Classical search with ChanceMix off stays node-for-node equal to `reference`.
//! ChanceMix and NNUE evaluation are opt-in through the search configuration.
//!
//! Values are seat-to-move utilities in [-1, 1]. Terminal results are exact;
//! heuristics stay strictly inside that interval, the global Star1 bounds.
//! Search consumes the FULL public State (including bag, scores, clock, chased
//! square, position history and last move), never a deal or the lossy public FEN.
//! Every move comes from `State::legal_moves`, so a chase the repetition rule
//! refuses is never searched and the only-legal-move exception is the rules'
//! own. Repetition never draws in this game; a position that repeats inside the
//! search is scored by `cycle_value` (see there).

use std::collections::HashMap;
use std::sync::Arc;
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

/// The default node budget and latency cap for a move.
pub const LEVEL_NODES: u64 = 2_000_000;
pub const LEVEL_MOVETIME_MS: u64 = 8_000;
pub const LEVEL_MAX_DEPTH: u8 = 64;
pub const LEVEL_QUIESCENCE_DEPTH: u8 = 4;
pub const LEVEL_TT_CAPACITY: usize = 262_144;

impl SearchConfig {
    /// The shipped level, with an optional time cap (`None` for matches).
    pub fn level(time_limit: Option<Duration>) -> Self {
        Self {
            max_depth: LEVEL_MAX_DEPTH,
            node_budget: LEVEL_NODES,
            time_limit,
            quiescence_depth: LEVEL_QUIESCENCE_DEPTH,
            tt_capacity: LEVEL_TT_CAPACITY,
            star1: true,
            chance_mode: ChanceMode::Star1,
        }
    }
}

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

/// `terminal` for a state made by `apply`/`apply_legal`, whose status is
/// already `result()`: no move generation.
fn stored_terminal(state: &State) -> Option<f64> {
    match state.status {
        Status::Finished { winner: None, .. } => Some(0.0),
        Status::Finished {
            winner: Some(winner),
            ..
        } => Some(if winner == state.turn() { MAX } else { MIN }),
        Status::Playing(_) => None,
    }
}

/// Value of a position the search reached again (it already occurred in the
/// current window, i.e. since the last flip or capture), for the seat to move.
///
/// Three Kingdoms banqi has no repetition draw. If both sides keep to a cycle,
/// no flip or capture happens, the scores cannot change, and the 40-ply clock
/// ends the game on the score margin: the side ahead wins, equal scores draw.
/// So a cycle is scored exactly as that clock settlement, from the side to
/// move's margin, the way a chess search scores a repetition as the draw it
/// would become. A side that would lose the settlement breaks the cycle at an
/// earlier node (with a flip, a capture or a fresh square) if it can. Which
/// chases may enter a cycle is movegen's call, never this function's. Never
/// applied at the search root, which is searched whatever its history.
pub fn cycle_value(state: &State) -> Option<f64> {
    if state.occurrences(&state.position_key()) < 2 {
        return None;
    }
    let seat = state.turn();
    let mine = state.scores[seat as usize];
    let theirs = state.scores[seat.other() as usize];
    Some(match mine.cmp(&theirs) {
        std::cmp::Ordering::Greater => MAX,
        std::cmp::Ordering::Less => MIN,
        std::cmp::Ordering::Equal => 0.0,
    })
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

/// Opt-in evaluation; plain SearchConfig remains source compatible and classical.
#[derive(Clone, Debug, Default)]
pub enum Eval {
    #[default]
    Classical,
    Nnue(Arc<crate::nnue::Net>),
    /// Float full-refresh oracle for tests and offline benchmarks.
    NnueReference(Arc<crate::nnue::Net>),
}
impl Eval {
    pub fn heuristic(&self, state: &State) -> f64 {
        match self {
            Self::Classical => heuristic(state),
            Self::Nnue(net) => net.utility(state),
            Self::NnueReference(net) => net.utility_reference(state),
        }
    }
    pub fn evaluate(&self, state: &State) -> f64 {
        terminal(state).unwrap_or_else(|| self.heuristic(state))
    }
}
pub struct EvalConfig {
    pub search: SearchConfig,
    pub eval: Eval,
}
impl SearchConfig {
    pub fn with_eval(self, eval: Eval) -> EvalConfig {
        EvalConfig { search: self, eval }
    }
}
impl From<SearchConfig> for EvalConfig {
    fn from(search: SearchConfig) -> Self {
        search.with_eval(Eval::Classical)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChanceMode {
    #[default]
    Star1,
    Mix {
        deep: usize,
        reduction: u8,
    },
}

impl ChanceMode {
    pub const fn mix() -> Self {
        Self::Mix {
            deep: 1,
            reduction: 6,
        }
    }
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
    pub chance_mode: ChanceMode,
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
            chance_mode: ChanceMode::Star1,
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
    eval: Eval,
    tt: HashMap<Key, Entry>,
    nodes: u64,
    chance_cutoffs: u64,
    tt_hits: u64,
    start: Option<Instant>,
    root_ply: u32,
    /// Root moves allowed by `search_moves` (UCI `searchmoves`); empty = all.
    root_moves: Vec<Move>,
    accumulators: Vec<crate::nnue::Accumulator>,
    #[cfg(test)]
    unguarded_mix_bound: bool,
}

type NodeResult = Result<(f64, Option<Move>), ()>;

impl Searcher {
    pub fn new(config: impl Into<EvalConfig>) -> Self {
        let config = config.into();
        Self {
            config: config.search,
            eval: config.eval,
            ..Self::default()
        }
    }

    /// Return the last COMPLETE iteration, or a deterministic legal fallback if
    /// even depth one is interrupted. No partial chance sum is ever committed.
    /// Requires a valid State (covered count agrees with the bag).
    pub fn search(&mut self, state: &State) -> SearchResult {
        self.search_moves(state, &[])
    }

    /// `search`, choosing only among `allowed` at the root (the tree below is
    /// not filtered). An empty list, or one with no legal move, searches all.
    pub fn search_moves(&mut self, state: &State, allowed: &[Move]) -> SearchResult {
        let legal = state.legal_moves();
        self.root_moves = legal
            .iter()
            .copied()
            .filter(|mv| allowed.contains(mv))
            .collect();
        self.tt.clear();
        self.nodes = 0;
        self.chance_cutoffs = 0;
        self.tt_hits = 0;
        self.start = Some(Instant::now());
        self.root_ply = state.ply;
        if let Eval::Nnue(net) = &self.eval {
            if net.can_quantize() {
                let plies =
                    self.config.max_depth as usize + self.config.quiescence_depth as usize + 2;
                self.accumulators.resize_with(plies, Default::default);
                net.refresh(state, &mut self.accumulators[0]);
            }
        }
        let value = terminal(state).unwrap_or_else(|| self.leaf_heuristic(state));
        let finished = terminal(state).is_some();
        let mut result = SearchResult {
            best_move: if finished {
                None
            } else {
                self.root_moves.first().or(legal.first()).copied()
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

    fn update_accumulator(&mut self, state: &State) {
        if let Eval::Nnue(net) = &self.eval {
            if net.can_quantize() {
                let ply = (state.ply - self.root_ply) as usize;
                if ply == 0 {
                    // Root was refreshed before iterative deepening.
                    return;
                }
                let (ancestors, descendants) = self.accumulators.split_at_mut(ply);
                net.update(&ancestors[ply - 1], state, &mut descendants[0]);
            }
        }
    }

    fn leaf_heuristic(&self, state: &State) -> f64 {
        if let Eval::Nnue(net) = &self.eval {
            if net.can_quantize() {
                return net.utility_accumulator(
                    &self.accumulators[(state.ply - self.root_ply) as usize],
                    state.turn(),
                );
            }
        }
        self.eval.heuristic(state)
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

    fn node(&mut self, state: &State, depth: u8, qleft: u8, alpha: f64, beta: f64) -> NodeResult {
        self.node_with_probe(state, depth, qleft, alpha, beta, false)
    }

    fn node_with_probe(
        &mut self,
        state: &State,
        depth: u8,
        qleft: u8,
        mut alpha: f64,
        beta: f64,
        probe: bool,
    ) -> NodeResult {
        self.tick()?;
        let below_root = state.ply > self.root_ply;
        // A child comes from `apply_legal`, which stores `result()` as its status.
        let finished = if below_root {
            stored_terminal(state)
        } else {
            terminal(state)
        };
        if let Some(value) = finished {
            return Ok((value, None));
        }
        if below_root {
            if let Some(value) = cycle_value(state) {
                return Ok((value, None));
            }
        }
        self.update_accumulator(state);
        if depth == 0 && qleft == 0 {
            return Ok((self.leaf_heuristic(state), None));
        }
        let original_alpha = alpha;
        let key = (depth > 0 && self.config.tt_capacity > 0).then(|| Key::from(state));
        let mut tt_move = None;
        if let Some(entry) = key.as_ref().and_then(|k| self.tt.get(k)).copied() {
            if !probe || matches!(entry.bound, Bound::Exact) {
                tt_move = entry.best;
            }
            // Equal depth preserves fixed-horizon values in differential tests.
            if if probe {
                entry.depth >= depth && matches!(entry.bound, Bound::Exact)
            } else {
                entry.depth == depth
            } {
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
            self.leaf_heuristic(state)
        } else {
            f64::NEG_INFINITY
        };
        let mut best_move = None;
        if best >= beta {
            return Ok((best, None));
        }
        alpha = alpha.max(best);
        let mut moves = state.legal_moves();
        if !below_root && !self.root_moves.is_empty() {
            moves.retain(|mv| self.root_moves.contains(mv));
        }
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
            let child = state.apply_legal(mv, None).expect("generated legal move");
            return Ok(-self.node(&child, next_depth, next_q, -beta, -alpha)?.0);
        }
        if let ChanceMode::Mix { .. } = self.config.chance_mode {
            return self.mix_value(state, mv, next_depth, next_q, alpha, beta);
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
                .apply_legal(mv, Some(code as i16))
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

    fn mix_value(
        &mut self,
        state: &State,
        mv: Move,
        depth: u8,
        qleft: u8,
        alpha: f64,
        beta: f64,
    ) -> Result<f64, ()> {
        let ChanceMode::Mix { deep, reduction } = self.config.chance_mode else {
            unreachable!()
        };
        let total = state.bag.iter().sum::<u32>();
        assert!(total > 0, "covered squares require a nonempty public bag");
        let identities = mix_identities(&state.bag);
        let mut values = [0.0; 14];
        for &code in &identities {
            let child = state
                .apply_legal(mv, Some(code as i16))
                .expect("public bag outcome");
            values[code] = -self
                .node_with_probe(
                    &child,
                    depth.saturating_sub(reduction),
                    qleft,
                    MIN,
                    MAX,
                    true,
                )?
                .0;
        }
        let mean = |values: &[f64; 14]| {
            identities
                .iter()
                .map(|&code| state.bag[code] as f64 / total as f64 * values[code])
                .sum::<f64>()
        };
        if depth == 0 {
            return Ok(mean(&values).clamp(MIN, MAX));
        }
        let selected = deep.max(1).min(identities.len());
        let mut sum = identities[selected..]
            .iter()
            .map(|&code| state.bag[code] as f64 / total as f64 * values[code])
            .sum::<f64>();
        let mut remaining: u32 = identities[..selected]
            .iter()
            .map(|&code| state.bag[code])
            .sum();
        // Least numerous first, as in jqe cvValue. Pending deep identities
        // need global bounds, not their probes, until their replacements are exact.
        for &code in identities[..selected].iter().rev() {
            remaining -= state.bag[code];
            let p = state.bag[code] as f64 / total as f64;
            let rem = remaining as f64 / total as f64;
            let a = (alpha - sum - rem * MAX) / p;
            let b = (beta - sum - rem * MIN) / p;
            if a >= MAX || b <= MIN {
                self.chance_cutoffs += 1;
                return Ok(if a >= MAX { alpha } else { beta });
            }
            let child = state
                .apply_legal(mv, Some(code as i16))
                .expect("public bag outcome");
            let mut ca = a.max(MIN);
            let mut cb = b.min(MAX);
            if ca >= cb {
                // Mapping a floating-point null window can collapse its width.
                ca = ca.next_down().max(MIN);
                cb = cb.next_up().min(MAX);
            }
            let value = -self.node(&child, depth, qleft, -cb, -ca)?.0;
            if value <= ca && a >= MIN {
                self.chance_cutoffs += 1;
                #[cfg(test)]
                if self.unguarded_mix_bound {
                    // Test-only fault: return the mapped upper bound as a value.
                    return Ok((sum + p * value + rem * MAX).clamp(MIN, MAX));
                }
                return Ok((sum + p * value + rem * MAX).clamp(MIN, MAX).min(alpha));
            }
            if value >= cb && b <= MAX {
                self.chance_cutoffs += 1;
                return Ok((sum + p * value + rem * MIN).clamp(MIN, MAX).max(beta));
            }
            sum += p * value;
        }
        Ok(sum.clamp(MIN, MAX))
    }
}

fn mix_identities(bag: &[u32; 14]) -> Vec<usize> {
    let mut identities: Vec<_> = (0..14).filter(|&code| bag[code] > 0).collect();
    identities.sort_by_key(|&code| (std::cmp::Reverse(bag[code]), code));
    identities
}

#[cfg(test)]
#[path = "chancemix_tests.rs"]
mod chancemix_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ink;

    #[test]
    fn narrow_bounds_do_not_poison_full_window_tt_results() {
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
        for mode in [
            ChanceMode::Star1,
            ChanceMode::mix(),
            ChanceMode::Mix {
                deep: 2,
                reduction: 6,
            },
        ] {
            let mut config = config.clone();
            config.chance_mode = mode;
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
