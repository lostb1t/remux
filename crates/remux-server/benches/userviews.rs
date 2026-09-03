use criterion::{Criterion, criterion_group, criterion_main};

#[path = "common.rs"]
mod common;

use common::run_bench;

fn userviews(c: &mut Criterion) {
    c.bench_function("userviews/with-empty-subcollections", |b| {
        run_bench(b, "/userviews");
    });
}

criterion_group!(benches, userviews);
criterion_main!(benches);
