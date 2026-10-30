use three_kingdoms_banqi_rust::engine::Searcher;
use three_kingdoms_banqi_rust::{Ink, State, EMPTY, NSQ};

fn playing(pieces: &[(usize, i16)]) -> State {
    let mut state = State {
        board: [EMPTY; NSQ],
        bag: [0; 14],
        first_color: Some(Ink::Red),
        ..State::initial()
    };
    for &(square, code) in pieces {
        state.board[square] = code;
    }
    // The fixture position is the first of its counting window.
    state.history = vec![state.position_key()];
    state
}

#[test]
fn search_takes_the_hundred_point_win() {
    let mut state = playing(&[(0, 6), (9, 8), (44, 13)]);
    state.scores = [95, 95];
    let result = Searcher::default().search(&state);
    assert_eq!(result.best_move, Some((0, 9)));
    assert_eq!(result.value, 1.0);
}

use std::time::Duration;
use three_kingdoms_banqi_rust::engine::{cycle_value, evaluate, SearchConfig};
use three_kingdoms_banqi_rust::{Reason, Seat, Status, DOWN, NO_PROGRESS_LIMIT, POINTS};

fn config(depth: u8) -> SearchConfig {
    SearchConfig {
        max_depth: depth,
        node_budget: u64::MAX,
        time_limit: None,
        quiescence_depth: 0,
        tt_capacity: 4096,
        star1: true,
    }
}

// Independent, unpruned expectimax. Every legal move and every nonzero bag
// entry is visited; no production chance helper, pruning or cache is used. A
// position repeated below the root is settled as the 40-ply clock would settle
// it, recomputed here from the scores rather than through the engine.
fn expectimax(state: &State, depth: u8) -> f64 {
    expectimax_below(state, depth, state.ply)
}

fn expectimax_below(state: &State, depth: u8, root_ply: u32) -> f64 {
    if matches!(state.result(), Status::Finished { .. }) {
        return evaluate(state);
    }
    if state.ply > root_ply && state.occurrences(&state.position_key()) >= 2 {
        let seat = state.turn() as usize;
        return (state.scores[seat] as f64 - state.scores[1 - seat] as f64).signum()
            * f64::from(u8::from(state.scores[0] != state.scores[1]));
    }
    if depth == 0 {
        return evaluate(state);
    }
    state
        .legal_moves()
        .into_iter()
        .map(|mv| {
            if mv.0 != mv.1 {
                -expectimax_below(&state.apply(mv, None).unwrap(), depth - 1, root_ply)
            } else {
                let total = state.bag.iter().sum::<u32>() as f64;
                state
                    .bag
                    .iter()
                    .enumerate()
                    .filter(|&(_, &n)| n > 0)
                    .map(|(code, &n)| {
                        -(n as f64) / total
                            * expectimax_below(
                                &state.apply(mv, Some(code as i16)).unwrap(),
                                depth - 1,
                                root_ply,
                            )
                    })
                    .sum()
            }
        })
        .fold(f64::NEG_INFINITY, f64::max)
}

fn assert_value(state: &State, depth: u8) -> u64 {
    let expected = expectimax(state, depth);
    let mut cutoffs = 0;
    for star1 in [false, true] {
        for capacity in [0, 4096] {
            let mut cfg = config(depth);
            cfg.star1 = star1;
            cfg.tt_capacity = capacity;
            let result = Searcher::new(cfg).search(state);
            assert!(result.completed);
            assert!(
                (result.value - expected).abs() < 1e-10,
                "expected {expected}, got {result:?}, state {state:?}"
            );
            assert!(result
                .best_move
                .is_some_and(|mv| state.legal_moves().contains(&mv)));
            if star1 {
                cutoffs += result.chance_cutoffs;
            }
        }
    }
    cutoffs
}

#[test]
fn exhaustive_one_and_two_piece_bags_match_expectimax() {
    let mut cutoffs = 0;
    for first in 0..14 {
        for second in first..14 {
            let mut state = playing(&[(0, 6), (44, 13), (12, DOWN), (32, DOWN)]);
            state.bag[first] += 1;
            state.bag[second] += 1;
            cutoffs += assert_value(&state, 2);
        }
        let mut state = playing(&[(0, 6), (44, 13), (12, DOWN)]);
        state.bag[first] = 1;
        cutoffs += assert_value(&state, 3);
    }
    assert!(cutoffs > 0, "the comparisons must exercise Star1 pruning");
}

#[test]
fn unequal_probabilities_binding_and_both_seats_match_expectimax() {
    for binding in [None, Some(Ink::Red), Some(Ink::Black)] {
        for ply in [0, 1] {
            let mut state = playing(&[(12, DOWN), (22, DOWN), (32, DOWN)]);
            state.first_color = binding;
            state.ply = ply;
            state.status = Status::Playing(state.turn());
            state.bag[3] = 2;
            state.bag[13] = 1;
            assert_value(&state, 3);
        }
    }
}

