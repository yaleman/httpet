//! Config handling

use tracing::log::LevelFilter;

/// Sets up logging based on the debug flag
pub fn setup_logging(debug: bool) -> Result<(), Box<std::io::Error>> {
    let level = if debug {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };

    let mut logger = simple_logger::SimpleLogger::new().with_level(level);
    if !debug {
        logger = logger
            .with_module_level("tracing", LevelFilter::Warn)
            .with_module_level("rustls", LevelFilter::Info)
            .with_module_level("hyper_util", LevelFilter::Info)
            .with_module_level("h2", LevelFilter::Info);
    }
    logger.init().map_err(|err| {
        eprintln!("Failed to initialize logger: {}", err);
        Box::new(std::io::Error::other(err))
    })
}
