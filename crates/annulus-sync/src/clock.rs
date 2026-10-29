//! A hybrid logical clock: wall-clock milliseconds in the high bits and a per-device number
//! in the low 16, never going backwards. Comparing two edits' times picks the newer one
//! even when device clocks disagree a little, and two devices never produce the same time.

/// One device's clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clock {
    last: u64,
    node: u16,
}

impl Clock {
    /// A clock for device `node`, continuing after `last` (the newest time seen).
    #[must_use]
    pub fn new(node: u16, last: u64) -> Self {
        Self { last, node }
    }

    /// The time of a new edit made at wall-clock `now_millis`.
    pub fn tick(&mut self, now_millis: u64) -> u64 {
        let wall = (now_millis << 16) | u64::from(self.node);
        let next = if wall > self.last {
            wall
        } else {
            // The wall clock went back or stood still: keep counting in the millisecond
            // part, so this device's node number stays in the low bits.
            ((self.last >> 16) + 1) << 16 | u64::from(self.node)
        };
        self.last = next;
        next
    }

    /// Takes note of a time seen on another device, so later local edits sort after it.
    pub fn observe(&mut self, time: u64) {
        self.last = self.last.max(time);
    }

    /// The newest time seen.
    #[must_use]
    pub fn last(&self) -> u64 {
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_only_go_forward_and_carry_the_node() {
        let mut clock = Clock::new(7, 0);
        let first = clock.tick(1_000);
        let same_ms = clock.tick(1_000);
        let earlier_wall = clock.tick(900);
        assert!(first < same_ms && same_ms < earlier_wall);
        assert_eq!(earlier_wall & 0xFFFF, 7);
    }

    #[test]
    fn an_edit_after_seeing_a_remote_one_is_newer() {
        let mut slow = Clock::new(1, 0);
        let remote = Clock::new(2, 0).tick(5_000);
        slow.observe(remote);
        assert!(slow.tick(10) > remote);
    }
}
