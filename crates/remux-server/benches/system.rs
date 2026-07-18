extern crate codspeed_divan_compat as divan;

#[path = "common.rs"]
mod common;

use common::run_bench;

fn main() {
    divan::main();
}

// Cheap, near-constant endpoints. These are a regression tripwire: their timing
// is dominated by routing + serialization overhead rather than DB work, so a
// jump here flags a regression in the common request machinery (middleware,
// extractors, JSON encoding) that every endpoint pays.
#[divan::bench(args = [
    "/system/info/public",
    "/system/info",
    "/system/ping",
])]
fn system_floor(bencher: divan::Bencher, url: &str) {
    run_bench(bencher, url);
}
