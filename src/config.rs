//! CLI and environment configuration, validated before the terminal opens.
//!
//! Flags and environment variables share one source of truth per setting.
//! Setting a flag and its matching environment variable at the same time is a
//! conflict and fails with an actionable message rather than silently picking
//! one. Every value is validated here, so `app::run` only ever sees a coherent,
//! supported configuration.

use clap::parser::ValueSource;
use clap::{ArgMatches, CommandFactory, FromArgMatches, Parser};

pub const DEFAULT_REFRESH_SECONDS: u64 = 60;
pub const MIN_REFRESH_SECONDS: u64 = 15;

/// Default news feed: the CoinDesk crypto headline RSS, keyless and HTTPS.
pub const DEFAULT_NEWS_URL: &str = "https://www.coindesk.com/arc/outboundfeeds/rss";

/// Typed CLI surface. Flags documented by clap derive; env alternatives are
/// listed on the same flag so `--help` is complete.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "coin-tui",
    version,
    about = "Fast, read-only cryptocurrency market dashboard for the terminal."
)]
pub struct Cli {
    /// Automatic refresh interval in seconds (minimum 15).
    #[arg(
        long,
        default_value_t = DEFAULT_REFRESH_SECONDS,
        value_parser = clap::value_parser!(u64).range(MIN_REFRESH_SECONDS..)
    )]
    refresh_seconds: u64,

    /// Quote currency; the MVP supports only USD.
    #[arg(long, default_value = "usd", value_parser = parse_currency)]
    currency: String,

    /// CoinGecko-compatible API base URL. Alternative: env `COIN_TUI_BASE_URL`.
    #[arg(
        long,
        env = "COIN_TUI_BASE_URL",
        hide_env_values = true,
        default_value = "https://api.coingecko.com/"
    )]
    base_url: String,

    /// News headline RSS feed URL. Alternative: env `COIN_TUI_NEWS_URL`.
    #[arg(
        long,
        env = "COIN_TUI_NEWS_URL",
        hide_env_values = true,
        default_value = DEFAULT_NEWS_URL
    )]
    news_url: String,

    /// CoinGecko Demo API key, sent as `x-cg-demo-api-key`. Alternative: env
    /// `COIN_TUI_API_KEY`.
    #[arg(long, env = "COIN_TUI_API_KEY", hide_env_values = true)]
    api_key: Option<String>,

    /// Append redacted diagnostics to this file. Alternative: env
    /// `COIN_TUI_LOG_FILE`.
    #[arg(long, env = "COIN_TUI_LOG_FILE", hide_env_values = true)]
    log_file: Option<String>,
}

/// Validated settings handed to the rest of the application.
#[derive(Debug, Clone)]
pub struct Config {
    pub refresh_seconds: u64,
    pub currency: String,
    pub base_url: String,
    pub news_url: String,
    pub api_key: Option<String>,
    pub log_file: Option<String>,
}

/// Parse CLI arguments, enforcing a common source per setting. Bad values and
/// `--help`/`--version` are handled by clap before this returns; the remaining
/// errors (conflicts) surface as actionable messages to `main`.
pub fn load() -> Result<Config, String> {
    let matches = Cli::command().get_matches();
    config_from_matches(matches)
}

fn config_from_matches(matches: ArgMatches) -> Result<Config, String> {
    let cli = Cli::from_arg_matches(&matches).map_err(|error| error.to_string())?;
    validate_sources(&matches)?;
    validate_base_url(&cli.base_url)?;
    validate_base_url(&cli.news_url)?;
    Ok(Config {
        refresh_seconds: cli.refresh_seconds,
        currency: cli.currency,
        base_url: cli.base_url,
        news_url: cli.news_url,
        api_key: cli.api_key,
        log_file: cli.log_file,
    })
}

fn validate_sources(matches: &clap::ArgMatches) -> Result<(), String> {
    for (id, flag, env_var) in [
        ("base_url", "--base-url", "COIN_TUI_BASE_URL"),
        ("news_url", "--news-url", "COIN_TUI_NEWS_URL"),
        ("api_key", "--api-key", "COIN_TUI_API_KEY"),
        ("log_file", "--log-file", "COIN_TUI_LOG_FILE"),
    ] {
        let flag_set = matches.value_source(id) == Some(ValueSource::CommandLine);
        let env_set = std::env::var_os(env_var).is_some();
        reject_conflict(flag_set, env_set, flag, env_var)?;
    }
    Ok(())
}

/// One setting must not come from both the command line and the environment.
fn reject_conflict(flag_set: bool, env_set: bool, flag: &str, env_var: &str) -> Result<(), String> {
    if flag_set && env_set {
        Err(conflict_message(flag, env_var))
    } else {
        Ok(())
    }
}

fn conflict_message(flag: &str, env_var: &str) -> String {
    format!(
        "conflicting configuration: '{flag}' is set and environment variable {env_var} is also set; use one source"
    )
}

fn parse_currency(raw: &str) -> Result<String, String> {
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized != "usd" {
        return Err(format!(
            "unsupported currency '{raw}'; the MVP supports only 'usd'"
        ));
    }
    Ok(normalized)
}

/// Mirrors the client's base-URL rule with an actionable message: an absolute
/// URL with a host, no credentials, and an HTTPS scheme (raw HTTP only for
/// loopback hosts, so a fixture server is usable).
fn validate_base_url(raw: &str) -> Result<(), String> {
    let url = url::Url::parse(raw)
        .map_err(|_| "invalid base URL: not a valid absolute URL".to_owned())?;
    let allowed_http = url.scheme() == "http" && url.host().map(is_loopback_host).unwrap_or(false);
    if url.scheme() != "https" && !allowed_http {
        return Err(
            "invalid base URL: scheme must be https (http is allowed only for loopback hosts)"
                .to_owned(),
        );
    }
    if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return Err("invalid base URL: must have a host and no credentials".to_owned());
    }
    Ok(())
}

