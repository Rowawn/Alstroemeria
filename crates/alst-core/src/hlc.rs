//! Hybrid logical clock (SPEC §4.4): 48 bits wall-clock ms + 16-bit counter.
//! Ties between devices are broken by `site_id`, compared alongside the value.

const COUNTER_BITS: u32 = 16;
const COUNTER_MASK: u64 = (1 << COUNTER_BITS) - 1;

pub fn pack(ms: u64, counter: u16) -> u64 {
    (ms << COUNTER_BITS) | counter as u64
}

pub fn physical_ms(hlc: u64) -> u64 {
    hlc >> COUNTER_BITS
}

pub fn counter(hlc: u64) -> u16 {
    (hlc & COUNTER_MASK) as u16
}

#[derive(Debug, Clone)]
pub struct Hlc {
    last: u64,
}

impl Hlc {
    pub fn new(last: u64) -> Self {
        Self { last }
    }

    /// Timestamp for a local event, given the current wall-clock ms.
    pub fn tick(&mut self, now_ms: u64) -> u64 {
        let next = pack(now_ms, 0).max(self.last + 1);
        self.last = next;
        next
    }

    /// Merge a timestamp received from another device, then tick.
    pub fn recv(&mut self, remote: u64, now_ms: u64) -> u64 {
        self.last = self.last.max(remote);
        self.tick(now_ms)
    }

    /// Note a timestamp seen from another device, so later local stamps are greater.
    pub fn observe(&mut self, remote: u64) {
        self.last = self.last.max(remote);
    }

    pub fn last(&self) -> u64 {
        self.last
    }
}

pub fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_roundtrip() {
        let h = pack(1_700_000_000_000, 42);
        assert_eq!(physical_ms(h), 1_700_000_000_000);
        assert_eq!(counter(h), 42);
    }

    #[test]
    fn tick_is_monotonic_when_wall_clock_stalls_or_goes_back() {
        let mut c = Hlc::new(0);
        let a = c.tick(1000);
        let b = c.tick(1000);
        let d = c.tick(500);
        assert!(a < b && b < d);
        assert_eq!(physical_ms(d), 1000);
        assert_eq!(counter(d), 2);
    }

    #[test]
    fn tick_resets_counter_when_wall_clock_advances() {
        let mut c = Hlc::new(0);
        c.tick(1000);
        c.tick(1000);
        let h = c.tick(2000);
        assert_eq!((physical_ms(h), counter(h)), (2000, 0));
    }

    #[test]
    fn recv_moves_ahead_of_remote() {
        let mut c = Hlc::new(0);
        let remote = pack(9000, 5);
        let h = c.recv(remote, 1000);
        assert!(h > remote);
    }

    #[test]
    fn site_id_breaks_ties() {
        // (hlc, site_id) tuples order lexicographically.
        assert!((5u64, "a") < (5u64, "b"));
        assert!((6u64, "a") > (5u64, "z"));
    }
}
