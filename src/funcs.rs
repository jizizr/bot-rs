use ferrisgram::error::GroupIteration;
use ferrisgram::ext::Context;
use ferrisgram::Bot;
use serde::de::DeserializeOwned;
pub mod command;
pub mod text;
async fn get<T: DeserializeOwned>(
    url: &str,
) -> Result<T, Box<dyn Send + Sync + std::error::Error>> {
    let resp = reqwest::get(url).await?;
    let model: T = resp.json().await?;
    Ok(model)
}
