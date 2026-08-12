use crate::database::models::{Setting, TranscriptSetting};
use crate::summary::CustomOpenAIConfig;
use sqlx::SqlitePool;

#[derive(serde::Deserialize, Debug)]
pub struct SaveModelConfigRequest {
    pub provider: String,
    pub model: String,
    #[serde(rename = "whisperModel")]
    pub whisper_model: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(rename = "ollamaEndpoint")]
    pub ollama_endpoint: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
pub struct SaveTranscriptConfigRequest {
    pub provider: String,
    pub model: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
}

pub struct SettingsRepository;

pub const RK_SUMMARY_ENDPOINT: &str = "http://127.0.0.1:8040/v1";
pub const RK_SUMMARY_MODEL: &str = "qwen3-4b";
pub const RK_SUMMARY_MAX_TOKENS: i32 = 768;
pub const RK_SUMMARY_TEMPERATURE: f32 = 0.3;
pub const RK_SUMMARY_TOP_P: f32 = 0.9;
pub const RK_TRANSCRIPT_PROVIDER: &str = "rk-asr";
pub const RK_TRANSCRIPT_MODEL: &str = "sensevoice-rknn";

// Transcript providers: localWhisper, deepgram, elevenLabs, groq, openai
// Summary providers: openai, claude, ollama, groq, added openrouter
// NOTE: Handle data exclusion in the higher layer as this is database abstraction layer(using SELECT *)

impl SettingsRepository {
    pub fn rk_summary_config() -> CustomOpenAIConfig {
        CustomOpenAIConfig {
            endpoint: RK_SUMMARY_ENDPOINT.to_string(),
            api_key: None,
            model: RK_SUMMARY_MODEL.to_string(),
            max_tokens: Some(RK_SUMMARY_MAX_TOKENS),
            temperature: Some(RK_SUMMARY_TEMPERATURE),
            top_p: Some(RK_SUMMARY_TOP_P),
        }
    }

    /// Seed the RK1828 summary endpoint for fresh installs and installations
    /// that still use Meetily's bundled-model default. Explicit user choices
    /// are preserved.
    pub async fn ensure_rk_summary_defaults(
        pool: &SqlitePool,
    ) -> std::result::Result<bool, sqlx::Error> {
        let should_apply = match Self::get_model_config(pool).await? {
            None => true,
            Some(setting) => {
                setting.provider == "builtin-ai" && setting.custom_openai_config.is_none()
            }
        };

        if !should_apply {
            return Ok(false);
        }

        Self::save_custom_openai_config(pool, &Self::rk_summary_config()).await?;
        Ok(true)
    }

    /// Seed the RK3588 ASR service for fresh installs and installations that
    /// still use Meetily's bundled Parakeet default. Explicit user choices
    /// are preserved.
    pub async fn ensure_rk_transcript_defaults(
        pool: &SqlitePool,
    ) -> std::result::Result<bool, sqlx::Error> {
        let should_apply = match Self::get_transcript_config(pool).await? {
            None => true,
            Some(setting) => {
                setting.provider == "parakeet"
                    && setting.model == crate::config::DEFAULT_PARAKEET_MODEL
            }
        };

        if !should_apply {
            return Ok(false);
        }

        Self::save_transcript_config(pool, RK_TRANSCRIPT_PROVIDER, RK_TRANSCRIPT_MODEL).await?;
        Ok(true)
    }

    pub async fn get_model_config(
        pool: &SqlitePool,
    ) -> std::result::Result<Option<Setting>, sqlx::Error> {
        let setting = sqlx::query_as::<_, Setting>("SELECT * FROM settings LIMIT 1")
            .fetch_optional(pool)
            .await?;
        Ok(setting)
    }

