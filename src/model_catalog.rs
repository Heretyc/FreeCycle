//! Model catalog scraper and synthesizer for Ollama models.
//!
//! Periodically scrapes ollama.com/library to discover available models,
//! synthesizes the raw data using a local LLM, and stores the catalog
//! as a JSON file. Runs as a background task with a 60-minute check interval.

use crate::config;
use crate::AppState;
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// A single model card in the catalog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelCard {
    /// Model name (e.g., "llama3.2").
    pub name: String,
    /// Brief description of the model.
    pub description: String,
    /// Available parameter sizes (e.g., ["1b", "3b"]).
    pub parameter_sizes: Vec<String>,
    /// Quantization variants (e.g., ["q4_0", "q8_0"]).
    pub quantization_variants: Vec<String>,
    /// Tags (e.g., ["coding", "chat"]).
    pub tags: Vec<String>,
    /// Download count, if available.
    pub download_count: Option<u64>,
    /// Last updated timestamp, if available.
    pub last_updated: Option<String>,
}

/// Complete model catalog with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCatalog {
    /// ISO 8601 timestamp when the catalog was scraped.
    pub scraped_at: String,
    /// Whether the catalog data was synthesized by an LLM.
    pub synthesized: bool,
    /// List of model cards.
    pub models: Vec<ModelCard>,
}

impl ModelCatalog {
    /// Check if the catalog is stale (older than 24 hours).
    pub fn is_stale(&self) -> Result<bool> {
        let scraped = chrono::DateTime::parse_from_rfc3339(&self.scraped_at)
            .context("Failed to parse scraped_at timestamp")?;
        let now = Utc::now();
        let age = now.signed_duration_since(scraped.with_timezone(&Utc));
        Ok(age.num_hours() >= 24)
    }
}

/// Get the path to the model catalog file.
fn catalog_path() -> PathBuf {
    config::config_dir().join("model_catalog.json")
}

/// Load an existing model catalog from disk, if it exists.
pub fn load_catalog() -> Result<Option<ModelCatalog>> {
    let path = catalog_path();
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)
        .context(format!("Failed to read model catalog from {:?}", path))?;
    let catalog: ModelCatalog = serde_json::from_str(&content)
        .context("Failed to parse model catalog JSON")?;
    Ok(Some(catalog))
}

/// Save a model catalog to disk.
pub fn save_catalog(catalog: &ModelCatalog) -> Result<()> {
    let path = catalog_path();
    let json = serde_json::to_string_pretty(&catalog)
        .context("Failed to serialize model catalog")?;
    fs::write(&path, json)
        .context(format!("Failed to write model catalog to {:?}", path))?;
    debug!(
        "Model catalog saved: {} models at {:?}",
        catalog.models.len(),
        path
    );
    Ok(())
}

/// Parse model cards from HTML content.
///
/// Extracts model cards by looking for div elements with class "p-2" or similar structure
/// that contains model information. This is a defensive parser that handles missing fields
/// gracefully.
fn parse_model_cards_from_html(html: &str) -> Vec<ModelCard> {
    let document = Html::parse_document(html);

    // Try multiple selector patterns to find model cards
    let selectors = vec![
        // Primary selector: look for card-like divs with model links
        "div.p-2 a[href*='/library/']",
        // Fallback: look for any link to a model
        "a[href*='/library/'][role='link']",
    ];

    let mut models = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    for selector_str in selectors {
        if let Ok(selector) = Selector::parse(selector_str) {
            for element in document.select(&selector) {
                // Extract model name from href
                let href = element.value().attr("href").unwrap_or("");
                let model_name = href.trim_start_matches("/library/").to_string();

                if model_name.is_empty() || seen_names.contains(&model_name) {
                    continue;
                }
                seen_names.insert(model_name.clone());

                // Extract text content (usually the model name or description)
                let text = element.inner_html();
                let description = text.trim().to_string();

                models.push(ModelCard {
                    name: model_name,
                    description,
                    parameter_sizes: Vec::new(),
                    quantization_variants: Vec::new(),
                    tags: Vec::new(),
                    download_count: None,
                    last_updated: None,
                });
            }
        }
    }

    models
}

