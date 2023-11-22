#![feature(result_option_inspect)]
#![feature(async_closure)]

use std::{path::PathBuf, process::ExitCode};

use npk_sandbox::current::{flavor::Config, Mappings};
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

fn main() -> ExitCode {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("failed to set subscriber");

    let mut mappings = Mappings::default();
    mappings
        .push_uid_range(0, 165537..=166537)
        .unwrap()
        .push_gid_range(0, 165537..=166537)
        .unwrap();
    npk_sandbox::current::flavor::main(
        Config {
            working_dir: PathBuf::from("/tmp/npk4"),
            mappings,
        },
        async move |mut c| {
            c.spawn_sandbox().await?;

            Ok(())
        },
    )
}

fn controller_main() {}