#[test]
fn every_capture_threshold_and_clock_boundary_are_exact() {
    for seat in [Seat::Red, Seat::Black] {
        for ink in [Ink::Red, Ink::Black] {
            for (role, &points) in POINTS.iter().enumerate() {
                let own = ink as i16 * 7;
                let enemy = ink.other() as i16 * 7;
                let mut state = playing(&[(0, own + 6), (9, enemy + role as i16), (44, enemy + 6)]);
                state.first_color = Some(if seat == Seat::Red { ink } else { ink.other() });
                state.ply = seat as u32;
                state.status = Status::Playing(seat);
                state.scores[seat as usize] = 100 - points;
                state.no_progress = 39;
                let result = Searcher::new(config(1)).search(&state);
                assert_eq!(result.best_move, Some((0, 9)));
                assert_eq!(result.value, 1.0);
                state.scores[seat as usize] -= 1;
                let below = state.apply((0, 9), None).unwrap();
                assert_eq!(below.scores[seat as usize], 99);
                assert!(matches!(below.status, Status::Playing(_)));
                state.scores[seat as usize] = 100;
                state.no_progress = NO_PROGRESS_LIMIT;
                assert_eq!(evaluate(&state), 1.0);
                assert_eq!(
                    state.result(),
                    Status::Finished {
                        winner: Some(seat),
                        reason: Reason::Score
                    }
                );
            }
        }
    }
    for scores in [[20, 15], [10, 25], [20, 20]] {
        let mut state = playing(&[(0, 6), (44, 13)]);
        state.scores = scores;
        state.no_progress = 39;
        let expected = match scores[0].cmp(&scores[1]) {
            std::cmp::Ordering::Greater => 1.0,
            std::cmp::Ordering::Less => -1.0,
            std::cmp::Ordering::Equal => 0.0,
        };
        assert_eq!(Searcher::new(config(1)).search(&state).value, expected);
        // Quiescence must also play the last clock ply, never stand pat here.
        let mut cfg = config(0);
        cfg.quiescence_depth = 1;
        assert_eq!(Searcher::new(cfg).search(&state).value, expected);
        state.no_progress = 40;
        assert_eq!(evaluate(&state), expected);
        assert_eq!(Searcher::default().search(&state).best_move, None);
    }
}

#[test]
fn capture_quiescence_sees_recapture_beyond_horizon() {
    let state = playing(&[(0, 3), (1, 13), (2, 10), (44, 6)]);
    let child = state.apply((0, 1), None).unwrap();
    let static_value = evaluate(&child);
    let mut cfg = config(0);
    cfg.quiescence_depth = 1;
    let quiet = Searcher::new(cfg).search(&child);
    let full = expectimax(&child, 1);
    assert!(quiet.value > static_value);
    assert!((quiet.value - full).abs() < 1e-10);
    assert_eq!(quiet.best_move, Some((2, 1)));
}

/// Red chariot a1 chases the black soldier b2 round a 2x2 block (a1 b1 / a2 b2)
/// for two laps; the black general on i5 gives black other moves.
fn chased_twice() -> State {
    let mut state = playing(&[(0, 3), (10, 13), (44, 7)]);
    state.last_move = Some((11, 10));
    for _ in 0..2 {
        for mv in [(0, 1), (10, 9), (1, 0), (9, 10)] {
            state = state.apply(mv, None).unwrap();
        }
    }
    state
}

#[test]
fn search_takes_chase_legality_from_movegen() {
    let state = chased_twice();
    assert_eq!(state.forbidden_chase_moves(), [(0, 1)]);
    assert!(!state.legal_moves().contains(&(0, 1)));
    assert_value(&state, 3);
    for depth in 1..=3 {
        let result = Searcher::new(config(depth)).search(&state);
        assert_ne!(result.best_move, Some((0, 1)), "depth {depth}");
    }
}

