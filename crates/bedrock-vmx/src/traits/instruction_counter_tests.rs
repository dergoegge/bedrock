// SPDX-License-Identifier: GPL-2.0

use super::*;

#[test]
fn test_null_instruction_counter() {
    let mut counter = NullInstructionCounter;

    // Should not be configured
    assert!(!counter.is_configured());

    // Operations should be no-ops
    counter.enable();
    counter.disable();

    // Should always return 0
    assert_eq!(counter.read(), 0);
}

#[test]
fn test_null_counter_is_copy() {
    let counter = NullInstructionCounter;
    let _copy = counter; // Should compile - NullInstructionCounter is Copy
    let _another = counter; // Can use original after copy
}

/// Mock counter for testing VM code that uses instruction counting.
#[derive(Debug)]
pub struct MockInstructionCounter {
    pub count: u64,
    pub enabled: bool,
    pub enable_count: u32,
    pub disable_count: u32,
}

impl Default for MockInstructionCounter {
    fn default() -> Self {
        Self {
            count: 0,
            enabled: false,
            enable_count: 0,
            disable_count: 0,
        }
    }
}

impl MockInstructionCounter {
    pub fn new(count: u64) -> Self {
        Self {
            count,
            ..Default::default()
        }
    }
}

impl InstructionCounter for MockInstructionCounter {
    fn set_guest_state(&mut self, _user_mode: bool, _rip: u64) {}

    fn clear_guest_state(&mut self) {}

    fn enable(&mut self) {
        if !self.enabled {
            self.enabled = true;
            self.enable_count += 1;
        }
    }

    fn disable(&mut self) {
        if self.enabled {
            self.enabled = false;
            self.disable_count += 1;
        }
    }

    fn read(&self) -> u64 {
        self.count
    }

    fn is_configured(&self) -> bool {
        true
    }

    fn perf_global_ctrl_values(&self) -> Option<(u64, u64)> {
        None
    }
}

#[test]
fn test_mock_instruction_counter() {
    let mut counter = MockInstructionCounter::new(1000);

    assert!(counter.is_configured());
    assert_eq!(counter.read(), 1000);
    assert!(!counter.enabled);

    // Enable
    counter.enable();
    assert!(counter.enabled);
    assert_eq!(counter.enable_count, 1);

    // Enable again should be no-op
    counter.enable();
    assert_eq!(counter.enable_count, 1);

    // Disable
    counter.disable();
    assert!(!counter.enabled);
    assert_eq!(counter.disable_count, 1);

    // Disable again should be no-op
    counter.disable();
    assert_eq!(counter.disable_count, 1);

    // Can modify count for testing
    counter.count = 5000;
    assert_eq!(counter.read(), 5000);
}
