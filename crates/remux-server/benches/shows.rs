use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

#[path = "common.rs"]
mod common;

use common::{BenchQuery, IntoBench, run_bench};
use remux_server::sdks::remux::GetItemsQuery;

fn nextup_scale(c: &mut Criterion) {
    let queries: Vec<BenchQuery> = vec![
        GetItemsQuery {
            limit: Some(50),
            ..Default::default()
        }
        .into_bench("/shows/nextup"),
        GetItemsQuery {
            limit: Some(200),
            ..Default::default()
        }
        .into_bench("/shows/nextup"),
        GetItemsQuery {
            limit: Some(500),
            ..Default::default()
        }
        .into_bench("/shows/nextup"),
    ];
    let mut group = c.benchmark_group("nextup_scale");
    for q in &queries {
        group.bench_with_input(BenchmarkId::from_parameter(&q.name), q, |b, q| {
            run_bench(b, &q.url);
        });
    }
    group.finish();
}

fn nextup_resumable(c: &mut Criterion) {
    let queries: Vec<BenchQuery> = vec![
        GetItemsQuery {
            limit: Some(500),
            enable_resumable: Some(true),
            ..Default::default()
        }
        .into_bench("/shows/nextup"),
        GetItemsQuery {
            limit: Some(500),
            enable_resumable: Some(false),
            ..Default::default()
        }
        .into_bench("/shows/nextup"),
    ];
    let mut group = c.benchmark_group("nextup_resumable");
    for q in &queries {
        group.bench_with_input(BenchmarkId::from_parameter(&q.name), q, |b, q| {
            run_bench(b, &q.url);
        });
    }
    group.finish();
}

fn nextup_date_cutoff(c: &mut Criterion) {
    let cutoffs = ["epoch", "30days"];
    let mut group = c.benchmark_group("nextup_date_cutoff");
    for cutoff in cutoffs {
        let url = match cutoff {
            "30days" => {
                let ts = chrono::Utc::now() - chrono::Duration::days(30);
                let encoded = urlencoding::encode(
                    &ts.format("%Y-%m-%dT%H:%M:%SZ")
                        .to_string(),
                )
                .into_owned();
                format!("/shows/nextup?limit=500&next_up_date_cutoff={encoded}")
            }
            _ => {
                "/shows/nextup?limit=500&next_up_date_cutoff=1970-01-01%2000%3A00%3A00"
                    .to_string()
            }
        };
        group.bench_function(BenchmarkId::from_parameter(cutoff), |b| {
            run_bench(b, &url);
        });
    }
    group.finish();
}

criterion_group!(benches, nextup_scale, nextup_resumable, nextup_date_cutoff);
criterion_main!(benches);
