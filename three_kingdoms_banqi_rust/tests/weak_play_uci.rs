mod common;
use std::{
    collections::HashSet,
    io::{Read, Write},
    process::{Command, Stdio},
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};
use three_kingdoms_banqi_rust::{
    engine::{ChanceMode, Eval, PlayOptions, PlayRng, SearchConfig, Searcher},
    nnue::Net,
    uci::{self, Session},
};

fn run(commands: &str, timeout: Duration) -> Vec<String> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_three-kingdoms-engine"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut output = String::new();
        stdout.read_to_string(&mut output).unwrap();
        let _ = tx.send(output);
    });
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{commands}\nquit").unwrap();
    drop(stdin);
    let output = match rx.recv_timeout(timeout) {
        Ok(output) => output,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("UCI exceeded {timeout:?}: {error}");
        }
    };
    assert!(child.wait().unwrap().success());
    output.lines().map(str::to_string).collect()
}
fn field<'a>(line: &'a str, key: &str) -> &'a str {
    let mut words = line.split_whitespace();
    words.find(|&word| word == key).unwrap();
    words.next().unwrap()
}
fn result(lines: &[String]) -> (String, i64, u8, u64) {
    let info = lines
        .iter()
        .find(|line| line.starts_with("info depth"))
        .unwrap();
    (
        lines
            .iter()
            .find(|line| line.starts_with("bestmove "))
            .unwrap()
            .clone(),
        field(info, "cp").parse().unwrap(),
        field(info, "depth").parse().unwrap(),
        field(info, "nodes").parse().unwrap(),
    )
}
fn fixture(index: usize) -> &'static str {
    include_str!("fixtures/uci-v010.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .nth(index)
        .unwrap()
        .split('\t')
        .next()
        .unwrap()
}

#[test]
fn advertises_exact_contract_and_validates_without_replacing_options() {
    let mut s = Session::default();
    let options: Vec<_> = s
        .handle("uci")
        .into_iter()
        .filter(|line| line.starts_with("option "))
        .collect();
    assert_eq!(
        options,
        [
            "option name EvalFile type string default <empty>",
            "option name ChanceMix type check default false",
            "option name Noise type string default 0",
            "option name NoiseDepth type spin default 1 min 1 max 8",
            "option name Seed type string default 0",
        ]
    );
    assert!(s
        .handle("uci")
        .contains(&"id name MistyThreeKingdomsBanqi 0.3.1".into()));
    for line in [
        "setoption name Noise value 0.3",
        "setoption name NoiseDepth value 2",
        "setoption name Seed value 18446744073709551615",
        "setoption name ChanceMix value true",
    ] {
        assert!(s.handle(line).is_empty());
    }
    s.handle(fixture(2));
    let expected = result(&s.handle("go nodes 10000 movetime 0"));
    for (name, bad) in [
        ("Noise", "-1"),
        ("Noise", "NaN"),
        ("Noise", "inf"),
        ("Noise", "text"),
        ("Noise", ""),
        ("NoiseDepth", "0"),
        ("NoiseDepth", "9"),
        ("NoiseDepth", "1.5"),
        ("NoiseDepth", "-1"),
        ("Seed", "-1"),
        ("Seed", "18446744073709551616"),
        ("Seed", "1 2"),
    ] {
        assert_eq!(
            s.handle(&format!("setoption name {name} value {bad}")),
            ["info string invalid or unknown option"]
        );
        s.handle("ucinewgame");
        s.handle(fixture(2));
        assert_eq!(result(&s.handle("go nodes 10000 movetime 0")), expected);
    }
}

