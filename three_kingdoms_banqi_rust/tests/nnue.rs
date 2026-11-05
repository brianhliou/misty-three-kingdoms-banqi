mod common;

use std::sync::Arc;
use three_kingdoms_banqi_rust::{
    engine::{Eval, Searcher},
    features::{self, FeatureSet},
    nnue::{Accumulator, Net},
    Ink, Seat, State, EMPTY, NSQ,
};

#[test]
fn generated_nets_have_exact_incremental_updates_and_bounded_float_error() {
    let mut states = common::positions(10_000);
    let mut edge = State {
        board: [EMPTY; NSQ],
        bag: [0; 14],
        first_color: Some(Ink::Black),
        ..State::initial()
    };
    edge.board[0] = 6;
    edge.board[44] = 13;
    edge.history = vec![edge.position_key()];
    for ply in 0..40 {
        states.push(edge.clone());
        edge = edge
            .apply([(44, 43), (0, 1), (43, 44), (1, 0)][ply % 4], None)
            .unwrap();
    }
    let mut coverage = [false; 6];
    for state in &states {
        coverage[0] |= state.first_color.is_none();
        coverage[1] |= (1..32).contains(&state.bag.iter().sum::<u32>());
        coverage[2] |= !state.captures.is_empty();
        coverage[3] |= state.no_progress == 39;
        coverage[4] |= state.chased_square.is_some();
        coverage[5] |= state.occurrences(&state.position_key()) >= 2;
        let sets = FeatureSet::both(state);
        for seat in [Seat::Red, Seat::Black] {
            assert_eq!(
                sets[seat as usize]
                    .indices()
                    .map(|i| i as u16)
                    .collect::<Vec<_>>(),
                features::active(state, seat)
            );
        }
    }
    assert_eq!(
        coverage, [true; 6],
        "unbound, flip, capture, clock39, chase, repeat"
    );
    for width in [1, 7, 31, 255, 257] {
        let file = common::TestFile::new(&common::net_bytes(width));
        let net = Net::load(&file.path).unwrap();
        assert!(net.can_quantize());
        let mut parent = Accumulator::default();
        let mut delta = Accumulator::default();
        let mut fresh = Accumulator::default();
        net.refresh(&State::initial(), &mut parent);
        for (i, state) in states.iter().enumerate() {
            net.update(&parent, state, &mut delta);
            net.refresh(state, &mut fresh);
            assert_eq!(delta, fresh, "width={width}, position={i}");
            let error = (net.utility_accumulator(&delta, state.turn())
                - net.utility_reference(state))
            .abs();
            assert!(error <= 0.02, "width={width}, position={i}, error={error}");
            std::mem::swap(&mut parent, &mut delta);
        }
    }
}

#[test]
fn generated_net_search_is_legal_deterministic_and_reusable() {
    let file = common::TestFile::new(&common::net_bytes(7));
    let net = Arc::new(Net::load(&file.path).unwrap());
    let config = common::config().with_eval(Eval::Nnue(net.clone()));
    let mut searcher = Searcher::new(config);
    for state in common::positions(2000).iter().step_by(40) {
        let result = searcher.search(state);
        assert!(result
            .best_move
            .is_some_and(|mv| state.legal_moves().contains(&mv)));
        assert_eq!(result, searcher.search(state));
        assert_eq!(
            result,
            Searcher::new(common::config().with_eval(Eval::Nnue(net.clone()))).search(state)
        );
    }
}

#[test]
fn malformed_formats_and_nonfinite_weights_are_rejected() {
    let valid = common::net_bytes(1);
    for (offset, word) in [(4, 2u32), (8, 831), (12, 0), (12, 4097), (16, 15), (20, 31)] {
        let mut bytes = valid.clone();
        bytes[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        assert!(Net::from_bytes(&bytes).is_err());
    }
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1_000_001.] {
        let mut bytes = valid.clone();
        bytes[24..28].copy_from_slice(&bad.to_le_bytes());
        assert!(Net::from_bytes(&bytes).is_err());
    }
    for length in [0, 4, 23, 24, valid.len() - 1] {
        assert!(Net::from_bytes(&valid[..length]).is_err());
    }
    let mut extra = valid.clone();
    extra.push(0);
    assert!(Net::from_bytes(&extra).is_err());
}
