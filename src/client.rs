use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};

#[async_trait]
pub trait RestClient {
    fn base_url(&self) -> &str;
    fn http_client(&self) -> &reqwest::Client;

    async fn get<T>(&self, path: &str, query: &impl Serialize) -> Result<T, reqwest::Error>
    where
        T: DeserializeOwned + Send,
    {
        let url = format!("{}/{}", self.base_url().trim_end_matches('/'), path);
        let response = self
            .http_client()
            .get(&url)
            .query(query)
            .send()
            .await?
            .error_for_status()?
            .json::<T>()
            .await?;

        Ok(response)
    }

    // Optional: default impls for POST/PUT/DELETE could go here
}