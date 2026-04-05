#![no_main]

use libfuzzer_sys::fuzz_target;
use noodikogu::query::Query;

fuzz_target!(|data: &str| {
    _ = Query::parse(data, 0);
});
