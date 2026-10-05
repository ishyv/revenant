//! Native prime operations with typed contracts, cancellation, and bounded output.
//!
//! The UI receives native task projections; it never executes a WASM module or
//! schedules Rust work in a web worker. The framework supplies the executable.
use revenant::{
    Application, Error, Progress, Result, TaskContext, contract, operation, operations,
};

/// Inclusive sieve limit. The native implementation bounds work and JSON output.
#[contract]
pub struct SieveInput {
    /// Largest candidate, from zero through 1,000,000 inclusive.
    pub limit: u32,
}
/// Single integer to check, represented exactly by a JavaScript number.
#[contract]
pub struct PrimeInput {
    /// Unsigned 32-bit candidate.
    pub value: u32,
}
/// Bounded sieve output, with every prime in ascending order.
#[contract]
pub struct SieveResult {
    /// Inclusive limit used for this computation.
    pub limit: u32,
    /// Number of primes found.
    pub count: u32,
    /// Prime values, bounded by the maximum accepted input limit.
    pub primes: Vec<u32>,
}

/// Check primality using trial division without allocating a sieve.
#[operation(id = "primes.isPrime")]
pub async fn is_prime(input: PrimeInput, context: TaskContext) -> Result<bool> {
    let n = input.value;
    context.checkpoint()?;
    if n < 2 {
        return Ok(false);
    }
    if n % 2 == 0 {
        return Ok(n == 2);
    }
    let mut divisor = 3u32;
    // Division avoids multiplication overflow near u32::MAX.
    while divisor <= n / divisor {
        if divisor % 1024 == 1 {
            context.checkpoint()?;
        }
        if n % divisor == 0 {
            return Ok(false);
        }
        divisor += 2;
    }
    Ok(true)
}

/// Find all primes through the inclusive limit using the Sieve of Eratosthenes.
///
/// Limits above 1,000,000 return `limit_exceeded` so task results stay bounded.
/// Native cancellation is checked during marking and result collection.
#[operation(id = "primes.upTo")]
pub async fn primes_up_to(input: SieveInput, context: TaskContext) -> Result<SieveResult> {
    let primes = sieve(input.limit, &context)?;
    Ok(SieveResult {
        limit: input.limit,
        count: primes.len() as u32,
        primes,
    })
}

/// Count primes using the same bounded sieve without returning the prime list.
#[operation(id = "primes.count")]
pub async fn prime_count(input: SieveInput, context: TaskContext) -> Result<u32> {
    Ok(sieve(input.limit, &context)?.len() as u32)
}

fn sieve(limit: u32, context: &TaskContext) -> Result<Vec<u32>> {
    if limit > 1_000_000 {
        return Err(Error::new(
            "limit_exceeded",
            "Choose a limit at most 1,000,000",
        ));
    }
    context.checkpoint()?;
    if limit < 2 {
        return Ok(Vec::new());
    }
    let mut marked = vec![true; limit as usize + 1];
    marked[0] = false;
    marked[1] = false;
    let mut i = 2usize;
    while i * i < marked.len() {
        context.checkpoint()?;
        if marked[i] {
            let mut j = i * i;
            let mut ticks = 0usize;
            while j < marked.len() {
                marked[j] = false;
                j += i;
                ticks += 1;
                if ticks % 4096 == 0 {
                    context.checkpoint()?;
                }
            }
        }
        i += 1;
    }
    let mut primes = Vec::new();
    for (value, prime) in marked.into_iter().enumerate() {
        if value % 4096 == 0 {
            context.checkpoint()?;
            context.progress(Progress {
                completed: (value as u64).into(),
                total: Some((limit as u64).into()),
                message: Some("Collecting prime values".into()),
            })?;
        }
        if prime {
            primes.push(value as u32);
        }
    }
    context.progress(Progress {
        completed: (limit as u64).into(),
        total: Some((limit as u64).into()),
        message: Some("Sieve complete".into()),
    })?;
    Ok(primes)
}

/// Register ordinary Rust functions; Revenant supplies the runtime and exports.
pub fn app() -> Application {
    Application::new().operations(operations![is_prime, primes_up_to, prime_count])
}
