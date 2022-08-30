use tracing::dispatcher::{self, Dispatch};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Registry;

pub fn init_logger() {
    let subscriber = Registry::default()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_logfmt::layer());

    dispatcher::set_global_default(Dispatch::new(subscriber))
        .expect("Global logger has already been set!");
}
