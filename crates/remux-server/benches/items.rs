extern crate codspeed_divan_compat as divan;

#[path = "common.rs"]
mod common;

use common::{BenchQuery, run_bench};

fn main() {
    divan::main();
}

#[divan::bench(args = [
    BenchQuery { name: "limit=20",  url: "/items/latest?limit=20" },
    BenchQuery { name: "limit=100", url: "/items/latest?limit=100" },
    BenchQuery { name: "limit=500", url: "/items/latest?limit=500" },
    BenchQuery { name: "Movie",     url: "/items/latest?limit=100&include_item_types=Movie" },
    BenchQuery { name: "Series",    url: "/items/latest?limit=100&include_item_types=Series" },
])]
fn items_latest(bencher: divan::Bencher, q: &BenchQuery) {
    run_bench(bencher, q.url);
}

#[divan::bench(args = [
    BenchQuery { name: "DateCreated/50",  url: "/items?limit=50&sort_by=DateCreated" },
    BenchQuery { name: "DateCreated/200", url: "/items?limit=200&sort_by=DateCreated" },
    BenchQuery { name: "DateCreated/500", url: "/items?limit=500&sort_by=DateCreated" },
    BenchQuery { name: "SortName/100",    url: "/items?limit=100&sort_by=SortName" },
    BenchQuery { name: "DatePlayed/100",  url: "/items?limit=100&sort_by=DatePlayed" },
])]
fn items_browse(bencher: divan::Bencher, q: &BenchQuery) {
    run_bench(bencher, q.url);
}

#[divan::bench(args = [
    BenchQuery { name: "limit=10",  url: "/useritems/resume?limit=10" },
    BenchQuery { name: "limit=50",  url: "/useritems/resume?limit=50" },
    BenchQuery { name: "limit=200", url: "/useritems/resume?limit=200" },
])]
fn resume(bencher: divan::Bencher, q: &BenchQuery) {
    run_bench(bencher, q.url);
}
