//! Sparse public-state features; indices are sorted and unique.
//!
//! | offset | length | meaning |
//! |---:|---:|---|
//! |0|630|relative piece code * 45 + point|
//! |630|45|covered point|
//! |675|32|relative bag counts, role-major thresholds|
//! |707|19|perspective score >= 5,10,...,95|
//! |726|19|opponent score thresholds|
//! |745|39|clock >= 1,...,39|
//! |784|45|chased point|
//! |829|1|current position occurred at least twice|
//! |830|1|perspective is to move|
//! |831|1|ink unbound|
use crate::{Ink, Seat, State, DOWN, HEIGHT, NSQ, START_COUNTS, WIDTH};
pub const PIECE: usize = 0;
pub const COVERED: usize = 630;
pub const BAG: usize = 675;
pub const OWN_SCORE: usize = 707;
pub const OPP_SCORE: usize = 726;
pub const CLOCK: usize = 745;
pub const CHASED: usize = 784;
pub const REPEAT: usize = 829;
pub const TURN: usize = 830;
pub const UNBOUND: usize = 831;
pub const N_FEATURES: usize = 832;

pub fn active(state: &State, perspective: Seat) -> Vec<u16> {
    let own = state.ink_for_seat(perspective).unwrap_or(Ink::Red) as usize;
    let relative = |code: usize| {
        if code / 7 == own {
            code % 7
        } else {
            7 + code % 7
        }
    };
    let mut out = Vec::with_capacity(180);
    for (sq, &code) in state.board.iter().enumerate() {
        if code >= 0 {
            out.push((PIECE + relative(code as usize) * NSQ + sq) as u16);
        } else if code == DOWN {
            out.push((COVERED + sq) as u16);
        }
    }
    let mut offset = BAG;
    for code in 0..14 {
        let absolute = if code < 7 {
            own * 7 + code
        } else {
            (1 - own) * 7 + code - 7
        };
        for j in 1..=START_COUNTS[code % 7] {
            if state.bag[absolute] >= j {
                out.push(offset as u16);
            }
            offset += 1;
        }
    }
    for (base, seat) in [(OWN_SCORE, perspective), (OPP_SCORE, perspective.other())] {
        for t in 1..=19 {
            if state.scores[seat as usize] >= 5 * t {
                out.push((base + t as usize - 1) as u16);
            }
        }
    }
    for j in 1..=39 {
        if state.no_progress >= j {
            out.push((CLOCK + j as usize - 1) as u16);
        }
    }
    if let Some(sq) = state.chased_square {
        out.push((CHASED + sq as usize) as u16);
    }
    if state.occurrences(&state.position_key()) >= 2 {
        out.push(REPEAT as u16);
    }
    if state.turn() == perspective {
        out.push(TURN as u16);
    }
    if state.first_color.is_none() {
        out.push(UNBOUND as u16);
    }
    out.sort_unstable();
    out
}

/// k bit 0 mirrors files; bit 1 mirrors ranks. No rotations on a rectangle.
pub fn reflect_point(sq: u8, k: u8) -> u8 {
    assert!(k < 4);
    let (x, y) = (sq as usize % WIDTH, sq as usize / WIDTH);
    ((if k & 2 != 0 { HEIGHT - 1 - y } else { y }) * WIDTH
        + if k & 1 != 0 { WIDTH - 1 - x } else { x }) as u8
}
pub fn reflect(state: &State, k: u8) -> State {
    let mut out = state.clone();
    for sq in 0..NSQ {
        out.board[reflect_point(sq as u8, k) as usize] = state.board[sq];
    }
    for (dst, src) in out.history.iter_mut().zip(&state.history) {
        for sq in 0..NSQ {
            dst.board[reflect_point(sq as u8, k) as usize] = src.board[sq];
        }
    }
    out.chased_square = state.chased_square.map(|s| reflect_point(s, k));
    out.last_move = state
        .last_move
        .map(|(a, b)| (reflect_point(a, k), reflect_point(b, k)));
    out
}

/// Allocation-free active set. Thirteen words cover all 832 features; iteration
/// is sorted like `active`. Rules context is read afresh, including repetition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeatureSet(pub(crate) [u64; N_FEATURES / 64]);
impl FeatureSet {
    fn insert(&mut self, index: usize) {
        self.0[index / 64] |= 1 << (index % 64);
    }
    fn range(&mut self, base: usize, count: usize) {
        if count == 0 {
            return;
        }
        let word = base / 64;
        let shift = base % 64;
        let mask = (1u64 << count) - 1;
        self.0[word] |= mask << shift;
        if shift + count > 64 {
            self.0[word + 1] |= mask >> (64 - shift);
        }
    }
    pub fn both(state: &State) -> [Self; 2] {
        let repeated = state.occurrences(&state.position_key()) >= 2;
        [Seat::Red, Seat::Black].map(|seat| {
            let own = state.ink_for_seat(seat).unwrap_or(Ink::Red) as usize;
            let mut out = Self::default();
            for (sq, &code) in state.board.iter().enumerate() {
                if code >= 0 {
                    let code = code as usize;
                    let relative = code % 7 + 7 * usize::from(code / 7 != own);
                    out.insert(relative * NSQ + sq);
                } else if code == DOWN {
                    out.insert(COVERED + sq);
                }
            }
            let mut offset = BAG;
            for code in 0..14 {
                let absolute = (own ^ (code / 7)) * 7 + code % 7;
                let max = START_COUNTS[code % 7] as usize;
                out.range(offset, (state.bag[absolute] as usize).min(max));
                offset += max;
            }
            out.range(
                OWN_SCORE,
                (state.scores[seat as usize] as usize / 5).min(19),
            );
            out.range(
                OPP_SCORE,
                (state.scores[seat.other() as usize] as usize / 5).min(19),
            );
            out.range(CLOCK, (state.no_progress as usize).min(39));
            if let Some(sq) = state.chased_square {
                out.insert(CHASED + sq as usize);
            }
            if repeated {
                out.insert(REPEAT);
            }
            if state.turn() == seat {
                out.insert(TURN);
            }
            if state.first_color.is_none() {
                out.insert(UNBOUND);
            }
            out
        })
    }
    pub fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(word, &bits)| {
            let mut remaining = bits;
            std::iter::from_fn(move || {
                if remaining == 0 {
                    return None;
                }
                let bit = remaining.trailing_zeros() as usize;
                remaining &= remaining - 1;
                Some(word * 64 + bit)
            })
        })
    }
}
