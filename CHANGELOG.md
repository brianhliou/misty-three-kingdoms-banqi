# Changelog

All notable changes to MistyThreeKingdomsBanqi.

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