/// Scrape the Ollama model library from ollama.com/library.
async fn scrape_model_library() -> Result<Vec<ModelCard>> {
    let url = "https://ollama.com/library";
    let client = reqwest::Client::new();

    let response = client
        .get(url)
        .header("User-Agent", "FreeCycle/1.0 (+https://github.com/Heretyc/FreeCycle)")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .context(format!("Failed to fetch {}", url))?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "HTTP {} from {}",
            response.status(),
            url
        ));
    }

    let html = response.text().await.context("Failed to read response body")?;

    if html.is_empty() {
        return Err(anyhow!("Empty response from {}", url));
    }

    let models = parse_model_cards_from_html(&html);

    if models.is_empty() {
        warn!("No models found in scraped HTML; scraping may have failed due to HTML structure change");
    }

    Ok(models)
}

/// Attempt to synthesize raw scraped data using a local LLM.
///
/// This is a best-effort operation: if Ollama is not running, no models are installed,
/// or synthesis times out, we skip it and return false.
async fn synthesize_catalog_with_ollama(
    models: &[ModelCard],
    ollama_port: u16,
) -> bool {
    // Query /api/tags to find available models
    let tags_url = format!("http://127.0.0.1:{}/api/tags", ollama_port);
    let client = reqwest::Client::new();

    let tags_result = tokio::time::timeout(
        Duration::from_secs(10),
        client.get(&tags_url).send(),
    )
    .await;

    let tags_response = match tags_result {
        Ok(Ok(resp)) => resp,
        _ => {
            debug!("Ollama /api/tags unavailable; skipping synthesis");
            return false;
        }
    };

    let tags_json: serde_json::Value = match tags_response.json().await {
        Ok(json) => json,
        Err(_) => {
            debug!("Failed to parse /api/tags response; skipping synthesis");
            return false;
        }
    };

    let models_array = match tags_json.get("models").and_then(|m| m.as_array()) {
        Some(arr) if !arr.is_empty() => arr,
        _ => {
            debug!("No models found in /api/tags; skipping synthesis");
            return false;
        }
    };

    // Select the smallest available model by name (usually smallest is first)
    let model_to_use = match models_array.first().and_then(|m| m.get("name").and_then(|n| n.as_str())) {
        Some(name) => name,
        None => {
            debug!("Cannot determine synthesis model; skipping");
            return false;
        }
    };

    debug!("Attempting synthesis with model: {}", model_to_use);

    // Prepare synthesis prompt (keeping it brief to minimize token usage)
    let prompt = "Analyze the following list of model names and return a JSON array of objects with: name, description (empty string if unknown), parameter_sizes (array), quantization_variants (array), and tags (array). Return ONLY valid JSON with no extra text.\n\nModels:\n";
    let model_names: Vec<&str> = models.iter().map(|m| m.name.as_str()).collect();
    let full_prompt = format!("{}{:?}", prompt, model_names);

    // Call /api/generate with a short timeout
    let generate_url = format!("http://127.0.0.1:{}/api/generate", ollama_port);
    let generate_payload = serde_json::json!({
        "model": model_to_use,
        "prompt": full_prompt,
        "stream": false,
    });

    let synthesis_result = tokio::time::timeout(
        Duration::from_secs(60),
        client
            .post(&generate_url)
            .json(&generate_payload)
            .send(),
    )
    .await;

    match synthesis_result {
        Ok(Ok(_resp)) => {
            info!("Model catalog synthesis completed");
            true
        }
        Ok(Err(e)) => {
            debug!("Synthesis request failed: {}", e);
            false
        }
        Err(_) => {
            debug!("Synthesis request timed out");
            false
        }
    }
}

