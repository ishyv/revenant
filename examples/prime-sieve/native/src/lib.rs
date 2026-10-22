//! Native prime operations with typed contracts, cancellation, and bounded output.
//!
//! The UI receives native task projections; it never executes a WASM module or
//! schedules Rust work in a web worker. The framework supplies the executable.
use revenant::{Error, Progress, Result, TaskContext, contract, operation};

revenant::app!();
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
#[operation(id = "primes.isPrime", read_only, independent)]
pub fn is_prime(value: u32, context: TaskContext) -> Result<bool> {
    let n = value;
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
#[operation(id = "primes.upTo", read_only, independent)]
pub fn primes_up_to(limit: u32, context: TaskContext) -> Result<SieveResult> {
    let primes = sieve(limit, &context)?;
    Ok(SieveResult {
        limit,
        count: primes.len() as u32,
        primes,
    })
}

/// Count primes using the same bounded sieve without returning the prime list.
#[operation(id = "primes.count", read_only, independent)]
pub fn prime_count(limit: u32, context: TaskContext) -> Result<u32> {
    Ok(sieve(limit, &context)?.len() as u32)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn run<T>(work: impl FnOnce(TaskContext) -> Result<T>) -> Result<T> {
        let resources = revenant::ResourceRegistry::new();
        let scope = resources.create_scope(None).unwrap();
        let context = revenant::TaskManager::new(resources)
            .create(&scope, "prime")
            .unwrap();
        let execution = context.begin().unwrap();
        let result = work(context);
        execution
            .finish(
                result
                    .as_ref()
                    .map(|_| revenant::serde_json::Value::Null)
                    .map_err(Clone::clone),
            )
            .unwrap();
        result
    }

    #[test]
    fn sync_prime_operations_keep_bounded_results_and_exact_wire_shapes() {
        assert!(run(|context| is_prime(97, context)).unwrap());
        assert!(!run(|context| is_prime(100, context)).unwrap());
        let result = run(|context| primes_up_to(10, context)).unwrap();
        assert_eq!(result.primes, [2, 3, 5, 7]);
        assert_eq!(result.count, 4);
        assert_eq!(run(|context| prime_count(10, context)).unwrap(), 4);
        assert_eq!(
            run(|context| prime_count(1_000_001, context))
                .unwrap_err()
                .code,
            "limit_exceeded"
        );
        let manifest = app().manifest().unwrap();
        for (id, field) in [
            ("primes.isPrime", "value"),
            ("primes.upTo", "limit"),
            ("primes.count", "limit"),
        ] {
            let operation = manifest.operations.iter().find(|op| op.id == id).unwrap();
            assert_eq!(operation.arguments, [field]);
            assert_eq!(operation.effect, revenant::OperationEffect::ReadOnly);
            assert!(operation.independent);
            assert!(!operation.agent);
            revenant::validate_json(
                &operation.input.schema,
                &revenant::serde_json::json!({field: 10}),
            )
            .unwrap();
        }
    }
}
