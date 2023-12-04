#![feature(result_option_inspect)]
#![feature(async_closure)]

use std::{
    ffi::OsStr, os::unix::prelude::PermissionsExt, path::Path, process::ExitCode, time::Duration,
};

use async_walkdir::WalkDir;
use config::Environment;
use futures_lite::StreamExt;
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
    let sb = c.new_sandbox().await?;
    sb.isolate_filesystem().await?;

    let root = Path::new("test/rootfs");
    let mut walk = WalkDir::new(root);

    tracing::trace!("copying into sandbox");
    while let Some(entry) = walk.next().await {
        let entry = entry?;
        let full_path = entry.path();
        let root_path = full_path.strip_prefix(root).unwrap();
        dbg!(root_path);

        if full_path.is_symlink() {
            let dest = tokio::fs::read_link(full_path.as_path()).await?;
            sb.symlink(dest, root_path).await?;
            continue;
        }

        let mode = entry.metadata().await?.permissions().mode();
        if full_path.is_dir() {
            sb.create_dir(full_path, mode).await?;
            continue;
        }

        dbg!(full_path.as_path());
        let mut file = OpenOptions::new()
            .read(true)
            .create(false)
            .truncate(false)
            .open(full_path.as_path())
            .await?;

        sb.write(root_path, &mut file, mode).await?;
    }

    let env = [
        (
            OsStr::new("PATH"),
            OsStr::new("/bin:/usr/bin:/sbin:/usr/sbin"),
        ),
        (OsStr::new("HOME"), OsStr::new("/no-home")),
        (OsStr::new("TERM"), OsStr::new("xterm-256color")),
        (OsStr::new("TMP"), OsStr::new("/tmp")),
        (OsStr::new("TMPDIR"), OsStr::new("/tmp")),
        (OsStr::new("TEMP"), OsStr::new("/tmp")),
        (OsStr::new("TEMPDIR"), OsStr::new("/tmp")),
    ];

    sb.create_dir("/src", 0o777).await?;
    sb.create_dir("/build/glibc-2.38", 0o777).await?;

    sb.exec(
        "/usr/bin/wget",
        [
            OsStr::new("https://ftp.gnu.org/gnu/glibc/glibc-2.38.tar.xz"),
            OsStr::new("-O"),
            OsStr::new("/src/glibc.tar.xz"),
        ],
        env,
        "/build",
    )
    .await?;

    sb.exec(
        "/usr/bin/tar",
        [
            OsStr::new("-xJvf"),
            OsStr::new("/src/glibc.tar.xz"),
            OsStr::new("-C"),
            OsStr::new("/src"),
        ],
        env,
        "/build",
    )
    .await?;

    sb.exec(
        "/src/glibc-2.38/configure",
        [
            OsStr::new("--prefix=/var/npk/store/glibc"),
            OsStr::new("--enable-kernel=4.14"),
            OsStr::new("--disable-nscd"),
            OsStr::new("CFLAGS=-g -O2 -march=x86-64-v3"),
            OsStr::new("libc_cv_slibdir=/var/npk/store/glibc/lib"),
        ],
        env,
        "/build/glibc-2.38",
    )
    .await?;

    sb.exec(
        "/usr/bin/make",
        [OsStr::new("-j"), OsStr::new("16")],
        env,
        "/build/glibc-2.38",
    )
    .await?;

    sb.exec(
        "/usr/bin/make",
        [OsStr::new("install"), OsStr::new("-j"), OsStr::new("16")],
        env,
        "/build/glibc-2.38",
    )
    .await?;

    tokio::time::sleep(Duration::from_secs(5)).await;

    drop(sb);

    Ok(())
}
