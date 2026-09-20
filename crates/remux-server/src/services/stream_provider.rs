use serde::Serialize;

/// Canonical streaming-provider metadata used by the dashboard and artwork
/// renderer. JSON names remain compatible with the existing Remux endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct StreamProvider {
    #[serde(rename = "provider_id")]
    pub id: i64,
    #[serde(rename = "provider_name")]
    pub name: String,
    #[serde(rename = "logo_path")]
    pub logo: Option<String>,
    pub color: Option<String>,
}

impl StreamProvider {
    pub fn new(id: i64, name: String, logo: Option<String>) -> Self {
        let color = Self::brand_color(&name).map(str::to_string);
        Self {
            id,
            name,
            logo,
            color,
        }
    }

    pub fn brand_color(provider_name: &str) -> Option<&'static str> {
        match provider_name
            .to_ascii_lowercase()
            .as_str()
        {
            "netflix" => Some("#B20710"),
            "amazon prime video" | "prime video" => Some("#00A8E1"),
            "disney plus" | "disney+" => Some("#113CCF"),
            "max" | "hbo max" => Some("#5825D8"),
            "hulu" => Some("#1CE783"),
            "paramount plus" | "paramount+" => Some("#0064FF"),
            "crunchyroll" => Some("#F47521"),
            "apple tv plus" | "apple tv+" | "apple tv" => Some("#000000"),
            "viaplay" => Some("#FF3D00"),
            "skyshowtime" => Some("#00AEEF"),
            "peacock" => Some("#F5C518"),
            "mubi" => Some("#FFFFFF"),
            "pluto tv" => Some("#FFDA00"),
            "youtube" | "youtube premium" => Some("#FF0000"),
            "amc+" | "amc plus" => Some("#00A99D"),
            "adn" | "animation digital network" => Some("#E91E63"),
            "funimation now" | "funimation" => Some("#5B2C83"),
            "illico+" | "illico plus" => Some("#E31837"),
            "sky mais" | "skymais" => Some("#00AEEF"),
            "c more" | "cmore" => Some("#F15A29"),
            "foxtel now" => Some("#00AEEF"),
            "jiohotstar" => Some("#1F4E9E"),
            "wetv" => Some("#00C4CC"),
            "chorki" => Some("#E91E63"),
            "tvp vod" => Some("#0057A8"),
            "kwelitv" => Some("#F4A300"),
            "vidio" => Some("#E53935"),
            "vivamax" => Some("#E31B23"),
            "ruutu" => Some("#FF6B00"),
            "samsung tv plus" => Some("#1428A0"),
            "craftsy" => Some("#7B3FF2"),
            "mx player" => Some("#F7B500"),
            "axn now" | "axnnow" => Some("#FF5A00"),
            "claro video" => Some("#E30613"),
            "dimsum" => Some("#FF6B00"),
            "catchplay+" | "catchplay" => Some("#F6C400"),
            "fanatiz" => Some("#FF4B55"),
            "movistar plus+" | "movistar+" | "movistar plus" => Some("#019DF4"),
            "discovery+" | "discovery plus" => Some("#00AEEF"),
            "kocowa" => Some("#F15A24"),
            "toku" => Some("#E52629"),
            "watcha" => Some("#FF5B7F"),
            "wakanim" => Some("#F58220"),
            "dplay" => Some("#FF6A00"),
            "filmdoo" => Some("#00AEEF"),
            "globoplay" => Some("#00A859"),
            "mtv katsomo" => Some("#FFCB05"),
            "u-next" | "unext" => Some("#00A6D6"),
            "rcti+" | "rcti plus" => Some("#00AEEF"),
            "nfl+" | "nfl plus" => Some("#013369"),
            "nasa+" | "nasa plus" => Some("#0B3D91"),
            "iwanttfc" | "iwant" => Some("#F05A28"),
            "kapamilya online live" => Some("#0B4F9C"),
            "hayu" => Some("#F15A24"),
            "watch it" => Some("#7A2C91"),
            "stan" => Some("#00D4FF"),
            "shudder" => Some("#E53935"),
            "roxi" => Some("#A100FF"),
            _ => None,
        }
    }
}
