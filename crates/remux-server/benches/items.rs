extern crate codspeed_divan_compat as divan;

#[path = "common.rs"]
mod common;

use common::{BenchQuery, IntoBench, run_bench};
use remux_server::sdks::remux::{GetItemsQuery, ItemSortBy, MediaType};

fn main() {
    divan::main();
}

#[divan::bench(args = [
    GetItemsQuery { limit: Some(20),  ..Default::default() }.into_bench("/items/latest"),
    GetItemsQuery { limit: Some(100), ..Default::default() }.into_bench("/items/latest"),
    GetItemsQuery { limit: Some(500), ..Default::default() }.into_bench("/items/latest"),
    GetItemsQuery { limit: Some(100), include_item_types: Some(vec![MediaType::Movie]),  ..Default::default() }.into_bench("/items/latest"),
    GetItemsQuery { limit: Some(100), include_item_types: Some(vec![MediaType::Series]), ..Default::default() }.into_bench("/items/latest"),
])]
fn items_latest(bencher: divan::Bencher, q: &BenchQuery) {
    run_bench(bencher, &q.url);
}

#[divan::bench(args = [
    GetItemsQuery { limit: Some(50),  sort_by: Some(vec![ItemSortBy::DateCreated]), ..Default::default() }.into_bench("/items"),
    GetItemsQuery { limit: Some(200), sort_by: Some(vec![ItemSortBy::DateCreated]), ..Default::default() }.into_bench("/items"),
    GetItemsQuery { limit: Some(500), sort_by: Some(vec![ItemSortBy::DateCreated]), ..Default::default() }.into_bench("/items"),
    GetItemsQuery { limit: Some(100), sort_by: Some(vec![ItemSortBy::SortName]),    ..Default::default() }.into_bench("/items"),
    GetItemsQuery { limit: Some(100), sort_by: Some(vec![ItemSortBy::DatePlayed]),  ..Default::default() }.into_bench("/items"),
])]
fn items_browse(bencher: divan::Bencher, q: &BenchQuery) {
    run_bench(bencher, &q.url);
}

#[divan::bench(args = [
    GetItemsQuery { limit: Some(10),  ..Default::default() }.into_bench("/useritems/resume"),
    GetItemsQuery { limit: Some(50),  ..Default::default() }.into_bench("/useritems/resume"),
    GetItemsQuery { limit: Some(200), ..Default::default() }.into_bench("/useritems/resume"),
])]
fn resume(bencher: divan::Bencher, q: &BenchQuery) {
    run_bench(bencher, &q.url);
}
