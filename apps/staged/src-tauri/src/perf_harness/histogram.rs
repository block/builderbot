//! A lock-free histogram of durations, sixteen buckets to the octave.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Each octave is cut into `2^SUB_BITS` equal buckets, so a bucket is at
/// most one sixteenth as wide as its lower bound.
const SUB_BITS: u32 = 4;
const SUB: usize = 1 << SUB_BITS;
/// Covers 1 µs to over an hour; anything longer lands in the last bucket.
const BUCKETS: usize = 30 * SUB;

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
    /// the bound before it. The harness derives percentiles from them as
    /// the upper bound of the bucket, so one is never under the true value
    /// and over it by less than 6.25 %.
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

/// Values below `SUB` have a bucket each. From there, octave `2^k` is cut
/// into `SUB` buckets of `2^(k - SUB_BITS)` each.
fn bucket(us: u64) -> usize {
    if us < SUB as u64 {
        return us as usize;
    }
    let octave = 63 - us.leading_zeros();
    let sub = (us >> (octave - SUB_BITS)) as usize & (SUB - 1);
    ((octave - SUB_BITS + 1) as usize * SUB + sub).min(BUCKETS - 1)
}

/// The first value above bucket `index`.
fn upper_us(index: usize) -> u64 {
    if index < SUB {
        return index as u64 + 1;
    }
    let octave = (index / SUB) as u32 + SUB_BITS - 1;
    let sub = (index % SUB) as u64;
    (1u64 << octave) + ((sub + 1) << (octave - SUB_BITS))
}

#[cfg(test)]
mod tests {
    use super::{bucket, upper_us, Histogram, BUCKETS};

    #[test]
    fn a_value_is_below_its_buckets_upper_bound_and_not_below_the_one_before() {
        for us in [
            0,
            1,
            2,
            3,
            15,
            16,
            17,
            31,
            32,
            33,
            999,
            1_000,
            1_500,
            196_000,
            250_000,
            3_000_000,
            3_600_000_000,
        ] {
            let index = bucket(us);
            assert!(us < upper_us(index), "{us} in bucket {index}");
            if index > 0 {
                assert!(us >= upper_us(index - 1), "{us} in bucket {index}");
            }
        }
    }

    #[test]
    fn a_bucket_is_at_most_a_sixteenth_wider_than_its_lower_bound() {
        for index in 16..BUCKETS - 1 {
            let (lower, upper) = (upper_us(index - 1), upper_us(index));
            assert!(upper > lower, "bucket {index}");
            assert!(
                (upper - lower) * 16 <= lower,
                "bucket {index}: {lower}..{upper}"
            );
        }
        // An hour fits below the last bucket, which takes everything longer.
        assert!(bucket(3_600_000_000) < BUCKETS - 1);
        assert_eq!(bucket(u64::MAX), BUCKETS - 1);
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
