//! Optional Windows multimedia timer-resolution request.
//!
//! `timeBeginPeriod(1)` only affects timeout/sleep granularity; raw input events
//! wake the winit event loop independently. Keep this behind an explicit A/B
//! toggle and pair every successful request with `timeEndPeriod(1)`.

#[derive(Debug, Default)]
pub struct TimerResolutionGuard {
    active: bool,
}

impl TimerResolutionGuard {
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn set_enabled(&mut self, enabled: bool) -> Result<(), String> {
        if self.active == enabled {
            return Ok(());
        }
        if enabled {
            platform::begin_1ms()?;
            self.active = true;
        } else {
            platform::end_1ms()?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for TimerResolutionGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = platform::end_1ms();
            self.active = false;
        }
    }
}

#[cfg(windows)]
mod platform {
    const TIMERR_NOERROR: u32 = 0;

    #[link(name = "winmm")]
    extern "system" {
        fn timeBeginPeriod(period_ms: u32) -> u32;
        fn timeEndPeriod(period_ms: u32) -> u32;
    }

    pub fn begin_1ms() -> Result<(), String> {
        let result = unsafe { timeBeginPeriod(1) };
        if result == TIMERR_NOERROR {
            Ok(())
        } else {
            Err(format!("timeBeginPeriod(1) failed with MMRESULT {result}"))
        }
    }

    pub fn end_1ms() -> Result<(), String> {
        let result = unsafe { timeEndPeriod(1) };
        if result == TIMERR_NOERROR {
            Ok(())
        } else {
            Err(format!("timeEndPeriod(1) failed with MMRESULT {result}"))
        }
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn begin_1ms() -> Result<(), String> {
        Err("1 ms timer resolution is only available on Windows".into())
    }
    pub fn end_1ms() -> Result<(), String> {
        Ok(())
    }
}
