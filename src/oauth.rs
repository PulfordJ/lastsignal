use anyhow::{Context, Result};
use axum::{
    extract::Query,
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tower_http::cors::CorsLayer;

#[derive(Debug, Serialize, Deserialize)]
pub struct WhoopTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub token_type: String,
}

#[derive(Debug, Deserialize)]
struct WhoopTokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    token_type: String,
}

#[derive(Debug, Deserialize)]
struct OAuthCallbackQuery {
    code: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug)]
pub struct WhoopOAuth {
    client: Client,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    data_directory: std::path::PathBuf,
}

impl WhoopOAuth {
    pub fn new(
        client_id: String,
        client_secret: String,
        redirect_uri: String,
        data_directory: std::path::PathBuf,
    ) -> Self {
        Self {
            client: Client::new(),
            client_id,
            client_secret,
            redirect_uri,
            data_directory,
        }
    }

    pub fn get_authorization_url(&self) -> String {
        let scopes = vec![
            "read:cycles",
            "read:sleep",
            "read:recovery",
            "read:profile",
            "offline",
        ];

        format!(
            "https://api.prod.whoop.com/oauth/oauth2/auth?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}",
            self.client_id,
            urlencoding::encode(&self.redirect_uri),
            urlencoding::encode(&scopes.join(" ")),
            "lastsignal_auth" // Simple state parameter
        )
    }

    pub async fn exchange_code_for_token(&self, code: &str) -> Result<WhoopTokens> {
        let mut form_data = HashMap::new();
        form_data.insert("grant_type", "authorization_code");
        form_data.insert("client_id", &self.client_id);
        form_data.insert("client_secret", &self.client_secret);
        form_data.insert("redirect_uri", &self.redirect_uri);
        form_data.insert("code", code);

        let response = self
            .client
            .post("https://api.prod.whoop.com/oauth/oauth2/token")
            .form(&form_data)
            .send()
            .await
            .context("Failed to exchange authorization code for token")?;

        let is_success = response.status().is_success();
        let response_text = response.text().await.unwrap_or_default();
        
        if !is_success {
            tracing::debug!("WHOOP token exchange failed response: {}", response_text);
            anyhow::bail!("Token exchange failed: {}", response_text);
        }

        tracing::debug!("WHOOP token exchange successful response: {}", response_text);

        let token_response: WhoopTokenResponse = serde_json::from_str(&response_text)
            .context("Failed to parse token response")?;

        // With offline scope, refresh_token should always be present
        if token_response.refresh_token.is_empty() {
            anyhow::bail!("No refresh token received despite requesting offline scope");
        }

        let expires_at = Utc::now() + chrono::Duration::seconds(token_response.expires_in as i64);

        let tokens = WhoopTokens {
            access_token: token_response.access_token,
            refresh_token: token_response.refresh_token,
            expires_at,
            token_type: token_response.token_type,
        };

        Ok(tokens)
    }

    pub async fn refresh_token(&self, refresh_token: &str) -> Result<WhoopTokens> {
        let mut form_data = HashMap::new();
        form_data.insert("grant_type", "refresh_token");
        form_data.insert("client_id", &self.client_id);
        form_data.insert("client_secret", &self.client_secret);
        form_data.insert("refresh_token", refresh_token);

        let response = self
            .client
            .post("https://api.prod.whoop.com/oauth/oauth2/token")
            .form(&form_data)
            .send()
            .await
            .context("Failed to refresh token")?;

        let is_success = response.status().is_success();
        let response_text = response.text().await.unwrap_or_default();
        
        if !is_success {
            tracing::debug!("WHOOP token refresh failed response: {}", response_text);
            anyhow::bail!("Token refresh failed: {}", response_text);
        }

        tracing::debug!("WHOOP token refresh successful response: {}", response_text);

        let token_response: WhoopTokenResponse = serde_json::from_str(&response_text)
            .context("Failed to parse refresh token response")?;

        // Refresh token response should always include a new refresh token
        if token_response.refresh_token.is_empty() {
            anyhow::bail!("No refresh token received in refresh response");
        }

        let expires_at = Utc::now() + chrono::Duration::seconds(token_response.expires_in as i64);

        let tokens = WhoopTokens {
            access_token: token_response.access_token,
            refresh_token: token_response.refresh_token,
            expires_at,
            token_type: token_response.token_type,
        };

        Ok(tokens)
    }

