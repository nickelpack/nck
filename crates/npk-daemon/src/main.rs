#![feature(result_option_inspect)]
#![feature(async_closure)]

use std::{process::ExitCode, time::Duration};

use config::Environment;
use npk_sandbox::current::Controller;
use tokio::fs::OpenOptions;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("failed to set subscriber");

    let config = config::Config::builder()
        .add_source(Environment::with_prefix("npk").separator("__"))
        .build()
        .unwrap();
    let config = config.try_deserialize().unwrap();

    let result = npk_sandbox::current::main(config, controller_main);
    match result {
        Some(Err(error)) => {
            tracing::error!(?error, "controller failed");
            ExitCode::FAILURE
        }
        _ => ExitCode::SUCCESS,
    }
}

async fn controller_main(mut c: Controller) -> anyhow::Result<()> {
    let sb = c.spawn_sandbox().await?;
    sb.isolate_filesystem().await?;
    let mut f = OpenOptions::new()
        .read(true)
        .create(false)
        .open("/tmp/test.txt")
        .await?;
    sb.write("/tmp/test2.txt", &mut f).await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    drop(sb);

    Ok(())
}
