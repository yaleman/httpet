use tracing::log::LevelFilter;

pub fn setup_logging(debug: bool) {
    let level = if debug {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };

    simple_logger::SimpleLogger::new()
        .with_level(level)
        .init()
        .expect("Failed to initialize logger");
}