    pub fn save_tokens(&self, tokens: &WhoopTokens) -> Result<()> {
        let tokens_file = self.data_directory.join("whoop_tokens.json");
        
        // Ensure the directory exists
        if let Some(parent) = tokens_file.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {:?}", parent))?;
        }

        let tokens_json = serde_json::to_string_pretty(tokens)
            .context("Failed to serialize tokens")?;

        std::fs::write(&tokens_file, tokens_json)
            .with_context(|| format!("Failed to write tokens file: {:?}", tokens_file))?;

        tracing::info!("Saved WHOOP tokens to: {:?}", tokens_file);
        Ok(())
    }

    pub fn load_tokens(&self) -> Result<WhoopTokens> {
        let tokens_file = self.data_directory.join("whoop_tokens.json");
        
        if !tokens_file.exists() {
            anyhow::bail!("No WHOOP tokens found. Please run 'lastsignal whoop-auth' first.");
        }

        let tokens_json = std::fs::read_to_string(&tokens_file)
            .with_context(|| format!("Failed to read tokens file: {:?}", tokens_file))?;

        let tokens: WhoopTokens = serde_json::from_str(&tokens_json)
            .context("Failed to parse tokens file")?;

        Ok(tokens)
    }

    pub async fn get_valid_access_token(&self) -> Result<String> {
        let mut tokens = self.load_tokens()?;

        // Check if token is expired or will expire within 5 minutes
        let now = Utc::now();
        let buffer = chrono::Duration::minutes(5);
        
        if tokens.expires_at <= now + buffer {
            tracing::info!("Access token expired or expiring soon, refreshing...");
            tokens = self.refresh_token(&tokens.refresh_token).await?;
            self.save_tokens(&tokens)?;
        }

        Ok(tokens.access_token)
    }
}

// OAuth callback handler
async fn oauth_callback(
    Query(query): Query<OAuthCallbackQuery>,
) -> impl IntoResponse {
    if let Some(error) = query.error {
        let error_desc = query.error_description.unwrap_or_default();
        return (
            StatusCode::BAD_REQUEST,
            Html(format!(
                r#"
                <html>
                <head><title>WHOOP Authentication Failed</title></head>
                <body>
                    <h1>Authentication Failed</h1>
                    <p>Error: {}</p>
                    <p>Description: {}</p>
                    <p>Please close this window and try again.</p>
                </body>
                </html>
                "#,
                error, error_desc
            )),
        );
    }

    if let Some(code) = query.code {
        // Store the code for the main application to retrieve
        if let Err(e) = std::fs::write("/tmp/whoop_auth_code.txt", &code) {
            tracing::error!("Failed to store auth code: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(
                    r#"
                    <html>
                    <head><title>WHOOP Authentication Error</title></head>
                    <body>
                        <h1>Authentication Error</h1>
                        <p>Failed to store authorization code. Please try again.</p>
                        <p>You can close this window now.</p>
                    </body>
                    </html>
                    "#.to_string(),
                ),
            );
        }

        return (
            StatusCode::OK,
            Html(
                r#"
                <html>
                <head><title>WHOOP Authentication Success</title></head>
                <body>
                    <h1>Authentication Successful!</h1>
                    <p>You have successfully authenticated with WHOOP.</p>
                    <p>You can now close this window and return to the terminal.</p>
                    <script>
                        setTimeout(() => {
                            window.close();
                        }, 3000);
                    </script>
                </body>
                </html>
                "#.to_string(),
            ),
        );
    }

    (
        StatusCode::BAD_REQUEST,
        Html(
            r#"
            <html>
            <head><title>WHOOP Authentication Error</title></head>
            <body>
                <h1>Authentication Error</h1>
                <p>No authorization code received. Please try again.</p>
                <p>You can close this window now.</p>
            </body>
            </html>
            "#.to_string(),
        ),
    )
}

pub async fn start_oauth_server(port: u16) -> Result<()> {
    let app = Router::new()
        .route("/auth/whoop/callback", get(oauth_callback))
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port))
        .await
        .with_context(|| format!("Failed to bind to port {}", port))?;

    tracing::info!("OAuth server listening on http://127.0.0.1:{}", port);

    axum::serve(listener, app)
        .await
        .context("OAuth server failed")?;

    Ok(())
}

