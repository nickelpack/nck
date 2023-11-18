#![feature(result_option_inspect)]
use std::time::Duration;

use tracing::Level;
use tracing_subscriber::FmtSubscriber;

fn main() {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("failed to set subscriber");

    let result = npk_sandbox::current::flavor::ControllerSpawner::new("/tmp/sock4").unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        match result.start().await.unwrap() {
            npk_sandbox::current::flavor::ControllerType::Main(m) => m.join().await,
            npk_sandbox::current::flavor::ControllerType::Zygote(z) => z.join().await,
        }
    });
}
