#![allow(dead_code)]

use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use three_kingdoms_banqi_rust::{
    engine::SearchConfig, features::N_FEATURES, State, Status, DOWN, EMPTY, NSQ,
};

pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    pub fn index(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn weight(&mut self, scale: f32) -> f32 {
        (self.index(2001) as f32 / 1000. - 1.) * scale
    }
}

pub fn deal(seed: u64) -> [i16; NSQ] {
    let start = State::initial();
    let mut codes = Vec::new();
    for (code, &count) in start.bag.iter().enumerate() {
        codes.extend(std::iter::repeat_n(code as i16, count as usize));
    }
    let mut rng = Rng(seed);
    for i in (1..codes.len()).rev() {
        codes.swap(i, rng.index(i + 1));
    }
    let mut out = [EMPTY; NSQ];
    let mut pieces = codes.into_iter();
    for (sq, &code) in start.board.iter().enumerate() {
        if code == DOWN {
            out[sq] = pieces.next().unwrap();
        }
    }
    out
}

pub fn positions(count: usize) -> Vec<State> {
    let mut out = Vec::with_capacity(count);
    for seed in 0.. {
        let hidden = deal(seed);
        let mut rng = Rng(seed + 900);
        let mut state = State::initial();
        for _ in 0..250 {
            if !matches!(state.status, Status::Playing(_)) {
                break;
            }
            out.push(state.clone());
            if out.len() == count {
                return out;
            }
            let legal = state.legal_moves();
            let mv = legal[rng.index(legal.len())];
            state = state
                .apply(mv, (mv.0 == mv.1).then_some(hidden[mv.0 as usize]))
                .unwrap();
        }
    }
    unreachable!()
}

pub fn config() -> SearchConfig {
    SearchConfig {
        node_budget: 5000,
        max_depth: 64,
        time_limit: None,
        quiescence_depth: 4,
        tt_capacity: 65_536,
        ..SearchConfig::default()
    }
}

/// Synthetic TKNN v1 weights. The nonzero output bias makes net loading observable.
pub fn net_bytes(hidden: usize) -> Vec<u8> {
    let mut rng = Rng(2026 + hidden as u64);
    let mut weights = Vec::new();
    weights.extend((0..N_FEATURES * hidden).map(|_| rng.weight(0.01)));
    weights.extend(std::iter::repeat_n(0.25f32, hidden));
    weights.extend((0..32 * hidden).map(|_| rng.weight(0.2 / (hidden as f32).sqrt())));
    weights.extend(std::iter::repeat_n(0.2f32, 16));
    weights.extend((0..512).map(|_| rng.weight(0.1)));
    weights.extend(std::iter::repeat_n(0.2f32, 32));
    weights.extend((0..32).map(|_| rng.weight(0.1)));
    weights.push(0.6);
    let mut bytes = b"TKNN".to_vec();
    for word in [1, N_FEATURES, hidden, 16, 32] {
        bytes.extend((word as u32).to_le_bytes());
    }
    for weight in weights {
        bytes.extend(weight.to_le_bytes());
    }
    bytes
}

pub struct TestFile {
    pub path: PathBuf,
    directory: PathBuf,
}
impl TestFile {
    pub fn new(bytes: &[u8]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "banqi test {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("generated net.bin");
        std::fs::write(&path, bytes).unwrap();
        Self { path, directory }
    }
}
impl Drop for TestFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
