window.BENCHMARK_DATA = {
  "lastUpdate": 1788605730758,
  "repoUrl": "https://github.com/lostb1t/remux",
  "entries": {
    "Benchmark": [
      {
        "commit": {
          "author": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "id": "9bff6dc53dcbf10f15fd538faf71b0390cb8d69b",
          "message": "chore(bench): replace codspeed/divan with Criterion + github-action-benchmark",
          "timestamp": "2026-09-01T07:17:55Z",
          "url": "https://github.com/lostb1t/remux/commit/9bff6dc53dcbf10f15fd538faf71b0390cb8d69b"
        },
        "date": 1788248800450,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 315903849,
            "range": "± 10288314",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 345898351,
            "range": "± 11511341",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 415273412,
            "range": "± 10656043",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 117787466,
            "range": "± 6506482",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 220331587,
            "range": "± 7657053",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 264014216,
            "range": "± 12370444",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 244180299,
            "range": "± 10875690",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 273314159,
            "range": "± 12196020",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 224739034,
            "range": "± 8218292",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 312075944,
            "range": "± 9471113",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 128562704,
            "range": "± 8667465",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 355249011,
            "range": "± 6885868",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 306392710,
            "range": "± 6513876",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 318077640,
            "range": "± 9370624",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 318607090,
            "range": "± 8053319",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 313638447,
            "range": "± 11291838",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 309953776,
            "range": "± 8386251",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 320923621,
            "range": "± 9311696",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 111725329,
            "range": "± 4552506",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "id": "9bff6dc53dcbf10f15fd538faf71b0390cb8d69b",
          "message": "chore(bench): replace codspeed/divan with Criterion + github-action-benchmark",
          "timestamp": "2026-09-01T07:17:55Z",
          "url": "https://github.com/lostb1t/remux/commit/9bff6dc53dcbf10f15fd538faf71b0390cb8d69b"
        },
        "date": 1788249120408,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 323855880,
            "range": "± 11807619",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 334157334,
            "range": "± 7888556",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 406976151,
            "range": "± 12127075",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 91883323,
            "range": "± 11271960",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 149753823,
            "range": "± 15066740",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 243409649,
            "range": "± 17776511",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 229964646,
            "range": "± 10901896",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 264498161,
            "range": "± 11099244",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 209450052,
            "range": "± 8786237",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 276363881,
            "range": "± 7326756",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 121842582,
            "range": "± 8677144",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 329449590,
            "range": "± 7964487",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 324909306,
            "range": "± 9103969",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 321366276,
            "range": "± 9075037",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 324063947,
            "range": "± 8865529",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 324351992,
            "range": "± 9745705",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 323494629,
            "range": "± 11853624",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 316709180,
            "range": "± 9713870",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 106564095,
            "range": "± 9986306",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "id": "b562c78f6581d17145b7ca7b5b9106b68ff02ba5",
          "message": "chore(bench): disable PR bench check until runner has build-essential",
          "timestamp": "2026-09-01T09:07:55Z",
          "url": "https://github.com/lostb1t/remux/commit/b562c78f6581d17145b7ca7b5b9106b68ff02ba5"
        },
        "date": 1788254047043,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 210200767,
            "range": "± 10015393",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 226824173,
            "range": "± 18296850",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 279408647,
            "range": "± 16902322",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 74682384,
            "range": "± 7684273",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 86969334,
            "range": "± 22749150",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 170196084,
            "range": "± 28177061",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 133381995,
            "range": "± 9305037",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 167068704,
            "range": "± 13207370",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 134215733,
            "range": "± 7820026",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 190172541,
            "range": "± 9071594",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 81796119,
            "range": "± 6139264",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 230407569,
            "range": "± 8698578",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 229973622,
            "range": "± 8456602",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 228513839,
            "range": "± 8761004",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 231328339,
            "range": "± 9364907",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 233266424,
            "range": "± 16374815",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 241289393,
            "range": "± 9249897",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 234282478,
            "range": "± 7136010",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 69394319,
            "range": "± 4512391",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "id": "b562c78f6581d17145b7ca7b5b9106b68ff02ba5",
          "message": "chore(bench): disable PR bench check until runner has build-essential",
          "timestamp": "2026-09-01T09:07:55Z",
          "url": "https://github.com/lostb1t/remux/commit/b562c78f6581d17145b7ca7b5b9106b68ff02ba5"
        },
        "date": 1788256850641,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 213682620,
            "range": "± 11145277",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 227889938,
            "range": "± 13532336",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 278934602,
            "range": "± 14815516",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 77957463,
            "range": "± 10324443",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 137585709,
            "range": "± 15631308",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 167936906,
            "range": "± 28614718",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 144117574,
            "range": "± 14599109",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 162231341,
            "range": "± 16506989",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 131470421,
            "range": "± 6875120",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 184120764,
            "range": "± 9597945",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 77737461,
            "range": "± 10928789",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 231022305,
            "range": "± 7683180",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 224583699,
            "range": "± 11094205",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 221015054,
            "range": "± 8487954",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 224634878,
            "range": "± 9552236",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 221725447,
            "range": "± 8242469",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 221350397,
            "range": "± 9257678",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 413521308,
            "range": "± 82887106",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 75321097,
            "range": "± 93015060",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "id": "76567e219b901a2965801966efe8fb9eaedf7e1e",
          "message": "fix(playback): audio codec profile failures no longer force video re-encode",
          "timestamp": "2026-09-02T06:56:49Z",
          "url": "https://github.com/lostb1t/remux/commit/76567e219b901a2965801966efe8fb9eaedf7e1e"
        },
        "date": 1788333229987,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 219594793,
            "range": "± 21744552",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 204272219,
            "range": "± 6523659",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 271883249,
            "range": "± 13848092",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 72290658,
            "range": "± 6978092",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 78581419,
            "range": "± 8925749",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 114802749,
            "range": "± 5932765",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 119991118,
            "range": "± 6411463",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 144466385,
            "range": "± 6686906",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 120920108,
            "range": "± 5224802",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 172995233,
            "range": "± 6060448",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 71468398,
            "range": "± 12028407",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 220959970,
            "range": "± 7360077",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 229578706,
            "range": "± 12003213",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 230796424,
            "range": "± 10933645",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 227546924,
            "range": "± 9331976",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 223885207,
            "range": "± 7906715",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 223043268,
            "range": "± 8233092",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 223345052,
            "range": "± 7805340",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 67659028,
            "range": "± 4661395",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostb1t",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "GitHub",
            "username": "web-flow",
            "email": "noreply@github.com"
          },
          "id": "6bc53fe1b91af32f253427a2130a13c78b1e09dd",
          "message": "fix(transcode): SW decode fallback when VPP tonemap can't survive subtitle burn-in (#408)",
          "timestamp": "2026-09-02T07:30:59Z",
          "url": "https://github.com/lostb1t/remux/commit/6bc53fe1b91af32f253427a2130a13c78b1e09dd"
        },
        "date": 1788335668868,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 212899989,
            "range": "± 12968733",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 245274848,
            "range": "± 16996450",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 274791674,
            "range": "± 13450328",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 74325708,
            "range": "± 8718089",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 95257965,
            "range": "± 6279631",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 115042039,
            "range": "± 5701380",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 120292152,
            "range": "± 5718620",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 139942637,
            "range": "± 7098038",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 120861833,
            "range": "± 9608827",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 170843451,
            "range": "± 5251919",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 69563767,
            "range": "± 5322934",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 221142840,
            "range": "± 6245817",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 229334553,
            "range": "± 11482612",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 225754801,
            "range": "± 9992955",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 229087343,
            "range": "± 11110957",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 220820580,
            "range": "± 8623495",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 223794273,
            "range": "± 9991300",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 221430096,
            "range": "± 9041077",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 71596458,
            "range": "± 5169236",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "id": "9d5339b87025c9cc4940e4cc0dce7226adc22d16",
          "message": "fix(db): timeout get_by_filter after 30s instead of hanging on lock contention",
          "timestamp": "2026-09-02T09:38:52Z",
          "url": "https://github.com/lostb1t/remux/commit/9d5339b87025c9cc4940e4cc0dce7226adc22d16"
        },
        "date": 1788343034096,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 223465786,
            "range": "± 12947199",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 243321512,
            "range": "± 18343974",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 276005717,
            "range": "± 14325882",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 68623796,
            "range": "± 7605233",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 112925173,
            "range": "± 6379639",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 114955346,
            "range": "± 5631706",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 117603414,
            "range": "± 5942807",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 139112544,
            "range": "± 6890141",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 117980685,
            "range": "± 4973370",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 166818476,
            "range": "± 6807512",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 68591013,
            "range": "± 5099979",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 220111576,
            "range": "± 7460149",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 225306400,
            "range": "± 9054681",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 226922381,
            "range": "± 11590375",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 230473451,
            "range": "± 9023805",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 222725045,
            "range": "± 12094490",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 220809486,
            "range": "± 6639176",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 219515679,
            "range": "± 9607551",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 61782761,
            "range": "± 5885420",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostb1t",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "GitHub",
            "username": "web-flow",
            "email": "noreply@github.com"
          },
          "id": "149917c130ac28967e2dc06f6f7d8af2fd2d2f6f",
          "message": "feat(probe): tag probe_data with its origin (ffprobe/remuxdb/filename-guess) (#410)",
          "timestamp": "2026-09-02T13:22:07Z",
          "url": "https://github.com/lostb1t/remux/commit/149917c130ac28967e2dc06f6f7d8af2fd2d2f6f"
        },
        "date": 1788356352493,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 226358856,
            "range": "± 17803935",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 245278037,
            "range": "± 15723092",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 282564768,
            "range": "± 13126329",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 85834962,
            "range": "± 15089601",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 95333135,
            "range": "± 7886525",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 115438073,
            "range": "± 5021238",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 121022933,
            "range": "± 6596626",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 141104388,
            "range": "± 7546465",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 119598017,
            "range": "± 4566950",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 166619485,
            "range": "± 6740395",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 68393643,
            "range": "± 6295753",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 223060673,
            "range": "± 12072923",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 227911539,
            "range": "± 12836023",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 227817851,
            "range": "± 9567526",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 225257182,
            "range": "± 9781187",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 220438020,
            "range": "± 7722215",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 223534190,
            "range": "± 7592553",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 219181829,
            "range": "± 9354736",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 66272071,
            "range": "± 5954733",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostb1t",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "GitHub",
            "username": "web-flow",
            "email": "noreply@github.com"
          },
          "id": "0f4b02828538f2b12eeab61eaf3b840e7594dfc2",
          "message": "fix(sdks): honor Retry-After on 429 instead of retrying on a blind backoff curve (#415)",
          "timestamp": "2026-09-02T21:17:32Z",
          "url": "https://github.com/lostb1t/remux/commit/0f4b02828538f2b12eeab61eaf3b840e7594dfc2"
        },
        "date": 1788418072209,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 223567702,
            "range": "± 10007593",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 245981591,
            "range": "± 19723000",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 289403160,
            "range": "± 14681107",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 75007413,
            "range": "± 11581667",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 125707662,
            "range": "± 7259952",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 118545234,
            "range": "± 6452496",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 120559034,
            "range": "± 5895541",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 142518467,
            "range": "± 10133490",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 121743469,
            "range": "± 6245613",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 173616985,
            "range": "± 6689335",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 71723036,
            "range": "± 6610314",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 232521129,
            "range": "± 7618079",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 222561719,
            "range": "± 9989941",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 221122343,
            "range": "± 8954559",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 221211787,
            "range": "± 8793861",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 223362235,
            "range": "± 12380584",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 224876899,
            "range": "± 8251255",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 224158972,
            "range": "± 9315189",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 66879900,
            "range": "± 6304526",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "id": "5fe49970b3e1d16f9646a37b98997cf308cdef21",
          "message": "fix(auth): decode Authorization header bytes lossily instead of rejecting on non-ASCII (fixes #397)",
          "timestamp": "2026-09-03T16:10:51Z",
          "url": "https://github.com/lostb1t/remux/commit/5fe49970b3e1d16f9646a37b98997cf308cdef21"
        },
        "date": 1788453746096,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 216268824,
            "range": "± 9742385",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 235189662,
            "range": "± 14497649",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 285005294,
            "range": "± 15360410",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 75028298,
            "range": "± 6270255",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 122698667,
            "range": "± 8470181",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 118122751,
            "range": "± 6329901",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 120325070,
            "range": "± 5065086",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 141236968,
            "range": "± 6321952",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 119713953,
            "range": "± 5895678",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 167857215,
            "range": "± 10741346",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 70286839,
            "range": "± 4580309",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 220484364,
            "range": "± 6756291",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 221497007,
            "range": "± 7950909",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 224258308,
            "range": "± 8428304",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 223547660,
            "range": "± 10490818",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 222528871,
            "range": "± 9228535",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 221228109,
            "range": "± 8419749",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 223998896,
            "range": "± 7836171",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 67073569,
            "range": "± 6887417",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "id": "7e2a9cb7bcdea26b6179a3f02e9c1692d8b45a80",
          "message": "fix: box search-result resolve chain to prevent stack overflow on playback",
          "timestamp": "2026-09-03T21:57:38Z",
          "url": "https://github.com/lostb1t/remux/commit/7e2a9cb7bcdea26b6179a3f02e9c1692d8b45a80"
        },
        "date": 1788473940448,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 226295228,
            "range": "± 13703828",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 239268396,
            "range": "± 17478516",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 290603196,
            "range": "± 22141791",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 73166420,
            "range": "± 7180222",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 131308448,
            "range": "± 7968802",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 163793534,
            "range": "± 29066893",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 147657264,
            "range": "± 17755211",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 164522425,
            "range": "± 11635029",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 148271401,
            "range": "± 12878494",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 186592564,
            "range": "± 11516772",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 87930968,
            "range": "± 10445878",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 248562843,
            "range": "± 9295594",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 229547580,
            "range": "± 17214875",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 224707524,
            "range": "± 8194821",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 225244232,
            "range": "± 8338487",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 227704830,
            "range": "± 11267129",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 232274992,
            "range": "± 12830665",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 220944453,
            "range": "± 9139753",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 66295051,
            "range": "± 3991674",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "id": "f4f0e919a87f664a53a5de27a31e08660af1badf",
          "message": "ci: pin cargo-bundle install to its lockfile to avoid dependency drift breaks",
          "timestamp": "2026-09-03T22:31:34Z",
          "url": "https://github.com/lostb1t/remux/commit/f4f0e919a87f664a53a5de27a31e08660af1badf"
        },
        "date": 1788475720101,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 231038962,
            "range": "± 16462396",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 244302030,
            "range": "± 15054147",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 294458924,
            "range": "± 19891426",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 77182002,
            "range": "± 6446994",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 129668157,
            "range": "± 8203523",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 171039918,
            "range": "± 21453571",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 137305798,
            "range": "± 9744043",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 162842945,
            "range": "± 14839846",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 140741218,
            "range": "± 6698075",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 186274229,
            "range": "± 8782697",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 87288181,
            "range": "± 11777555",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 231867948,
            "range": "± 7783446",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 221832502,
            "range": "± 8107533",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 223166980,
            "range": "± 12163621",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 224511369,
            "range": "± 10112200",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 225684601,
            "range": "± 10325238",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 235442340,
            "range": "± 10462150",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 232348878,
            "range": "± 10262828",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 70071712,
            "range": "± 5123364",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "id": "5c2ea4679c6f04aaf64de17ee54a608477d1004b",
          "message": "fix: catalog membership dropped after id dedup, and empty promoted collection groups hidden\n\nprocess_meta_batch discarded the final UUID when find_existing_id_by_ext\nadopted an existing row (dedup by external ID), so import_catalog_items\nrecorded catalog relations and stale-member diffs against a pre-remap id\nthat was never written, silently dropping membership (fixes #426).\n\ngroup_container_has_visible_content hid a freshly promoted\ncollection-of-collections that has no sub-collections yet, conflating\nthat with #414's actual case of a group whose existing children are all\nempty (fixes #425).",
          "timestamp": "2026-09-04T08:23:10Z",
          "url": "https://github.com/lostb1t/remux/commit/5c2ea4679c6f04aaf64de17ee54a608477d1004b"
        },
        "date": 1788511263941,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 219991799,
            "range": "± 14772825",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 241725113,
            "range": "± 17923232",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 282702360,
            "range": "± 18309865",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 74166960,
            "range": "± 6568006",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 86757711,
            "range": "± 10255263",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 169075964,
            "range": "± 20344405",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 141646548,
            "range": "± 12925081",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 164451309,
            "range": "± 14705328",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 141537731,
            "range": "± 7721477",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 187332566,
            "range": "± 9483477",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 86667267,
            "range": "± 9820099",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 224250989,
            "range": "± 6884772",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 225040760,
            "range": "± 8770174",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 225575261,
            "range": "± 10126784",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 225559164,
            "range": "± 10096183",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 223628637,
            "range": "± 9318776",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 234065523,
            "range": "± 11804209",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 236269158,
            "range": "± 12709035",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 69080635,
            "range": "± 4996972",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "id": "b473cc15279e615cfbcb3b3c2467d0f3480edfa4",
          "message": "ci: fix invalid matrix context reference that broke PR Docker entirely\n\nPrevious commit (ff7479ba) gated build-server's macOS legs with a\njob-level `if: inputs.desktop || matrix.os_name == 'linux'` — job-level\nif can't reference the matrix context (only github/inputs/needs/vars\nare available there), so GitHub rejected the whole workflow file at\ndispatch time with zero jobs created.\n\nMoved the desktop/docker-only split into the matrix data itself via a\nsmall server-targets job that computes build-server's `include` list\nwith jq before the matrix fans out, so the macOS legs are excluded\noutright for docker-only builds instead of conditioned on per-leg.\nValidated both workflow files with actionlint this time.",
          "timestamp": "2026-09-04T19:43:43Z",
          "url": "https://github.com/lostb1t/remux/commit/b473cc15279e615cfbcb3b3c2467d0f3480edfa4"
        },
        "date": 1788591967919,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 220230563,
            "range": "± 13793523",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 232242860,
            "range": "± 14663877",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 274948453,
            "range": "± 13692184",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 72590667,
            "range": "± 6082233",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 103717873,
            "range": "± 13473154",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 141387666,
            "range": "± 14886424",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 132404889,
            "range": "± 8889890",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 158455448,
            "range": "± 11044602",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 127762473,
            "range": "± 4352863",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 179543676,
            "range": "± 8888884",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 76655925,
            "range": "± 5601445",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 232702598,
            "range": "± 7625790",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 233790214,
            "range": "± 8554621",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 218827611,
            "range": "± 8098636",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 224727118,
            "range": "± 8509925",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 223653551,
            "range": "± 10678390",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 226608577,
            "range": "± 10092232",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 220869649,
            "range": "± 8700850",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 68677201,
            "range": "± 5210839",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostb1t",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "GitHub",
            "username": "web-flow",
            "email": "noreply@github.com"
          },
          "id": "6fcd41b04990bcbb9f4c49e97b949ca8ed368fa0",
          "message": "feat(collections): image configurator (#405)",
          "timestamp": "2026-09-05T09:05:52Z",
          "url": "https://github.com/lostb1t/remux/commit/6fcd41b04990bcbb9f4c49e97b949ca8ed368fa0"
        },
        "date": 1788600289393,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 222287993,
            "range": "± 17998939",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 236398501,
            "range": "± 15231701",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 300128503,
            "range": "± 17797193",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 79764446,
            "range": "± 8463853",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 134357140,
            "range": "± 11166384",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 145882870,
            "range": "± 17498632",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 142661879,
            "range": "± 14009620",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 164023995,
            "range": "± 13526207",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 129139079,
            "range": "± 6013625",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 179495495,
            "range": "± 12038660",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 76024118,
            "range": "± 6620165",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 248373795,
            "range": "± 11050400",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 219996106,
            "range": "± 8623471",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 218922744,
            "range": "± 10718997",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 222222843,
            "range": "± 9303113",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 222456214,
            "range": "± 8099515",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 218257464,
            "range": "± 8817865",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 227612852,
            "range": "± 12040841",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 73021113,
            "range": "± 10513791",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "committer": {
            "name": "lostbit",
            "username": "lostb1t",
            "email": "coding-mosses0z@icloud.com"
          },
          "id": "80b31144de62aeb582a76e45ffa65d1868fd3780",
          "message": "fix: stop collection image preview from persisting changes, scope poster selection to the collection",
          "timestamp": "2026-09-05T10:09:51Z",
          "url": "https://github.com/lostb1t/remux/commit/80b31144de62aeb582a76e45ffa65d1868fd3780"
        },
        "date": 1788605729136,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 226116364,
            "range": "± 11812308",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 262299893,
            "range": "± 20569440",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 291977768,
            "range": "± 15653709",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 77759577,
            "range": "± 7974444",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 106934598,
            "range": "± 15637495",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 139129883,
            "range": "± 13112829",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 142840138,
            "range": "± 15333192",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 159349202,
            "range": "± 11573998",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 126419993,
            "range": "± 7084915",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 179599560,
            "range": "± 6387184",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 76467219,
            "range": "± 6175229",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 239910939,
            "range": "± 9689288",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 227771954,
            "range": "± 9758261",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 229766480,
            "range": "± 9012004",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 224345650,
            "range": "± 10725297",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 220170496,
            "range": "± 8768806",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 221735743,
            "range": "± 9204896",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 225362296,
            "range": "± 10927390",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 72133085,
            "range": "± 5491683",
            "unit": "ns/iter"
          }
        ]
      }
    ]
  }
}