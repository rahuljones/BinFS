use std::{path::PathBuf, sync::Arc};

use binfs::{
    fuse::BinFuse,
    service::{BinFsConfig, BinFsService},
};
use clap::Parser;
use lab::lab3;
use tribbler::{err::TribResult, storage::BinStorage};

#[derive(Parser, Debug)]
#[command(name = "binfs-mount")]
struct Options {
    #[arg(long, value_delimiter = ',')]
    backs: Vec<String>,

    #[arg(long)]
    mount: PathBuf,

    #[arg(long, default_value = "__fs_meta__")]
    metadata_bin: String,

    #[arg(long, default_value_t = 65_536)]
    chunk_size: usize,

    #[arg(long, default_value_t = 128)]
    data_bins: usize,
}

fn boxed_to_arc(boxed: Box<dyn BinStorage>) -> Arc<dyn BinStorage> {
    Arc::from(boxed)
}

fn main() -> TribResult<()> {
    let options = Options::parse();
    let runtime = tokio::runtime::Runtime::new()?;
    let bins = runtime.block_on(lab3::new_bin_client(options.backs))?;
    let service = BinFsService::new(
        boxed_to_arc(bins),
        BinFsConfig {
            metadata_bin: options.metadata_bin,
            chunk_size: options.chunk_size,
            data_bins: options.data_bins,
        },
    );
    runtime
        .block_on(service.health_check())
        .map_err(|err| format!("binfs backend health check failed: {err}"))?;
    let fuse = BinFuse::new(service)?;
    fuse.mount(options.mount)?;
    Ok(())
}
