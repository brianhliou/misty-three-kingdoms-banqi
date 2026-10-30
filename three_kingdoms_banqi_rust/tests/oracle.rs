use std::process::Command;
use three_kingdoms_banqi_rust::{
    Ink, Move, MoveError, Reason, Seat, State, Status, DOWN, EMPTY, NSQ, POINTS, SCORE_TARGET,
    START_COUNTS,
};

// Python is only a standard-library JSONL reader: every rule assertion runs in Rust.
// This keeps the crate dependency-free and its offline gate independent of Node.
fn fixture() -> String {
    let output = Command::new("python3")
        .args(["-c", r#"
import json, sys
roles = ['general', 'advisor', 'elephant', 'chariot', 'horse', 'cannon', 'soldier']
def status(s):
    if s['type'] == 'playing':
        return 'playing:' + s['turn']
    return 'finished:' + (s['winner'] or '-') + ':' + s['reason']
with open(sys.argv[1]) as source:
    for line in source:
        game = json.loads(line)
        deal = []
        for rank in range(1, 6):
            for file in 'abcdefghi':
                p = game['deal'].get(file + str(rank))
                deal.append(str(7 * (p['color'] == 'black') + roles.index(p['role'])) if p else '-1')
        print('GAME', game['game'], ','.join(deal), sep='\t')
        for p in game['plies']:
            a = p['after']
            print('PLY', p['fen'], p['turn'], ','.join(p['legal']), p['move'],
                  a['scores']['red'], a['scores']['black'], a['noProgressClock'],
                  a['chasedSquare'] or '-', a['repCount'],
                  a['firstColor'] or '-', status(a['status']), ','.join(p['forbidden']) or '-',
                  sep='\t')
        print('FINAL', status(game['final']), sep='\t')
"#])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tk-oracle.jsonl"))
        .output()
        .expect("python3 standard-library fixture reader");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn square(i: u8) -> String {
    format!("{}{}", (b'a' + i % 9) as char, i / 9 + 1)
}

fn moves_text(state: &State) -> String {
    list_text(&state.legal_moves())
}

fn list_text(moves: &[Move]) -> String {
    let mut moves: Vec<_> = moves
        .iter()
        .map(|&(from, to)| format!("{}{}", square(from), square(to)))
        .collect();
    moves.sort();
    moves.join(",")
}

#[test]
fn opening_moves_match_typescript_oracle() {
    let fixture = fixture();
    let opening: Vec<_> = fixture
        .lines()
        .find(|line| line.starts_with("PLY\t"))
        .unwrap()
        .split('\t')
        .collect();
    assert_eq!(moves_text(&State::initial()), opening[3]);
}

fn sq(name: &str) -> u8 {
    let b = name.as_bytes();
    b[0] - b'a' + (b[1] - b'1') * 9
}

fn mv(from: &str, to: &str) -> Move {
    (sq(from), sq(to))
}

fn seat(text: &str) -> Seat {
    match text {
        "red" => Seat::Red,
        "black" => Seat::Black,
        _ => panic!("invalid seat: {text}"),
    }
}

fn ink(text: &str) -> Option<Ink> {
    match text {
        "red" => Some(Ink::Red),
        "black" => Some(Ink::Black),
        "-" => None,
        _ => panic!("invalid ink"),
    }
}

fn status(text: &str) -> Status {
    let fields: Vec<_> = text.split(':').collect();
    match fields.as_slice() {
        ["playing", turn] => Status::Playing(seat(turn)),
        ["finished", winner, reason] => Status::Finished {
            winner: if *winner == "-" {
                None
            } else {
                Some(seat(winner))
            },
            reason: match *reason {
                "score" => Reason::Score,
                "no-progress" => Reason::NoProgress,
                "no-legal-move" => Reason::NoLegalMove,
                _ => panic!("invalid reason: {reason}"),
            },
        },
        _ => panic!("invalid status: {text}"),
    }
}

fn square_opt(text: &str) -> Option<u8> {
    (text != "-").then(|| sq(text))
}

/// Independent projection from a canonical test-only deal. A live engine only
/// receives DOWN and the multiset; there is no per-square identity in State.
fn project(state: &State, canonical: &[i16; NSQ], covered: &[bool; NSQ]) -> State {
    let mut projected = state.clone();
    projected.bag = [0; 14];
    for (i, &piece) in canonical.iter().enumerate() {
        projected.board[i] = if covered[i] {
            projected.bag[piece as usize] += 1;
            DOWN
        } else {
            piece
        };
    }
    projected
}

fn assert_hidden_permutation(state: &State, canonical: &[i16; NSQ], covered: &[bool; NSQ]) {
    let hidden: Vec<_> = (0..NSQ).filter(|&i| covered[i]).collect();
    let mut permuted = *canonical;
    for (n, &i) in hidden.iter().enumerate() {
        permuted[i] = canonical[hidden[(n + 1) % hidden.len()]];
    }
    let projection = project(state, &permuted, covered);
    assert_eq!(
        state,
        &project(state, canonical, covered),
        "board and bag conservation"
    );
    assert_eq!(state.public_fen(), projection.public_fen());
    assert_eq!(state.result(), projection.result());
    for seat in [Seat::Red, Seat::Black] {
        assert_eq!(state.observation(seat), projection.observation(seat));
    }
}

#[test]
fn all_10157_positions_match_moves_chases_results_scores_and_hidden_observations() {
    let fixture = fixture();
    let mut state = State::initial();
    let mut canonical = [EMPTY; NSQ];
    let mut covered = [false; NSQ];
    let mut game = 0;
    let (mut games, mut positions, mut chased, mut forbidding, mut repeated) = (0, 0, 0, 0, 0);
    let mut endings = [0; 2];
    for line in fixture.lines() {
        let f: Vec<_> = line.split('\t').collect();
        match f[0] {
            "GAME" => {
                games += 1;
                game = f[1].parse::<usize>().unwrap();
                state = State::initial();
                let deal: Vec<_> = f[2].split(',').map(|c| c.parse::<i16>().unwrap()).collect();
                canonical.copy_from_slice(&deal);
                covered = std::array::from_fn(|i| canonical[i] != EMPTY);
                assert_eq!(state, project(&state, &canonical, &covered));
            }
            "PLY" => {
                let context = format!("game {game}, ply {}", state.ply);
                assert_eq!(state.public_fen(), f[1], "FEN: {context}");
                assert_eq!(state.turn(), seat(f[2]), "turn: {context}");
                assert_eq!(
                    state.result(),
                    Status::Playing(seat(f[2])),
                    "result: {context}"
                );
                assert_eq!(moves_text(&state), f[3], "legal moves: {context}");
                let forbidden = list_text(&state.forbidden_chase_moves());
                assert_eq!(
                    if forbidden.is_empty() {
                        "-"
                    } else {
                        &forbidden
                    },
                    f[12],
                    "forbidden chases: {context}"
                );
                assert_hidden_permutation(&state, &canonical, &covered);
                chased += usize::from(state.chased_square.is_some());
                forbidding += usize::from(f[12] != "-");
                let action = mv(&f[4][..2], &f[4][2..]);
                let (from, to) = (action.0 as usize, action.1 as usize);
                let reveal = (from == to).then_some(canonical[from]);
                state = state.apply(action, reveal).unwrap();
                if from == to {
                    covered[from] = false;
                } else {
                    canonical[to] = canonical[from];
                    canonical[from] = EMPTY;
                }
                assert_eq!(
                    state.scores,
                    [f[5].parse().unwrap(), f[6].parse().unwrap()],
                    "scores: {context}"
                );
                assert_eq!(
                    state.no_progress,
                    f[7].parse::<u32>().unwrap(),
                    "clock: {context}"
                );
                assert_eq!(state.chased_square, square_opt(f[8]), "chased: {context}");
                let count = state.occurrences(&state.position_key());
                assert_eq!(count, f[9].parse::<u32>().unwrap(), "repetition: {context}");
                repeated += usize::from(count >= 3);
                assert_eq!(state.first_color, ink(f[10]), "binding: {context}");
                assert_eq!(state.status, status(f[11]), "status: {context}");
                assert_eq!(state.result(), status(f[11]), "result after: {context}");
                positions += 1;
            }
            "FINAL" => {
                assert_eq!(state.result(), status(f[1]), "final game {game}");
                assert_hidden_permutation(&state, &canonical, &covered);
                match state.status {
                    Status::Finished {
                        reason: Reason::Score,
                        ..
                    } => endings[0] += 1,
                    Status::Finished {
                        reason: Reason::NoProgress,
                        ..
                    } => endings[1] += 1,
                    other => panic!("unexpected fixture ending: {other:?}"),
                }
            }
            other => panic!("unexpected record: {other}"),
        }
    }
    assert_eq!(
        (games, positions, chased, forbidding, repeated),
        (42, 10_157, 2_599, 127, 303)
    );
    assert_eq!(endings, [30, 12]);
    println!("42 games, 10157 positions (127 with a refused chase): zero move/chase/result/score discrepancies; hidden permutations match for both seats");
}

fn playing(pieces: &[(&str, i16)]) -> State {
    let mut state = State {
        board: [EMPTY; NSQ],
        bag: [0; 14],
        first_color: Some(Ink::Red),
        ..State::initial()
    };
    for &(square, code) in pieces {
        state.board[sq(square) as usize] = code;
    }
    state
}

fn dests(state: &State, from: &str) -> Vec<String> {
    let mut dests: Vec<_> = state
        .legal_moves()
        .into_iter()
        .filter(|m| m.0 == sq(from))
        .map(|m| square(m.1))
        .collect();
    dests.sort();
    dests
}

fn finished(winner: Option<Seat>, reason: Reason) -> Status {
    Status::Finished { winner, reason }
}

#[test]
fn geometry_has_sliders_unblocked_leapers_and_no_rank_or_royal_rules() {
    for role in [0, 3, 5] {
        assert_eq!(
            dests(&playing(&[("e3", role)]), "e3"),
            ["a3", "b3", "c3", "d3", "e1", "e2", "e4", "e5", "f3", "g3", "h3", "i3"]
        );
    }
    for role in [0, 3] {
        assert_eq!(
            dests(
                &playing(&[("e3", role), ("g3", 6), ("c3", DOWN), ("e5", 7)]),
                "e3"
            ),
            ["d3", "e1", "e2", "e4", "e5", "f3"]
        );
    }
    let blockers = [("e2", DOWN), ("e4", DOWN), ("d3", 6), ("f3", 12)];
    for (role, expected) in [
        (4, vec!["c2", "c4", "d1", "d5", "f1", "f5", "g2", "g4"]),
        (2, vec!["c1", "c5", "g1", "g5"]),
    ] {
        let mut pieces = vec![("e3", role)];
        pieces.extend(blockers);
        // Also occupy every elephant eye.
        pieces.extend([("d2", DOWN), ("d4", 5), ("f2", 13), ("f4", DOWN)]);
        assert_eq!(dests(&playing(&pieces), "e3"), expected);
    }
    assert_eq!(dests(&playing(&[("a1", 4)]), "a1"), ["b3", "c2"]);
    assert_eq!(dests(&playing(&[("a1", 2)]), "a1"), ["c3"]);
    assert_eq!(
        dests(&playing(&[("e3", 1)]), "e3"),
        ["d2", "d4", "f2", "f4"]
    );
    assert_eq!(
        dests(&playing(&[("e3", 6)]), "e3"),
        ["d3", "e2", "e4", "f3"]
    );
    assert_eq!(dests(&playing(&[("i5", 6)]), "i5"), ["h5", "i4"]);
    // All 49 attacker/victim pairs can capture, including soldier/general.
    for (attacker, from) in ["a3", "d2", "c1", "a3", "c2", "a3", "d3"]
        .iter()
        .enumerate()
    {
        for victim in 0..7 {
            let mut state = playing(&[(from, attacker as i16), ("e3", 7 + victim)]);
            if attacker == 5 {
                state.board[sq("c3") as usize] = DOWN;
            }
            assert!(
                state.legal_moves().contains(&mv(from, "e3")),
                "attacker {attacker}, victim {victim}"
            );
        }
    }
    let exposed = playing(&[("a1", 0), ("i1", 10), ("d3", 6)]);
    assert!(exposed.legal_moves().contains(&mv("d3", "d4")));
}

#[test]
fn cannon_uses_exactly_one_screen_and_covered_tiles_are_flip_only() {
    let state = playing(&[
        ("e3", 5),
        ("e4", DOWN),
        ("e5", 7),
        ("g3", 6),
        ("i3", 10),
        ("c3", 11),
        ("a3", 1),
        ("e2", 8),
        ("e1", DOWN),
    ]);
    assert_eq!(dests(&state, "e3"), ["d3", "e5", "f3", "i3"]);
    for pieces in [
        vec![("a1", 5), ("a5", 7)],
        vec![("a1", 5), ("a2", DOWN), ("a3", 6), ("a5", 7)],
        vec![("a1", 5), ("a2", 6), ("a5", DOWN)],
    ] {
        assert!(!dests(&playing(&pieces), "a1").contains(&"a5".to_owned()));
    }
    let state = playing(&[("a1", 3), ("a2", DOWN), ("b1", 11)]);
    assert_eq!(dests(&state, "a2"), ["a2"]);
    assert!(dests(&state, "b1").is_empty());
    assert!(!dests(&state, "a1").iter().any(|d| d == "a2" || d == "a3"));
}

#[test]
fn flips_bind_ink_decrement_bag_reset_clock_and_reject_invalid_outcomes() {
    let mut state = State::initial();
    state.no_progress = 39;
    let saved = state.clone();
    for reveal in [None, Some(-2), Some(14)] {
        assert_eq!(
            state.apply(mv("a1", "a1"), reveal),
            Err(MoveError::InvalidReveal)
        );
    }
    assert_eq!(state, saved);
    state = state.apply(mv("a1", "a1"), Some(7)).unwrap();
    assert_eq!(state.first_color, Some(Ink::Black));
    assert_eq!(state.ink_for_seat(Seat::Red), Some(Ink::Black));
    assert_eq!(state.ink_for_seat(Seat::Black), Some(Ink::Red));
    assert_eq!(state.bag[7], 0);
    assert_eq!(
        (state.no_progress, state.ply, state.move_number()),
        (0, 1, 1)
    );
    assert_eq!(
        state.apply(mv("b1", "b1"), Some(7)),
        Err(MoveError::InvalidReveal)
    );
    state = state.apply(mv("b1", "b1"), Some(13)).unwrap();
    assert_eq!(state.first_color, Some(Ink::Black));
    assert_eq!((state.ply, state.move_number()), (2, 2));
    assert_eq!(state.bag.iter().sum::<u32>(), 30);
    assert_eq!(state.board.iter().filter(|&&p| p == DOWN).count(), 30);
    assert!(state.observation(Seat::Black).legal_moves.is_empty());
}

#[test]
fn captures_award_role_points_to_seat_reset_clock_and_do_not_capture_bag() {
    for (role, &points) in POINTS.iter().enumerate() {
        let mut state = playing(&[("a1", 10), ("a2", role as i16), ("i5", 6)]);
        state.first_color = Some(Ink::Black);
        state.no_progress = 39;
        state.chased_square = Some(sq("a1"));
        state.history = vec![state.position_key(); 2];
        let next = state.apply(mv("a1", "a2"), None).unwrap();
        assert_eq!(next.scores, [points, 0]);
        assert_eq!(next.captures, [role as i16]);
        assert_eq!(next.no_progress, 0);
        assert_eq!(next.chased_square, None);
        assert_eq!(next.history, [next.position_key()]);
        assert_eq!(next.bag, state.bag);
        assert_eq!(next.status, Status::Playing(Seat::Black));
    }
    let mut state = playing(&[("a1", 13), ("a2", 1), ("i5", 6)]);
    state.ply = 1;
    state.status = Status::Playing(Seat::Black);
    state.scores = [0, 95];
    assert_eq!(
        state.apply(mv("a1", "a2"), None).unwrap().status,
        finished(Some(Seat::Black), Reason::Score)
    );
}

#[test]
fn score_precedes_clock_and_forty_quiet_plies_settle_by_score() {
    let mut state = playing(&[("a1", 6), ("a2", 7), ("i5", 13)]);
    state.scores = [85, 90];
    state.no_progress = 39;
    let next = state.apply(mv("a1", "a2"), None).unwrap();
    assert_eq!(next.scores, [105, 90]);
    assert_eq!(next.status, finished(Some(Seat::Red), Reason::Score));
    assert!(next.legal_moves().is_empty());
    assert_eq!(
        next.apply(mv("i5", "i4"), None),
        Err(MoveError::IllegalMove)
    );
    state.scores = [100, 90];
    state.no_progress = 40;
    assert_eq!(state.result(), finished(Some(Seat::Red), Reason::Score));
    for (scores, winner) in [
        ([20, 15], Some(Seat::Red)),
        ([10, 25], Some(Seat::Black)),
        ([20, 20], None),
    ] {
        let mut state = playing(&[("a1", 6), ("i5", 13)]);
        state.scores = scores;
        for ply in 0..40 {
            let action = [
                mv("a1", "b1"),
                mv("i5", "h5"),
                mv("b1", "a1"),
                mv("h5", "i5"),
            ][ply % 4];
            state = state.apply(action, None).unwrap();
            if ply < 39 {
                assert!(matches!(state.status, Status::Playing(_)));
            }
        }
        assert_eq!(state.no_progress, 40);
        assert_eq!(state.status, finished(winner, Reason::NoProgress));
    }
}

/// The position `mv` would reach from `state` (quiet moves only).
fn key_after(state: &State, m: Move) -> three_kingdoms_banqi_rust::PositionKey {
    let mut next = state.clone();
    next.board[m.1 as usize] = next.board[m.0 as usize];
    next.board[m.0 as usize] = EMPTY;
    next.ply += 1;
    next.position_key()
}

/// Red chariot a1 chases the black soldier b2 round a 2x2 block; the black
/// general on i5 never moves, so red always has other moves.
fn chase_cycle() -> State {
    let mut state = playing(&[("a1", 3), ("b2", 13), ("i5", 7)]);
    state.last_move = Some(mv("c2", "b2"));
    state
}

#[test]
fn a_cycling_chase_is_refused_at_its_third_occurrence_and_nothing_draws() {
    let mut state = chase_cycle();
    let chases = [mv("a1", "b1"), mv("b1", "a1")];
    let flees = [mv("b2", "a2"), mv("a2", "b2")];
    for lap in 0..2 {
        for i in 0..2 {
            state = state.apply(chases[i], None).unwrap();
            assert_eq!(state.chased_square, Some(flees[i].0), "lap {lap}");
            assert_eq!(state.occurrences(&state.position_key()), lap + 1);
            state = state.apply(flees[i], None).unwrap();
            assert_eq!(state.chased_square, None);
        }
    }
    assert_eq!(state.occurrences(&key_after(&state, mv("a1", "b1"))), 2);
    assert_eq!(state.forbidden_chase_moves(), [mv("a1", "b1")]);
    assert_eq!(
        state.apply(mv("a1", "b1"), None),
        Err(MoveError::IllegalMove)
    );
    assert!(state.legal_moves().contains(&mv("a1", "a3")));
    assert_eq!(state.status, Status::Playing(Seat::Red));
}

#[test]
fn the_defender_is_never_limited_and_an_ordinary_repetition_does_not_end_the_game() {
    let mut state = chase_cycle().apply(mv("a1", "b1"), None).unwrap();
    let flee_key = key_after(&state, mv("b2", "a2"));
    state.history.extend([flee_key; 2]);
    assert!(state.forbidden_chase_moves().is_empty());
    let fled = state.apply(mv("b2", "a2"), None).unwrap();
    assert_eq!(fled.occurrences(&fled.position_key()), 3);
    assert_eq!(fled.status, Status::Playing(Seat::Red));
}

#[test]
fn a_mutual_chase_is_ordinary_and_never_restricted() {
    let mut state = playing(&[("a1", 3), ("i1", 10), ("e5", 6)]);
    state.last_move = Some(mv("i2", "i1"));
    state.chased_square = Some(sq("a1"));
    let cycle = [
        mv("a1", "b1"),
        mv("i1", "h1"),
        mv("b1", "a1"),
        mv("h1", "i1"),
    ];
    for ply in 0..16 {
        assert!(state.forbidden_chase_moves().is_empty(), "ply {ply}");
        assert!(!state.is_chase(cycle[ply % 4]));
        state = state.apply(cycle[ply % 4], None).unwrap();
        assert!(state.chased_square.is_some());
    }
    assert_eq!(state.occurrences(&state.position_key()), 4);
}

#[test]
fn a_herding_chase_onto_new_squares_is_never_limited() {
    let mut state = playing(&[("a1", 3), ("b5", 13)]);
    state.last_move = Some(mv("c5", "b5"));
    let files = b"abcdefghi";
    for i in 1..9 {
        let at = |f: u8, r: char| format!("{}{}", f as char, r);
        assert!(state.forbidden_chase_moves().is_empty());
        state = state
            .apply(mv(&at(files[i - 1], '1'), &at(files[i], '1')), None)
            .unwrap();
        assert_eq!(
            state.chased_square,
            Some(sq(&at(files[i], '5'))),
            "chase {i}"
        );
        assert_eq!(state.occurrences(&state.position_key()), 1);
        if i + 1 < 9 {
            state = state
                .apply(mv(&at(files[i], '5'), &at(files[i + 1], '5')), None)
                .unwrap();
        }
    }
    assert_eq!(state.ply, 15);
}

#[test]
fn the_only_legal_move_exception_keeps_a_forbidden_chase_legal() {
    let mut pieces = vec![
        ("a1", 2),
        ("c3", 2),
        ("e1", 2),
        ("e5", 2),
        ("g3", 2),
        ("i1", 2),
        ("i5", 2),
        ("a5", 2),
        ("b1", 6),
        ("c2", 10),
    ];
    let mut base = playing(&pieces);
    base.last_move = Some(mv("d2", "c2"));
    assert_eq!(dests(&base, "b1"), ["b2", "c1"]);
    let mut one = base.clone();
    one.history = vec![key_after(&base, mv("b1", "c1")); 2];
    assert_eq!(one.forbidden_chase_moves(), [mv("b1", "c1")]);
    assert_eq!(dests(&one, "b1"), ["b2"]);
    let mut both = one.clone();
    both.history.extend([key_after(&base, mv("b1", "b2")); 2]);
    assert!(both.forbidden_chase_moves().is_empty());
    assert_eq!(dests(&both, "b1"), ["b2", "c1"]);
    // A covered tile is a flip, which counts as another legal move.
    pieces.push(("h4", DOWN));
    let mut tiled = playing(&pieces);
    tiled.bag[11] = 1;
    tiled.last_move = Some(mv("d2", "c2"));
    tiled.history = vec![
        key_after(&tiled, mv("b1", "c1")),
        key_after(&tiled, mv("b1", "c1")),
        key_after(&tiled, mv("b1", "b2")),
        key_after(&tiled, mv("b1", "b2")),
    ];
    assert_eq!(list_text(&tiled.forbidden_chase_moves()), "b1b2,b1c1");
    assert_eq!(tiled.legal_moves(), [mv("h4", "h4")]);
}

#[test]
fn chase_detection_uses_cannon_screens_for_the_second_seat() {
    let mut state = playing(&[("a1", 12), ("b2", DOWN), ("b3", 6)]);
    state.bag[0] = 1;
    state.ply = 1;
    state.status = Status::Playing(Seat::Black);
    state.last_move = Some(mv("c3", "b3"));
    assert_eq!(state.chase_target(mv("a1", "b1")), Some(sq("b3")));
    assert_eq!(
        state.apply(mv("a1", "b1"), None).unwrap().chased_square,
        Some(sq("b3"))
    );
    state.history = vec![key_after(&state, mv("a1", "b1")); 2];
    assert_eq!(state.forbidden_chase_moves(), [mv("a1", "b1")]);
    assert_eq!(
        state.apply(mv("a1", "b1"), None),
        Err(MoveError::IllegalMove)
    );
}

#[test]
fn stalemate_loses_but_a_covered_tile_keeps_a_blocked_player_active() {
    // This synthetic oracle unit fixture uses eight elephants to block all eyes.
    let mut state = playing(&[
        ("a1", 2),
        ("c3", 2),
        ("e1", 2),
        ("e5", 2),
        ("g3", 2),
        ("i1", 2),
        ("i5", 2),
        ("a5", 2),
        ("i2", 13),
    ]);
    assert!(state.legal_moves().is_empty());
    assert_eq!(
        state.result(),
        finished(Some(Seat::Black), Reason::NoLegalMove)
    );
    let mut before = state.clone();
    before.board[sq("i2") as usize] = EMPTY;
    before.board[sq("h2") as usize] = 13;
    before.ply = 1;
    before.status = Status::Playing(Seat::Black);
    assert_eq!(
        before.apply(mv("h2", "i2"), None).unwrap().status,
        state.result()
    );
    state.board[sq("h4") as usize] = DOWN;
    state.bag[7] = 1;
    assert_eq!(state.result(), Status::Playing(Seat::Red));
}

#[test]
fn wipe_out_cannot_happen_because_the_fourteenth_capture_reaches_100() {
    let mut values: Vec<u32> = START_COUNTS
        .iter()
        .zip(POINTS)
        .flat_map(|(&count, points)| std::iter::repeat_n(points, count as usize))
        .collect();
    values.sort_unstable();
    assert_eq!((values.len(), values.iter().sum::<u32>()), (16, 135));
    let mut sum = 0;
    let crossing = values
        .iter()
        .position(|v| {
            sum += v;
            sum >= SCORE_TARGET
        })
        .unwrap()
        + 1;
    assert_eq!(crossing, 14);
    // Played out: red has 85 from black's thirteen cheapest pieces and takes a
    // chariot; the game ends on score with two black pieces left.
    let mut state = playing(&[("a1", 3), ("a3", 10), ("i3", 10), ("i5", 7)]);
    state.scores = [85, 0];
    let next = state.apply(mv("a1", "a3"), None).unwrap();
    assert_eq!(next.status, finished(Some(Seat::Red), Reason::Score));
    assert_eq!(next.board.iter().filter(|&&c| c >= 7).count(), 2);
}

#[test]
fn illegal_actions_preserve_state_and_public_fen_is_not_a_save_format() {
    let state = playing(&[("a1", 6), ("i5", 13)]);
    let saved = state.clone();
    assert_eq!(
        state.apply(mv("a1", "c1"), None),
        Err(MoveError::IllegalMove)
    );
    assert_eq!(
        state.apply((255, 255), Some(0)),
        Err(MoveError::IllegalMove)
    );
    assert_eq!(
        state.apply(mv("a1", "b1"), Some(0)),
        Err(MoveError::InvalidReveal)
    );
    let mut next = state.apply(mv("a1", "b1"), None).unwrap();
    next.board[sq("b1") as usize] = 0;
    next.scores[0] = 50;
    assert_eq!(state, saved);
    assert_eq!(
        State::initial().public_fen(),
        "XXXX1XXXX/XXXX1XXXX/9/XXXX1XXXX/XXXX1XXXX r - 0 0 0 1"
    );
    let mut exposed = playing(&[("a1", 0), ("i5", 10), ("b2", DOWN)]);
    exposed.first_color = Some(Ink::Black);
    exposed.ply = 1;
    exposed.scores = [20, 15];
    exposed.no_progress = 12;
    assert_eq!(exposed.public_fen(), "8r/9/9/1X7/G8 b b 20 15 12 1");
    let fen = exposed.public_fen();
    exposed.bag[11] = 1;
    exposed.chased_square = Some(sq("a1"));
    exposed.history = vec![exposed.position_key(); 2];
    assert_eq!(exposed.public_fen(), fen); // pool, chase and history MUST travel outside FEN.
}
