//! Check the public executable over stdin/stdout, including option transitions.
mod common;

use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::Arc,
};
use three_kingdoms_banqi_rust::{
    engine::{ChanceMode, Eval, Searcher},
    nnue::Net,
    reference,
    uci::{self, move_to_uci},
    State, DOWN,
};

const GO: &str = "go nodes 5000 movetime 0";

struct Binary {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Binary {
    fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_three-kingdoms-engine"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
        }
    }
    fn ask(&mut self, command: &str) -> Vec<String> {
        writeln!(self.input, "{command}\nisready").unwrap();
        self.input.flush().unwrap();
        let mut replies = Vec::new();
        loop {
            let mut line = String::new();
            assert_ne!(
                self.output.read_line(&mut line).unwrap(),
                0,
                "engine exited"
            );
            let line = line.trim_end();
            if line == "readyok" {
                return replies;
            }
            replies.push(line.to_string());
        }
    }
    fn position(&mut self, command: &str) {
        assert!(self.ask(command).is_empty(), "{command}");
    }
}
impl Drop for Binary {
    fn drop(&mut self) {
        let _ = writeln!(self.input, "quit");
        let _ = self.input.flush();
        let _ = self.child.wait();
    }
}

type ResultFields = (String, i64, u8, u64);
fn result(lines: Vec<String>) -> ResultFields {
    assert_eq!(lines.len(), 2, "{lines:?}");
    let words: Vec<_> = lines[0].split_whitespace().collect();
    let field = |name| words[words.iter().position(|&w| w == name).unwrap() + 1];
    (
        lines[1].strip_prefix("bestmove ").unwrap().into(),
        field("cp").parse().unwrap(),
        field("depth").parse().unwrap(),
        field("nodes").parse().unwrap(),
    )
}
fn fixtures() -> Vec<(&'static str, ResultFields)> {
    let rows: Vec<_> = include_str!("fixtures/uci-v010.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(fields.len(), 5);
            (
                fields[0],
                (
                    fields[1].into(),
                    fields[2].parse().unwrap(),
                    fields[3].parse().unwrap(),
                    fields[4].parse().unwrap(),
                ),
            )
        })
        .collect();
    assert_eq!(rows.len(), 200);
    rows
}
fn parse(command: &str) -> State {
    let (fen, moves) = command
        .strip_prefix("position fen ")
        .unwrap()
        .split_once(" moves")
        .unwrap();
    uci::parse_position(fen, &moves.split_whitespace().collect::<Vec<_>>()).unwrap()
}
fn expected(command: &str, eval: Eval, chance_mode: ChanceMode) -> ResultFields {
    let mut config = common::config();
    config.chance_mode = chance_mode;
    let result = Searcher::new(config.with_eval(eval)).search(&parse(command));
    (
        result.best_move.map(move_to_uci).unwrap_or("(none)".into()),
        (result.value * 1000.).round() as i64,
        result.completed_depth,
        result.nodes,
    )
}

#[test]
fn default_and_explicit_false_preserve_010_fixtures_except_corrected_repetitions() {
    // Keep the historical 0.1.0 fixture immutable. These four positions used
    // the incorrect cycle-settlement shortcut in their search trees (#1).
    // Check them against the corrected independent reference; every other
    // position must retain its exact historical UCI output.
    const REPETITION_AFFECTED: [usize; 4] = [81, 103, 104, 167];
    let mut binary = Binary::new();
    for explicit in [false, true] {
        if explicit {
            binary.position("setoption name ChanceMix value true");
            binary.position("setoption name ChanceMix value false");
        }
        for (index, (command, baseline)) in fixtures().into_iter().enumerate() {
            let expected = if REPETITION_AFFECTED.contains(&index) {
                let config = common::config();
                let corrected = reference::Searcher::new(reference::SearchConfig {
                    max_depth: config.max_depth,
                    node_budget: config.node_budget,
                    time_limit: config.time_limit,
                    quiescence_depth: config.quiescence_depth,
                    tt_capacity: config.tt_capacity,
                    star1: config.star1,
                })
                .search(&parse(command));
                let corrected = (
                    corrected
                        .best_move
                        .map(move_to_uci)
                        .unwrap_or("(none)".into()),
                    (corrected.value * 1000.0).round() as i64,
                    corrected.completed_depth,
                    corrected.nodes,
                );
                assert_ne!(
                    corrected, baseline,
                    "unneeded repetition exception: {index}"
                );
                corrected
            } else {
                baseline
            };
            binary.position("ucinewgame");
            binary.position(command);
            assert_eq!(result(binary.ask(GO)), expected, "{command}");
        }
    }
}

