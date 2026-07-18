extern crate codspeed_divan_compat as divan;

#[path = "common.rs"]
mod common;

use common::run_bench;

fn main() {
    divan::main();
}

// `/search/hints` runs the full item query with a `search_term`, so it exercises
// the LIKE/search path over the seeded 30k titles ("Bench Movie N" /
// "Bench Series N"). A term that matches many rows stresses the ranking/limit
// path; a more specific term exercises the narrower result set.
#[divan::bench(args = [
    "/search/hints?searchTerm=Bench&limit=20",
    "/search/hints?searchTerm=Bench&limit=50",
    "/search/hints?searchTerm=Bench%20Movie&limit=50",
])]
fn search_hints(bencher: divan::Bencher, url: &str) {
    run_bench(bencher, url);
}
