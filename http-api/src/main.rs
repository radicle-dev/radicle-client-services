use std::net;
use std::path::PathBuf;
use std::str::FromStr;

use radicle_http_api as api;

use argh::FromArgs;

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
/// Radicle HTTP API.
#[derive(FromArgs)]
pub struct Options {
    /// listen on the following address for HTTP connections (default: 0.0.0.0:8777)
    #[argh(option, default = "std::net::SocketAddr::from(([0, 0, 0, 0], 8777))")]
    pub listen: net::SocketAddr,

    /// radicle root path, for key and git storage
    #[argh(option)]
    pub root: PathBuf,

    /// TLS certificate path
    #[argh(option)]
    pub tls_cert: Option<PathBuf>,

    /// TLS key path
    #[argh(option)]
    pub tls_key: Option<PathBuf>,

    /// syntax highlight theme
    #[argh(option, default = r#"String::from("base16-ocean.dark")"#)]
    pub theme: String,

    /// either "plain" or "gcp" (gcp available only when compiled-in)
    #[argh(option, default = "LogFmt::Plain")]
    pub log_format: LogFmt,
}

impl Options {
    pub fn from_env() -> Self {
        argh::from_env()
    }
}

impl From<Options> for api::Options {
    fn from(other: Options) -> Self {
        Self {
            root: other.root,
            tls_cert: other.tls_cert,
            tls_key: other.tls_key,
            listen: other.listen,
            theme: other.theme,
        }
    }
}

fn set_up_plain_log_format(_opts: &Options) {
    tracing_subscriber::fmt()
        .init();
}

#[cfg(not(feature = "gcp"))]
fn init_logger(opts: &Options) {
    set_up_plain_log_format(opts)
}

#[cfg(feature = "gcp")]
fn init_logger(opts: &Options) {
    match opts.log_format {
        LogFmt::Plain => set_up_plain_log_format(opts),
        LogFmt::Gcp => {
            use tracing_stackdriver::Stackdriver;
            use tracing_subscriber::{layer::SubscriberExt, Registry};
            let stackdriver = Stackdriver::with_writer(std::io::stderr); // writes to std::io::Stderr
            let subscriber = Registry::default().with(stackdriver);
            tracing::subscriber::set_global_default(subscriber)
                .expect("Could not set up global logger");
        },
    }
}

#[tokio::main]
async fn main() {
    let options = Options::from_env();

    init_logger(&options);

    api::run(options.into()).await;
}
