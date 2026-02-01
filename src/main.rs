//! HTTPet main binary

#![allow(clippy::multiple_crate_versions)]
#![deny(clippy::all)]
#![deny(clippy::await_holding_lock)]
#![deny(clippy::complexity)]
#![deny(clippy::correctness)]
#![deny(clippy::disallowed_methods)]
#![deny(clippy::expect_used)]
#![deny(clippy::needless_pass_by_value)]
#![deny(clippy::panic)]
#![deny(clippy::perf)]
#![deny(clippy::trivially_copy_pass_by_ref)]
#![deny(clippy::unreachable)]
#![deny(clippy::unwrap_used)]
#![deny(warnings)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::process::ExitCode;

use clap::Parser;
use httpet::config::setup_logging;
use sea_orm_migration::MigratorTrait;
use tokio::signal::{unix::SignalKind, unix::signal};
use tracing::{error, info, warn};

#[tokio::main(flavor = "multi_thread", worker_threads = 32)]
async fn main() -> Result<ExitCode, Box<std::io::Error>> {
    let cli = httpet::cli::CliOptions::parse();

    setup_logging(cli.debug)?;

    let db = match httpet::db::connect_db(
        cli.database_path.as_deref().unwrap_or("./db/httpet.sqlite"),
        cli.debug,
    )
    .await
    {
        Ok(db) => db,
        Err(err) => {
            error!("Database connection error: {}", err);
            return Err(Box::new(std::io::Error::other(err)));
        }
    };

    if let Err(err) = httpet::db::migrations::Migrator::up(db.as_ref(), None).await {
        error!(error=?err, db_path=cli.database_path.as_deref().unwrap_or("./db/httpet.sqlite"), "Database migration error");
        return Err(Box::new(std::io::Error::other(err)));
    }

    let mut hangup_waiter = signal(SignalKind::hangup())?;

    loop {
        let enabled_pets = match httpet::db::entities::pets::Entity::enabled(db.as_ref()).await {
            Ok(pets) => pets.into_iter().map(|pet| pet.name).collect(),
            Err(err) => {
                error!("Failed to load enabled pets: {}", err);
                return Err(Box::new(std::io::Error::other(err)));
            }
        };
        tokio::select! {
            res =  httpet::web::setup_server(
            &cli,
            enabled_pets,
            db.clone(),
        ) => {
            if let Err(err) = res {
                    error!("Server error: {}", err);
                    break;
                } else {
                    info!("Server has shut down gracefully.");
                };
            }

            _ = hangup_waiter.recv() => {
                warn!("Received SIGHUP, shutting down.");
                break;
                // TODO: Implement configuration reload logic here

            }
            _ = tokio::signal::ctrl_c() => {
                info!("Received Ctrl-C, shutting down.");
                break;
            }
        }
    }

    Ok(ExitCode::SUCCESS)
}