#[test]
fn all_option_combinations_match_public_search_on_200_positions() {
    let file = common::TestFile::new(&common::net_bytes(7));
    let net = Arc::new(Net::load(&file.path).unwrap());
    let mut binary = Binary::new();
    let mut changed_by_mix = 0;
    let mut changed_by_net = 0;
    for use_net in [false, true] {
        for mix in [false, true] {
            let mode = if mix {
                ChanceMode::mix()
            } else {
                ChanceMode::Star1
            };
            let eval = if use_net {
                Eval::Nnue(net.clone())
            } else {
                Eval::Classical
            };
            binary.position(&format!("setoption name ChanceMix value {mix}"));
            binary.position(&format!(
                "setoption name EvalFile value {}",
                if use_net {
                    file.path.to_str().unwrap()
                } else {
                    "<empty>"
                }
            ));
            for (command, baseline) in fixtures() {
                binary.position("ucinewgame");
                binary.position(command);
                let actual = result(binary.ask(GO));
                assert_eq!(
                    actual,
                    expected(command, eval.clone(), mode),
                    "net={use_net}, mix={mix}, {command}"
                );
                if use_net {
                    changed_by_net += usize::from(actual.0 != baseline.0 || actual.1 != baseline.1);
                } else if mix {
                    changed_by_mix += usize::from(actual.0 != baseline.0 || actual.1 != baseline.1);
                }
            }
        }
    }
    assert!(changed_by_mix > 0, "must observe ChanceMix being enabled");
    assert!(changed_by_net > 0, "must observe EvalFile being loaded");
}

fn witness() -> &'static str {
    fixtures()
        .into_iter()
        .map(|(command, _)| command)
        .find(|command| {
            let mix = expected(command, Eval::Classical, ChanceMode::mix());
            let exact = expected(command, Eval::Classical, ChanceMode::Star1);
            mix.0 != exact.0 || mix.1 != exact.1
        })
        .expect("fixtures must contain a position that distinguishes chance modes")
}

#[test]
fn advertised_options_validate_values_and_keep_the_previous_mode_on_error() {
    let mut binary = Binary::new();
    let options = binary.ask("uci");
    for line in [
        "id name MistyThreeKingdomsBanqi 0.3.0",
        "option name ChanceMix type check default false",
        "option name EvalFile type string default <empty>",
        "uciok",
    ] {
        assert!(options.contains(&line.to_string()));
    }
    let command = witness();
    for (value, mode) in [("true", ChanceMode::mix()), ("false", ChanceMode::Star1)] {
        binary.position(&format!("setoption name ChanceMix value {value}"));
        let wanted = expected(command, Eval::Classical, mode);
        for invalid in ["", "TRUE", "0", "maybe", "true false", "<empty>"] {
            assert_eq!(
                binary.ask(&format!("setoption name ChanceMix value {invalid}")),
                ["info string error ChanceMix: expected true or false"]
            );
            binary.position("ucinewgame");
            binary.position(command);
            assert_eq!(result(binary.ask(GO)), wanted);
        }
    }
}

