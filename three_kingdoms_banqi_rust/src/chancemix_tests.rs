//! Chance-node properties using only the public rules state.
use super::*;
use crate::{Ink, DOWN, EMPTY};

struct Rng(u64);
impl Rng {
    fn index(&mut self, limit: usize) -> usize {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        ((z ^ (z >> 31)) % limit as u64) as usize
    }
}

fn position(seed: u64) -> State {
    let mut rng = Rng(seed);
    let mut state = State {
        board: [EMPTY; NSQ],
        bag: [0; 14],
        first_color: Some(if rng.index(2) == 0 {
            Ink::Red
        } else {
            Ink::Black
        }),
        scores: [rng.index(100) as u32, rng.index(100) as u32],
        ply: rng.index(2) as u32,
        ..State::initial()
    };
    let mut squares: Vec<_> = (0..NSQ).collect();
    for i in 0..7 {
        let other = i + rng.index(NSQ - i);
        squares.swap(i, other);
    }
    for &sq in &squares[..3] {
        state.board[sq] = DOWN;
        state.bag[rng.index(14)] += 1;
    }
    for (i, &sq) in squares[3..7].iter().enumerate() {
        state.board[sq] = (7 * (i % 2) + rng.index(7)) as i16;
    }
    state.no_progress = if seed.is_multiple_of(7) {
        39
    } else {
        rng.index(39) as u32
    };
    state.history = vec![state.position_key()];
    state.status = state.result();
    state
}

fn searcher(mode: ChanceMode, state: &State) -> Searcher {
    let mut s = Searcher::new(SearchConfig {
        node_budget: u64::MAX,
        time_limit: None,
        tt_capacity: 0,
        chance_mode: mode,
        ..SearchConfig::default()
    });
    s.start = Some(Instant::now());
    s.root_ply = state.ply;
    s
}

fn valid_bound(exact: f64, bound: f64, alpha: f64, beta: f64) -> bool {
    if bound <= alpha {
        exact <= alpha
    } else if bound >= beta {
        exact >= beta
    } else {
        (bound - exact).abs() < 1e-9
    }
}

#[test]
fn default_mode_and_identity_order_are_stable() {
    assert_eq!(SearchConfig::default().chance_mode, ChanceMode::Star1);
    assert_eq!(SearchConfig::level(None).chance_mode, ChanceMode::Star1);
    assert_eq!(
        ChanceMode::mix(),
        ChanceMode::Mix {
            deep: 1,
            reduction: 6
        }
    );
    let mut bag = [0; 14];
    for (code, count) in [(0, 1), (3, 2), (6, 5), (13, 5)] {
        bag[code] = count;
    }
    assert_eq!(mix_identities(&bag), [6, 13, 3, 0]);
}

#[test]
fn depth_one_matches_exact_bag_expectation() {
    for seed in 0..30 {
        let state = position(seed);
        let mv = state
            .legal_moves()
            .into_iter()
            .find(|mv| mv.0 == mv.1)
            .unwrap();
        let total = state.bag.iter().sum::<u32>() as f64;
        let mut expected = 0.;
        for code in mix_identities(&state.bag) {
            let child = state.apply(mv, Some(code as i16)).unwrap();
            expected -= state.bag[code] as f64 / total
                * searcher(ChanceMode::Star1, &state)
                    .node(&child, 0, 2, MIN, MAX)
                    .unwrap()
                    .0;
        }
        for mode in [ChanceMode::Star1, ChanceMode::mix()] {
            let value = searcher(mode, &state)
                .move_value(&state, mv, 1, 2, MIN, MAX)
                .unwrap();
            assert!((value - expected).abs() < 1e-10, "seed={seed}");
        }
    }
}

#[test]
fn mix_windows_preserve_exact_values_and_bound_directions() {
    for reduction in [1, 6] {
        for deep in [1, 2, 14] {
            let mode = ChanceMode::Mix { deep, reduction };
            for seed in 35..115 {
                let state = position(seed);
                let depth = 1 + (seed % 3) as u8;
                let mv = state
                    .legal_moves()
                    .into_iter()
                    .find(|mv| mv.0 == mv.1)
                    .unwrap();
                let exact = searcher(mode, &state)
                    .move_value(&state, mv, depth, 1, MIN, MAX)
                    .unwrap();
                for (a, b) in [
                    (-0.7, -0.6),
                    (0.6, 0.7),
                    (exact - 0.01, exact + 0.01),
                    (0.1, 0.1f64.next_up()),
                ] {
                    let bound = searcher(mode, &state)
                        .move_value(&state, mv, depth, 1, a, b)
                        .unwrap();
                    assert!(
                        valid_bound(exact, bound, a, b),
                        "seed={seed}, mode={mode:?}, exact={exact}, bound={bound}, window={a}..{b}"
                    );
                }
            }
        }
    }
}

