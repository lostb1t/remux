window.BENCHMARK_DATA = {
  "lastUpdate": 1789904123012,
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
      },
      {
        "commit": {
          "author": {
            "name": "Daniel Chan",
            "username": "brokenthumbs",
            "email": "daniel.chan@bilt.com"
          },
          "committer": {
            "name": "GitHub",
            "username": "web-flow",
            "email": "noreply@github.com"
          },
          "id": "2b151b388420814a365ba46557506bae8bbf3ca2",
          "message": "fix(playback): resolve item ids used as MediaSourceId instead of serving no-streams placeholder (#432)",
          "timestamp": "2026-09-06T05:33:55Z",
          "url": "https://github.com/lostb1t/remux/commit/2b151b388420814a365ba46557506bae8bbf3ca2"
        },
        "date": 1788679040788,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 237464191,
            "range": "± 14466508",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 260944537,
            "range": "± 21784156",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 308497933,
            "range": "± 20482341",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 76835294,
            "range": "± 5521137",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 84625107,
            "range": "± 11162329",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 130390129,
            "range": "± 9369665",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 141423745,
            "range": "± 9013111",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 153978963,
            "range": "± 7373014",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 144592779,
            "range": "± 8265875",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 185784070,
            "range": "± 10203307",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 80782708,
            "range": "± 10886410",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 236561066,
            "range": "± 9292014",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 223126570,
            "range": "± 9691247",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 223573412,
            "range": "± 11561814",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 228301119,
            "range": "± 12448769",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 232974722,
            "range": "± 8314712",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 225771677,
            "range": "± 9208484",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 226202261,
            "range": "± 16054398",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 74608721,
            "range": "± 5355909",
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
          "id": "3624c1eaea2f7d54998c533aabc32e83cc699560",
          "message": "fix(subtitles): add ffmpeg reconnect flags for remote subtitle extraction (fixes #434)",
          "timestamp": "2026-09-06T12:19:32Z",
          "url": "https://github.com/lostb1t/remux/commit/3624c1eaea2f7d54998c533aabc32e83cc699560"
        },
        "date": 1788766205798,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 272426494,
            "range": "± 17502229",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 301139177,
            "range": "± 27510563",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 356470946,
            "range": "± 18383579",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 103733281,
            "range": "± 15300693",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 117718964,
            "range": "± 16831664",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 162672924,
            "range": "± 14619517",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 192429888,
            "range": "± 22109533",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 221872002,
            "range": "± 22641145",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 168714148,
            "range": "± 14150317",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 224758011,
            "range": "± 20938328",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 119359108,
            "range": "± 12118233",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 295433479,
            "range": "± 21146971",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 275425956,
            "range": "± 17782718",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 265420884,
            "range": "± 15906021",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 268315647,
            "range": "± 18782132",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 261296139,
            "range": "± 16388986",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 272859629,
            "range": "± 20988265",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 271703501,
            "range": "± 21527749",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 83209501,
            "range": "± 9488005",
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
          "id": "4955b9244269df45e0841ab7e6a14a92a0f7f34e",
          "message": "feat(dashboard): expose send-all-properties, trim-whitespace, skip-empty-body toggles (#443)",
          "timestamp": "2026-09-07T16:09:25Z",
          "url": "https://github.com/lostb1t/remux/commit/4955b9244269df45e0841ab7e6a14a92a0f7f34e"
        },
        "date": 1788798604356,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 247678829,
            "range": "± 16553441",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 265548633,
            "range": "± 21704117",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 315807316,
            "range": "± 21494023",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 85190641,
            "range": "± 10216186",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 96461284,
            "range": "± 11565496",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 157804338,
            "range": "± 15173450",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 183258391,
            "range": "± 22652114",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 187737991,
            "range": "± 18166143",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 166203217,
            "range": "± 19127525",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 252522305,
            "range": "± 21196059",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 105604984,
            "range": "± 11907997",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 285273807,
            "range": "± 21564825",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 252153752,
            "range": "± 21774927",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 238508373,
            "range": "± 17346841",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 248319162,
            "range": "± 27754645",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 250044250,
            "range": "± 18401317",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 252198532,
            "range": "± 23781909",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 243675910,
            "range": "± 17196516",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 83676924,
            "range": "± 12825599",
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
          "id": "88dbacd736154d69dddc687e4a82f839533fb73d",
          "message": "fix(ci): bump dioxus-cli pin to match the dioxus library version (0.7.9)\n\nCo-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01XLFFGASL5q4kVHpTYnr81e",
          "timestamp": "2026-09-07T16:39:26Z",
          "url": "https://github.com/lostb1t/remux/commit/88dbacd736154d69dddc687e4a82f839533fb73d"
        },
        "date": 1788800459856,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 259909268,
            "range": "± 14444673",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 294791944,
            "range": "± 30698405",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 366350395,
            "range": "± 28626581",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 90380006,
            "range": "± 8240384",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 120260333,
            "range": "± 17597464",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 163393426,
            "range": "± 12650274",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 177435280,
            "range": "± 16869652",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 229076110,
            "range": "± 38240997",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 163686920,
            "range": "± 57388893",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 218819051,
            "range": "± 31518008",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 121606458,
            "range": "± 17172294",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 269170917,
            "range": "± 44230517",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 295286151,
            "range": "± 56255803",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 322403385,
            "range": "± 23134046",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 387432812,
            "range": "± 85397612",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 273085280,
            "range": "± 21287382",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 266648753,
            "range": "± 20162319",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 271804039,
            "range": "± 26657654",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 80526035,
            "range": "± 9903826",
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
          "id": "9f05c16299f195e4f59621de32026ed881a9261d",
          "message": "fix(collections): skip image configurator pipeline for GIF posters (#437)",
          "timestamp": "2026-09-08T06:41:14Z",
          "url": "https://github.com/lostb1t/remux/commit/9f05c16299f195e4f59621de32026ed881a9261d"
        },
        "date": 1788851036220,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 311812387,
            "range": "± 34801775",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 337021124,
            "range": "± 29597239",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 391335878,
            "range": "± 30991427",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 109623179,
            "range": "± 14105496",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 160626891,
            "range": "± 18818445",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 223387831,
            "range": "± 25250324",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 203876981,
            "range": "± 25099256",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 257363677,
            "range": "± 32998626",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 195126724,
            "range": "± 18687533",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 281588140,
            "range": "± 29326495",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 125698009,
            "range": "± 17579658",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 323369618,
            "range": "± 26907902",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 284402505,
            "range": "± 64488857",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 303470478,
            "range": "± 71082003",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 289103033,
            "range": "± 51778631",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 301387633,
            "range": "± 48451698",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 289676322,
            "range": "± 56783289",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 298092854,
            "range": "± 75167973",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 106640397,
            "range": "± 31573608",
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
          "id": "7f6cefdd693db68180ae68430444739fb84a6723",
          "message": "fix(collections): show poster configurator again after deleting a GIF (#452)",
          "timestamp": "2026-09-08T07:21:02Z",
          "url": "https://github.com/lostb1t/remux/commit/7f6cefdd693db68180ae68430444739fb84a6723"
        },
        "date": 1788853334995,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 258517432,
            "range": "± 25433987",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 248563412,
            "range": "± 17308793",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 302452130,
            "range": "± 25096882",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 47669095,
            "range": "± 3569065",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 55624618,
            "range": "± 5556256",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 129010479,
            "range": "± 13789223",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 156475419,
            "range": "± 24191344",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 172316464,
            "range": "± 15590475",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 83153079,
            "range": "± 5290957",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 146860253,
            "range": "± 10005198",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 88533156,
            "range": "± 8668632",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 274930474,
            "range": "± 20208814",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 295732347,
            "range": "± 26143158",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 296772749,
            "range": "± 26459572",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 309283145,
            "range": "± 31637006",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 301449931,
            "range": "± 27429675",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 299952009,
            "range": "± 31839147",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 307281813,
            "range": "± 28573522",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 69405435,
            "range": "± 7456867",
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
          "id": "9876612c20f211bf5238e9c5671091c66d1bebdd",
          "message": "fix(items): tailor metadata editor content-type and external-id fields to item kind (#454)",
          "timestamp": "2026-09-08T11:45:14Z",
          "url": "https://github.com/lostb1t/remux/commit/9876612c20f211bf5238e9c5671091c66d1bebdd"
        },
        "date": 1788869254435,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 233252565,
            "range": "± 20507961",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 259366619,
            "range": "± 22523551",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 276111206,
            "range": "± 20328890",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 49276532,
            "range": "± 5845062",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 58198270,
            "range": "± 8921883",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 130404041,
            "range": "± 11273351",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 129570719,
            "range": "± 10732398",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 159411836,
            "range": "± 13470554",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 92571507,
            "range": "± 7863770",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 147974287,
            "range": "± 9782343",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 77131125,
            "range": "± 7838308",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 268121930,
            "range": "± 22589964",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 263296499,
            "range": "± 16652209",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 257728185,
            "range": "± 19583294",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 258399448,
            "range": "± 30843654",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 261107891,
            "range": "± 22198467",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 285091573,
            "range": "± 30563672",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 258004347,
            "range": "± 21192745",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 63145583,
            "range": "± 4108930",
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
          "id": "2b9a424d8be1a63e8fe1de86dbbc0915505ce091",
          "message": "fix(test): stop sharing external ids in the duplicate-item stream test",
          "timestamp": "2026-09-08T16:20:13Z",
          "url": "https://github.com/lostb1t/remux/commit/2b9a424d8be1a63e8fe1de86dbbc0915505ce091"
        },
        "date": 1788885611775,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 226868958,
            "range": "± 20639497",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 248672326,
            "range": "± 17675693",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 287505496,
            "range": "± 30402305",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 52005717,
            "range": "± 6072881",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 62115722,
            "range": "± 8899005",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 134752703,
            "range": "± 14728668",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 141153051,
            "range": "± 14891298",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 169376854,
            "range": "± 13722609",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 88073984,
            "range": "± 5306645",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 158472328,
            "range": "± 11886646",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 88359034,
            "range": "± 11901347",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 264498929,
            "range": "± 19783581",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 286477115,
            "range": "± 42494673",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 278047754,
            "range": "± 33645953",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 287483716,
            "range": "± 34333454",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 286585210,
            "range": "± 36978186",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 277685262,
            "range": "± 35339173",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 257172447,
            "range": "± 21120392",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 63020488,
            "range": "± 4496188",
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
          "id": "64850e0420c9b5fffc23110990affb5cd5a729b5",
          "message": "fix(media): scope external-id unique indexes to movie/series/tv_program",
          "timestamp": "2026-09-08T17:49:15Z",
          "url": "https://github.com/lostb1t/remux/commit/64850e0420c9b5fffc23110990affb5cd5a729b5"
        },
        "date": 1788890923696,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 227026314,
            "range": "± 21042507",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 242532075,
            "range": "± 18795370",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 280142231,
            "range": "± 34046977",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 52125634,
            "range": "± 19445962",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 61318318,
            "range": "± 6753510",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 121329625,
            "range": "± 17237558",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 152770864,
            "range": "± 29638019",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 168663595,
            "range": "± 23475440",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 93431206,
            "range": "± 11716025",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 183757852,
            "range": "± 37461178",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 75823286,
            "range": "± 6174089",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 260839713,
            "range": "± 23577089",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 281245283,
            "range": "± 31447465",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 283681994,
            "range": "± 49343689",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 271486982,
            "range": "± 34672236",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 265434066,
            "range": "± 25745391",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 273192275,
            "range": "± 31535668",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 265149559,
            "range": "± 38809606",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 76079174,
            "range": "± 17005851",
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
          "id": "75561a497863fd7a317dd8fc51ed1536769b16ca",
          "message": "perf: speed up metadata refresh (#459)",
          "timestamp": "2026-09-09T10:04:51Z",
          "url": "https://github.com/lostb1t/remux/commit/75561a497863fd7a317dd8fc51ed1536769b16ca"
        },
        "date": 1788949512330,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 257047194,
            "range": "± 22477896",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 280751378,
            "range": "± 43127038",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 293356509,
            "range": "± 42146116",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 73927184,
            "range": "± 37935146",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 125608837,
            "range": "± 32601360",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 138213594,
            "range": "± 25093582",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 151367238,
            "range": "± 44627073",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 210422198,
            "range": "± 48592441",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 155807727,
            "range": "± 25538522",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 225933655,
            "range": "± 30751410",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 100830797,
            "range": "± 47774684",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 295564952,
            "range": "± 38437193",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 293710784,
            "range": "± 39100862",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 272248361,
            "range": "± 26096712",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 303767464,
            "range": "± 39400941",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 288513345,
            "range": "± 23968366",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 301170921,
            "range": "± 36545656",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 279238335,
            "range": "± 28081052",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 68559570,
            "range": "± 21062741",
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
          "id": "1ec8c0efb0f117e04f4b496e6799fba18ed79a21",
          "message": "feat: unify next up with continue watching (#458)",
          "timestamp": "2026-09-10T04:56:23Z",
          "url": "https://github.com/lostb1t/remux/commit/1ec8c0efb0f117e04f4b496e6799fba18ed79a21"
        },
        "date": 1789025500499,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 265480772,
            "range": "± 39302115",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 284718397,
            "range": "± 29987420",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 335912355,
            "range": "± 37972144",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 84673748,
            "range": "± 27763258",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 91231427,
            "range": "± 31030608",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 150076954,
            "range": "± 28209723",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 171069277,
            "range": "± 18214512",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 197325759,
            "range": "± 29541313",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 175134330,
            "range": "± 26102289",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 257956558,
            "range": "± 34937253",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 99261607,
            "range": "± 37825287",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 306471207,
            "range": "± 34991156",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 277032103,
            "range": "± 22686369",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 288322638,
            "range": "± 43115887",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 279947791,
            "range": "± 22375990",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 292301301,
            "range": "± 45391478",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 281181566,
            "range": "± 31569510",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 329298706,
            "range": "± 52976256",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 79107495,
            "range": "± 8013802",
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
          "id": "1fdd4f09bd4cae18b5bd7fa735c6bedc12e6cc92",
          "message": "fix(media): dedupe sibling media_relations before remapping to avoid uniq_media_relation conflicts",
          "timestamp": "2026-09-11T05:44:41Z",
          "url": "https://github.com/lostb1t/remux/commit/1fdd4f09bd4cae18b5bd7fa735c6bedc12e6cc92"
        },
        "date": 1789109087586,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 989991161,
            "range": "± 75355905",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 1067004032,
            "range": "± 87465100",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 1261901019,
            "range": "± 189828938",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 268967668,
            "range": "± 92256927",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 290522618,
            "range": "± 75940206",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 542697081,
            "range": "± 94016277",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 566326621,
            "range": "± 83503998",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 704038683,
            "range": "± 135699370",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 586585290,
            "range": "± 68815833",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 907713407,
            "range": "± 67942368",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 363156808,
            "range": "± 188391055",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 1233177273,
            "range": "± 188406802",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 1170711620,
            "range": "± 120002460",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 1109622232,
            "range": "± 83477751",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 1107000708,
            "range": "± 110475505",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 1151407597,
            "range": "± 91383625",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 1117262627,
            "range": "± 95710065",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 1127471815,
            "range": "± 84788923",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 267121427,
            "range": "± 48660424",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "northernpowerhouse",
            "username": "northernpowerhouse",
            "email": "65088662+northernpowerhouse@users.noreply.github.com"
          },
          "committer": {
            "name": "GitHub",
            "username": "web-flow",
            "email": "noreply@github.com"
          },
          "id": "790ba7faf64a778d14374493de86d34bdd08d263",
          "message": "fix: stop opendal-local .strm scan from deadlocking on URL collisions (#467)",
          "timestamp": "2026-09-11T12:35:47Z",
          "url": "https://github.com/lostb1t/remux/commit/790ba7faf64a778d14374493de86d34bdd08d263"
        },
        "date": 1789199662339,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 958640035,
            "range": "± 93865891",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 1040314336,
            "range": "± 58021781",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 1191961596,
            "range": "± 98717587",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 261609470,
            "range": "± 100065358",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 287969608,
            "range": "± 68080152",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 524748799,
            "range": "± 102509166",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 557969612,
            "range": "± 82961915",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 712177990,
            "range": "± 162357679",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 563828232,
            "range": "± 44801186",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 860396599,
            "range": "± 95117976",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 316737966,
            "range": "± 108367448",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 1149191165,
            "range": "± 58619814",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 1069440730,
            "range": "± 40753684",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 1077617886,
            "range": "± 66287746",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 1078695476,
            "range": "± 57125561",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 1073949414,
            "range": "± 50249730",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 1088292222,
            "range": "± 103926122",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 1098537750,
            "range": "± 84636464",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 265439376,
            "range": "± 53308138",
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
          "id": "ad896eeee74ef0c0dc34bc11b730baea207588f7",
          "message": "fix(search): replace matched search results with the stored row wholesale (#479)",
          "timestamp": "2026-09-13T07:01:52Z",
          "url": "https://github.com/lostb1t/remux/commit/ad896eeee74ef0c0dc34bc11b730baea207588f7"
        },
        "date": 1789288821810,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 1051770139,
            "range": "± 105799126",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 1102707403,
            "range": "± 79531586",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 1294036142,
            "range": "± 111972476",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 277400574,
            "range": "± 159626094",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 306141518,
            "range": "± 106062000",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 541196250,
            "range": "± 93285972",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 565466775,
            "range": "± 93855172",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 696408819,
            "range": "± 119953342",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 580635729,
            "range": "± 89238888",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 866260881,
            "range": "± 60878354",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 345699571,
            "range": "± 146335186",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 1226174615,
            "range": "± 95801782",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 1106811314,
            "range": "± 69329404",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 1093001908,
            "range": "± 76885904",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 1084383159,
            "range": "± 53280281",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 1093994971,
            "range": "± 73010346",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 1130406474,
            "range": "± 98857981",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 1134313163,
            "range": "± 104551739",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 275521788,
            "range": "± 70653025",
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
          "id": "325e7e39cfdb097e08381e2051df2baa269f6439",
          "message": "refactor(remuxdb): use the /api/media/{external_id}/versions probe route\n\nRemuxDB replaced GET /api/media/info?imdb_id={} with GET\n/api/media/{external_id}/versions. external_id is an imdb id or, absent\nthat, a tmdb:{id}-prefixed id, matching ExternalIds::stremio_lookup_id's\nexisting priority (imdb > custom_stremio_id > tmdb) — the caller now\nbuilds it with that instead of extracting imdb only.\n\nCo-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01XLFFGASL5q4kVHpTYnr81e",
          "timestamp": "2026-09-14T03:33:41Z",
          "url": "https://github.com/lostb1t/remux/commit/325e7e39cfdb097e08381e2051df2baa269f6439"
        },
        "date": 1789375301272,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 1002715173,
            "range": "± 171955660",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 1070297728,
            "range": "± 150453589",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 1289304391,
            "range": "± 144411253",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 259268432,
            "range": "± 59075499",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 325829889,
            "range": "± 78886955",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 550061576,
            "range": "± 133826798",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 600581117,
            "range": "± 104585590",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 714856980,
            "range": "± 109849697",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 588544193,
            "range": "± 50942120",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 898342689,
            "range": "± 130861135",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 307029474,
            "range": "± 149480101",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 1181933082,
            "range": "± 141779783",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 1104538747,
            "range": "± 60029521",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 1111758270,
            "range": "± 103631621",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 1104898503,
            "range": "± 52413401",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 1105341470,
            "range": "± 44735795",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 1115470115,
            "range": "± 66648647",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 1118239514,
            "range": "± 88796140",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 298846967,
            "range": "± 57193484",
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
          "id": "31a31c2263e615902982415d446c196050a9b210",
          "message": "fix(playback): pick HEVC sample-entry tag from the client's DeviceProfile (#413)",
          "timestamp": "2026-09-14T11:09:23Z",
          "url": "https://github.com/lostb1t/remux/commit/31a31c2263e615902982415d446c196050a9b210"
        },
        "date": 1789387646294,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 1029148708,
            "range": "± 94129703",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 1089861522,
            "range": "± 112496754",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 1270104158,
            "range": "± 121980768",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 280793723,
            "range": "± 88367838",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 302182194,
            "range": "± 50586852",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 540601382,
            "range": "± 102316074",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 581068233,
            "range": "± 121328495",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 751402280,
            "range": "± 155411770",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 605939157,
            "range": "± 82072713",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 939538329,
            "range": "± 135740597",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 361917319,
            "range": "± 156651010",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 1306367353,
            "range": "± 180354757",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 1107395075,
            "range": "± 79747078",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 1096455123,
            "range": "± 84312679",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 1104828749,
            "range": "± 102336210",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 1085204721,
            "range": "± 57804630",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 1103597129,
            "range": "± 62690838",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 1095171297,
            "range": "± 72245711",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 269533304,
            "range": "± 77065477",
            "unit": "ns/iter"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "name": "semantic-release-bot",
            "username": "semantic-release-bot",
            "email": "semantic-release-bot@martynus.net"
          },
          "committer": {
            "name": "semantic-release-bot",
            "username": "semantic-release-bot",
            "email": "semantic-release-bot@martynus.net"
          },
          "id": "1c848b906a216cf1500c2627beb8e47db1b9b11e",
          "message": "chore(release): 0.32.0 [skip ci]\n\n# [0.32.0](https://github.com/lostb1t/remux/compare/v0.31.0...v0.32.0) (2026-09-16)\n\n### Bug Fixes\n\n* **addons:** refresh capabilities when config changes ([#480](https://github.com/lostb1t/remux/issues/480)) ([d296284](https://github.com/lostb1t/remux/commit/d2962844a4ef7249be470f6f9b12ab94ac52e83e))\n* **ffmpeg:** only apply HTTP reconnect flags to HTTP inputs ([#478](https://github.com/lostb1t/remux/issues/478)) ([69b10d8](https://github.com/lostb1t/remux/commit/69b10d84dfe47607307c13e942694e7972f97304))\n* **playback:** honor the probe fallback on the stream request that follows ([#464](https://github.com/lostb1t/remux/issues/464)) ([bf44efc](https://github.com/lostb1t/remux/commit/bf44efcacc0d6c76875dd22049e401e1cff14fd3))\n* **playback:** pick HEVC sample-entry tag from the client's DeviceProfile ([#413](https://github.com/lostb1t/remux/issues/413)) ([31a31c2](https://github.com/lostb1t/remux/commit/31a31c2263e615902982415d446c196050a9b210))\n* **playback:** serve mkv-source direct stream as-is to preserve HTTP Range support ([#440](https://github.com/lostb1t/remux/issues/440)) ([baa6502](https://github.com/lostb1t/remux/commit/baa6502c28eb6e41ab0d6d2320e7510e49b67f17))\n* **search:** replace matched search results with the stored row wholesale ([#479](https://github.com/lostb1t/remux/issues/479)) ([ad896ee](https://github.com/lostb1t/remux/commit/ad896eeee74ef0c0dc34bc11b730baea207588f7))\n* **sessions:** persist stop reports that arrive without a play session ([#463](https://github.com/lostb1t/remux/issues/463)) ([69790b7](https://github.com/lostb1t/remux/commit/69790b7b307d172e5df2314d9195b6bf16bc84d7))\n* stop opendal-local .strm scan from deadlocking on URL collisions ([#467](https://github.com/lostb1t/remux/issues/467)) ([790ba7f](https://github.com/lostb1t/remux/commit/790ba7faf64a778d14374493de86d34bdd08d263))\n* **torrent:** release torrents after their last playback user ([#476](https://github.com/lostb1t/remux/issues/476)) ([c177d11](https://github.com/lostb1t/remux/commit/c177d11f732fa4b9b269f94c759ae146406a9688))\n* **users:** collapse a saved OrderedViews that matches the live default ([#489](https://github.com/lostb1t/remux/issues/489)) ([a4e4933](https://github.com/lostb1t/remux/commit/a4e4933ed451937a6084fdad94a71dd5f7d8b0fe))\n* **webhooks:** include SeriesProviderIds in Episode/Season webhook payloads ([#487](https://github.com/lostb1t/remux/issues/487)) ([53842ca](https://github.com/lostb1t/remux/commit/53842ca9b00274216dda70a1413cb0e8df6e9e40))\n\n### Features\n\n* **streams:** make the stream groups page use the drag-and-drop list ([#496](https://github.com/lostb1t/remux/issues/496)) ([1b8a4b6](https://github.com/lostb1t/remux/commit/1b8a4b6341197b6740fc80b18c217356903a47c8))",
          "timestamp": "2026-09-16T06:15:44Z",
          "url": "https://github.com/lostb1t/remux/commit/1c848b906a216cf1500c2627beb8e47db1b9b11e"
        },
        "date": 1789547656034,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 1180672160,
            "range": "± 121478779",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 1240080549,
            "range": "± 102505717",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 1476114956,
            "range": "± 175404129",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 305622746,
            "range": "± 100335536",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 359241990,
            "range": "± 147198376",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 699555644,
            "range": "± 133494791",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 691587107,
            "range": "± 155645536",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 905605011,
            "range": "± 147514004",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 771051017,
            "range": "± 138656822",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 1023849958,
            "range": "± 188158081",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 397409349,
            "range": "± 131146874",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 1382889857,
            "range": "± 174422187",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 1130686590,
            "range": "± 103889475",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 1126080736,
            "range": "± 99770384",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 1121119000,
            "range": "± 147161692",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 1117778210,
            "range": "± 100970429",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 1104473122,
            "range": "± 79175151",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 1111086573,
            "range": "± 126763507",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 273584107,
            "range": "± 79760221",
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
          "id": "48e8a88bc2f3266b37e56c6c743ee2ab1f38652d",
          "message": "perf: share 429 cooldowns and cap addon fetch timeouts (#501)",
          "timestamp": "2026-09-17T06:12:49Z",
          "url": "https://github.com/lostb1t/remux/commit/48e8a88bc2f3266b37e56c6c743ee2ab1f38652d"
        },
        "date": 1789633899671,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 1076787653,
            "range": "± 130568873",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 1094910002,
            "range": "± 120899715",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 1365440428,
            "range": "± 152555208",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 272493877,
            "range": "± 69724278",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 303532605,
            "range": "± 54620467",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 572189856,
            "range": "± 101327155",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 617512955,
            "range": "± 107206451",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 809888192,
            "range": "± 191369542",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 616750999,
            "range": "± 106279489",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 889293126,
            "range": "± 112414005",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 341919191,
            "range": "± 88342762",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 1255653515,
            "range": "± 126320295",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 1188164196,
            "range": "± 110677639",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 1109488111,
            "range": "± 99048231",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 1119452087,
            "range": "± 107100480",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 1186262453,
            "range": "± 118071561",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 1199135805,
            "range": "± 121260598",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 1149017997,
            "range": "± 113119706",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 298547331,
            "range": "± 65502505",
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
          "id": "7fa739c7e95138cc0162cc7fa6e45908de8d6290",
          "message": "fix(ratings): stop dropping critic ratings that RemuxDB only lists as a source (#507)",
          "timestamp": "2026-09-17T17:10:17Z",
          "url": "https://github.com/lostb1t/remux/commit/7fa739c7e95138cc0162cc7fa6e45908de8d6290"
        },
        "date": 1789716843725,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 245914021,
            "range": "± 26357761",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 259830422,
            "range": "± 30966615",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 297293996,
            "range": "± 41312664",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 78543439,
            "range": "± 24510911",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 124114303,
            "range": "± 21731011",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 137685590,
            "range": "± 35188306",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 142381361,
            "range": "± 33742545",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 177058176,
            "range": "± 39956721",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 158258222,
            "range": "± 19960476",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 226244310,
            "range": "± 22854750",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 94594670,
            "range": "± 43742342",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 278453325,
            "range": "± 20062725",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 262049756,
            "range": "± 26456437",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 245593775,
            "range": "± 26310606",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 254780637,
            "range": "± 24225965",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 259200113,
            "range": "± 31836624",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 253965601,
            "range": "± 26268181",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 261880670,
            "range": "± 30808745",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 66939303,
            "range": "± 20273587",
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
          "id": "660cc2f98bc7a082076cbb2afb3e224c916607b3",
          "message": "fix: merge duplicate root media rows that collide on disjoint external ids\n\nCo-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01XLFFGASL5q4kVHpTYnr81e",
          "timestamp": "2026-09-18T14:51:58Z",
          "url": "https://github.com/lostb1t/remux/commit/660cc2f98bc7a082076cbb2afb3e224c916607b3"
        },
        "date": 1789746250632,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 241214353,
            "range": "± 19606511",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 262585724,
            "range": "± 19575671",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 307838809,
            "range": "± 28214217",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 77132609,
            "range": "± 9833320",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 82692945,
            "range": "± 8833917",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 142079848,
            "range": "± 27515424",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 149575237,
            "range": "± 12481268",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 171534229,
            "range": "± 25953794",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 192199200,
            "range": "± 16769816",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 245879817,
            "range": "± 21689794",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 86779489,
            "range": "± 15938624",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 292574803,
            "range": "± 21980456",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 258812656,
            "range": "± 20442570",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 257716617,
            "range": "± 18544427",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 256822776,
            "range": "± 25237765",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 257444719,
            "range": "± 17458330",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 280061571,
            "range": "± 27724555",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 254077448,
            "range": "± 18740594",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 70897658,
            "range": "± 9765951",
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
          "id": "6a851975cc16901dc6350c82d028c7e5087ba6d6",
          "message": "fix: make RefreshLibrary progress reflect real work and stop endless refresh of items with no digital date (#513)",
          "timestamp": "2026-09-18T18:59:54Z",
          "url": "https://github.com/lostb1t/remux/commit/6a851975cc16901dc6350c82d028c7e5087ba6d6"
        },
        "date": 1789759213958,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 230829153,
            "range": "± 17502620",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 240105931,
            "range": "± 22999109",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 280791659,
            "range": "± 32506240",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 73389957,
            "range": "± 18855848",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 135704299,
            "range": "± 9767032",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 135038934,
            "range": "± 15940261",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 150215571,
            "range": "± 28610980",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 188579808,
            "range": "± 33311447",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 175631674,
            "range": "± 16637832",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 240206506,
            "range": "± 18532847",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 87588017,
            "range": "± 11839071",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 266266071,
            "range": "± 18941085",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 266499670,
            "range": "± 21020611",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 255368388,
            "range": "± 21171684",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 257495312,
            "range": "± 24971914",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 256212662,
            "range": "± 22673184",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 254898270,
            "range": "± 17321562",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 260735702,
            "range": "± 20864254",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 68857867,
            "range": "± 8861744",
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
          "id": "d8dfe06e0d5607e479bb8e844fa1bb6f9ed5a11a",
          "message": "perf: stop RefreshPopularity from fully hydrating every media row\n\nCo-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01XLFFGASL5q4kVHpTYnr81e",
          "timestamp": "2026-09-19T06:58:28Z",
          "url": "https://github.com/lostb1t/remux/commit/d8dfe06e0d5607e479bb8e844fa1bb6f9ed5a11a"
        },
        "date": 1789802481465,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 256551159,
            "range": "± 18660178",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 282403042,
            "range": "± 31957241",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 325247256,
            "range": "± 32517441",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 77965212,
            "range": "± 10372508",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 91085741,
            "range": "± 19653593",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 155341014,
            "range": "± 23712793",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 159568568,
            "range": "± 14225681",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 195658964,
            "range": "± 26422537",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 195941716,
            "range": "± 35062886",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 276968547,
            "range": "± 21375051",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 116376447,
            "range": "± 17880266",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 346695983,
            "range": "± 26309875",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 375272670,
            "range": "± 147465326",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 352849854,
            "range": "± 220882932",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 307386700,
            "range": "± 27720653",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 324525372,
            "range": "± 36517474",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 312091010,
            "range": "± 35452823",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 335264365,
            "range": "± 64782905",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 89561051,
            "range": "± 16548849",
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
          "id": "d8dfe06e0d5607e479bb8e844fa1bb6f9ed5a11a",
          "message": "perf: stop RefreshPopularity from fully hydrating every media row\n\nCo-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01XLFFGASL5q4kVHpTYnr81e",
          "timestamp": "2026-09-19T06:58:28Z",
          "url": "https://github.com/lostb1t/remux/commit/d8dfe06e0d5607e479bb8e844fa1bb6f9ed5a11a"
        },
        "date": 1789803029008,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 282006471,
            "range": "± 18876694",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 317116970,
            "range": "± 39714665",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 340508818,
            "range": "± 48308566",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 79993115,
            "range": "± 11374450",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 101190840,
            "range": "± 18214397",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 151913891,
            "range": "± 20457212",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 162130184,
            "range": "± 21110616",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 209061040,
            "range": "± 32295199",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 177081599,
            "range": "± 22370668",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 246560790,
            "range": "± 35416050",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 128514861,
            "range": "± 20501575",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 335123564,
            "range": "± 27790765",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 292380547,
            "range": "± 53341614",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 284568151,
            "range": "± 48070018",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 293707942,
            "range": "± 53916688",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 292164354,
            "range": "± 29051287",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 286090420,
            "range": "± 28541963",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 278762028,
            "range": "± 20085830",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 86211889,
            "range": "± 11323848",
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
          "id": "22b42a477303331f530be758303ad7f6ee88b0a4",
          "message": "fix: stop collection Backdrop deriving from Primary, alias Thumb to it instead, and disable image resizing (fixes #435)\n\nCo-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01XLFFGASL5q4kVHpTYnr81e",
          "timestamp": "2026-09-19T08:24:33Z",
          "url": "https://github.com/lostb1t/remux/commit/22b42a477303331f530be758303ad7f6ee88b0a4"
        },
        "date": 1789807917365,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 287301002,
            "range": "± 27429384",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 285506395,
            "range": "± 23573941",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 353819064,
            "range": "± 34809545",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 87064477,
            "range": "± 12774713",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 162398582,
            "range": "± 28677528",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 157991658,
            "range": "± 24420347",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 164601619,
            "range": "± 21284608",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 201535366,
            "range": "± 22857541",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 190504720,
            "range": "± 24214344",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 244863265,
            "range": "± 30424963",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 104851475,
            "range": "± 12339231",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 326026395,
            "range": "± 36666407",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 277443565,
            "range": "± 22933524",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 303902797,
            "range": "± 31369311",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 302657732,
            "range": "± 26708599",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 346721988,
            "range": "± 80431540",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 306260575,
            "range": "± 26938886",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 311317769,
            "range": "± 34539959",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 78669467,
            "range": "± 10563616",
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
          "id": "d4bd171584d72da8fa4f04e3deae61ad271f3e11",
          "message": "fix: exclude episodes from search (fixes #517)",
          "timestamp": "2026-09-20T06:41:28Z",
          "url": "https://github.com/lostb1t/remux/commit/d4bd171584d72da8fa4f04e3deae61ad271f3e11"
        },
        "date": 1789891133964,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 269865666,
            "range": "± 20580243",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 289448529,
            "range": "± 35640151",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 336254239,
            "range": "± 27765308",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 89370614,
            "range": "± 23175521",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 164279400,
            "range": "± 17701849",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 147551884,
            "range": "± 13319588",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 155979671,
            "range": "± 26182964",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 210573610,
            "range": "± 33246916",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 173811684,
            "range": "± 21889379",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 253275615,
            "range": "± 21030820",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 107648315,
            "range": "± 39049790",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 318477547,
            "range": "± 34264743",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 303976090,
            "range": "± 32401342",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 286708575,
            "range": "± 20191230",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 300032012,
            "range": "± 39033454",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 292770605,
            "range": "± 27515643",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 307143201,
            "range": "± 28255291",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 290037030,
            "range": "± 26311492",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 88332856,
            "range": "± 10727465",
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
          "id": "5c907e8c3edacae188fa114563d75f060f000321",
          "message": "fix: create collections via Jellyfin API (#519)",
          "timestamp": "2026-09-20T10:33:56Z",
          "url": "https://github.com/lostb1t/remux/commit/5c907e8c3edacae188fa114563d75f060f000321"
        },
        "date": 1789904120960,
        "tool": "cargo",
        "benches": [
          {
            "name": "items_latest/limit=20&recursive=false",
            "value": 280184993,
            "range": "± 23251808",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&recursive=false",
            "value": 294141997,
            "range": "± 61009741",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=500&recursive=false",
            "value": 328413940,
            "range": "± 36547941",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Movie&recursive=false",
            "value": 82694197,
            "range": "± 10374875",
            "unit": "ns/iter"
          },
          {
            "name": "items_latest/limit=100&include_item_types=Series&recursive=false",
            "value": 97339203,
            "range": "± 19525768",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=20&recursive=false",
            "value": 154577632,
            "range": "± 24702286",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&recursive=false",
            "value": 160157456,
            "range": "± 28667230",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=500&recursive=false",
            "value": 207354173,
            "range": "± 30720520",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Movie&recursive=false",
            "value": 189756158,
            "range": "± 21344367",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&include_item_types=Series&recursive=false",
            "value": 257510389,
            "range": "± 22296490",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&filters=IsPlayed&recursive=false",
            "value": 113750577,
            "range": "± 19944400",
            "unit": "ns/iter"
          },
          {
            "name": "items_get/limit=100&sort_by=DateCreated&recursive=false",
            "value": 322007590,
            "range": "± 32258106",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=50&recursive=false",
            "value": 299356190,
            "range": "± 34733509",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=200&recursive=false",
            "value": 270034473,
            "range": "± 21616109",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_scale/limit=500&recursive=false",
            "value": 295412591,
            "range": "± 38908556",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=true&recursive=false",
            "value": 283239264,
            "range": "± 27613107",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_resumable/limit=500&enable_resumable=false&recursive=false",
            "value": 294736249,
            "range": "± 33840182",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/epoch",
            "value": 280695266,
            "range": "± 33263155",
            "unit": "ns/iter"
          },
          {
            "name": "nextup_date_cutoff/30days",
            "value": 77677591,
            "range": "± 13598592",
            "unit": "ns/iter"
          }
        ]
      }
    ]
  }
}