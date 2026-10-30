//! Play the engine's level in a terminal.
//!
//! cargo run --release --example play -- [--seat red|black] [--seed N]
//!
//! Enter moves as `e3e4`; flip a covered tile with `e3e3` (or just `e3`).
//! The deal is shuffled from the seed; the bot sees only what the frontend
//! would send it (FEN window + moves), never the covered identities.

use std::io::{self, BufRead, Write};

use three_kingdoms_banqi_rust::uci::{self, Session};
use three_kingdoms_banqi_rust::{Move, Seat, State, Status, DOWN, HEIGHT, NSQ, WIDTH};

fn deal(seed: u64) -> [i16; NSQ] {
    let mut s = seed;
    let mut next = || {
        s = s.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = s;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    };
    let start = State::initial();
    let mut codes = Vec::new();
    for (code, &n) in start.bag.iter().enumerate() {
        codes.extend(std::iter::repeat_n(code as i16, n as usize));
    }
    for i in (1..codes.len()).rev() {
        codes.swap(i, (next() % (i + 1) as u64) as usize);
    }
    let mut out = [-1; NSQ];
    let mut it = codes.into_iter();
    for (sq, &p) in start.board.iter().enumerate() {
        if p == DOWN {
            out[sq] = it.next().unwrap();
        }
    }
    out
}

fn show(state: &State) {
    let fen = state.public_fen();
    let board = fen.split(' ').next().unwrap();
    println!();
    for (i, row) in board.split('/').enumerate() {
        let mut line = String::new();
        for ch in row.chars() {
            match ch.to_digit(10) {
                Some(n) => (0..n).for_each(|_| line.push_str(" .")),
                None => {
                    line.push(' ');
                    line.push(ch);
                }
            }
        }
        println!("{} {line}", HEIGHT - i);
    }
    let files: String = (0..WIDTH)
        .map(|f| format!(" {}", (b'a' + f as u8) as char))
        .collect();
    println!("  {files}");
    println!(
        "scores red seat {} / black seat {}, clock {}/40  (upper case = red ink, X = covered)",
        state.scores[0], state.scores[1], state.no_progress
    );
}

fn main() {
    let mut seat = Seat::Red;
    let mut seed = 1u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match (a.as_str(), args.next()) {
            ("--seat", Some(v)) if v == "black" => seat = Seat::Black,
            ("--seat", Some(_)) => seat = Seat::Red,
            ("--seed", Some(v)) => seed = v.parse().expect("--seed N"),
            _ => panic!("usage: play [--seat red|black] [--seed N]"),
        }
    }
    let identities = deal(seed);
    let mut states = vec![State::initial()];
    let mut moves: Vec<Move> = Vec::new();
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        let state = states.last().unwrap().clone();
        if let Status::Finished { winner, reason } = state.status {
            show(&state);
            let verdict = match winner {
                Some(w) if w == seat => "you win",
                Some(_) => "the bot wins",
                None => "draw",
            };
            println!("game over: {verdict} ({reason:?})");
            return;
        }
        let mv = if state.turn() == seat {
            show(&state);
            print!("your move> ");
            io::stdout().flush().ok();
            let Some(Ok(text)) = lines.next() else { return };
            let text = text.trim();
            let text = if text.len() == 2 {
                format!("{text}{text}")
            } else {
                text.to_string()
            };
            match uci::uci_to_move(&text) {
                Some(mv) if state.legal_moves().contains(&mv) => mv,
                _ => {
                    println!("not a legal move: {text}");
                    continue;
                }
            }
        } else {
            let (start, window) = uci::window(&states, &moves);
            let mut command = format!("position fen {}", uci::engine_fen(start));
            if !window.is_empty() {
                command.push_str(" moves ");
                command.push_str(&window.join(" "));
            }
            let mut session = Session::default();
            session.handle(&command);
            let reply = session.handle("go");
            println!("bot: {}", reply[0]);
            uci::uci_to_move(reply[1].strip_prefix("bestmove ").unwrap()).expect("bot move")
        };
        let reveal = (mv.0 == mv.1).then_some(identities[mv.0 as usize]);
        states.push(state.apply(mv, reveal).expect("legal"));
        moves.push(mv);
    }
}
