pub fn init_logger() {
    use tracing::dispatcher::{self, Dispatch};
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Registry;

    let subscriber = Registry::default().with(tracing_logfmt::layer());

    dispatcher::set_global_default(Dispatch::new(subscriber))
        .expect("Global logger has already been set!");
}