    pub async fn save_model_config(
        pool: &SqlitePool,
        provider: &str,
        model: &str,
        whisper_model: &str,
        ollama_endpoint: Option<&str>,
    ) -> std::result::Result<(), sqlx::Error> {
        // Using id '1' for backward compatibility
        sqlx::query(
            r#"
            INSERT INTO settings (id, provider, model, whisperModel, ollamaEndpoint)
            VALUES ('1', $1, $2, $3, $4)
            ON CONFLICT(id) DO UPDATE SET
                provider = excluded.provider,
                model = excluded.model,
                whisperModel = excluded.whisperModel,
                ollamaEndpoint = excluded.ollamaEndpoint
            "#,
        )
        .bind(provider)
        .bind(model)
        .bind(whisper_model)
        .bind(ollama_endpoint)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn save_api_key(
        pool: &SqlitePool,
        provider: &str,
        api_key: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        // Custom OpenAI uses JSON config (customOpenAIConfig) instead of a separate API key column
        if provider == "custom-openai" {
            return Err(sqlx::Error::Protocol(
                "custom-openai provider should use save_custom_openai_config() instead of save_api_key()".into(),
            ));
        }

        let api_key_column = match provider {
            "openai" => "openaiApiKey",
            "claude" => "anthropicApiKey",
            "ollama" => "ollamaApiKey",
            "groq" => "groqApiKey",
            "openrouter" => "openRouterApiKey",
            "builtin-ai" => return Ok(()), // No API key needed
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        let query = format!(
            r#"
            INSERT INTO settings (id, provider, model, whisperModel, "{}")
            VALUES ('1', 'openai', 'gpt-4o-2024-11-20', 'large-v3', $1)
            ON CONFLICT(id) DO UPDATE SET
                "{}" = $1
            "#,
            api_key_column, api_key_column
        );
        sqlx::query(&query).bind(api_key).execute(pool).await?;

        Ok(())
    }

    pub async fn get_api_key(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<Option<String>, sqlx::Error> {
        // Custom OpenAI uses JSON config - extract API key from there
        if provider == "custom-openai" {
            let config = Self::get_custom_openai_config(pool).await?;
            return Ok(config.and_then(|c| c.api_key));
        }

        let api_key_column = match provider {
            "openai" => "openaiApiKey",
            "ollama" => "ollamaApiKey",
            "groq" => "groqApiKey",
            "claude" => "anthropicApiKey",
            "openrouter" => "openRouterApiKey",
            "builtin-ai" => return Ok(None), // No API key needed
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        let query = format!(
            "SELECT {} FROM settings WHERE id = '1' LIMIT 1",
            api_key_column
        );
        let api_key = sqlx::query_scalar(&query).fetch_optional(pool).await?;
        Ok(api_key)
    }

    pub async fn get_transcript_config(
        pool: &SqlitePool,
    ) -> std::result::Result<Option<TranscriptSetting>, sqlx::Error> {
        let setting =
            sqlx::query_as::<_, TranscriptSetting>("SELECT * FROM transcript_settings LIMIT 1")
                .fetch_optional(pool)
                .await?;
        Ok(setting)

    }

    pub async fn save_transcript_config(
        pool: &SqlitePool,
        provider: &str,
        model: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO transcript_settings (id, provider, model)
            VALUES ('1', $1, $2)
            ON CONFLICT(id) DO UPDATE SET
                provider = excluded.provider,
                model = excluded.model
            "#,
        )
        .bind(provider)
        .bind(model)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn save_transcript_api_key(
        pool: &SqlitePool,
        provider: &str,
        api_key: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        let api_key_column = match provider {
            "localWhisper" => "whisperApiKey",
            "parakeet" | "rk-asr" => return Ok(()), // Local providers do not need API keys
            "deepgram" => "deepgramApiKey",
            "elevenLabs" => "elevenLabsApiKey",
            "groq" => "groqApiKey",
            "openai" => "openaiApiKey",
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        let query = format!(
            r#"
            INSERT INTO transcript_settings (id, provider, model, "{}")
            VALUES ('1', 'parakeet', '{}', $1)
            ON CONFLICT(id) DO UPDATE SET
                "{}" = $1
            "#,
            api_key_column, crate::config::DEFAULT_PARAKEET_MODEL, api_key_column
        );
        sqlx::query(&query).bind(api_key).execute(pool).await?;

        Ok(())
    }

    pub async fn get_transcript_api_key(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<Option<String>, sqlx::Error> {
        let api_key_column = match provider {
            "localWhisper" => "whisperApiKey",
            "parakeet" | "rk-asr" => return Ok(None), // Local providers do not need API keys
            "deepgram" => "deepgramApiKey",
            "elevenLabs" => "elevenLabsApiKey",
            "groq" => "groqApiKey",
            "openai" => "openaiApiKey",
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        let query = format!(
            "SELECT {} FROM transcript_settings WHERE id = '1' LIMIT 1",
            api_key_column
        );
        let api_key = sqlx::query_scalar(&query).fetch_optional(pool).await?;
        Ok(api_key)
    }

    pub async fn delete_api_key(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        // Custom OpenAI uses JSON config - clear the entire config
        if provider == "custom-openai" {
            sqlx::query("UPDATE settings SET customOpenAIConfig = NULL WHERE id = '1'")
                .execute(pool)
                .await?;
            return Ok(());
        }

        let api_key_column = match provider {
            "openai" => "openaiApiKey",
            "ollama" => "ollamaApiKey",
            "groq" => "groqApiKey",
            "claude" => "anthropicApiKey",
            "openrouter" => "openRouterApiKey",
            "builtin-ai" => return Ok(()), // No API key needed
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        let query = format!(
            "UPDATE settings SET {} = NULL WHERE id = '1'",
            api_key_column
        );
        sqlx::query(&query).execute(pool).await?;

        Ok(())
    }

    // ===== CUSTOM OPENAI CONFIG METHODS =====

    /// Gets the custom OpenAI configuration from JSON
    ///
    /// # Returns
    /// * `Ok(Some(CustomOpenAIConfig))` - Config exists and is valid JSON
    /// * `Ok(None)` - No config stored
    /// * `Err(sqlx::Error)` - Database error
    pub async fn get_custom_openai_config(
        pool: &SqlitePool,
    ) -> std::result::Result<Option<CustomOpenAIConfig>, sqlx::Error> {
        use sqlx::Row;

        let row = sqlx::query(
            r#"
            SELECT customOpenAIConfig
            FROM settings
            WHERE id = '1'
            LIMIT 1
            "#
        )
        .fetch_optional(pool)
        .await?;

        match row {
            Some(record) => {
                let config_json: Option<String> = record.get("customOpenAIConfig");

                if let Some(json) = config_json {
                    // Parse JSON into CustomOpenAIConfig
                    let config: CustomOpenAIConfig = serde_json::from_str(&json)
                        .map_err(|e| sqlx::Error::Protocol(
                            format!("Invalid JSON in customOpenAIConfig: {}", e).into()
                        ))?;

                    Ok(Some(config))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    /// Saves the custom OpenAI configuration as JSON
    ///
    /// # Arguments
    /// * `pool` - Database connection pool
    /// * `config` - CustomOpenAIConfig to save (includes endpoint, apiKey, model, maxTokens, temperature, topP)
    ///
    /// # Returns
    /// * `Ok(())` - Config saved successfully
    /// * `Err(sqlx::Error)` - Database or JSON serialization error
    pub async fn save_custom_openai_config(
        pool: &SqlitePool,
        config: &CustomOpenAIConfig,
    ) -> std::result::Result<(), sqlx::Error> {
        // Serialize config to JSON
        let config_json = serde_json::to_string(config)
            .map_err(|e| sqlx::Error::Protocol(
                format!("Failed to serialize config to JSON: {}", e).into()
            ))?;

        // Upsert into settings table
        sqlx::query(
            r#"
            INSERT INTO settings (id, provider, model, whisperModel, customOpenAIConfig)
            VALUES ('1', 'custom-openai', $1, 'large-v3', $2)
            ON CONFLICT(id) DO UPDATE SET
                provider = excluded.provider,
                model = excluded.model,
                customOpenAIConfig = excluded.customOpenAIConfig
            "#,
        )
        .bind(&config.model)
        .bind(config_json)
        .execute(pool)
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn settings_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory database should open");

        sqlx::query(
            r#"
            CREATE TABLE settings (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                whisperModel TEXT NOT NULL,
                customOpenAIConfig TEXT,
                ollamaEndpoint TEXT,
                openaiApiKey TEXT,
                anthropicApiKey TEXT,
                ollamaApiKey TEXT,
                groqApiKey TEXT,
                openRouterApiKey TEXT
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("settings table should be created");

        sqlx::query(
            r#"
            CREATE TABLE transcript_settings (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                whisperApiKey TEXT,
                deepgramApiKey TEXT,
                elevenLabsApiKey TEXT,
                groqApiKey TEXT,
                openaiApiKey TEXT
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("transcript settings table should be created");

        pool
    }

    #[tokio::test]
    async fn saving_custom_openai_config_activates_that_provider_atomically() {
        let pool = settings_pool().await;
        SettingsRepository::save_model_config(
            &pool,
            "builtin-ai",
            "qwen3.5:4b",
            "large-v3",
            None,
        )
        .await
        .expect("bundled default should be saved");

        let config = CustomOpenAIConfig {
            endpoint: "http://127.0.0.1:8040/v1".to_string(),
            api_key: None,
            model: "qwen3-4b".to_string(),
            max_tokens: Some(768),
            temperature: Some(0.3),
            top_p: Some(0.9),
        };
        SettingsRepository::save_custom_openai_config(&pool, &config)
            .await
            .expect("custom provider should be saved");

        let saved = SettingsRepository::get_model_config(&pool)
            .await
            .expect("model config should load")
            .expect("model config should exist");
        assert_eq!(saved.provider, "custom-openai");
        assert_eq!(saved.model, "qwen3-4b");
        assert_eq!(saved.get_custom_openai_config().unwrap().endpoint, config.endpoint);
    }

    #[tokio::test]
    async fn rk_defaults_replace_only_the_bundled_summary_default() {
        let pool = settings_pool().await;
        SettingsRepository::save_model_config(
            &pool,
            "builtin-ai",
            "qwen3.5:4b",
            "large-v3",
            None,
        )
        .await
        .unwrap();

        assert!(SettingsRepository::ensure_rk_summary_defaults(&pool).await.unwrap());
        let rk = SettingsRepository::get_model_config(&pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rk.provider, "custom-openai");
        assert_eq!(rk.model, RK_SUMMARY_MODEL);
        let rk_config = rk.get_custom_openai_config().unwrap();
        assert_eq!(rk_config.endpoint, RK_SUMMARY_ENDPOINT);
        assert_eq!(rk_config.max_tokens, Some(768));
        assert_eq!(rk_config.temperature, Some(0.3));
        assert_eq!(rk_config.top_p, Some(0.9));

        SettingsRepository::save_model_config(
            &pool,
            "openai",
            "gpt-4o",
            "large-v3",
            None,
        )
        .await
        .unwrap();
        assert!(!SettingsRepository::ensure_rk_summary_defaults(&pool).await.unwrap());
        let preserved = SettingsRepository::get_model_config(&pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(preserved.provider, "openai");
        assert_eq!(preserved.model, "gpt-4o");
    }

    #[tokio::test]
    async fn rk_asr_defaults_replace_only_the_bundled_transcription_default() {
        let pool = settings_pool().await;
        SettingsRepository::save_transcript_config(
            &pool,
            "parakeet",
            crate::config::DEFAULT_PARAKEET_MODEL,
        )
        .await
        .unwrap();

        assert!(SettingsRepository::ensure_rk_transcript_defaults(&pool)
            .await
            .unwrap());
        let rk = SettingsRepository::get_transcript_config(&pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rk.provider, RK_TRANSCRIPT_PROVIDER);
        assert_eq!(rk.model, RK_TRANSCRIPT_MODEL);

        SettingsRepository::save_transcript_config(&pool, "localWhisper", "large-v3")
            .await
            .unwrap();
        assert!(!SettingsRepository::ensure_rk_transcript_defaults(&pool)
            .await
            .unwrap());
        let preserved = SettingsRepository::get_transcript_config(&pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(preserved.provider, "localWhisper");
        assert_eq!(preserved.model, "large-v3");
    }
}
