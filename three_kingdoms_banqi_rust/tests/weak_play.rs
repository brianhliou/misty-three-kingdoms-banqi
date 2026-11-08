mod common;
use std::{collections::HashSet, sync::Arc, time::Duration};
use three_kingdoms_banqi_rust::{
    engine::{
        guarded_root_moves, ChanceMode, Eval, PlayOptions, PlayRng, SearchConfig, SearchResult,
        Searcher,
    },
    Ink, Move, Seat, State, DOWN, EMPTY, NSQ,
};

fn position(pieces: &[(usize, i16)]) -> State {
    let mut s = State {
        board: [EMPTY; NSQ],
        bag: [0; 14],
        first_color: Some(Ink::Red),
        ..State::initial()
    };
    for &(sq, code) in pieces {
        s.board[sq] = code;
    }
    s.history = vec![s.position_key()];
    s
}
fn masked() -> State {
    let mut s = position(&[(0, 6), (12, DOWN), (22, DOWN), (32, DOWN), (44, 13)]);
    s.bag[1] = 2;
    s.bag[10] = 1;
    s
}
fn config() -> SearchConfig {
    SearchConfig {
        max_depth: 4,
        node_budget: u64::MAX,
        time_limit: None,
        quiescence_depth: 0,
        ..SearchConfig::default()
    }
}
fn at(noise_depth: i32) -> PlayOptions {
    PlayOptions {
        noise: 0.3,
        noise_depth,
    }
}
fn select(s: &State, c: SearchConfig, eval: Eval, play: PlayOptions, seed: u64) -> SearchResult {
    Searcher::new(c.with_eval(eval)).select_move_with_options(
        s,
        &[],
        &play,
        &mut PlayRng::new(seed),
    )
}
fn bits(r: &SearchResult) -> (Option<Move>, u64, u8, u64) {
    (r.best_move, r.value.to_bits(), r.completed_depth, r.nodes)
}

#[test]
fn zero_noise_preserves_every_result_field_and_rng_with_flips_evals_and_mix() {
    let evals = [
        Eval::Classical,
        Eval::Nnue(Arc::new(
            three_kingdoms_banqi_rust::nnue::Net::from_bytes(&common::net_bytes(16)).unwrap(),
        )),
    ];
    for eval in evals {
        for mode in [ChanceMode::Star1, ChanceMode::mix()] {
            for s in [State::initial(), masked(), position(&[(0, 6), (44, 13)])] {
                let mut c = config();
                c.node_budget = 400;
                c.chance_mode = mode;
                let mut weak = Searcher::new(c.clone().with_eval(eval.clone()));
                for allowed in [vec![], vec![s.legal_moves()[0]]] {
                    let old =
                        Searcher::new(c.clone().with_eval(eval.clone())).search_moves(&s, &allowed);
                    for seed in [0, 123, u64::MAX] {
                        let mut rng = PlayRng::new(seed);
                        let before = format!("{rng:?}");
                        let new = weak.select_move_with_options(
                            &s,
                            &allowed,
                            &PlayOptions {
                                noise: 0.0,
                                noise_depth: 8,
                            },
                            &mut rng,
                        );
                        assert_eq!(new, old);
                        assert_eq!(new.value.to_bits(), old.value.to_bits());
                        assert_eq!(before, format!("{rng:?}"));
                    }
                }
            }
        }
    }
}

#[test]
fn terminal_wins_override_any_noise_for_both_seats_and_bindings() {
    for ply in [0, 1] {
        let mut s = position(&[(0, 6), (9, 8), (44, 13)]);
        s.ply = ply;
        s.first_color = Some(if ply == 0 { Ink::Red } else { Ink::Black });
        s.scores = [95, 95];
        assert_eq!(guarded_root_moves(&s), [(0, 9)]);
        for seed in 0..32 {
            let r = select(
                &s,
                config(),
                Eval::Classical,
                PlayOptions {
                    noise: f64::MAX,
                    noise_depth: 8,
                },
                seed,
            );
            assert_eq!(r.best_move, Some((0, 9)));
            assert_eq!((r.value, r.nodes), (1.0, 0));
        }
    }
    // No-legal-move wins and the score settlement at clock 40 also count.
    let no_moves = position(&[(0, 3), (1, 13)]);
    assert_eq!(guarded_root_moves(&no_moves), [(0, 1)]);
    let mut clock = position(&[(0, 6), (44, 13)]);
    clock.no_progress = 39;
    clock.scores = [10, 5];
    let r = select(&clock, config(), Eval::Classical, at(1), 123);
    assert_eq!((r.value, r.nodes), (1.0, 0));
}

