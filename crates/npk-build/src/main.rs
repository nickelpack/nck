#![feature(result_option_inspect)]
#![feature(async_closure)]

use std::{path::PathBuf, process::ExitCode};

use npk_sandbox::current::{flavor::Config, Controller, Mappings};
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

fn main() -> ExitCode {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("failed to set subscriber");

    let mut mappings = Mappings::default();
    mappings
        .push_uid_range(0, 165537..=165538)
        .unwrap()
        .push_gid_range(0, 165537..=165538)
        .unwrap()
        .push_uid_range(1000, 165538..=166539)
        .unwrap()
        .push_gid_range(1000, 165538..=165539)
        .unwrap();
    if let Err(err) = npk_sandbox::current::flavor::main(
        Config {
            working_dir: PathBuf::from("/tmp/npk4"),
            mappings,
        },
        Ok(()),
        async move |mut c| controller_main(&mut c).await,
    ) {
        eprintln!("failed: {:?}", err);
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

async fn controller_main(c: &mut Controller) -> anyhow::Result<()> {
    c.spawn_sandbox().await?;

    Ok(())
}