pub async fn run_whoop_authentication(
    client_id: String,
    client_secret: String,
    data_directory: std::path::PathBuf,
) -> Result<()> {
    let port = 3000; // Default port for OAuth redirect
    let redirect_uri = format!("http://127.0.0.1:{}/auth/whoop/callback", port);
    
    let oauth_client = WhoopOAuth::new(client_id, client_secret, redirect_uri, data_directory);

    // Start the OAuth server in the background
    let server_handle = tokio::spawn(async move {
        if let Err(e) = start_oauth_server(port).await {
            tracing::error!("OAuth server error: {}", e);
        }
    });

    // Generate and display authorization URL
    let auth_url = oauth_client.get_authorization_url();
    println!("\n🔗 Please open the following URL in your browser to authenticate with WHOOP:");
    println!("{}", auth_url);
    println!("\nAfter authentication, the browser will redirect to localhost and you should see a success message.");
    println!("Waiting for authentication...\n");

    // Wait for the authorization code
    let mut attempts = 0;
    let max_attempts = 120; // 2 minutes timeout
    let auth_code = loop {
        if std::path::Path::new("/tmp/whoop_auth_code.txt").exists() {
            match std::fs::read_to_string("/tmp/whoop_auth_code.txt") {
                Ok(code) => {
                    // Clean up the temporary file
                    let _ = std::fs::remove_file("/tmp/whoop_auth_code.txt");
                    break code.trim().to_string();
                }
                Err(e) => {
                    tracing::warn!("Failed to read auth code file: {}", e);
                }
            }
        }

        attempts += 1;
        if attempts >= max_attempts {
            server_handle.abort();
            anyhow::bail!("Timeout waiting for authentication. Please try again.");
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    };

    server_handle.abort();

    // Exchange code for tokens
    println!("🔄 Exchanging authorization code for access token...");
    let tokens = oauth_client.exchange_code_for_token(&auth_code).await?;
    
    // Save tokens
    oauth_client.save_tokens(&tokens)?;
    
    println!("✅ Successfully authenticated with WHOOP!");
    println!("📁 Tokens saved to: {:?}", oauth_client.data_directory.join("whoop_tokens.json"));
    println!("\nYou can now use the WHOOP adapter in your LastSignal configuration.");
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_whoop_oauth_initialization() {
        let temp_dir = tempdir().unwrap();
        let oauth_client = WhoopOAuth::new(
            "test_client_id".to_string(),
            "test_client_secret".to_string(),
            "http://localhost:3000/callback".to_string(),
            temp_dir.path().to_path_buf(),
        );

        assert_eq!(oauth_client.client_id, "test_client_id");
        assert_eq!(oauth_client.client_secret, "test_client_secret");
        assert_eq!(oauth_client.redirect_uri, "http://localhost:3000/callback");
    }

    #[test]
    fn test_authorization_url_generation() {
        let temp_dir = tempdir().unwrap();
        let oauth_client = WhoopOAuth::new(
            "test_client_id".to_string(),
            "test_client_secret".to_string(),
            "http://localhost:3000/callback".to_string(),
            temp_dir.path().to_path_buf(),
        );

        let auth_url = oauth_client.get_authorization_url();
        
        assert!(auth_url.contains("https://api.prod.whoop.com/oauth/oauth2/auth"));
        assert!(auth_url.contains("client_id=test_client_id"));
        assert!(auth_url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A3000%2Fcallback"));
        assert!(auth_url.contains("response_type=code"));
        assert!(auth_url.contains("read%3Acycles"));
        assert!(auth_url.contains("read%3Asleep"));
        assert!(auth_url.contains("read%3Arecovery"));
        assert!(auth_url.contains("offline"));
        assert!(auth_url.contains("state=lastsignal_auth"));
    }

    #[test]
    fn test_token_serialization() {
        let tokens = WhoopTokens {
            access_token: "test_access_token".to_string(),
            refresh_token: "test_refresh_token".to_string(),
            expires_at: Utc::now(),
            token_type: "Bearer".to_string(),
        };

        let serialized = serde_json::to_string(&tokens).unwrap();
        let deserialized: WhoopTokens = serde_json::from_str(&serialized).unwrap();

        assert_eq!(tokens.access_token, deserialized.access_token);
        assert_eq!(tokens.refresh_token, deserialized.refresh_token);
        assert_eq!(tokens.token_type, deserialized.token_type);
    }
}