#[test]
fn inverse_mapping_roundoff_cannot_turn_a_bound_into_an_exact_value() {
    let mode = ChanceMode::mix();
    let mut checked = 0;
    let mut detected = 0;
    for seed in 35..235 {
        let state = position(seed);
        let mv = state
            .legal_moves()
            .into_iter()
            .find(|mv| mv.0 == mv.1)
            .unwrap();
        let identities = mix_identities(&state.bag);
        let code = identities[0];
        let total = state.bag.iter().sum::<u32>() as f64;
        let p = state.bag[code] as f64 / total;
        let rest: f64 = identities[1..]
            .iter()
            .map(|&other| {
                let child = state.apply(mv, Some(other as i16)).unwrap();
                -(state.bag[other] as f64) / total
                    * searcher(mode, &state)
                        .node(&child, 0, 1, MIN, MAX)
                        .unwrap()
                        .0
            })
            .sum();
        let child = state.apply(mv, Some(code as i16)).unwrap();
        let child_value = -searcher(mode, &state)
            .node(&child, 2, 1, MIN, MAX)
            .unwrap()
            .0;
        let exact = searcher(mode, &state)
            .move_value(&state, mv, 3, 1, MIN, MAX)
            .unwrap();
        for j in -999..1000 {
            let bound = j as f64 / 1000.;
            let mapped = rest + p * bound;
            let alpha = mapped.next_down();
            if bound <= child_value + 0.001 || bound > (alpha - rest) / p {
                continue;
            }
            let beta = mapped + 0.01;
            for broken in [false, true] {
                let mut s = searcher(mode, &state);
                s.config.tt_capacity = 4096;
                s.unguarded_mix_bound = broken;
                s.tt.insert(
                    Key::from(&child),
                    Entry {
                        depth: 2,
                        value: -bound,
                        bound: Bound::Lower,
                        best: None,
                    },
                );
                let actual = s.move_value(&state, mv, 3, 1, alpha, beta).unwrap();
                let valid = valid_bound(exact, actual, alpha, beta);
                if broken {
                    detected += usize::from(!valid);
                } else {
                    assert!(valid, "seed={seed}, exact={exact}, actual={actual}");
                }
            }
            checked += 1;
            break;
        }
        if checked == 10 {
            break;
        }
    }
    assert_eq!(checked, 10);
    assert!(
        detected > 0,
        "test must detect the intentionally unclamped bound"
    );
}

#[test]
fn probes_use_only_exact_entries_at_sufficient_depth() {
    let state = position(59);
    let mv = state
        .legal_moves()
        .into_iter()
        .find(|mv| mv.0 == mv.1)
        .unwrap();
    let child = state
        .apply(mv, Some(mix_identities(&state.bag)[0] as i16))
        .unwrap();
    for bound in [Bound::Lower, Bound::Upper, Bound::Exact] {
        for depth in [1, 2, 3] {
            let mut s = searcher(ChanceMode::mix(), &state);
            s.config.tt_capacity = 4096;
            s.tt.insert(
                Key::from(&child),
                Entry {
                    depth,
                    value: 0.875,
                    bound,
                    best: None,
                },
            );
            let actual = s.node_with_probe(&child, 2, 1, MIN, MAX, true).unwrap().0;
            if matches!(bound, Bound::Exact) && depth >= 2 {
                assert_eq!(actual, 0.875);
                assert_eq!(s.nodes, 1);
            } else {
                assert_eq!(
                    actual,
                    searcher(ChanceMode::mix(), &state)
                        .node_with_probe(&child, 2, 1, MIN, MAX, true)
                        .unwrap()
                        .0
                );
            }
        }
    }
}

#[test]
fn selecting_every_identity_matches_star1_and_single_identity_matches_reveal() {
    for seed in 0..14 {
        let mut state = position(seed);
        let mv = state
            .legal_moves()
            .into_iter()
            .find(|mv| mv.0 == mv.1)
            .unwrap();
        let mode = ChanceMode::Mix {
            deep: 14,
            reduction: 6,
        };
        let mix = searcher(mode, &state)
            .move_value(&state, mv, 3, 0, MIN, MAX)
            .unwrap();
        let exact = searcher(ChanceMode::Star1, &state)
            .move_value(&state, mv, 3, 0, MIN, MAX)
            .unwrap();
        assert!((mix - exact).abs() < 1e-10);
        state.bag = [0; 14];
        state.bag[seed as usize] = 3;
        let child = state.apply(mv, Some(seed as i16)).unwrap();
        let mix = searcher(ChanceMode::mix(), &state)
            .move_value(&state, mv, 3, 1, MIN, MAX)
            .unwrap();
        let exact = -searcher(ChanceMode::mix(), &state)
            .node(&child, 2, 1, MIN, MAX)
            .unwrap()
            .0;
        assert!((mix - exact).abs() < 1e-10);
    }
}
