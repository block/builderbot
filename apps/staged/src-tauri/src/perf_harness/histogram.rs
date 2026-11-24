//! A lock-free histogram of durations, two buckets to the octave.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Covers 1 µs to over an hour; anything longer lands in the last bucket.
const BUCKETS: usize = 64;

pub(super) struct Histogram {
    count: AtomicU64,
    sum_us: AtomicU64,
    max_us: AtomicU64,
    buckets: [AtomicU64; BUCKETS],
}

impl Histogram {
    pub(super) const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            sum_us: AtomicU64::new(0),
            max_us: AtomicU64::new(0),
            buckets: [const { AtomicU64::new(0) }; BUCKETS],
        }
    }

    pub(super) fn record(&self, us: u64) {
        self.count.fetch_add(1, Relaxed);
        self.sum_us.fetch_add(us, Relaxed);
        self.max_us.fetch_max(us, Relaxed);
        self.buckets[bucket(us)].fetch_add(1, Relaxed);
    }

    pub(super) fn reset(&self) {
        self.count.store(0, Relaxed);
        self.sum_us.store(0, Relaxed);
        self.max_us.store(0, Relaxed);
        for bucket in &self.buckets {
            bucket.store(0, Relaxed);
        }
    }

    /// `buckets[i]` counts the values below `upper_us(i)` and at or above
    /// the bound before it. The harness derives percentiles from them, so
    /// one is exact to half an octave.
    pub(super) fn snapshot(&self) -> serde_json::Value {
        let buckets: Vec<[u64; 2]> = self
            .buckets
            .iter()
            .enumerate()
            .map(|(i, n)| [upper_us(i), n.load(Relaxed)])
            .filter(|[_, n]| *n > 0)
            .collect();
        serde_json::json!({
            "count": self.count.load(Relaxed),
            "sumUs": self.sum_us.load(Relaxed),
            "maxUs": self.max_us.load(Relaxed),
            "buckets": buckets,
        })
    }
}

/// Bucket `2k` holds `[2^k, 1.5 * 2^k)` and `2k + 1` the rest of the octave.
/// The first octave is the one value 1, which has no lower half.
fn bucket(us: u64) -> usize {
    let us = us.max(1);
    let octave = 63 - us.leading_zeros() as usize;
    let upper_half = octave == 0 || us & (1 << (octave - 1)) != 0;
    (octave * 2 + usize::from(upper_half)).min(BUCKETS - 1)
}

fn upper_us(index: usize) -> u64 {
    let octave = (index / 2) as u32;
    if index % 2 == 0 {
        (1u64 << octave) + (1u64 << octave) / 2
    } else {
        1u64 << (octave + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::{bucket, upper_us, Histogram};

    #[test]
    fn a_value_is_below_its_buckets_upper_bound_and_not_below_the_one_before() {
        for us in [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 999, 1_000, 1_500, 250_000, 3_000_000,
        ] {
            let index = bucket(us);
            assert!(us.max(1) < upper_us(index), "{us} in bucket {index}");
            if index > 0 {
                assert!(us.max(1) >= upper_us(index - 1), "{us} in bucket {index}");
            }
        }
    }

    #[test]
    fn a_snapshot_counts_what_was_recorded_and_a_reset_empties_it() {
        let histogram = Histogram::new();
        histogram.record(10);
        histogram.record(1_000);
        let snapshot = histogram.snapshot();
        assert_eq!(snapshot["count"], 2);
        assert_eq!(snapshot["sumUs"], 1_010);
        assert_eq!(snapshot["maxUs"], 1_000);
        assert_eq!(snapshot["buckets"].as_array().unwrap().len(), 2);
        histogram.reset();
        assert_eq!(histogram.snapshot()["count"], 0);
    }
}
