# Changelog

All notable changes to MistyThreeKingdomsBanqi.

## [0.3.1] (unreleased)

- Fix #1: repeated positions continue through search rather than receiving an
  immediate settlement value. Move generation still refuses a chase's third
  occurrence under Mistboard's rules, with the existing only-move exception.
- Widen NNUE's sigmoid utility range below the exact settlement bounds and
  blend in 5% public score margin so winning moves retain useful ordering even
  when network activations saturate.
- Regressions cover the reported chase, the third-occurrence restriction,
  utility ordering and bounds, and the L8 `c3a5` move with an external tk-v17q net.

## [0.3.0]

- UCI `Noise`, `NoiseDepth` and `Seed` options for weaker play. Positive Noise
  adds Gaussian noise to every root move's utility, including flips, with a
  scoring horizon of 1 to 8 plies and replayable nonzero seeds.
- Immediate terminal wins take priority; moves that allow an immediate loss
  under any reveal are excluded unless every move is unsafe. Deeper scoring
  passes share node/time limits and retain the last complete pass, after a
  mandatory depth-one pass. Noise=0 preserves the original search unchanged.
- Library and UCI tests cover terminal guards, seeded replay, both chance modes,
  generated NNUE nets, budgets and zero-noise bestmove/score/depth/nodes parity.
  The package and UCI engine name now report 0.3.0.

## [0.2.0]

- Optional ChanceMix search (`setoption name ChanceMix value true`, default off):
  probe every possible flip identity at reduced depth, then search the most probable
  identity at full depth and combine the results by their public bag probabilities.
- Optional NNUE evaluation (`setoption name EvalFile value <path>`) reads TKNN v1
  files; evaluation stays classical when unset. No net ships with the engine.
  Clearing the option restores classical evaluation; invalid files also report load errors through UCI.
- UCI binary `three-kingdoms-engine` reports `id name MistyThreeKingdomsBanqi 0.2.0`.
  Tests cover both options, generated nets, fixed UCI fixtures and the original
  node-for-node equality check against the frozen reference with default options.

## [0.1.0]

- Initial standalone engine for Three Kingdoms banqi (三國暗棋): αβ + **Star1 chance-node**
  search over exact bag outcomes, capture quiescence, a bounded transposition table, and
  perpetual-chase history in the search state; one level at 2,000,000 nodes a move with an 8 s
  time cap.
- UCI binary `three-kingdoms-engine` (`id name MistyThreeKingdomsBanqi 0.1.0`): `position
  startpos|fen ... [moves ...]`, `go [nodes] [movetime] [depth] [searchmoves]`, with a validated
  FEN that carries the covered-piece pool and the last move.
- Tests: a differential replay of 42 games (10,157 positions) recorded from Mistboard's
  TypeScript rules kernel, an unpruned expectimax check of the search, and a node-for-node
  equality check of the engine against its frozen reference search.
