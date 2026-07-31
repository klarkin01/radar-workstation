//! Dev-only seeded mutator for parser hardening tests. Not shipped in any
//! binary — consumed as a `[dev-dependencies]` path dependency by crates
//! whose untrusted-input parsers need a `mutated_inputs_never_panic` test
//! (see `http-ingest`'s `response.rs` and `nexrad-decoder`'s corpus tests).

/// Minimal xorshift64 PRNG. No external dependency, and a fixed seed makes
/// any failure reproducible rather than a flake.
pub struct XorShift64(pub u64);

impl XorShift64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// Produce one mutated copy of a randomly chosen seed from `seeds`, via a
/// randomly chosen strategy: bit flip, byte splice from another seed, or
/// truncation. `seeds` must be non-empty; empty individual seeds are handled.
pub fn mutate_one(rng: &mut XorShift64, seeds: &[Vec<u8>]) -> Vec<u8> {
    let seed = &seeds[(rng.next_u64() as usize) % seeds.len()];
    if seed.is_empty() {
        return Vec::new();
    }
    let mut mutated = seed.clone();
    match rng.next_u64() % 3 {
        0 => {
            // bit flip
            let idx = (rng.next_u64() as usize) % mutated.len();
            let bit = (rng.next_u64() % 8) as u8;
            mutated[idx] ^= 1 << bit;
        }
        1 => {
            // byte splice from another seed
            let other = &seeds[(rng.next_u64() as usize) % seeds.len()];
            if !other.is_empty() {
                let src_idx = (rng.next_u64() as usize) % other.len();
                let dst_idx = (rng.next_u64() as usize) % mutated.len();
                mutated[dst_idx] = other[src_idx];
            }
        }
        _ => {
            // truncation
            let new_len = (rng.next_u64() as usize) % (mutated.len() + 1);
            mutated.truncate(new_len);
        }
    }
    mutated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_is_deterministic() {
        let mut a = XorShift64::new(42);
        let mut b = XorShift64::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn mutate_one_never_panics_on_single_byte_seeds() {
        let seeds = vec![vec![0u8], vec![1u8]];
        let mut rng = XorShift64::new(1);
        for _ in 0..1000 {
            let _ = mutate_one(&mut rng, &seeds);
        }
    }

    #[test]
    fn mutate_one_handles_empty_seed_in_pool() {
        let seeds = vec![Vec::new(), vec![1, 2, 3]];
        let mut rng = XorShift64::new(7);
        for _ in 0..1000 {
            let _ = mutate_one(&mut rng, &seeds);
        }
    }
}
