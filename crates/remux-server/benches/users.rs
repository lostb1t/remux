extern crate codspeed_divan_compat as divan;

#[path = "common.rs"]
mod common;

use common::{fixture, run_bench};

fn main() {
    divan::main();
}

// `/userviews` assembles the user's library views from the full media set; it
// is hit on nearly every client home-screen load.
#[divan::bench]
fn userviews(bencher: divan::Bencher) {
    run_bench(bencher, "/userviews");
}

// `/users/{user_id}/views` is the legacy alias that delegates to `userviews`;
// benched separately so a divergence between the two routes is visible.
#[divan::bench]
fn user_views_by_id(bencher: divan::Bencher) {
    let f = fixture();
    run_bench(bencher, &format!("/users/{}/views", f.user_id));
}

// `/users/me` — cheap authenticated identity lookup; acts as a per-request
// auth/serialization floor to catch regressions in the common path.
#[divan::bench]
fn users_me(bencher: divan::Bencher) {
    run_bench(bencher, "/users/me");
}
