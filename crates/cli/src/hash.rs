use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use nck_hashing::SupportedHasher;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use crate::CommandExec;

#[derive(Debug, Args)]
#[command(name = "hash", about = "Calculate hashes.", long_about = None)]
pub struct Hash {
    #[arg(short = 'f', long = "file", default_value = "-")]
    file: PathBuf,

    #[arg(short = 'a', long = "algorithm", default_value = "blake3")]
    algorithm: HashAlgorithm,
}

#[derive(Debug, Default, Clone, Copy, ValueEnum)]
pub enum HashAlgorithm {
    #[value(name = "blake3")]
    #[default]
    Blake3,
}

impl CommandExec for Hash {
    async fn execute(self) -> anyhow::Result<()> {
        let mut hasher = match self.algorithm {
            HashAlgorithm::Blake3 => SupportedHasher::blake3(),
        };

        let mut reader: BufReader<Box<dyn AsyncRead + Send + Unpin>> =
            if self.file.as_path() == Path::new("-") {
                BufReader::new(Box::new(tokio::io::stdin()))
            } else {
                let open = tokio::fs::OpenOptions::new()
                    .create(false)
                    .truncate(false)
                    .write(false)
                    .read(true)
                    .open(self.file)
                    .await?;
                BufReader::new(Box::new(open))
            };

        loop {
            let buf = reader.fill_buf().await?;
            let len = buf.len();
            if len == 0 {
                break;
            }

            hasher.update(buf);
            reader.consume(len);
        }

        let hash = hasher.finalize();
        println!("{hash}");

        Ok(())
    }
}
