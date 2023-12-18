#![feature(never_type)]
#![feature(lazy_cell)]
#![feature(unix_socket_ancillary_data)]

use runtime::linux::PendingController;
use tracing_subscriber::prelude::*;

mod runtime;
mod spec;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let controller = runtime::native::create_controller()?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(controller))
}

async fn async_main(controller: PendingController) -> anyhow::Result<()> {
    let controller = controller.into_controller().await?;
    let sandbox = controller.spawn_async().await?;

    Ok(())
}
