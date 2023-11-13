use sandbox::SandboxOptions;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

mod ipc;
mod proto;
mod sandbox;

fn main() {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("failed to set subscriber");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        sandbox::start(SandboxOptions::new(Path::new("/tmp/npk-build/a"))).await;
    });
}
