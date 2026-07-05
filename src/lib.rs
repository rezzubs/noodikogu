#![allow(
    dead_code,
    reason = "nothing outside tests calls into the library yet; remove once the CLI/web server wire it up"
)]
#![cfg_attr(
    test,
    allow(clippy::unwrap_used, reason = "unwrap is fine in test code")
)]
pub mod catalogue;

#[allow(
    clippy::pedantic,
    reason = "slated for a rewrite soon; not worth acting on yet"
)]
pub mod query;
