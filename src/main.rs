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
use httpet::{config::setup_logging, status_codes::STATUS_CODES};
use sea_orm_migration::MigratorTrait;
use tokio::signal::{unix::SignalKind, unix::signal};
use tracing::log::{error, info, warn};

#[tokio::main(flavor = "multi_thread", worker_threads = 32)]
async fn main() -> ExitCode {
    let cli = httpet::cli::CliOptions::parse();

    if let Err(err) = setup_logging(cli.debug) {
        eprintln!("Logging setup error: {}", err);
        return ExitCode::FAILURE;
    };

    // to make sure it's loaded
    let _ = STATUS_CODES;

    let db = match httpet::db::connect_db(
        cli.database_path.as_deref().unwrap_or("./db/httpet.sqlite"),
        cli.debug,
    )
    .await
    {
        Ok(db) => db,
        Err(err) => {
            error!("Database connection error: {}", err);
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = httpet::db::migrations::Migrator::up(db.as_ref(), None).await {
        tracing::error!(error=?error, db_path=cli.database_path.as_deref().unwrap_or("./db/httpet.sqlite"), "Database migration error");
        return ExitCode::FAILURE;
    }

    let mut hangup_waiter = match signal(SignalKind::hangup()) {
        Ok(signal) => signal,
        Err(err) => {
            error!("Failed to set up SIGHUP handler: {}", err);
            return ExitCode::FAILURE;
        }
    };

    loop {
        let enabled_pets = match httpet::db::entities::pets::Entity::enabled(db.as_ref()).await {
            Ok(pets) => pets.into_iter().map(|pet| pet.name).collect(),
            Err(err) => {
                error!("Failed to load enabled pets: {}", err);
                return ExitCode::FAILURE;
            }
        };
        tokio::select! {
            res =  httpet::web::setup_server(
            &cli,
            enabled_pets,
            db.clone(),
        ) => {
            if let Err(error) = res {
                    error!("Server error: {:?}", error);
                    return ExitCode::FAILURE
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

    ExitCode::SUCCESS
}
