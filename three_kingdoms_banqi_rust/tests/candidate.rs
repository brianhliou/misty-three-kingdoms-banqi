//! The engine is the reference search made faster, not changed, and the
//! frontend's FEN carries every input the search reads.

use three_kingdoms_banqi_rust::engine::{SearchConfig, Searcher};
use three_kingdoms_banqi_rust::uci::{self, Session};
use three_kingdoms_banqi_rust::{reference, Move, State, Status, DOWN, NSQ};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
}

/// Games of a cheap search against random moves (so positions include
/// chases, repetitions and both seats ahead), each kept with its move list.
fn games(count: u64) -> Vec<(Vec<State>, Vec<Move>)> {
    let mut out = Vec::new();
    for seed in 0..count {
        let mut rng = Rng(seed * 7919 + 3);
        let start = State::initial();
        let mut codes = Vec::new();
        for (code, &n) in start.bag.iter().enumerate() {
            codes.extend(std::iter::repeat_n(code as i16, n as usize));
        }
        for i in (1..codes.len()).rev() {
            codes.swap(i, (rng.next() % (i + 1) as u64) as usize);
        }
        let mut deal = [-1; NSQ];
        let mut it = codes.into_iter();
        for (sq, &p) in start.board.iter().enumerate() {
            if p == DOWN {
                deal[sq] = it.next().unwrap();
            }
        }
        let mut searcher = Searcher::new(SearchConfig {
            max_depth: 2,
            node_budget: 300,
            time_limit: None,
            quiescence_depth: 2,
            tt_capacity: 1024,
            star1: true,
        });
        let (mut states, mut moves) = (vec![start], Vec::new());
        while matches!(states.last().unwrap().status, Status::Playing(_)) && moves.len() < 300 {
            let state = states.last().unwrap();
            let mv = if rng.next().is_multiple_of(3) {
                let legal = state.legal_moves();
                legal[(rng.next() % legal.len() as u64) as usize]
            } else {
                searcher.search(state).best_move.unwrap()
            };
            let reveal = (mv.0 == mv.1).then_some(deal[mv.0 as usize]);
            let next = state.apply(mv, reveal).unwrap();
            moves.push(mv);
            states.push(next);
        }
        out.push((states, moves));
    }
    out
}

#[test]
fn the_candidate_search_is_node_for_node_the_frozen_reference_search() {
    let configs = [(3u8, 20_000u64, 3u8), (6, 5_000, 4), (64, 40_000, 4)];
    let mut compared = 0;
    for (states, _) in games(6) {
        for state in states.iter().step_by(7) {
            for &(depth, nodes, q) in &configs {
                let new = Searcher::new(SearchConfig {
                    max_depth: depth,
                    node_budget: nodes,
                    time_limit: None,
                    quiescence_depth: q,
                    tt_capacity: 8192,
                    star1: true,
                })
                .search(state);
                let old = reference::Searcher::new(reference::SearchConfig {
                    max_depth: depth,
                    node_budget: nodes,
                    time_limit: None,
                    quiescence_depth: q,
                    tt_capacity: 8192,
                    star1: true,
                })
                .search(state);
                assert_eq!(
                    (
                        new.best_move,
                        new.value,
                        new.completed_depth,
                        new.nodes,
                        new.chance_cutoffs,
                        new.tt_hits,
                        new.completed
                    ),
                    (
                        old.best_move,
                        old.value,
                        old.completed_depth,
                        old.nodes,
                        old.chance_cutoffs,
                        old.tt_hits,
                        old.completed
                    ),
                    "{}",
                    uci::engine_fen(state)
                );
                compared += 1;
            }
        }
    }
    assert!(compared > 100, "only {compared} searches compared");
}

#[test]
fn the_fen_window_rebuilds_every_position_of_every_game() {
    let (mut positions, mut chased, mut with_history) = (0, 0, 0);
    for (states, moves) in games(12) {
        for ply in 0..states.len() {
            let (start, window) = uci::window(&states[..=ply], &moves[..ply]);
            let fen = uci::engine_fen(start);
            let refs: Vec<&str> = window.iter().map(String::as_str).collect();
            let rebuilt = uci::parse_position(&fen, &refs).unwrap_or_else(|e| panic!("{e}: {fen}"));
            let mut truth = states[ply].clone();
            let mut rebuilt = rebuilt;
            truth.captures.sort();
            rebuilt.captures.sort();
            assert_eq!(rebuilt, truth, "ply {ply}: {fen} moves {refs:?}");
            assert!(fen.starts_with(&start.public_fen()));
            positions += 1;
            chased += usize::from(truth.chased_square.is_some());
            with_history += usize::from(truth.history.len() > 3);
        }
    }
    assert!(positions > 1000 && chased > 50 && with_history > 200);
}

