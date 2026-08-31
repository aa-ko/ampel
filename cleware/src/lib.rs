//! Drive a Cleware USB traffic light.
//!
//! The vendor driver in `vendor/cleware/` is compiled into this crate (see
//! `build.rs`) and reached through a single `extern "C"` shim. It opens and
//! closes the USB device on every call, so there is no handle to hold on to —
//! [`set`] is all the hardware access there is. [`Light`] adds the rate limiting
//! the physical device needs.
//!
//! Requires read/write access to `/dev/usb/hiddev*` (see
//! `packaging/99-cleware.rules`).

use std::ffi::c_int;
use std::fmt;
use std::time::{Duration, Instant};

extern "C" {
    fn cleware_set_traffic(traffic: c_int) -> c_int;
}

/// Lamp bits of the vendor's `tl_enum` (`USBswitchCmd.cpp`). `OFF` is "no lamp
/// selected" — deliberately not the vendor's `TL_none`, which switches to the
/// plain on/off relay path instead of the traffic-light path.
const TL_OFF: c_int = 0;
const TL_RED: c_int = 2;
const TL_GREEN: c_int = 4;
const TL_YELLOW: c_int = 8;

/// The vendor driver reports every failure — missing device included — as -1.
const DO_SWITCH_FAILED: c_int = -1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Off,
    Red,
    Yellow,
    Green,
}

impl Color {
    fn traffic_mask(self) -> c_int {
        match self {
            Color::Off => TL_OFF,
            Color::Red => TL_RED,
            Color::Yellow => TL_YELLOW,
            Color::Green => TL_GREEN,
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Color::Off => "off",
            Color::Red => "red",
            Color::Yellow => "yellow",
            Color::Green => "green",
        })
    }
}

/// No light responded: none plugged in, no permission on `/dev/usb/hiddev*`, or
/// the device stopped answering. Callers are expected to log and carry on — an
/// absent light is not a reason to die.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unavailable;

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no Cleware traffic light responded")
    }
}

impl std::error::Error for Unavailable {}

/// Set the light immediately, with no rate limiting. Prefer [`Light`] in a loop.
///
/// The vendor driver prints `USBswitch not found` on stdout when no device is
/// present; there is no way to silence it short of patching the vendor sources.
// ponytail: vendor stdout noise left alone, patch USBswitchCmd.cpp if it ever
// pollutes machine-readable output.
pub fn set(color: Color) -> Result<(), Unavailable> {
    // SAFETY: the shim takes a plain int, owns every pointer it passes on to the
    // vendor code, and leaves nothing allocated behind.
    let state = unsafe { cleware_set_traffic(color.traffic_mask()) };
    if state == DO_SWITCH_FAILED {
        return Err(Unavailable);
    }
    Ok(())
}

/// Whether the requested colour differs from what the light already shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Changed {
    Yes,
    No,
}

/// Rate-limited handle to the light.
///
/// Callers may request a colour as often as they like. The hardware is only
/// touched when the colour actually changes, never more often than `cooldown`
/// (the light is fragile, and changes inside the window are coalesced so the
/// newest colour wins), and re-asserted every `reassert` so it recovers after
/// being re-plugged. `Duration::ZERO` disables re-asserting.
pub struct Light {
    cooldown: Duration,
    reassert: Duration,
    applied: Option<Color>,
    last_switch: Option<Instant>,
}

impl Light {
    pub fn new(cooldown: Duration, reassert: Duration) -> Self {
        Light {
            cooldown,
            reassert,
            applied: None,
            last_switch: None,
        }
    }

    /// Drive the light towards `desired`. A request deferred by the cooldown, or
    /// one that matches the current colour, is `Ok(())` without touching USB.
    pub fn request(&mut self, desired: Color) -> Result<(), Unavailable> {
        let changed = if self.applied == Some(desired) {
            Changed::No
        } else {
            Changed::Yes
        };
        let since_last = self.last_switch.map(|t| t.elapsed());
        if !should_switch(changed, since_last, self.cooldown, self.reassert) {
            return Ok(());
        }

        let result = set(desired);
        // Advance even when the device is missing, so a light that is not there
        // is not retried on every single tick.
        self.applied = Some(desired);
        self.last_switch = Some(Instant::now());
        result
    }

    /// The colour last pushed to the hardware, if any.
    pub fn current(&self) -> Option<Color> {
        self.applied
    }
}

/// Pure rate-limiting rule behind [`Light::request`]. `since_last` is `None`
/// before the first switch.
fn should_switch(
    changed: Changed,
    since_last: Option<Duration>,
    cooldown: Duration,
    reassert: Duration,
) -> bool {
    let stale = !reassert.is_zero() && since_last.map_or(false, |d| d >= reassert);
    if changed == Changed::No && !stale {
        return false;
    }
    since_last.map_or(true, |d| d >= cooldown)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COOLDOWN: Duration = Duration::from_secs(3);
    const REASSERT: Duration = Duration::from_secs(30);

    #[test]
    fn first_request_switches_immediately() {
        assert!(should_switch(Changed::Yes, None, COOLDOWN, REASSERT));
    }

    #[test]
    fn change_inside_cooldown_is_deferred() {
        assert!(!should_switch(
            Changed::Yes,
            Some(Duration::from_secs(1)),
            COOLDOWN,
            REASSERT
        ));
        assert!(should_switch(
            Changed::Yes,
            Some(Duration::from_secs(4)),
            COOLDOWN,
            REASSERT
        ));
    }

    #[test]
    fn unchanged_colour_is_reasserted_only_once_stale() {
        assert!(!should_switch(
            Changed::No,
            Some(Duration::from_secs(10)),
            COOLDOWN,
            REASSERT
        ));
        assert!(should_switch(
            Changed::No,
            Some(Duration::from_secs(31)),
            COOLDOWN,
            REASSERT
        ));
    }

    #[test]
    fn zero_reassert_never_refreshes() {
        assert!(!should_switch(
            Changed::No,
            Some(Duration::from_secs(3600)),
            COOLDOWN,
            Duration::ZERO
        ));
    }
}
