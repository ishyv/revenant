use wasm_bindgen::prelude::*;

/// Check whether a single number is prime.
#[wasm_bindgen]
pub fn is_prime(n: u32) -> bool {
    if n < 2 {
        return false;
    }
    if n == 2 {
        return true;
    }
    if n % 2 == 0 {
        return false;
    }
    let mut i = 3u32;
    while i.saturating_mul(i) <= n {
        if n % i == 0 {
            return false;
        }
        i += 2;
    }
    true
}

/// Return all primes up to `limit` (inclusive) using the Sieve of Eratosthenes.
/// The result is a Uint32Array in JavaScript.
#[wasm_bindgen]
pub fn primes_up_to(limit: u32) -> Vec<u32> {
    if limit < 2 {
        return vec![];
    }

    let size = (limit + 1) as usize;
    let mut sieve = vec![true; size];
    sieve[0] = false;
    sieve[1] = false;

    let mut i = 2usize;
    while i * i < size {
        if sieve[i] {
            let mut j = i * i;
            while j < size {
                sieve[j] = false;
                j += i;
            }
        }
        i += 1;
    }

    sieve
        .iter()
        .enumerate()
        .filter(|(_, &is_p)| is_p)
        .map(|(n, _)| n as u32)
        .collect()
}

/// Count how many primes exist up to `limit` without allocating the full list.
#[wasm_bindgen]
pub fn prime_count(limit: u32) -> u32 {
    primes_up_to(limit).len() as u32
}