#[test]
fn avoids_losing_replies_including_any_unsafe_flip_reveal() {
    let mut s = position(&[(0, 6), (2, 10)]);
    s.scores[1] = 95;
    assert_eq!(guarded_root_moves(&s), [(0, 9)]);
    for seed in 0..32 {
        assert_eq!(
            select(&s, config(), Eval::Classical, at(1), seed).best_move,
            Some((0, 9))
        );
    }
    let mut s = position(&[(0, 6), (1, DOWN), (22, DOWN), (43, 13)]);
    s.bag[1] = 1; // An advisor of our ink is safe.
    s.bag[10] = 1; // An enemy chariot can capture a1 immediately for 100 points.
    s.scores[1] = 95;
    let allowed = guarded_root_moves(&s);
    assert!(!allowed.contains(&(1, 1)));
    assert!(allowed.contains(&(22, 22)));
    let revealed = s.apply((1, 1), Some(10)).unwrap();
    assert!(matches!(
        revealed.apply((1, 0), None).unwrap().status,
        three_kingdoms_banqi_rust::Status::Finished {
            winner: Some(Seat::Black),
            ..
        }
    ));
    for seed in 0..32 {
        assert_ne!(
            select(
                &s,
                config(),
                Eval::Classical,
                PlayOptions {
                    noise: 100.0,
                    ..at(1)
                },
                seed
            )
            .best_move,
            Some((1, 1))
        );
    }
}

#[test]
fn all_unsafe_retains_legal_moves_and_clock_draws_are_not_wins() {
    let mut s = position(&[(0, 6), (10, 10)]);
    s.scores[1] = 95;
    assert_eq!(guarded_root_moves(&s), s.legal_moves());
    let mut seen = HashSet::new();
    for seed in 0..32 {
        let r = select(&s, config(), Eval::Classical, at(1), seed);
        assert!(s.legal_moves().contains(&r.best_move.unwrap()));
        seen.insert(r.best_move);
    }
    assert_eq!(seen.len(), 2);
    let mut draw = position(&[(0, 6), (44, 13)]);
    draw.no_progress = 39;
    let r = select(&draw, config(), Eval::Classical, at(1), 1);
    assert_eq!(r.value, 0.0);
    assert!(r.nodes > 0);
}

#[test]
fn three_ply_terminal_scores_take_precedence_over_even_huge_noise() {
    // Quiet a1a2 reaches the score settlement in three plies; capturing b1
    // resets the clock. Neither move ends the game within the immediate guard.
    for scores in [[10, 0], [0, 10]] {
        let mut s = position(&[(0, 6), (1, 13), (44, 13)]);
        s.scores = scores;
        s.no_progress = 37;
        let quiet = (0, 9);
        let capture = (0, 1);
        let guarded = guarded_root_moves(&s);
        assert!(guarded.contains(&quiet) && guarded.contains(&capture));
        let mut c = config();
        c.max_depth = 3;
        let quiet_value = Searcher::new(c.clone()).search_moves(&s, &[quiet]).value;
        let capture_value = Searcher::new(c.clone()).search_moves(&s, &[capture]).value;
        assert_eq!(quiet_value, if scores[0] > scores[1] { 1.0 } else { -1.0 });
        assert!(capture_value.abs() < 1.0);
        for seed in 0..32 {
            let r = select(
                &s,
                c.clone(),
                Eval::Classical,
                PlayOptions {
                    noise: f64::MAX,
                    noise_depth: 3,
                },
                seed,
            );
            assert_eq!(
                r.best_move,
                Some(if scores[0] > scores[1] {
                    quiet
                } else {
                    capture
                })
            );
        }
    }
}

