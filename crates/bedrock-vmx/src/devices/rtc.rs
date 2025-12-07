// SPDX-License-Identifier: GPL-2.0

//! RTC (CMOS Real-Time Clock) emulation.
//!
//! The RTC is accessed via I/O ports 0x70 (index) and 0x71 (data).
//! Port 0x70 selects which register to access, port 0x71 reads/writes it.

#[cfg(not(feature = "cargo"))]
use super::super::logging::{StateHash, Xxh64Hasher};
#[cfg(feature = "cargo")]
use crate::logging::{StateHash, Xxh64Hasher};

/// RTC (CMOS clock) state for time emulation.
///
/// The RTC is accessed via I/O ports 0x70 (index) and 0x71 (data).
/// Port 0x70 selects which register to access, port 0x71 reads/writes it.
///
/// Time is derived from the emulated TSC for deterministic execution.
/// The formula is: current_time = base_time + (emulated_tsc / tsc_frequency)
///
/// Register layout (BCD format by default):
/// - 0x00: Seconds (0-59)
/// - 0x02: Minutes (0-59)
/// - 0x04: Hours (0-23)
/// - 0x06: Day of week (1-7, Sunday=1)
/// - 0x07: Day of month (1-31)
/// - 0x08: Month (1-12)
/// - 0x09: Year (0-99, offset from century)
/// - 0x0A: Status Register A (update in progress)
/// - 0x0B: Status Register B (format control)
/// - 0x0C: Status Register C (interrupt flags)
/// - 0x0D: Status Register D (battery status)
/// - 0x32: Century (19-20)
#[derive(Clone, Debug)]
pub struct RtcState {
    /// Currently selected CMOS register index (set via port 0x70).
    pub index: u8,
    /// Base Unix timestamp (seconds since 1970-01-01 00:00:00 UTC).
    /// The actual time returned is base_time + elapsed seconds from TSC.
    pub base_time: u64,
}

impl Default for RtcState {
    fn default() -> Self {
        Self {
            index: 0,
            // Default to 2024-01-01 00:00:00 UTC
            base_time: 1704067200,
        }
    }
}

impl RtcState {
    /// Set the base time from a Unix timestamp.
    pub fn set_time(&mut self, timestamp: u64) {
        self.base_time = timestamp;
    }

    /// Convert a value to BCD format.
    #[inline]
    fn to_bcd(value: u8) -> u8 {
        ((value / 10) << 4) | (value % 10)
    }

    /// Read the currently selected RTC register.
    ///
    /// Time is derived from the emulated TSC for determinism:
    /// current_time = base_time + (emulated_tsc / tsc_frequency)
    ///
    /// # Arguments
    ///
    /// * `emulated_tsc` - Current emulated TSC value
    /// * `tsc_frequency` - TSC frequency in Hz
    pub fn read_register_with_tsc(&self, emulated_tsc: u64, tsc_frequency: u64) -> u8 {
        // Calculate elapsed seconds from emulated TSC
        let elapsed_secs = if tsc_frequency > 0 {
            emulated_tsc / tsc_frequency
        } else {
            0
        };

        // Break down Unix timestamp into date/time components
        // This is a simplified calculation - doesn't handle all edge cases
        let secs = self.base_time + elapsed_secs;

        // Days since epoch
        let days = secs / 86400;
        let time_of_day = secs % 86400;

        let hours = (time_of_day / 3600) as u8;
        let minutes = ((time_of_day % 3600) / 60) as u8;
        let seconds = (time_of_day % 60) as u8;

        // Calculate year, month, day from days since epoch (1970-01-01)
        // Simplified: doesn't account for leap seconds
        let (year, month, day, day_of_week) = Self::days_to_date(days);

        match self.index {
            0x00 => Self::to_bcd(seconds),
            0x02 => Self::to_bcd(minutes),
            0x04 => Self::to_bcd(hours), // 24-hour format
            0x06 => Self::to_bcd(day_of_week),
            0x07 => Self::to_bcd(day),
            0x08 => Self::to_bcd(month),
            0x09 => Self::to_bcd((year % 100) as u8),
            0x0A => 0x00,                             // Status A: no update in progress
            0x0B => 0x02,                             // Status B: 24-hour mode, BCD format
            0x0C => 0x00,                             // Status C: no interrupts
            0x0D => 0x80,                             // Status D: battery OK
            0x32 => Self::to_bcd((year / 100) as u8), // Century
            _ => 0x00,
        }
    }

    /// Convert days since Unix epoch to (year, month, day, day_of_week).
    fn days_to_date(days: u64) -> (u32, u8, u8, u8) {
        // Day of week: 1970-01-01 was Thursday (day 4), Sunday = 1
        let day_of_week = (((days + 4) % 7) + 1) as u8;

        // Calculate year
        let mut year = 1970u32;
        let mut remaining_days = days;

        loop {
            let days_in_year = if Self::is_leap_year(year) { 366 } else { 365 };
            if remaining_days < days_in_year {
                break;
            }
            remaining_days -= days_in_year;
            year += 1;
        }

        // Calculate month and day
        let leap = Self::is_leap_year(year);
        let days_in_months: [u64; 12] = if leap {
            [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        } else {
            [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        };

        let mut month = 1u8;
        for days_in_month in days_in_months.iter() {
            if remaining_days < *days_in_month {
                break;
            }
            remaining_days -= *days_in_month;
            month += 1;
        }

        let day = (remaining_days + 1) as u8;

        (year, month, day, day_of_week)
    }

    /// Check if a year is a leap year.
    #[inline]
    fn is_leap_year(year: u32) -> bool {
        (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
    }
}

impl StateHash for RtcState {
    fn state_hash(&self) -> u64 {
        let mut h = Xxh64Hasher::new();
        h.write_u8(self.index);
        h.write_u64(self.base_time);
        h.finish()
    }
}
