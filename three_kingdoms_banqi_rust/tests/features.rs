mod common;

use three_kingdoms_banqi_rust::{
    engine::{ChanceMode, Searcher},
    features::{active, reflect, reflect_point, N_FEATURES},
    Seat, State, DOWN,
};

#[test]
fn public_features_and_legal_moves_preserve_board_reflections() {
    let mut chases = 0;
    for state in common::positions(6000) {
        for symmetry in 0..4 {
            let reflected = reflect(&state, symmetry);
            assert_eq!(reflect(&reflected, symmetry), state);
            let mut expected: Vec<_> = state
                .legal_moves()
                .iter()
                .map(|&(a, b)| (reflect_point(a, symmetry), reflect_point(b, symmetry)))
                .collect();
            let mut actual = reflected.legal_moves();
            expected.sort();
            actual.sort();
            assert_eq!(actual, expected);
        }
        for seat in [Seat::Red, Seat::Black] {
            let features = active(&state, seat);
            assert!(features.windows(2).all(|pair| pair[0] < pair[1]));
            assert!(features.iter().all(|&i| (i as usize) < N_FEATURES));
        }
        chases += usize::from(state.chased_square.is_some());
    }
    assert!(chases > 50, "only {chases} chased positions");
}

#[test]
fn unseen_identity_swaps_cannot_change_features_or_mix_search() {
    let original = common::deal(7123);
    let mut swapped = original;
    let covered: Vec<_> = State::initial()
        .board
        .iter()
        .enumerate()
        .filter(|(_, code)| **code == DOWN)
        .map(|(sq, _)| sq)
        .collect();
    let a = *covered.last().unwrap();
    let b = *covered
        .iter()
        .find(|&&sq| original[sq] != original[a])
        .unwrap();
    swapped.swap(a, b);
    assert_ne!(original, swapped);
    let mut states = [State::initial(), State::initial()];
    let mut config = common::config();
    config.node_budget = 2000;
    config.chance_mode = ChanceMode::mix();
    for &square in covered.iter().filter(|&&sq| sq != a && sq != b).take(12) {
        assert_eq!(states[0], states[1]);
        for seat in [Seat::Red, Seat::Black] {
            assert_eq!(active(&states[0], seat), active(&states[1], seat));
        }
        assert_eq!(
            Searcher::new(config.clone()).search(&states[0]),
            Searcher::new(config.clone()).search(&states[1])
        );
        for (state, deal) in states.iter_mut().zip([original, swapped]) {
            *state = state
                .apply((square as u8, square as u8), Some(deal[square]))
                .unwrap();
        }
    }
    assert_eq!(states[0], states[1]);
}
