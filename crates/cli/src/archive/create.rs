use std::{collections::VecDeque, os::unix::prelude::PermissionsExt, path::PathBuf};

use clap::Args;
use nck_archive::{Entry, EntryFlags, Writer};
use nck_hashing::SupportedHasher;
use tokio::{
    fs::File,
    io::{AsyncWrite, BufWriter},
};

use crate::CommandExec;

#[derive(Debug, Args)]
#[command(name = "create", about = "Creates a nck archive.", long_about = None)]
pub struct Cli {
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,

    #[arg(short = 'C')]
    directory: Option<PathBuf>,

    #[arg(last = true)]
    files: Vec<PathBuf>,
}

type DynWriter = BufWriter<Box<dyn AsyncWrite + Send + Unpin>>;

impl CommandExec for Cli {
    async fn execute(mut self) -> anyhow::Result<()> {
        let writer: BufWriter<Box<dyn AsyncWrite + Send + Unpin>> = match self.output {
            Some(v) => {
                let open = tokio::fs::OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .write(true)
                    .read(false)
                    .append(false)
                    .open(v)
                    .await?;
                BufWriter::new(Box::new(open))
            }
            None => BufWriter::new(Box::new(tokio::io::stdout())),
        };
        let writer = Writer::new_async(writer).await?;
        let cwd = std::env::current_dir()?;
        let directory = self
            .directory
            .map(|v| if v.has_root() { v } else { cwd.join(v) })
            .unwrap_or(cwd);

        let (data_send, mut data_recv) =
            tokio::sync::mpsc::channel::<(PathBuf, File, EntryFlags)>(20);
        let (entry_send, mut entry_recv) = tokio::sync::mpsc::unbounded_channel::<Entry>();
        let writer_task: tokio::task::JoinHandle<std::io::Result<Writer<DynWriter>>> = {
            let entry_send = entry_send.clone();
            tokio::spawn(async move {
                let mut writer = writer;

                while let Some((path, mut file, flags)) = data_recv.recv().await {
                    let hash = SupportedHasher::blake3();
                    let mut w = writer.write_data_async(hash).await?;
                    tokio::io::copy(&mut file, &mut w).await?;
                    let (w, hash) = w.finish_async().await?;
                    writer = w;

                    if entry_send
                        .send(Entry::data(path, hash, Some(flags)))
                        .is_err()
                    {
                        break;
                    }
                }

                Ok(writer)
            })
        };

        self.files.sort_unstable();
        let mut files = VecDeque::from(self.files);
        while let Some(file) = files.pop_front() {
            let full = if file.has_root() {
                file.clone()
            } else {
                directory.join(file.as_path())
            };

            let stat = tokio::fs::symlink_metadata(full.as_path()).await?;

            let mut flags = EntryFlags::empty();
            if (stat.permissions().mode() & 0o111) != 0 {
                flags |= EntryFlags::EXECUTABLE;
            }

            if stat.is_dir() {
                if entry_send.send(Entry::directory(file.as_path())).is_err() {
                    break;
                }

                let mut nested = Vec::new();
                let mut entries = tokio::fs::read_dir(full.as_path()).await?;
                while let Some(f) = entries.next_entry().await? {
                    let f = f.path();
                    let f = f
                        .strip_prefix(full.as_path()) // Remove the full path
                        .map(|v| file.join(v)) // Re-add the directory path
                        .unwrap_or(f);
                    nested.push(f);
                }

                nested.sort_unstable();
                files.extend(nested.into_iter());
            } else if stat.is_file() {
                let open = tokio::fs::OpenOptions::new()
                    .create(false)
                    .truncate(false)
                    .write(false)
                    .read(true)
                    .append(false)
                    .open(full.as_path())
                    .await?;
                if data_send.send((file, open, flags)).await.is_err() {
                    break;
                }
            } else if stat.is_symlink() {
                let src = tokio::fs::read_link(full).await?;
                if entry_send
                    .send(Entry::link(file, src, Some(flags)))
                    .is_err()
                {
                    break;
                }
            }
        }

        drop(data_send);
        drop(entry_send);

        let mut writer = writer_task.await??;
        while let Some(entry) = entry_recv.recv().await {
            writer.write_entry_async(entry).await?;
        }

        Ok(())
    }
}
