use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

#[path = "common.rs"]
mod common;

use common::{BenchQuery, IntoBench, run_bench};
use remux_server::sdks::remux::{GetItemsQuery, ItemFilter, ItemSortBy, MediaType};

fn items_latest(c: &mut Criterion) {
    let queries: Vec<BenchQuery> = vec![
        GetItemsQuery {
            limit: Some(20),
            ..Default::default()
        }
        .into_bench("/items/latest"),
        GetItemsQuery {
            limit: Some(100),
            ..Default::default()
        }
        .into_bench("/items/latest"),
        GetItemsQuery {
            limit: Some(500),
            ..Default::default()
        }
        .into_bench("/items/latest"),
        GetItemsQuery {
            limit: Some(100),
            include_item_types: Some(vec![MediaType::Movie]),
            ..Default::default()
        }
        .into_bench("/items/latest"),
        GetItemsQuery {
            limit: Some(100),
            include_item_types: Some(vec![MediaType::Series]),
            ..Default::default()
        }
        .into_bench("/items/latest"),
    ];
    let mut group = c.benchmark_group("items_latest");
    for q in &queries {
        group.bench_with_input(BenchmarkId::from_parameter(&q.name), q, |b, q| {
            run_bench(b, &q.url);
        });
    }
    group.finish();
}

fn items_get(c: &mut Criterion) {
    let queries: Vec<BenchQuery> = vec![
        GetItemsQuery {
            limit: Some(20),
            ..Default::default()
        }
        .into_bench("/items"),
        GetItemsQuery {
            limit: Some(100),
            ..Default::default()
        }
        .into_bench("/items"),
        GetItemsQuery {
            limit: Some(500),
            ..Default::default()
        }
        .into_bench("/items"),
        GetItemsQuery {
            limit: Some(100),
            include_item_types: Some(vec![MediaType::Movie]),
            ..Default::default()
        }
        .into_bench("/items"),
        GetItemsQuery {
            limit: Some(100),
            include_item_types: Some(vec![MediaType::Series]),
            ..Default::default()
        }
        .into_bench("/items"),
        GetItemsQuery {
            limit: Some(100),
            filters: Some(vec![ItemFilter::IsPlayed]),
            ..Default::default()
        }
        .into_bench("/items"),
        GetItemsQuery {
            limit: Some(100),
            sort_by: Some(vec![ItemSortBy::DateCreated]),
            ..Default::default()
        }
        .into_bench("/items"),
    ];
    let mut group = c.benchmark_group("items_get");
    for q in &queries {
        group.bench_with_input(BenchmarkId::from_parameter(&q.name), q, |b, q| {
            run_bench(b, &q.url);
        });
    }
    group.finish();
}

criterion_group!(benches, items_latest, items_get);
criterion_main!(benches);
