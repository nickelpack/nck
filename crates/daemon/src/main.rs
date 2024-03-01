#![feature(never_type)]
#![feature(lazy_cell)]
#![feature(unix_socket_ancillary_data)]
#![feature(register_tool)]
#![feature(custom_inner_attributes)]
#![register_tool(tarpaulin)]

use build::linux::PendingController;
use settings::Settings;
use store::Store;
use tracing_subscriber::prelude::*;

mod axum_extensions;
mod build;
mod frontend;
mod settings;
mod store;
mod string_types;

fn main() -> anyhow::Result<()> {
    // TODO: Move this into each process and send traces via the channels
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let settings: Settings = config::Config::builder()
        .add_source(
            config::Environment::with_prefix("nck")
                .separator("__")
                .try_parsing(true),
        )
        .build()?
        .try_deserialize()?;

    let controller = build::native::create_controller(settings.clone())?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(controller, settings))
}

async fn async_main(controller: PendingController, settings: Settings) -> anyhow::Result<()> {
    let controller = controller.into_controller().await?;
    let store = Store::new(controller, &settings.store).await?;

    let front_end = tokio::spawn(frontend::frontend(store.clone(), settings.clone()));

    front_end.await?
}
