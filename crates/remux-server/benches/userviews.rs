extern crate codspeed_divan_compat as divan;

#[path = "common.rs"]
mod common;

use common::run_bench;

fn main() {
    divan::main();
}

#[divan::bench]
fn userviews(bencher: divan::Bencher) {
    run_bench(bencher, "/userviews");
}
