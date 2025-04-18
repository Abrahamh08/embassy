//! Watchdog Timer (WDT) driver.
//!
//! This HAL implements a basic watchdog timer with 1..=8 handles.
//! Once the watchdog has been started, it cannot be stopped.

use core::marker::PhantomData;

use crate::pac::wdt::vals;
pub use crate::pac::wdt::vals::{Halt as HaltConfig, Sleep as SleepConfig};
use crate::{peripherals, Peri};

const MIN_TICKS: u32 = 15;

/// WDT configuration.
#[non_exhaustive]
pub struct Config {
    /// Number of 32768 Hz ticks in each watchdog period.
    ///
    /// Note: there is a minimum of 15 ticks (458 microseconds). If a lower
    /// number is provided, 15 ticks will be used as the configured value.
    pub timeout_ticks: u32,

    /// Should the watchdog continue to count during sleep modes?
    pub action_during_sleep: SleepConfig,

    /// Should the watchdog continue to count when the CPU is halted for debug?
    pub action_during_debug_halt: HaltConfig,
}

impl Config {
    /// Create a config structure from the current configuration of the WDT
    /// peripheral.
    pub fn try_new(_wdt: &peripherals::WDT) -> Option<Self> {
        #[cfg(not(feature = "nrf5340-app-s"))]
        let r = crate::pac::WDT;
        #[cfg(feature = "nrf5340-app-s")]
        let r = crate::pac::WDT0;

        #[cfg(all(not(feature = "_nrf91"), not(feature = "nrf5340-app-s")))]
        let runstatus = r.runstatus().read().runstatus();
        #[cfg(any(feature = "_nrf91", feature = "nrf5340-app-s"))]
        let runstatus = r.runstatus().read().runstatuswdt();

        if runstatus {
            let config = r.config().read();
            Some(Self {
                timeout_ticks: r.crv().read(),
                action_during_sleep: config.sleep(),
                action_during_debug_halt: config.halt(),
            })
        } else {
            None
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout_ticks: 32768, // 1 second
            action_during_debug_halt: HaltConfig::RUN,
            action_during_sleep: SleepConfig::RUN,
        }
    }
}

/// Watchdog driver.
pub struct Watchdog {
    _wdt: Peri<'static, peripherals::WDT>,
}

impl Watchdog {
    /// Try to create a new watchdog driver.
    ///
    /// This function will return an error if the watchdog is already active
    /// with a `config` different to the requested one, or a different number of
    /// enabled handles.
    ///
    /// `N` must be between 1 and 8, inclusive.
    #[inline]
    pub fn try_new<const N: usize>(
        wdt: Peri<'static, peripherals::WDT>,
        config: Config,
    ) -> Result<(Self, [WatchdogHandle; N]), Peri<'static, peripherals::WDT>> {
        assert!(N >= 1 && N <= 8);

        #[cfg(not(feature = "nrf5340-app-s"))]
        let r = crate::pac::WDT;
        #[cfg(feature = "nrf5340-app-s")]
        let r = crate::pac::WDT0;

        let crv = config.timeout_ticks.max(MIN_TICKS);
        let rren = crate::pac::wdt::regs::Rren((1u32 << N) - 1);

        #[cfg(all(not(feature = "_nrf91"), not(feature = "nrf5340-app-s")))]
        let runstatus = r.runstatus().read().runstatus();
        #[cfg(any(feature = "_nrf91", feature = "nrf5340-app-s"))]
        let runstatus = r.runstatus().read().runstatuswdt();

        if runstatus {
            let curr_config = r.config().read();
            if curr_config.halt() != config.action_during_debug_halt
                || curr_config.sleep() != config.action_during_sleep
                || r.crv().read() != crv
                || r.rren().read() != rren
            {
                return Err(wdt);
            }
        } else {
            r.config().write(|w| {
                w.set_sleep(config.action_during_sleep);
                w.set_halt(config.action_during_debug_halt);
            });
            r.intenset().write(|w| w.set_timeout(true));

            r.crv().write_value(crv);
            r.rren().write_value(rren);
            r.tasks_start().write_value(1);
        }

        let this = Self { _wdt: wdt };

        let mut handles = [const {
            WatchdogHandle {
                _wdt: PhantomData,
                index: 0,
            }
        }; N];
        for i in 0..N {
            handles[i] = WatchdogHandle {
                _wdt: PhantomData,
                index: i as u8,
            };
            handles[i].pet();
        }

        Ok((this, handles))
    }

    /// Enable the watchdog interrupt.
    ///
    /// NOTE: Although the interrupt will occur, there is no way to prevent
    /// the reset from occurring. From the time the event was fired, the
    /// system will reset two LFCLK ticks later (61 microseconds) if the
    /// interrupt has been enabled.
    #[inline(always)]
    pub fn enable_interrupt(&mut self) {
        #[cfg(not(feature = "nrf5340-app-s"))]
        crate::pac::WDT.intenset().write(|w| w.set_timeout(true));
        #[cfg(feature = "nrf5340-app-s")]
        crate::pac::WDT0.intenset().write(|w| w.set_timeout(true));
    }

    /// Disable the watchdog interrupt.
    ///
    /// NOTE: This has no effect on the reset caused by the Watchdog.
    #[inline(always)]
    pub fn disable_interrupt(&mut self) {
        #[cfg(not(feature = "nrf5340-app-s"))]
        crate::pac::WDT.intenclr().write(|w| w.set_timeout(true));
        #[cfg(feature = "nrf5340-app-s")]
        crate::pac::WDT0.intenclr().write(|w| w.set_timeout(true));
    }

    /// Is the watchdog still awaiting pets from any handle?
    ///
    /// This reports whether sufficient pets have been received from all
    /// handles to prevent a reset this time period.
    #[inline(always)]
    pub fn awaiting_pets(&self) -> bool {
        #[cfg(not(feature = "nrf5340-app-s"))]
        let r = crate::pac::WDT;
        #[cfg(feature = "nrf5340-app-s")]
        let r = crate::pac::WDT0;
        let enabled = r.rren().read().0;
        let status = r.reqstatus().read().0;
        (status & enabled) == 0
    }
}

/// Watchdog handle.
pub struct WatchdogHandle {
    _wdt: PhantomData<Peri<'static, peripherals::WDT>>,
    index: u8,
}

impl WatchdogHandle {
    /// Pet the watchdog.
    ///
    /// This function pets the given watchdog handle.
    ///
    /// NOTE: All active handles must be pet within the time interval to
    /// prevent a reset from occurring.
    #[inline]
    pub fn pet(&mut self) {
        #[cfg(not(feature = "nrf5340-app-s"))]
        let r = crate::pac::WDT;
        #[cfg(feature = "nrf5340-app-s")]
        let r = crate::pac::WDT0;
        r.rr(self.index as usize).write(|w| w.set_rr(vals::Rr::RELOAD));
    }

    /// Has this handle been pet within the current window?
    pub fn is_pet(&self) -> bool {
        #[cfg(not(feature = "nrf5340-app-s"))]
        let r = crate::pac::WDT;
        #[cfg(feature = "nrf5340-app-s")]
        let r = crate::pac::WDT0;
        !r.reqstatus().read().rr(self.index as usize)
    }

    /// Steal a watchdog handle by index.
    ///
    /// # Safety
    /// Watchdog must be initialized and `index` must be between `0` and `N-1`
    /// where `N` is the handle count when initializing.
    pub unsafe fn steal(index: u8) -> Self {
        Self {
            _wdt: PhantomData,
            index,
        }
    }
}