#[test]
fn binary_and_library_share_noisy_classical_net_and_chancemix_selection() {
    let dir = std::env::temp_dir().join(format!("tk weak play {}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test net.tknn");
    std::fs::write(&path, common::net_bytes(16)).unwrap();
    let mut differs_from_classical = false;
    for mix in [false, true] {
        let mut classical = Vec::new();
        for net in [false, true] {
            for index in [0, 2, 7] {
                let command = fixture(index);
                let rest = command.strip_prefix("position fen ").unwrap();
                let (fen, tail) = rest.split_once(" moves").unwrap();
                let state =
                    uci::parse_position(fen, &tail.split_whitespace().collect::<Vec<_>>()).unwrap();
                let mut c = SearchConfig::level(None);
                c.node_budget = 10000;
                c.chance_mode = if mix {
                    ChanceMode::mix()
                } else {
                    ChanceMode::Star1
                };
                let eval = if net {
                    Eval::Nnue(Arc::new(Net::load(&path).unwrap()))
                } else {
                    Eval::Classical
                };
                let play = PlayOptions {
                    noise: 0.15,
                    noise_depth: 2,
                };
                let expected = Searcher::new(c.with_eval(eval)).select_move_with_options(
                    &state,
                    &[],
                    &play,
                    &mut PlayRng::for_position(123, &state),
                );
                let commands = format!(
                    "uci\nsetoption name Noise value 0.15\nsetoption name NoiseDepth value 2\nsetoption name Seed value 123\nsetoption name EvalFile value {}\nsetoption name ChanceMix value {mix}\nucinewgame\n{command}\ngo nodes 10000 movetime 0",
                    if net { path.to_str().unwrap() } else { "<empty>" });
                let actual = result(&run(&commands, Duration::from_secs(10)));
                assert_eq!(
                    actual,
                    (
                        format!("bestmove {}", uci::move_to_uci(expected.best_move.unwrap())),
                        (expected.value * 1000.0).round() as i64,
                        expected.completed_depth,
                        expected.nodes
                    )
                );
                if net {
                    differs_from_classical |= actual != classical[index_to_slot(index)];
                } else {
                    classical.push(actual);
                }
            }
        }
    }
    assert!(differs_from_classical, "NNUE must actually affect search");
    std::fs::remove_dir_all(dir).unwrap();
}
fn index_to_slot(index: usize) -> usize {
    match index {
        0 => 0,
        2 => 1,
        7 => 2,
        _ => unreachable!(),
    }
}

#[test]
fn binary_seeded_replay_varies_across_seeds_and_zero_noise_matches_fixtures() {
    let commands = format!("setoption name Noise value 0.3\nsetoption name NoiseDepth value 1\nsetoption name Seed value 123\nucinewgame\n{}\ngo nodes 10000 movetime 0\ngo nodes 10000 movetime 0", fixture(0));
    let lines = run(&commands, Duration::from_secs(5));
    let best: Vec<_> = lines
        .iter()
        .filter(|line| line.starts_with("bestmove "))
        .collect();
    assert_eq!(best.len(), 2);
    assert_eq!(best[0], best[1]);
    let infos: Vec<_> = lines
        .iter()
        .filter(|line| line.starts_with("info depth"))
        .collect();
    for key in ["depth", "nodes", "cp", "pv"] {
        assert_eq!(field(infos[0], key), field(infos[1], key));
    }
    let mut s = Session::default();
    s.handle("setoption name Noise value 3");
    s.handle("position startpos");
    let mut seen = HashSet::new();
    for seed in 1..17 {
        s.handle(&format!("setoption name Seed value {seed}"));
        seen.insert(result(&s.handle("go nodes 10000 movetime 0")).0);
    }
    assert!(seen.len() > 1);
    s.handle("setoption name Noise value 0");
    s.handle("setoption name NoiseDepth value 8");
    for row in include_str!("fixtures/uci-v010.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .take(8)
    {
        let fields: Vec<_> = row.split('\t').collect();
        s.handle(fields[0]);
        assert_eq!(
            result(&s.handle("go nodes 5000 movetime 0")),
            (
                format!("bestmove {}", fields[1]),
                fields[2].parse().unwrap(),
                fields[3].parse().unwrap(),
                fields[4].parse().unwrap()
            )
        );
    }
}

#[test]
fn zero_noise_binary_matches_unset_options_on_startpos_and_midgame_fixtures() {
    let positions = [
        "position startpos",
        fixture(2),
        fixture(7),
        fixture(50),
        fixture(100),
        fixture(175),
    ];
    for position in positions {
        let baseline = result(&run(
            &format!("{position}\ngo nodes 5000 movetime 0"),
            Duration::from_secs(5),
        ));
        for seed in [1, 123, u64::MAX] {
            for depth in [1, 4, 8] {
                let actual = result(&run(
                    &format!(
                        "setoption name Noise value 0\nsetoption name NoiseDepth value {depth}\nsetoption name Seed value {seed}\n{position}\ngo nodes 5000 movetime 0"
                    ),
                    Duration::from_secs(5),
                ));
                assert_eq!(
                    actual, baseline,
                    "{position}, Seed={seed}, NoiseDepth={depth}"
                );
            }
        }
    }
}

#[test]
fn noisy_opening_depth_four_obeys_nodes_and_movetime() {
    for go in [
        "go nodes 10000 movetime 0",
        "go nodes 18446744073709551615 movetime 100",
    ] {
        let started = Instant::now();
        let lines = run(&format!("setoption name Noise value 0.01\nsetoption name NoiseDepth value 4\nsetoption name Seed value 123\nposition startpos\n{go}"), Duration::from_secs(3));
        assert!(started.elapsed() < Duration::from_secs(3));
        let (best, _, depth, nodes) = result(&lines);
        assert_ne!(best, "bestmove (none)");
        assert!((1..4).contains(&depth));
        if go.starts_with("go nodes 10000 ") {
            assert_eq!(nodes, 10000);
        } else {
            assert!(nodes > 0);
        }
    }
}
