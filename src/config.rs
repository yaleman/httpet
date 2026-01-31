//! Config handling

use tracing::log::LevelFilter;

/// Sets up logging based on the debug flag
pub fn setup_logging(debug: bool) -> Result<(), Box<std::io::Error>> {
    let level = if debug {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };

    simple_logger::SimpleLogger::new()
        .with_level(level)
        .init()
        .map_err(|err| {
            eprintln!("Failed to initialize logger: {}", err);
            Box::new(std::io::Error::other(err))
        })
}
