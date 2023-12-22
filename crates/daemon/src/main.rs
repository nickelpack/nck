#![feature(never_type)]
#![feature(lazy_cell)]
#![feature(unix_socket_ancillary_data)]

use build::linux::PendingController;
use store::Store;
use tracing_subscriber::prelude::*;

mod build;
mod frontend;
mod settings;
mod spec;
mod store;
mod string_types;

fn main() -> anyhow::Result<()> {
    // TODO: Move this into each process and send traces via the channels
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let controller = build::native::create_controller()?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(controller))
}

async fn async_main(controller: PendingController) -> anyhow::Result<()> {
    let controller = controller.into_controller().await?;
    let store = Store::new(controller).await?;

    let front_end = tokio::spawn(frontend::frontend(store.clone()));

    front_end.await?
}
