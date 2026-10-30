//! UCI-style frontend for Three Kingdoms banqi. Protocol: `uci` module docs.

use std::io::{self, BufRead, Write};
use three_kingdoms_banqi_rust::uci::Session;

fn main() {
    let mut session = Session::default();
    let stdout = io::stdout();
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim() == "quit" {
            break;
        }
        let mut out = stdout.lock();
        for reply in session.handle(&line) {
            writeln!(out, "{reply}").ok();
        }
        // Hosts read line by line; flush every response.
        out.flush().ok();
    }
}