#[test]
fn malformed_or_inconsistent_fens_are_refused() {
    let good = uci::engine_fen(&State::initial());
    assert!(uci::parse_engine_fen(&good).is_ok());
    let (states, _) = games(1).remove(0);
    let mid = uci::engine_fen(&states[40]);
    assert!(uci::parse_engine_fen(&mid).is_ok(), "{mid}");
    let field = |fen: &str, i: usize, value: &str| {
        let mut f: Vec<String> = fen.split(' ').map(String::from).collect();
        f[i] = value.into();
        f.join(" ")
    };
    let bad = [
        good.replacen(' ', "  x ", 1), // extra field
        field(&good, 0, "XXXXXXXXX/XXXXXXXXX/XXXXXXXXX/XXXXXXXXX"), // four ranks
        field(
            &good,
            0,
            "XXXXXXXXXX/XXXXXXXX/XXXXXXXXX/XXXXXXXXX/XXXXXXXXX",
        ),
        field(&good, 1, "w"),
        field(&good, 5, "41"),                           // past the clock
        field(&good, 6, "0"),                            // move numbers are 1-based
        field(&good, 7, "G1A2E2R2H2C2S5g1a2e2r2h2c2s4"), // pool short of the tiles
        field(&good, 7, "G2A2E2R2H2C2S5g1a2e2r2h2c2s4"), // two red generals
        field(&good, 8, "z9z9"),
        field(&mid, 3, "95"), // a score the captures do not explain
    ];
    for fen in bad {
        assert!(uci::parse_engine_fen(&fen).is_err(), "accepted {fen}");
    }
}

#[test]
fn the_session_speaks_the_banqi_frontend_protocol() {
    let mut s = Session::default();
    let id = s.handle("uci");
    assert_eq!(
        id.first().unwrap(),
        &format!("id name {}", uci::ENGINE_NAME)
    );
    assert_eq!(id.last().unwrap(), "uciok");
    assert_eq!(s.handle("isready"), ["readyok"]);
    assert!(s.handle("ucinewgame").is_empty());
    assert!(s.handle("setoption name Hash value 64").is_empty());
    // No position: an error line and a null move, never a guess.
    assert_eq!(s.handle("go nodes 100").last().unwrap(), "bestmove (none)");

    let (states, moves) = games(1).remove(0);
    let ply = 60;
    let (start, window) = uci::window(&states[..=ply], &moves[..ply]);
    let command = format!(
        "position fen {} moves {}",
        uci::engine_fen(start),
        window.join(" ")
    );
    assert!(s.handle(&command).is_empty());
    let reply = s.handle("go nodes 3000 movetime 0");
    assert_eq!(reply.len(), 2);
    assert!(reply[0].starts_with("info depth ") && reply[0].contains(" nodes "));
    let best = uci::uci_to_move(reply[1].strip_prefix("bestmove ").unwrap()).unwrap();
    assert!(states[ply].legal_moves().contains(&best));
    assert!(reply[0].ends_with(&format!(" pv {}", uci::move_to_uci(best))));

    // searchmoves restricts the root to the listed moves.
    let other = *states[ply]
        .legal_moves()
        .iter()
        .find(|&&m| m != best)
        .unwrap();
    let reply = s.handle(&format!(
        "go nodes 3000 movetime 0 searchmoves {}",
        uci::move_to_uci(other)
    ));
    assert_eq!(reply[1], format!("bestmove {}", uci::move_to_uci(other)));

    // A flip in the move list has no outcome to replay: refused, null move.
    let flip = states[ply].legal_moves().into_iter().find(|m| m.0 == m.1);
    if let Some(flip) = flip {
        let reply = s.handle(&format!("{command} {}", uci::move_to_uci(flip)));
        assert!(reply[0].starts_with("info string error flip"));
        assert_eq!(s.handle("go nodes 100").last().unwrap(), "bestmove (none)");
    }
    assert!(s
        .handle("position fen nonsense")
        .pop()
        .unwrap()
        .starts_with("info string error"));
    assert!(s.handle("position startpos").is_empty());
    let opening = s.handle("go nodes 2000 movetime 0");
    let mv = uci::uci_to_move(opening[1].strip_prefix("bestmove ").unwrap()).unwrap();
    assert_eq!(mv.0, mv.1, "the opening move is a flip, written from == to");
}
