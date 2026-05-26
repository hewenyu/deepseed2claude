use std::env;
use std::net::SocketAddr;

use crate::Error;

#[derive(Clone, Debug)]
pub struct Config {
    pub deepseek_api_key: String,
    pub deepseek_base_url: String,
    pub upstream_protocol: String,
    pub server_host: String,
    pub server_port: u16,
    pub default_deepseek_model: String,
    pub claude_opus_model: String,
    pub claude_sonnet_model: String,
    pub claude_haiku_model: String,
    pub deepseek_thinking: Option<String>,
    pub deepseek_reasoning_effort: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self, Error> {
        let deepseek_api_key = required_env("DEEPSEEK_API_KEY")?;
        Ok(Self {
            deepseek_api_key,
            deepseek_base_url: env_or("DEEPSEEK_BASE_URL", "https://api.deepseek.com/anthropic"),
            upstream_protocol: env_or("DEEPSEEK_UPSTREAM_PROTOCOL", "anthropic"),
            server_host: env_or("SERVER_HOST", "127.0.0.1"),
            server_port: env_or("SERVER_PORT", "3000")
                .parse()
                .map_err(|_| Error::Config("SERVER_PORT must be a valid TCP port".to_owned()))?,
            default_deepseek_model: env_or("DEFAULT_DEEPSEEK_MODEL", "deepseek-v4-flash"),
            claude_opus_model: env_or("CLAUDE_OPUS_MODEL", "deepseek-v4-pro"),
            claude_sonnet_model: env_or("CLAUDE_SONNET_MODEL", "deepseek-v4-flash"),
            claude_haiku_model: env_or("CLAUDE_HAIKU_MODEL", "deepseek-v4-flash"),
            deepseek_thinking: optional_env("DEEPSEEK_THINKING"),
            deepseek_reasoning_effort: optional_env("DEEPSEEK_REASONING_EFFORT"),
        })
    }

    pub fn for_test(base_url: String) -> Self {
        Self {
            deepseek_api_key: "test-deepseek-key".to_owned(),
            deepseek_base_url: base_url,
            upstream_protocol: "anthropic".to_owned(),
            server_host: "127.0.0.1".to_owned(),
            server_port: 0,
            default_deepseek_model: "deepseek-v4-flash".to_owned(),
            claude_opus_model: "deepseek-v4-pro".to_owned(),
            claude_sonnet_model: "deepseek-v4-flash".to_owned(),
            claude_haiku_model: "deepseek-v4-flash".to_owned(),
            deepseek_thinking: Some("disabled".to_owned()),
            deepseek_reasoning_effort: Some("high".to_owned()),
        }
    }

    pub fn listen_addr(&self) -> Result<SocketAddr, Error> {
        format!("{}:{}", self.server_host, self.server_port)
            .parse()
            .map_err(|err| Error::Config(format!("invalid listen address: {err}")))
    }

    pub fn messages_url(&self) -> String {
        let base = self.deepseek_base_url.trim_end_matches('/');
        if base.ends_with("/v1/messages") {
            base.to_owned()
        } else {
            format!("{base}/v1/messages")
        }
    }

    pub fn count_tokens_url(&self) -> String {
        format!("{}/count_tokens", self.messages_url())
    }

    pub fn map_model(&self, requested_model: &str) -> String {
        let model = requested_model.to_ascii_lowercase();
        if model == "deepseek-v4-flash" || model == "deepseek-v4-pro" {
            return requested_model.to_owned();
        }
        if model.contains("opus") {
            return self.claude_opus_model.clone();
        }
        if model.contains("sonnet") {
            return self.claude_sonnet_model.clone();
        }
        if model.contains("haiku") {
            return self.claude_haiku_model.clone();
        }
        self.default_deepseek_model.clone()
    }
}

fn required_env(name: &str) -> Result<String, Error> {
    let value = env::var(name).map_err(|_| Error::Config(format!("{name} is required")))?;
    if value.trim().is_empty() {
        return Err(Error::Config(format!("{name} is required")));
    }
    Ok(value)
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn env_or(name: &str, default: &str) -> String {
    optional_env(name).unwrap_or_else(|| default.to_owned())
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn maps_common_claude_names_to_deepseek_v4() {
        let config = Config::for_test("http://upstream".to_owned());

        assert_eq!(
            config.map_model("claude-opus-4-1-20250805"),
            "deepseek-v4-pro"
        );
        assert_eq!(
            config.map_model("claude-sonnet-4-5-20250929"),
            "deepseek-v4-flash"
        );
        assert_eq!(
            config.map_model("claude-3-5-haiku-20241022"),
            "deepseek-v4-flash"
        );
        assert_eq!(config.map_model("unknown-model"), "deepseek-v4-flash");
        assert_eq!(config.map_model("deepseek-v4-pro"), "deepseek-v4-pro");
    }
}
