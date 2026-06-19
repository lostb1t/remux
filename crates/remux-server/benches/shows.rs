extern crate codspeed_divan_compat as divan;

#[path = "common.rs"]
mod common;

use common::{auth_header, fixture};
use http::header;

fn main() {
    divan::main();
}

#[divan::bench(args = [50, 200, 500])]
fn nextup_all_scale(bencher: divan::Bencher, limit: usize) {
    let f = fixture();
    let url = format!("{}/shows/nextup?limit={limit}", f.base_url);
    let auth = auth_header(&f.token);

    bencher.bench(|| {
        f.rt.block_on(async {
            f.client
                .get(&url)
                .header(header::AUTHORIZATION, &auth)
                .send()
                .await
                .unwrap();
        })
    });
}

#[divan::bench(args = [true, false])]
fn nextup_all_resumable(bencher: divan::Bencher, enable: bool) {
    let f = fixture();
    let url = format!(
        "{}/shows/nextup?limit=500&enable_resumable={enable}",
        f.base_url
    );
    let auth = auth_header(&f.token);

    bencher.bench(|| {
        f.rt.block_on(async {
            f.client
                .get(&url)
                .header(header::AUTHORIZATION, &auth)
                .send()
                .await
                .unwrap();
        })
    });
}

#[divan::bench(args = ["epoch", "30days"])]
fn nextup_all_date_cutoff(bencher: divan::Bencher, cutoff: &str) {
    let f = fixture();
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
    let url = format!(
        "{}/shows/nextup?limit=500&next_up_date_cutoff={cutoff_param}",
        f.base_url
    );
    let auth = auth_header(&f.token);

    bencher.bench(|| {
        f.rt.block_on(async {
            f.client
                .get(&url)
                .header(header::AUTHORIZATION, &auth)
                .send()
                .await
                .unwrap();
        })
    });
}
