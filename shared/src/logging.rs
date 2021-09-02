use std::str::FromStr;

#[derive(Clone, Copy)]
pub enum LogFmt {
    Plain,

    #[cfg(feature = "gcp")]
    Gcp,
}

impl FromStr for LogFmt {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "plain" => Ok(LogFmt::Plain),

            #[cfg(feature = "gcp")]
            "gcp" => Ok(LogFmt::Gcp),

            _ => Err("Unrecognized log format"),
        }
    }
}

pub fn init_logger(log_format: LogFmt) {
    match log_format {
        LogFmt::Plain => {
            tracing_subscriber::fmt()
                .with_ansi(false)
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .init();
        }

        #[cfg(feature = "gcp")]
        LogFmt::Gcp => {
            use tracing_stackdriver::Stackdriver;
            use tracing_subscriber::Layer;
            use tracing_subscriber::{layer::SubscriberExt, Registry};
            let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
            let stackdriver_layer = Stackdriver::default();
            let subscriber = Registry::default().with(stackdriver_layer);
            let result = env_filter.with_subscriber(subscriber);
            tracing::subscriber::set_global_default(result)
                .expect("Could not set up global logger");
        }
    }
}
