//! CLI parser
use clap::Parser;
use std::num::NonZeroU16;

#[derive(Parser, Debug)]
/// CLI Options
pub struct CliOptions {
    #[clap(long, help = "Enable debug logging", env = "HTTPET_DEBUG")]
    /// Enable debug logging. Env: HTTPET_DEBUG
    pub debug: bool,
    #[clap(long, short, default_value = "9000", env = "HTTPET_PORT")]
    /// http listener, defaults to `9000`.`
    /// Env: HTTPET_PORT
    pub port: NonZeroU16,
    #[clap(
        long,
        short,
        default_value = "127.0.0.1",
        env = "HTTPET_LISTEN_ADDRESS"
    )]
    /// Liten address, defaults to `127.0.0.1``.
    /// Env: HTTPET_LISTEN_ADDRESS
    pub listen_address: String,
    #[clap(long, short, default_value = "localhost", env = "HTTPET_BASE_DOMAIN")]
    /// Base domain, defaults to localhost, needs to be httpet.org in prod.
    /// Env: HTTPET_BASE_DOMAIN
    pub base_domain: String,

    #[clap(long, short, env = "HTTPET_DATABASE_PATH")]
    /// Path to the database file, eg `/data/httpet.sqlite`.
    /// Env: HTTPET_DATABASE_PATH
    pub database_path: Option<String>,
}
