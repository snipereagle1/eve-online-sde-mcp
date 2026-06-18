/// SDE build number the offline test fixtures are pinned to. Only referenced from
/// test code (fixture scans + the ignored network integration check), so it is
/// gated to `cfg(test)` to stay out of release builds.
#[cfg(test)]
pub const PINNED_BUILD: u64 = 3333874;