#[test]
fn repetition_is_never_a_draw_and_a_cycle_settles_by_score_margin() {
    let cycle = [(0, 1), (44, 43), (1, 0), (43, 44)];
    for (scores, settled) in [([20, 15], 1.0), ([15, 20], -1.0), ([20, 20], 0.0)] {
        let mut state = playing(&[(0, 6), (44, 13)]);
        state.scores = scores;
        for mv in cycle {
            state = state.apply(mv, None).unwrap();
        }
        // Red to move in a position seen twice: the clock would settle it.
        assert_eq!(state.occurrences(&state.position_key()), 2);
        assert_eq!(cycle_value(&state), Some(settled));
        assert_eq!(state.status, Status::Playing(Seat::Red));
        assert_value(&state, 3);
        let fresh = playing(&[(0, 6), (44, 13)]);
        assert_eq!(cycle_value(&fresh), None);
    }

    // Ahead, red can re-enter the cycle (b1 was seen once), so a shallow search
    // finds the clock win long before the clock runs (clock 4 of 40).
    let mut ahead = playing(&[(0, 6), (44, 13)]);
    ahead.scores = [20, 15];
    for mv in cycle {
        ahead = ahead.apply(mv, None).unwrap();
    }
    assert_eq!(ahead.no_progress, 4);
    let result = Searcher::new(config(2)).search(&ahead);
    assert_eq!(result.value, 1.0);

    // The root itself is searched whatever its history: nine laps in, the clock
    // reaches 40 inside a depth-4 search and red wins on score.
    let mut repeated = playing(&[(0, 6), (44, 13)]);
    repeated.scores = [20, 15];
    for _ in 0..9 {
        for mv in cycle {
            repeated = repeated.apply(mv, None).unwrap();
        }
    }
    assert_eq!(repeated.board, playing(&[(0, 6), (44, 13)]).board);
    assert_eq!(Searcher::new(config(4)).search(&repeated).value, 1.0);
}

#[test]
fn no_legal_move_is_a_loss_with_or_without_pieces() {
    // Unreachable in play (wipe-out cannot happen) but still no legal move.
    let absent = playing(&[(0, 13)]);
    assert_eq!(evaluate(&absent), -1.0);
    let blocked = playing(&[
        (0, 2),
        (20, 2),
        (4, 2),
        (40, 2),
        (24, 2),
        (8, 2),
        (44, 2),
        (36, 2),
        (17, 13),
    ]);
    assert_eq!(
        blocked.result(),
        Status::Finished {
            winner: Some(Seat::Black),
            reason: Reason::NoLegalMove
        }
    );
    let result = Searcher::default().search(&blocked);
    assert_eq!(result.value, -1.0);
    assert_eq!(result.best_move, None);
}

#[test]
fn mixed_small_bags_near_thresholds_and_clock_match_expectimax() {
    for (bag, scores, clock) in [
        ([0, 6, 13], [85, 95], 0),
        ([3, 10, 13], [95, 80], 38),
        ([4, 4, 12], [90, 95], 39),
        ([5, 6, 7], [20, 20], 39),
    ] {
        let mut state = playing(&[(0, 6), (44, 13), (12, DOWN), (22, DOWN), (32, DOWN)]);
        for code in bag {
            state.bag[code] += 1;
        }
        state.scores = scores;
        state.no_progress = clock;
        assert_value(&state, 3);
        // Sample descendants carrying real chase, capture and flip histories.
        for step in 0..6 {
            let moves = state.legal_moves();
            if matches!(state.result(), Status::Finished { .. }) {
                break;
            }
            let mv = moves[(step * 7 + 3) % moves.len()];
            let reveal = if mv.0 == mv.1 {
                state
                    .bag
                    .iter()
                    .position(|&n| n > 0)
                    .map(|code| code as i16)
            } else {
                None
            };
            state = state.apply(mv, reveal).unwrap();
            if matches!(state.result(), Status::Playing(_)) {
                assert_value(&state, 3);
            }
        }
    }
}

#[test]
fn budgets_keep_only_complete_iterations_and_a_legal_fallback() {
    let state = State::initial();
    let mut cfg = config(3);
    cfg.node_budget = 0;
    let result = Searcher::new(cfg.clone()).search(&state);
    assert_eq!(result.completed_depth, 0);
    assert_eq!(result.nodes, 0);
    assert!(!result.completed);
    assert!(state.legal_moves().contains(&result.best_move.unwrap()));

    cfg.max_depth = 1;
    cfg.node_budget = u64::MAX;
    let one = Searcher::new(cfg.clone()).search(&state);
    assert_eq!(one.completed_depth, 1);
    cfg.max_depth = 3;
    cfg.node_budget = one.nodes + 3;
    let partial = Searcher::new(cfg.clone()).search(&state);
    assert_eq!(partial.completed_depth, 1);
    assert_eq!(partial.best_move, one.best_move);
    assert_eq!(partial.value, one.value);
    assert_eq!(partial.nodes, cfg.node_budget);
    assert!(!partial.completed);

    cfg.time_limit = Some(Duration::ZERO);
    let expired = Searcher::new(cfg).search(&state);
    assert_eq!(expired.nodes, 0);
    assert!(!expired.completed);
}

#[test]
fn search_does_not_mutate_state_and_reusing_searcher_is_deterministic() {
    let mut state = playing(&[(0, 6), (44, 13), (22, DOWN)]);
    state.bag[4] = 1;
    let before = state.clone();
    let mut searcher = Searcher::new(config(3));
    let first = searcher.search(&state);
    assert_eq!(state, before);
    assert_eq!(first, searcher.search(&state));
}
