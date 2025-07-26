use serde::{Serialize, Deserialize};
use crate::sdks::core::{CommaSeparatedList, Endpoint, QueryParams};


#[derive(Debug, Clone)]
pub struct GetDisplayPreferences {
    pub user_id: String,
    pub display_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DisplayPreferencesDto {
    pub view_type: Option<String>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    pub show_backdrop: Option<bool>,
    pub remember_indexing: Option<bool>,
    pub primary_image_height: Option<i32>,
    pub custom_prefs: Option<std::collections::HashMap<String, String>>,
}

impl Endpoint for GetDisplayPreferences {
    type Output = DisplayPreferencesDto;

    fn endpoint(&self) -> String {
        format!("/Users/{}/DisplayPreferences/{}", self.user_id, self.display_id)
    }
}


#[derive(Debug, Clone)]
pub struct SetDisplayPreferences {
    pub user_id: String,
    pub display_id: String,
    pub preferences: DisplayPreferencesDto,
}

impl Endpoint for SetDisplayPreferences {
    type Output = ();

    fn method(&self) -> http::Method {
        http::Method::POST
    }

    fn endpoint(&self) -> String {
        format!("/Users/{}/DisplayPreferences/{}", self.user_id, self.display_id)
    }

    fn body(&self) -> Option<String> {
        Some(serde_json::to_string(&self.preferences).unwrap())
    }
}