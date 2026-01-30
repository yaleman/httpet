use clap::Parser;
use std::num::NonZeroU16;

#[derive(Parser, Debug)]
pub struct CliOptions {
    #[clap(long, short, help = "Enable debug logging")]
    pub debug: bool,
    #[clap(long, short, default_value = "3000", env = "HTTPET_PORT")]
    pub port: NonZeroU16,
    #[clap(
        long,
        short,
        default_value = "127.0.0.1",
        env = "HTTPET_LISTEN_ADDRESS"
    )]
    pub listen_address: String,
}