#[test]
fn bad_or_cleared_evalfile_reverts_to_classical_and_preserves_chance_mode() {
    let valid = common::TestFile::new(&common::net_bytes(7));
    let truncated = common::TestFile::new(b"TKNN\x01\0\0\0");
    let mut unsafe_weights = common::net_bytes(1);
    unsafe_weights[24..28].copy_from_slice(&200f32.to_le_bytes());
    let unsafe_file = common::TestFile::new(&unsafe_weights);
    assert!(!Net::load(&unsafe_file.path).unwrap().can_quantize());
    let missing = valid.path.with_file_name("missing net.bin");
    let command = witness();
    for mix in [false, true] {
        let mode = if mix {
            ChanceMode::mix()
        } else {
            ChanceMode::Star1
        };
        let mut binary = Binary::new();
        let wanted = expected(command, Eval::Classical, mode);
        for (reset, error) in [
            (missing.to_str().unwrap(), true),
            (truncated.path.to_str().unwrap(), true),
            (unsafe_file.path.to_str().unwrap(), true),
            ("<empty>", false),
            ("", false),
        ] {
            // Reverse the setting order used by the all-combinations test.
            binary.position(&format!(
                "setoption name EvalFile value {}",
                valid.path.display()
            ));
            binary.position(&format!("setoption name ChanceMix value {mix}"));
            binary.position(command);
            assert_eq!(
                result(binary.ask(GO)),
                expected(
                    command,
                    Eval::Nnue(Arc::new(Net::load(&valid.path).unwrap())),
                    mode
                )
            );
            let reply = binary.ask(&format!("setoption name EvalFile value {reset}"));
            if error {
                assert_eq!(reply.len(), 1);
                assert!(reply[0].starts_with("info string error EvalFile:"));
                assert!(reply[0].ends_with("using classical"));
            } else {
                assert!(reply.is_empty());
            }
            binary.position("ucinewgame");
            binary.position(command);
            assert_eq!(result(binary.ask(GO)), wanted, "reset={reset}");
        }
    }
}

#[test]
fn paths_with_spaces_and_protocol_whitespace_load_a_generated_net() {
    let file = common::TestFile::new(&common::net_bytes(1));
    let command = fixtures()[7].0;
    let mut binary = Binary::new();
    binary.position(&format!(
        "setoption\tname  evalfile\tvalue  {}",
        file.path.display()
    ));
    binary.position("setoption name chancemix value true");
    binary.position(command);
    assert_eq!(
        result(binary.ask(GO)),
        expected(
            command,
            Eval::Nnue(Arc::new(Net::load(&file.path).unwrap())),
            ChanceMode::mix()
        )
    );
}

#[test]
fn swapping_unseen_deal_identities_preserves_protocol_results() {
    let deal = common::deal(7123);
    let covered: Vec<_> = State::initial()
        .board
        .iter()
        .enumerate()
        .filter(|(_, code)| **code == DOWN)
        .map(|(sq, _)| sq)
        .collect();
    let a = *covered.last().unwrap();
    let b = *covered.iter().find(|&&sq| deal[sq] != deal[a]).unwrap();
    let mut swapped = deal;
    swapped.swap(a, b);
    assert_ne!(deal, swapped);
    let mut commands = Vec::new();
    for identities in [deal, swapped] {
        let mut states = vec![State::initial()];
        let mut moves = Vec::new();
        for &sq in covered.iter().filter(|&&sq| sq != a && sq != b).take(12) {
            let mv = (sq as u8, sq as u8);
            states.push(
                states
                    .last()
                    .unwrap()
                    .apply(mv, Some(identities[sq]))
                    .unwrap(),
            );
            moves.push(mv);
        }
        let (base, tail) = uci::window(&states, &moves);
        commands.push(format!(
            "position fen {} moves {}",
            uci::engine_fen(base),
            tail.join(" ")
        ));
    }
    assert_eq!(commands[0], commands[1]);
    let file = common::TestFile::new(&common::net_bytes(7));
    let net = Arc::new(Net::load(&file.path).unwrap());
    let mut binary = Binary::new();
    for use_net in [false, true] {
        binary.position(&format!(
            "setoption name EvalFile value {}",
            if use_net {
                file.path.to_str().unwrap()
            } else {
                "<empty>"
            }
        ));
        for mix in [false, true] {
            binary.position(&format!("setoption name ChanceMix value {mix}"));
            let outputs: Vec<_> = commands
                .iter()
                .map(|command| {
                    binary.position(command);
                    result(binary.ask(GO))
                })
                .collect();
            assert_eq!(outputs[0], outputs[1]);
            assert_eq!(
                outputs[0],
                expected(
                    &commands[0],
                    if use_net {
                        Eval::Nnue(net.clone())
                    } else {
                        Eval::Classical
                    },
                    if mix {
                        ChanceMode::mix()
                    } else {
                        ChanceMode::Star1
                    }
                )
            );
        }
    }
}
