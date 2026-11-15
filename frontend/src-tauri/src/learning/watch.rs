//! Stops a training run when the Mac struggles.
//!
//! On the night of 14→15.11 a pilot run slowed from 1.8 to 46 s a piece for
//! nine hours: memory ran out and macOS swapped. The watch compares each
//! step with the first steps of the same run and stops the run when it
//! becomes several times slower, when swap keeps growing, or when the
//! trainer uses far more memory than the Mac can give.

use serde::{Deserialize, Serialize};

/// Steps taken as the run's normal speed
const WARMUP_STEPS: usize = 20;
/// Slower than normal by this factor, for this many steps in a row: stop
const SLOW_FACTOR: f64 = 4.0;
const SLOW_STEPS: usize = 10;
/// Swap grown by this much since the start (GB): stop
const MAX_SWAP_GROWTH_GB: f64 = 4.0;
/// Trainer memory above this share of the Mac's memory: stop
const MAX_MEMORY_SHARE: f64 = 0.8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    TooSlow,
    Swapping,
    TooMuchMemory,
}

#[derive(Debug, Clone)]
pub struct Watch {
    mac_memory_gb: f64,
    swap_at_start_gb: f64,
    first: Vec<f64>,
    slow_in_a_row: usize,
}

impl Watch {
    pub fn new(mac_memory_gb: f64, swap_at_start_gb: f64) -> Self {
        Watch { mac_memory_gb, swap_at_start_gb, first: Vec::new(), slow_in_a_row: 0 }
    }

    fn normal(&self) -> Option<f64> {
        if self.first.len() < WARMUP_STEPS {
            return None;
        }
        let mut v = self.first.clone();
        v.sort_by(|a, b| a.total_cmp(b));
        Some(v[v.len() / 2])
    }

    /// One training step: its speed, the trainer's memory and the Mac's swap now
    pub fn step(&mut self, seconds_per_piece: f64, memory_gb: f64, swap_gb: f64) -> Option<Stop> {
        if memory_gb > self.mac_memory_gb * MAX_MEMORY_SHARE {
            return Some(Stop::TooMuchMemory);
        }
        if swap_gb - self.swap_at_start_gb > MAX_SWAP_GROWTH_GB {
            return Some(Stop::Swapping);
        }
        match self.normal() {
            None => self.first.push(seconds_per_piece),
            Some(normal) => {
                if seconds_per_piece > normal * SLOW_FACTOR {
                    self.slow_in_a_row += 1;
                    if self.slow_in_a_row >= SLOW_STEPS {
                        return Some(Stop::TooSlow);
                    }
                } else {
                    self.slow_in_a_row = 0;
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_night_of_14_11_is_stopped() {
        let mut w = Watch::new(24.0, 1.0);
        for _ in 0..WARMUP_STEPS {
            assert_eq!(w.step(1.85, 9.0, 1.0), None);
        }
        let mut stopped = None;
        for k in 0..30 {
            if let Some(s) = w.step(46.0, 14.0, 1.5 + k as f64 * 0.05) {
                stopped = Some((k, s));
                break;
            }
        }
        assert_eq!(stopped, Some((SLOW_STEPS - 1, Stop::TooSlow)));
    }

    #[test]
    fn a_short_hiccup_is_not_a_stop() {
        let mut w = Watch::new(24.0, 0.0);
        for _ in 0..WARMUP_STEPS {
            w.step(1.8, 9.0, 0.0);
        }
        for _ in 0..5 {
            assert_eq!(w.step(20.0, 9.0, 0.0), None);
        }
        assert_eq!(w.step(1.9, 9.0, 0.0), None);
        for _ in 0..5 {
            assert_eq!(w.step(20.0, 9.0, 0.0), None);
        }
    }

    #[test]
    fn swap_and_memory_stop_at_once() {
        assert_eq!(Watch::new(24.0, 2.0).step(1.8, 9.0, 6.5), Some(Stop::Swapping));
        assert_eq!(Watch::new(16.0, 0.0).step(1.8, 13.5, 0.0), Some(Stop::TooMuchMemory));
    }
}