fn is_loopback_host(host: url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(address) => address == std::net::Ipv4Addr::LOCALHOST,
        url::Host::Ipv6(address) => address.is_loopback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    const CONFIG_ENV: [&str; 4] = [
        "COIN_TUI_BASE_URL",
        "COIN_TUI_NEWS_URL",
        "COIN_TUI_API_KEY",
        "COIN_TUI_LOG_FILE",
    ];

    struct EnvGuard {
        _lock: MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvGuard {
        fn clean() -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let saved = CONFIG_ENV
                .into_iter()
                .map(|name| {
                    let value = std::env::var_os(name);
                    std::env::remove_var(name);
                    (name, value)
                })
                .collect();
            Self { _lock: lock, saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in self.saved.drain(..) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    fn cli_defaults_match_the_config_contract() {
        let _env = EnvGuard::clean();
        let cli = Cli::try_parse_from(["coin-tui"]).unwrap();
        assert_eq!(cli.refresh_seconds, DEFAULT_REFRESH_SECONDS);
        assert_eq!(cli.currency, "usd");
        assert_eq!(cli.base_url, "https://api.coingecko.com/");
        assert_eq!(cli.news_url, DEFAULT_NEWS_URL);
        assert_eq!(cli.api_key, None);
        assert_eq!(cli.log_file, None);
    }

    #[test]
    fn help_lists_every_config_surface() {
        let _env = EnvGuard::clean();
        let help = Cli::command().render_help().to_string();
        for expected in [
            "--refresh-seconds",
            "--currency",
            "--base-url",
            "COIN_TUI_BASE_URL",
            "--news-url",
            "COIN_TUI_NEWS_URL",
            "--api-key",
            "COIN_TUI_API_KEY",
            "--log-file",
            "COIN_TUI_LOG_FILE",
        ] {
            assert!(help.contains(expected), "{expected} missing from:\n{help}");
        }
    }

    #[test]
    fn interval_below_the_floor_is_rejected_with_the_minimum() {
        let _env = EnvGuard::clean();
        let error = Cli::try_parse_from(["coin-tui", "--refresh-seconds", "5"]).unwrap_err();
        let message = error.to_string();
        let floor = MIN_REFRESH_SECONDS.to_string();
        assert!(message.contains(&floor), "{message}");
        let error = Cli::try_parse_from(["coin-tui", "--refresh-seconds", "abc"]).unwrap_err();
        assert!(error.to_string().contains("invalid value"));
    }

    #[test]
    fn a_legal_interval_is_accepted() {
        let _env = EnvGuard::clean();
        let cli = Cli::try_parse_from(["coin-tui", "--refresh-seconds", "45"]).unwrap();
        assert_eq!(cli.refresh_seconds, 45);
    }

    #[test]
    fn currency_is_case_insensitive_and_usd_only() {
        let _env = EnvGuard::clean();
        for raw in ["usd", "USD", "Usd"] {
            let cli = Cli::try_parse_from(["coin-tui", "--currency", raw]).unwrap();
            assert_eq!(cli.currency, "usd", "{raw}");
        }
        for raw in ["eur", "btc", ""] {
            let error = Cli::try_parse_from(["coin-tui", "--currency", raw]).unwrap_err();
            assert!(
                error.to_string().contains("unsupported currency"),
                "{raw}: {}",
                error
            );
        }
    }

    #[test]
    fn base_url_accepts_https_and_loopback_http_only() {
        let _env = EnvGuard::clean();
        for good in ["https://api.coingecko.com/", "http://localhost:8787/"] {
            let config = config_from_args(["coin-tui", "--base-url", good]).unwrap();
            assert_eq!(config.base_url, good);
        }
        for bad in [
            "ftp://example.com/",
            "not a url",
            "api.coingecko.com",
            "http://example.com/",
            "https://user:pass@example.com/",
        ] {
            let error = config_from_args(["coin-tui", "--base-url", bad]).unwrap_err();
            assert!(error.contains("invalid base URL"), "{bad:?}: {error}");
        }
    }

    #[test]
    fn base_url_errors_do_not_echo_credentials() {
        let _env = EnvGuard::clean();
        let error = config_from_args([
            "coin-tui",
            "--base-url",
            "https://user:secret-password@example.com/",
        ])
        .unwrap_err();
        assert!(error.contains("invalid base URL"), "{error}");
        assert!(!error.contains("secret-password"), "{error}");
        assert!(!error.contains("user:secret-password"), "{error}");
    }

    #[test]
    fn a_flag_and_its_environment_variable_conflict() {
        let _env = EnvGuard::clean();
        std::env::set_var("COIN_TUI_BASE_URL", "http://localhost:7777/");
        let message =
            config_from_args(["coin-tui", "--base-url", "http://localhost:8888/"]).unwrap_err();
        assert!(
            message.contains("--base-url") && message.contains("COIN_TUI_BASE_URL"),
            "{message}"
        );
    }

    fn config_from_args<const N: usize>(args: [&str; N]) -> Result<Config, String> {
        let matches = Cli::command()
            .try_get_matches_from(args)
            .map_err(|error| error.to_string())?;
        config_from_matches(matches)
    }
}
