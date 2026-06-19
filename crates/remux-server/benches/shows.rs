extern crate codspeed_divan_compat as divan;

#[path = "common.rs"]
mod common;

use common::{BenchQuery, IntoBench, run_bench};
use remux_server::sdks::remux::GetItemsQuery;

fn main() {
    divan::main();
}

#[divan::bench(args = [
    GetItemsQuery { limit: Some(50),  ..Default::default() }.into_bench("/shows/nextup"),
    GetItemsQuery { limit: Some(200), ..Default::default() }.into_bench("/shows/nextup"),
    GetItemsQuery { limit: Some(500), ..Default::default() }.into_bench("/shows/nextup"),
])]
fn nextup_scale(bencher: divan::Bencher, q: &BenchQuery) {
    run_bench(bencher, &q.url);
}

#[divan::bench(args = [
    GetItemsQuery { limit: Some(500), enable_resumable: Some(true),  ..Default::default() }.into_bench("/shows/nextup"),
    GetItemsQuery { limit: Some(500), enable_resumable: Some(false), ..Default::default() }.into_bench("/shows/nextup"),
])]
fn nextup_resumable(bencher: divan::Bencher, q: &BenchQuery) {
    run_bench(bencher, &q.url);
}

#[divan::bench(args = ["epoch", "30days"])]
fn nextup_date_cutoff(bencher: divan::Bencher, cutoff: &str) {
    let cutoff_param = match cutoff {
        "30days" => {
            let ts = chrono::Utc::now() - chrono::Duration::days(30);
            urlencoding::encode(
                &ts.format("%Y-%m-%dT%H:%M:%SZ")
                    .to_string(),
            )
            .into_owned()
        }
        _ => "1970-01-01%2000%3A00%3A00".to_string(),
    };
    run_bench(
        bencher,
        &format!("/shows/nextup?limit=500&next_up_date_cutoff={cutoff_param}"),
    );
}
