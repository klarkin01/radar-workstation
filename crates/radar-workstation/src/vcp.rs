//! VCP operational cadence (plan §2.6). Deliberately **not** in
//! `nexrad-decoder::types::vcp`: a volume's nominal duration is not decoded
//! from any message — it is knowledge about how the WSR-88D is operated, used
//! only to decide when a displayed scan has aged past operational usefulness.

use std::time::Duration;

/// Nominal time to complete one volume for `vcp_number`. Nominal: SAILS and
/// MRLE insert extra cuts and can extend a real volume by ~1 min per added
/// cut, which is why callers compare against a *multiple* of this rather than
/// against it directly. An unrecognised VCP falls back to the longest defined
/// pattern (10 min), so an unknown pattern under-warns rather than crying wolf.
pub fn nominal_volume_duration(vcp_number: u16) -> Duration {
    let secs = match vcp_number {
        12 => 252,  // 4.2 min
        212 => 270, // 4.5 min
        112 => 330, // 5.5 min
        21 | 121 | 215 => 360, // 6.0 min
        35 => 420,  // 7.0 min
        31 | 32 => 600, // 10.0 min
        _ => 600,   // longest defined cycle
    };
    Duration::from_secs(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vcps_have_their_published_cycle_times() {
        assert_eq!(nominal_volume_duration(12), Duration::from_secs(252));
        assert_eq!(nominal_volume_duration(35), Duration::from_secs(420));
        assert_eq!(nominal_volume_duration(212), Duration::from_secs(270));
        assert_eq!(nominal_volume_duration(31), Duration::from_secs(600));
    }

    #[test]
    fn an_unknown_vcp_falls_back_to_the_longest_defined_cycle() {
        assert_eq!(nominal_volume_duration(0), Duration::from_secs(600));
        assert_eq!(nominal_volume_duration(999), Duration::from_secs(600));
    }
}