#[test]
fn full_window_root_scores_replay_and_vary_including_flips_nnue_and_mix() {
    let s = masked();
    for eval in [
        Eval::Classical,
        Eval::Nnue(Arc::new(
            three_kingdoms_banqi_rust::nnue::Net::from_bytes(&common::net_bytes(16)).unwrap(),
        )),
    ] {
        for mode in [ChanceMode::Star1, ChanceMode::mix()] {
            for depth in [1, 3] {
                let mut c = config();
                c.chance_mode = mode;
                c.max_depth = depth;
                let roots: Vec<_> = s
                    .legal_moves()
                    .into_iter()
                    .map(|mv| {
                        (
                            mv,
                            Searcher::new(c.clone().with_eval(eval.clone()))
                                .search_moves(&s, &[mv])
                                .value,
                        )
                    })
                    .collect();
                let mut seen = HashSet::new();
                for seed in 0..16 {
                    let a = select(&s, c.clone(), eval.clone(), at(depth as i32), seed);
                    let b = select(&s, c.clone(), eval.clone(), at(depth as i32), seed);
                    assert_eq!(a, b);
                    assert_eq!(a.completed_depth, depth);
                    let expected = roots
                        .iter()
                        .find(|(mv, _)| Some(*mv) == a.best_move)
                        .unwrap()
                        .1;
                    assert!((a.value - expected).abs() < 1e-12, "{a:?} vs {expected}");
                    seen.insert(a.best_move.unwrap());
                }
                assert!(seen.len() > 1);
                assert!(seen.iter().any(|mv| mv.0 == mv.1), "flips must participate");
            }
        }
    }
}

#[test]
fn noise_depth_and_partial_passes_obey_total_nodes_and_time() {
    let s = masked();
    let one = select(&s, config(), Eval::Classical, at(1), 123);
    assert_eq!(one, select(&s, config(), Eval::Classical, at(0), 123));
    let two = select(&s, config(), Eval::Classical, at(2), 123);
    let three = select(&s, config(), Eval::Classical, at(3), 123);
    for (budget, expected) in [
        (one.nodes + (two.nodes - one.nodes) / 2, &one),
        (two.nodes + (three.nodes - two.nodes) / 2, &two),
    ] {
        let mut c = config();
        c.node_budget = budget;
        let r = select(&s, c, Eval::Classical, at(4), 123);
        assert_eq!(
            (r.best_move, r.value.to_bits(), r.completed_depth),
            (
                expected.best_move,
                expected.value.to_bits(),
                expected.completed_depth
            )
        );
        assert_eq!(r.nodes, budget);
        assert!(!r.completed);
    }
    for (nodes, limit) in [(0, None), (u64::MAX, Some(Duration::ZERO))] {
        let mut c = config();
        c.node_budget = nodes;
        c.time_limit = limit;
        let r = select(&s, c, Eval::Classical, at(4), 123);
        assert_eq!(bits(&r), bits(&one)); // Mandatory complete depth-one floor.
        assert!(!r.completed);
    }
    assert!(three.nodes > two.nodes && two.nodes > one.nodes);
    let mut after = Vec::new();
    for depth in [1, 3] {
        let mut rng = PlayRng::new(123);
        Searcher::new(config()).select_move_with_options(&s, &[], &at(depth), &mut rng);
        after.push(format!("{rng:?}"));
    }
    assert_eq!(
        after[0], after[1],
        "deeper passes reuse the same noise draws"
    );
}

#[test]
fn filtered_terminal_and_reused_searchers_remain_valid() {
    let s = masked();
    let mut searcher = Searcher::new(config());
    let mut rng = PlayRng::new(123);
    let only = s.legal_moves()[0];
    assert_eq!(
        searcher
            .select_move_with_options(&s, &[only], &at(3), &mut rng)
            .best_move,
        Some(only)
    );
    let r = searcher.search(&s);
    assert_eq!(r, Searcher::new(config()).search(&s));
    let mut won = position(&[(0, 6), (44, 13)]);
    won.scores[0] = 100;
    assert_eq!(
        select(&won, config(), Eval::Classical, at(3), 123).best_move,
        None
    );
}

#[test]
fn options_reject_bad_values() {
    assert_eq!(PlayOptions::default().noise, 0.0);
    assert_eq!(PlayOptions::default().noise_depth, 1);
    for noise in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(PlayOptions { noise, ..at(1) }.validate().is_err());
    }
    for noise_depth in [-1, 9] {
        assert!(at(noise_depth).validate().is_err());
    }
    for noise in [0.0, 0.005, 3.0, f64::MAX] {
        assert!(PlayOptions { noise, ..at(1) }.validate().is_ok());
    }
}
