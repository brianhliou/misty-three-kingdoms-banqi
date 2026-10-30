# MistyThreeKingdomsBanqi

[![ci](https://github.com/brianhliou/misty-three-kingdoms-banqi/actions/workflows/ci.yml/badge.svg)](https://github.com/brianhliou/misty-three-kingdoms-banqi/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/brianhliou/misty-three-kingdoms-banqi)](https://github.com/brianhliou/misty-three-kingdoms-banqi/releases/latest)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

An engine for **Three Kingdoms banqi** (三國暗棋) in Rust: αβ search with **Star1 chance-node
expectiminimax** for the hidden-tile flips, a transposition table, capture quiescence and a
handcrafted evaluation. Ships as a small, dependency-free UCI binary.

Three Kingdoms banqi is a hidden-information game like [Banqi](https://en.wikipedia.org/wiki/Banqi)
(Chinese Dark Chess), played on a 9x5 board where pieces move like xiangqi pieces and every
capture scores points. Pieces start face-down and flip to reveal during play, so a turn is either
a deterministic move or capture, or a chance event drawn from the bag of unrevealed pieces. It is a
sibling of [MistyBanqi](https://github.com/brianhliou/misty-banqi) and shares its search design,
written for [Mistboard](https://mistboard.com), which hosts banqi and its variants.

## Rules

The rules the engine plays are Mistboard's. The mode comes from Gamesofa's mobile app 神來也暗棋2
(2026), which publishes no rules text for it, so the open questions (what a chase limit does,
whether a capture resets the clock, and so on) were decided from observed play and written down.
This project is not affiliated with Gamesofa.

- **Board.** Nine files (a to i) by five ranks (1 to 5). The 32 pieces start face-down in four 4x2
  blocks at the corners; file e and rank 3 start empty.
- **Pieces.** Each side has a general, two advisors, two elephants, two chariots, two horses, two
  cannons and five soldiers.
- **Movement.** Xiangqi moves with no palace, river or blocking: the general and the chariot slide
  orthogonally any distance; the cannon slides the same way and captures by jumping exactly one
  piece; the horse leaps one-and-two with no leg block; the elephant jumps two points diagonally
  with no eye block; the advisor steps one point diagonally; the soldier steps one point
  orthogonally in any direction. There is no check.
- **Captures.** Any face-up piece may capture any face-up enemy piece it reaches; there is no rank
  order. Face-down tiles cannot be captured, block lines, and can screen a cannon.
- **Turns.** A turn is a move or a flip. The red seat moves first, and the colour it flips first
  becomes its colour; a flip may reveal an enemy piece.
- **Scoring.** A capture scores the piece's points: general 20, chariot 15, horse 10, cannon 10,
  advisor 5, elephant 5, soldier 5. The first side to 100 wins at once.
- **40-ply count.** After 40 plies with no capture and no flip, the game ends on score: the higher
  score wins, equal scores draw.
- **Perpetual chase.** A chase (a quiet move after which the moved piece could capture the enemy
  piece that just moved) may not make its resulting position appear a third time, unless it is the
  only legal move. Only the chaser is limited, and repetition is never a draw.
- **No legal move loses.** A wipe-out cannot happen: a side's sixteen pieces total 135 points and its
  thirteen cheapest total 85, so the capture that would take a side's last piece comes after 100.

## How it works

**Search: αβ + Star1 over a mixed decision/chance tree.** A flip is a chance node: the engine
knows only the public bag of covered pieces, not which one a given tile holds. Star1 takes the
probability-weighted expectiminimax value over the exact bag outcomes with αβ-style bounds,
pruning chance branches instead of expanding all of them. Decision nodes are ordinary αβ negamax
with iterative deepening, capture and hash move ordering, capture quiescence and a bounded
transposition table. A stopped iteration never replaces the last complete one, and a deterministic
legal fallback covers a budget too small to finish depth one.

**Repetition, scored the way the clock would settle it.** The perpetual-chase rule depends on
history, so the chased square, the positions since the last flip or capture, and the last move are
all part of the search state and the transposition key. Every searched move comes from the rules'
own legal-move list, so a refused chase is never searched. A position that repeats below the root
is scored as the 40-ply count would end it: with no capture or flip the scores cannot change, so
the side ahead wins, the side behind loses, and equal scores draw.

**Evaluation.** Terminal results (100 points, the 40-ply count, no legal move) are exact utilities
of -1, 0 and 1. Otherwise the score margin dominates, with smaller terms for remaining movement
potential (including what the covered bag can still produce) and mobility; heuristic values stay
inside +/-0.95. The coefficients are new for this game and untuned.

**One level.** The engine plays at 2,000,000 nodes a move with an 8 s time cap that only bounds
latency on a slow host. Strength is the node budget, so it plays the same on any CPU.

## Strength

There is no human or third-party engine result to quote. What has been measured:

- Over 400 games (200 paired deals, seats swapped), the shipped level scored 84.9% (339 wins, 1
  draw, 60 losses) against the engine's first search at a 2,000-node budget.
- That first search, at 2,000 nodes, won 400 of 400 games against a greedy baseline (an immediate
  win, else the largest capture, else a random flip, else a random move).

The current search is the first one made about three times faster, not changed:
`tests/candidate.rs` holds the two equal node for node.

## Correctness

The rules are a port of Mistboard's TypeScript rules kernel, and `tests/oracle.rs` replays 42
recorded kernel games (10,157 positions, `tests/fixtures/tk-oracle.jsonl`) against the Rust state:
legal moves, refused chases, chased square, repetition counts, scores and results must all agree.
`tests/search.rs` checks the search against an independent, unpruned expectimax over every one-
and two-piece bag, with Star1 and the transposition table each switched on and off.

## Build & run

```sh
cargo build --release
echo "uci" | ./target/release/three-kingdoms-engine   # id name MistyThreeKingdomsBanqi 0.1.0
cargo test --release                                  # debug-mode tests are much slower
```

Play against it in a terminal (moves as `e3e4`, flip a tile with `e3`):

```sh
cargo run --release --example play -- --seat red --seed 1
```

## UCI protocol

A UCI subset, one process per request or a long-running session. The engine never sees a covered
tile's identity.

```
uci                                          -> id name ..., id author ..., uciok
isready                                      -> readyok
ucinewgame
position startpos [moves m1 m2 ...]
position fen <FEN> [moves m1 m2 ...]
go [nodes N] [movetime MS] [depth D] [searchmoves m1 m2 ...]
                                             -> info depth D nodes N time MS score cp C pv M
                                                bestmove M
quit
```

Bare `go` plays the level; `movetime 0` removes the time cap (node budget only, reproducible).
`searchmoves` restricts the root to the listed moves. A position the engine refuses, or a finished
game, answers `info string error <why>` and `bestmove (none)`. `score cp` is the value in
thousandths of a win, from the side to move.

**Moves** are `<from><to>` on points a1 to i5 (rank 1 at the bottom). A flip has from equal to
to (`e3e3`).

**FEN** has nine space-separated fields:

```
<board> <turn> <first> <red score> <black score> <clock> <movenum> <pool> <last>
```

- `board`: five ranks separated by `/`, rank 5 first, nine files per rank. A face-up piece is its
  letter (G general, A advisor, E elephant, R chariot, H horse, C cannon, S soldier), upper case for
  red, lower case for black; `X` is a covered tile; a digit is a run of empty points.
- `turn`: `r` or `b`, the seat to move (the red seat moves first), not a colour.
- `first`: the colour the red seat flipped first (`r` or `b`), `-` before the first flip.
- the two scores by seat, the 40-ply clock, and the 1-based move number.
- `pool`: the covered pieces as letter-count pairs, such as `G1A2E2R2H2C2S5g1a2e2r2h2c2s5`, or `-`
  when nothing is covered. These are public: both players can derive them from the start counts, the
  face-up pieces and the captures. The counts must sum to the number of `X` tiles.
- `last`: the move that produced this position (`e3e4`, or a flip `a1a1`), or `-`. It decides
  whether the next quiet move is a chase.

To apply the perpetual-chase rule exactly, send the position right after the last flip or capture
and every ply since as `moves`; the engine replays them to rebuild the chase history. Flips cannot
appear in `moves`, since their outcome is not in the text. A FEN of the current position with no
moves is accepted, with history starting there. Every field is validated, scores included against
the captures the pool implies; nothing is guessed.

Example, the red seat to move with a capture available:

```
position fen XXXX1XXXX/XXXX1XXXX/9/XXXX1XXXX/RsXX1XXXX r r 0 0 0 2 G1A2E2R1H2C2S5g1a2e2r2h2c2s4 b1b1
go nodes 50000
info depth 1 nodes 50000 time 22 score cp 43 pv a1b1
bestmove a1b1
```

## Layout

```
three_kingdoms_banqi_rust/
  src/lib.rs          rules state: move generation, chase rule, results, public FEN
  src/engine.rs       the search and evaluation, and the level
  src/reference.rs    the first search, frozen; the engine is held equal to it
  src/uci.rs          the UCI contract: FEN parsing, move text, the command session
  src/bin/            the three-kingdoms-engine binary
  tests/              oracle replay, search checks, engine-vs-reference and UCI round trips
  examples/play.rs    play against the engine in a terminal
.github/workflows/    ci (test + uci smoke) and release (tag -> published binary)
```

## License

MIT, see [LICENSE](LICENSE).
