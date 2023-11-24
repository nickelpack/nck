#![feature(result_option_inspect)]
#![feature(async_closure)]

use std::process::ExitCode;

use config::Environment;
use npk_sandbox::current::Controller;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

fn main() -> ExitCode {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("failed to set subscriber");

    let config = config::Config::builder()
        .add_source(Environment::with_prefix("npk").separator("__"))
        .build()
        .unwrap();
    dbg!(&config);
    let config = config.try_deserialize().unwrap();

    let result = npk_sandbox::current::flavor::main(config, controller_main);
    match result {
        Some(Err(error)) => {
            tracing::error!(?error, "controller failed");
            ExitCode::FAILURE
        }
        _ => ExitCode::SUCCESS,
    }
}

async fn controller_main(mut c: Controller) -> anyhow::Result<()> {
    c.spawn_sandbox().await?;

    Ok(())
}
