use std::time::Instant;

use ports::Clock;

/// The machine's monotonic clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_goes_backwards() {
        let a = SystemClock.now();
        let b = SystemClock.now();
        assert!(b >= a);
    }
}
