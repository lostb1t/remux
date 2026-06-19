extern crate codspeed_divan_compat as divan;

#[path = "common.rs"]
mod common;

use common::{BenchQuery, run_bench};

fn main() {
    divan::main();
}

#[divan::bench(args = [
    BenchQuery { name: "limit=50",  url: "/shows/nextup?limit=50" },
    BenchQuery { name: "limit=200", url: "/shows/nextup?limit=200" },
    BenchQuery { name: "limit=500", url: "/shows/nextup?limit=500" },
])]
fn nextup_scale(bencher: divan::Bencher, q: &BenchQuery) {
    run_bench(bencher, q.url);
}

#[divan::bench(args = [
    BenchQuery { name: "resumable=true",  url: "/shows/nextup?limit=500&enable_resumable=true" },
    BenchQuery { name: "resumable=false", url: "/shows/nextup?limit=500&enable_resumable=false" },
])]
fn nextup_resumable(bencher: divan::Bencher, q: &BenchQuery) {
    run_bench(bencher, q.url);
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