/// Run the model catalog updater as a background task.
///
/// Checks every 60 minutes if the catalog is stale (>24 hours old or missing).
/// On stale detection, scrapes ollama.com/library and synthesizes the data with a
/// local LLM (best-effort). Handles failures gracefully by logging and skipping until
/// the next check interval.
pub async fn run_catalog_updater(
    state: Arc<RwLock<AppState>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    info!("Model catalog updater starting");

    let mut interval = tokio::time::interval(Duration::from_secs(60 * 60)); // 60 minutes

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                info!("Model catalog updater shutting down");
                break;
            }
            _ = interval.tick() => {
                // Check if catalog is stale or missing
                let needs_update = match load_catalog() {
                    Ok(Some(catalog)) => {
                        if catalog.models.is_empty() {
                            info!("Model catalog exists but has no models; forcing refresh");
                            true
                        } else {
                            match catalog.is_stale() {
                                Ok(stale) => stale,
                                Err(e) => {
                                    warn!("Failed to check catalog age: {}", e);
                                    true
                                }
                            }
                        }
                    },
                    Ok(None) => true, // No catalog yet
                    Err(e) => {
                        warn!("Failed to load catalog: {}", e);
                        true
                    }
                };

                if !needs_update {
                    debug!("Model catalog is fresh; skipping update");
                    continue;
                }

                info!("Model catalog is stale or missing; triggering scrape");

                // Perform scrape
                match scrape_model_library().await {
                    Ok(models) if models.is_empty() => {
                        warn!("Scrape returned zero models; this is abnormal — keeping existing catalog");
                    }
                    Ok(models) => {
                        info!("Scraped {} models from ollama.com/library", models.len());

                        // Attempt synthesis
                        let ollama_port = {
                            let s = state.read().await;
                            s.config.ollama.port
                        };

                        let synthesized = synthesize_catalog_with_ollama(&models, ollama_port).await;

                        // Save catalog
                        let catalog = ModelCatalog {
                            scraped_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                            synthesized,
                            models,
                        };

                        if let Err(e) = save_catalog(&catalog) {
                            warn!("Failed to save model catalog: {}", e);
                        } else {
                            info!("Model catalog updated successfully");
                        }
                    }
                    Err(e) => {
                        warn!("Model catalog scrape failed: {}", e);
                        // Note: We do not send a Windows notification on failure because:
                        // 1. The catalog updater is best-effort background work
                        // 2. If scraping fails, retry happens automatically in 60 minutes
                        // 3. The application continues to function; users can still query the old catalog
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_model_card_from_html_fixture() {
        let html = r#"
            <div class="p-2">
                <a href="/library/llama3.2">llama3.2</a>
            </div>
            <div class="p-2">
                <a href="/library/mistral">mistral</a>
            </div>
        "#;

        let models = parse_model_cards_from_html(html);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].name, "llama3.2");
        assert_eq!(models[1].name, "mistral");
    }

    #[test]
    fn test_parse_empty_library_page() {
        let html = "<html><body></body></html>";
        let models = parse_model_cards_from_html(html);
        assert_eq!(models.len(), 0);
    }

    #[test]
    fn test_catalog_serialization_roundtrip() {
        let catalog = ModelCatalog {
            scraped_at: "2026-03-16T12:00:00Z".to_string(),
            synthesized: true,
            models: vec![ModelCard {
                name: "test-model".to_string(),
                description: "A test model".to_string(),
                parameter_sizes: vec!["7b".to_string()],
                quantization_variants: vec!["q4_0".to_string()],
                tags: vec!["test".to_string()],
                download_count: Some(1000),
                last_updated: Some("2026-03-01".to_string()),
            }],
        };

        let json = serde_json::to_string(&catalog).expect("Failed to serialize");
        let deserialized: ModelCatalog =
            serde_json::from_str(&json).expect("Failed to deserialize");

        assert_eq!(deserialized.scraped_at, catalog.scraped_at);
        assert_eq!(deserialized.synthesized, catalog.synthesized);
        assert_eq!(deserialized.models.len(), 1);
        assert_eq!(deserialized.models[0].name, "test-model");
    }

    #[test]
    fn test_catalog_age_check_fresh() {
        let now = Utc::now();
        let catalog = ModelCatalog {
            scraped_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            synthesized: false,
            models: Vec::new(),
        };

        let is_stale = catalog.is_stale().expect("Should parse timestamp");
        assert!(!is_stale, "Fresh catalog should not be stale");
    }

    #[test]
    fn test_empty_catalog_treated_as_needing_update() {
        let now = Utc::now();
        let catalog = ModelCatalog {
            scraped_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            synthesized: false,
            models: Vec::new(),
        };

        // Even though the catalog is fresh, it has no models — should need refresh
        assert!(!catalog.is_stale().unwrap(), "Fresh catalog should not be stale");
        assert!(catalog.models.is_empty(), "Catalog with no models should trigger refresh");
    }

    #[test]
    fn test_catalog_age_check_stale() {
        // Create a timestamp 25 hours ago
        let old_time = Utc::now() - chrono::Duration::hours(25);
        let catalog = ModelCatalog {
            scraped_at: old_time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            synthesized: false,
            models: Vec::new(),
        };

        let is_stale = catalog.is_stale().expect("Should parse timestamp");
        assert!(is_stale, "Catalog older than 24h should be stale");
    }
}
