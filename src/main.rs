use clap::Parser;
use httpet::config::setup_logging;
use sea_orm_migration::MigratorTrait;
use tracing::error;

#[tokio::main(flavor = "multi_thread", worker_threads = 32)]
async fn main() {
    let cli = httpet::cli::CliOptions::parse();

    setup_logging(cli.debug);

    let db = match httpet::db::connect_db("httpet.sqlite").await {
        Ok(db) => db,
        Err(err) => {
            error!("Database connection error: {}", err);
            return;
        }
    };

    if let Err(err) = httpet::db::migrations::Migrator::up(&db, None).await {
        error!("Database migration error: {}", err);
        return;
    }

    let enabled_pets = match httpet::db::entities::pets::enabled(&db).await {
        Ok(pets) => pets,
        Err(err) => {
            error!("Failed to load enabled pets: {}", err);
            return;
        }
    };

    if let Err(err) = httpet::web::setup_server(
        &cli.listen_address,
        cli.port,
        &cli.base_domain,
        enabled_pets,
        db,
    )
    .await
    {
        error!("Application error: {}", err);
    }
}
