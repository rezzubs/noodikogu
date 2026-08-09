#![allow(
    dead_code,
    reason = "not every Catalogue method is called outside tests yet; remove once the TUI/web server wire up the rest"
)]
#![cfg_attr(
    test,
    allow(clippy::unwrap_used, reason = "unwrap is fine in test code")
)]
pub mod catalogue;
pub mod import;

#[allow(
    clippy::pedantic,
    reason = "slated for a rewrite soon; not worth acting on yet"
)]
pub mod query;
