use std::{
    error::Error,
    fs::{self, File},
    future::Future,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use binfs::{BinFsConfig, BinFsService, FsOp, ROOT_ID};
use clap::{Args, Parser, Subcommand, ValueEnum};
use lab::lab3;
use tokio::sync::Barrier;
use tribbler::{
    err::TribResult,
    storage::{BinStorage, KeyValue},
};
use uuid::Uuid;

const OPS_KEY: &str = "fs:ops";
const MIB: f64 = 1024.0 * 1024.0;

type EvalResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Parser, Debug)]
#[command(name = "binfs-eval")]
#[command(about = "Repeatable correctness and performance experiments for BinFS")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Benchmark BinFS directly through the replicated Lab 3 RPC storage.
    Service(ServiceArgs),
    /// Benchmark a mounted filesystem path, such as BinFS or native APFS/ext4.
    Filesystem(FilesystemArgs),
}

#[derive(Args, Debug)]
struct ServiceArgs {
    #[arg(long, value_delimiter = ',', required = true)]
    backs: Vec<String>,

    #[arg(long, default_value = "-")]
    output: String,

    #[arg(long, value_enum, default_value_t = ServiceExperiment::All)]
    only: ServiceExperiment,

    #[arg(long, value_delimiter = ',', default_value = "0,100,1000,5000,10000")]
    metadata_entries: Vec<usize>,

    #[arg(
        long,
        value_delimiter = ',',
        default_value = "4096,65536,1048576,16777216"
    )]
    file_sizes: Vec<usize>,

    #[arg(long, value_delimiter = ',', default_value = "4096,65536,1048576")]
    chunk_sizes: Vec<usize>,

    #[arg(long, value_delimiter = ',', default_value = "1,2,4,8,16")]
    clients: Vec<usize>,

    #[arg(long, default_value_t = 30)]
    iterations: usize,

    #[arg(long, default_value_t = 5)]
    warmup: usize,

    #[arg(long, default_value_t = 128)]
    data_bins: usize,

    #[arg(long, default_value_t = 4096)]
    concurrency_file_size: usize,

    #[arg(long)]
    metadata_bin_prefix: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
enum ServiceExperiment {
    All,
    Metadata,
    File,
    Concurrency,
}

#[derive(Args, Debug)]
struct FilesystemArgs {
    #[arg(long)]
    root: PathBuf,

    #[arg(long)]
    label: String,

    #[arg(long, default_value = "-")]
    output: String,

    #[arg(
        long,
        value_delimiter = ',',
        default_value = "4096,65536,1048576,16777216"
    )]
    file_sizes: Vec<usize>,

    #[arg(long, default_value_t = 30)]
    iterations: usize,

    #[arg(long, default_value_t = 5)]
    warmup: usize,

    #[arg(long, default_value_t = 100)]
    metadata_files: usize,
}

#[derive(Default)]
struct Measurements {
    milliseconds: Vec<f64>,
    failures: usize,
    wall_seconds: f64,
}

impl Measurements {
    fn successes(&self) -> usize {
        self.milliseconds.len()
    }
}

#[derive(Default)]
struct Row<'a> {
    experiment: &'a str,
    operation: &'a str,
    label: &'a str,
    metadata_entries: Option<usize>,
    file_size_bytes: Option<usize>,
    chunk_size_bytes: Option<usize>,
    clients: Option<usize>,
    bytes_per_success: Option<usize>,
}

#[tokio::main]
async fn main() -> EvalResult<()> {
    match Cli::parse().command {
        Command::Service(args) => run_service(args).await,
        Command::Filesystem(args) => run_filesystem(args),
    }
}

async fn run_service(args: ServiceArgs) -> EvalResult<()> {
    if args.iterations == 0 {
        return Err("--iterations must be greater than zero".into());
    }
    if args.backs.is_empty() {
        return Err("at least one backend address is required".into());
    }

    let boxed = lab3::new_bin_client(args.backs.clone()).await?;
    let bins: Arc<dyn BinStorage> = Arc::from(boxed);
    let prefix = args
        .metadata_bin_prefix
        .clone()
        .unwrap_or_else(|| format!("__binfs_eval_{}__", Uuid::new_v4()));
    let mut output = open_output(&args.output)?;
    write_header(&mut output)?;

    let health = new_service(
        bins.clone(),
        format!("{prefix}_health"),
        65_536,
        args.data_bins,
    );
    health.health_check().await?;

    if matches!(
        args.only,
        ServiceExperiment::All | ServiceExperiment::Metadata
    ) {
        run_metadata_experiments(&args, bins.clone(), &prefix, &mut output).await?;
    }
    if matches!(args.only, ServiceExperiment::All | ServiceExperiment::File) {
        run_file_experiments(&args, bins.clone(), &prefix, &mut output).await?;
    }
    if matches!(
        args.only,
        ServiceExperiment::All | ServiceExperiment::Concurrency
    ) {
        run_concurrency_experiments(&args, bins, &prefix, &mut output).await?;
    }
    output.flush()?;
    Ok(())
}

async fn run_metadata_experiments(
    args: &ServiceArgs,
    bins: Arc<dyn BinStorage>,
    prefix: &str,
    output: &mut dyn Write,
) -> EvalResult<()> {
    for entries in &args.metadata_entries {
        let metadata_bin = format!("{prefix}_metadata_{entries}");
        seed_metadata(bins.clone(), &metadata_bin, *entries).await?;
        let service = new_service(bins.clone(), metadata_bin, 65_536, args.data_bins);

        let snapshot = measure_async(args.warmup, args.iterations, || {
            let service = service.clone();
            async move { service.snapshot().await.map(|_| ()) }
        })
        .await;
        write_row(
            output,
            Row {
                experiment: "metadata",
                operation: "snapshot",
                label: "rpc",
                metadata_entries: Some(*entries),
                ..Row::default()
            },
            &snapshot,
        )?;

        let list = measure_async(args.warmup, args.iterations, || {
            let service = service.clone();
            async move { service.list_dir_path("/").await.map(|_| ()) }
        })
        .await;
        write_row(
            output,
            Row {
                experiment: "metadata",
                operation: "list_root",
                label: "rpc",
                metadata_entries: Some(*entries),
                ..Row::default()
            },
            &list,
        )?;

        if *entries > 0 {
            let read = measure_async(args.warmup, args.iterations, || {
                let service = service.clone();
                async move { service.read_file_path("/file-0").await.map(|_| ()) }
            })
            .await;
            write_row(
                output,
                Row {
                    experiment: "metadata",
                    operation: "read_empty_file",
                    label: "rpc",
                    metadata_entries: Some(*entries),
                    ..Row::default()
                },
                &read,
            )?;
        }

        let mut next_directory = 0usize;
        let mkdir = measure_async(args.warmup, args.iterations, || {
            let service = service.clone();
            let name = format!("measured-dir-{next_directory}");
            next_directory += 1;
            async move { service.mkdir(ROOT_ID, &name, 0o755).await.map(|_| ()) }
        })
        .await;
        write_row(
            output,
            Row {
                experiment: "metadata",
                operation: "mkdir_growing_log",
                label: "rpc",
                metadata_entries: Some(*entries + args.warmup),
                ..Row::default()
            },
            &mkdir,
        )?;
    }
    Ok(())
}

async fn run_file_experiments(
    args: &ServiceArgs,
    bins: Arc<dyn BinStorage>,
    prefix: &str,
    output: &mut dyn Write,
) -> EvalResult<()> {
    for chunk_size in &args.chunk_sizes {
        for file_size in &args.file_sizes {
            let metadata_bin = format!("{prefix}_file_{chunk_size}_{file_size}");
            let service = new_service(bins.clone(), metadata_bin, *chunk_size, args.data_bins);
            let file = service
                .create_file(ROOT_ID, "bench-file", 0o644, false)
                .await?;
            let data = deterministic_data(*file_size);

            let writes = measure_async(args.warmup, args.iterations, || {
                let service = service.clone();
                let object = file.id.clone();
                let data = data.clone();
                async move { service.commit_file(&object, &data).await.map(|_| ()) }
            })
            .await;
            write_row(
                output,
                Row {
                    experiment: "file",
                    operation: "write",
                    label: "rpc",
                    file_size_bytes: Some(*file_size),
                    chunk_size_bytes: Some(*chunk_size),
                    bytes_per_success: Some(*file_size),
                    ..Row::default()
                },
                &writes,
            )?;

            let reads = measure_async(args.warmup, args.iterations, || {
                let service = service.clone();
                let object = file.id.clone();
                let expected = data.clone();
                async move {
                    let actual = service.read_file(&object).await?;
                    if actual != expected {
                        return Err(binfs::FsError::new(libc::EIO, "read data mismatch"));
                    }
                    Ok(())
                }
            })
            .await;
            write_row(
                output,
                Row {
                    experiment: "file",
                    operation: "read",
                    label: "rpc",
                    file_size_bytes: Some(*file_size),
                    chunk_size_bytes: Some(*chunk_size),
                    bytes_per_success: Some(*file_size),
                    ..Row::default()
                },
                &reads,
            )?;
        }
    }
    Ok(())
}

async fn run_concurrency_experiments(
    args: &ServiceArgs,
    bins: Arc<dyn BinStorage>,
    prefix: &str,
    output: &mut dyn Write,
) -> EvalResult<()> {
    let payload = deterministic_data(args.concurrency_file_size);
    for clients in &args.clients {
        if *clients == 0 {
            continue;
        }
        let metadata_bin = format!("{prefix}_concurrency_{clients}");
        let service = new_service(bins.clone(), metadata_bin, 65_536, args.data_bins);

        for round in 0..args.warmup {
            let _ = concurrency_round(
                service.clone(),
                *clients,
                format!("warmup-{round}"),
                payload.clone(),
            )
            .await;
        }

        let mut measurements = Measurements::default();
        for round in 0..args.iterations {
            let current = concurrency_round(
                service.clone(),
                *clients,
                format!("measured-{round}"),
                payload.clone(),
            )
            .await;
            measurements
                .milliseconds
                .extend(current.milliseconds.into_iter());
            measurements.failures += current.failures;
            measurements.wall_seconds += current.wall_seconds;
        }

        write_row(
            output,
            Row {
                experiment: "concurrency",
                operation: "create_and_write_distinct",
                label: "rpc",
                file_size_bytes: Some(args.concurrency_file_size),
                chunk_size_bytes: Some(65_536),
                clients: Some(*clients),
                bytes_per_success: Some(args.concurrency_file_size),
                ..Row::default()
            },
            &measurements,
        )?;
    }
    Ok(())
}

async fn concurrency_round(
    service: BinFsService,
    clients: usize,
    round: String,
    payload: Vec<u8>,
) -> Measurements {
    let barrier = Arc::new(Barrier::new(clients + 1));
    let mut tasks = Vec::with_capacity(clients);
    for client in 0..clients {
        let service = service.clone();
        let barrier = barrier.clone();
        let path = format!("/{round}-client-{client}");
        let payload = payload.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            let start = Instant::now();
            let result = service.write_file_path(&path, &payload).await;
            (start.elapsed().as_secs_f64() * 1000.0, result.is_ok())
        }));
    }

    let wall_start = Instant::now();
    barrier.wait().await;
    let mut measurements = Measurements::default();
    for task in tasks {
        match task.await {
            Ok((milliseconds, true)) => measurements.milliseconds.push(milliseconds),
            Ok((_, false)) | Err(_) => measurements.failures += 1,
        }
    }
    measurements.wall_seconds = wall_start.elapsed().as_secs_f64();
    measurements
}

async fn seed_metadata(
    bins: Arc<dyn BinStorage>,
    metadata_bin: &str,
    entries: usize,
) -> TribResult<()> {
    let storage = bins.bin(metadata_bin).await?;
    for index in 0..entries {
        let operation = FsOp::CreateFile {
            op_id: format!("seed-op-{index}"),
            parent: ROOT_ID.to_string(),
            name: format!("file-{index}"),
            object: format!("seed-file-{index}"),
            mode: 0o644,
            mtime_ms: index as u64,
        };
        storage
            .list_append(&KeyValue {
                key: OPS_KEY.to_string(),
                value: serde_json::to_string(&operation)?,
            })
            .await?;
    }
    Ok(())
}

fn new_service(
    bins: Arc<dyn BinStorage>,
    metadata_bin: String,
    chunk_size: usize,
    data_bins: usize,
) -> BinFsService {
    BinFsService::new(
        bins,
        BinFsConfig {
            metadata_bin,
            chunk_size,
            data_bins,
        },
    )
}

fn run_filesystem(args: FilesystemArgs) -> EvalResult<()> {
    if args.iterations == 0 {
        return Err("--iterations must be greater than zero".into());
    }
    if !args.root.is_dir() {
        return Err(format!("filesystem root does not exist: {}", args.root.display()).into());
    }

    let mut output = open_output(&args.output)?;
    write_header(&mut output)?;
    let work_dir = args.root.join(format!(".binfs-eval-{}", Uuid::new_v4()));
    fs::create_dir(&work_dir)?;

    let result = run_filesystem_experiments(&args, &work_dir, &mut output);
    let cleanup_result = fs::remove_dir_all(&work_dir);
    result?;
    cleanup_result?;
    output.flush()?;
    Ok(())
}

fn run_filesystem_experiments(
    args: &FilesystemArgs,
    work_dir: &Path,
    output: &mut dyn Write,
) -> EvalResult<()> {
    for file_size in &args.file_sizes {
        let path = work_dir.join(format!("file-{file_size}"));
        let data = deterministic_data(*file_size);

        let writes = measure_sync(args.warmup, args.iterations, || fs::write(&path, &data));
        write_row(
            output,
            Row {
                experiment: "filesystem",
                operation: "write",
                label: &args.label,
                file_size_bytes: Some(*file_size),
                bytes_per_success: Some(*file_size),
                ..Row::default()
            },
            &writes,
        )?;

        let reads = measure_sync(args.warmup, args.iterations, || {
            let actual = fs::read(&path)?;
            if actual != data {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "read data mismatch",
                ));
            }
            Ok(())
        });
        write_row(
            output,
            Row {
                experiment: "filesystem",
                operation: "read",
                label: &args.label,
                file_size_bytes: Some(*file_size),
                bytes_per_success: Some(*file_size),
                ..Row::default()
            },
            &reads,
        )?;
    }

    let metadata_dir = work_dir.join("metadata");
    fs::create_dir(&metadata_dir)?;
    for index in 0..args.metadata_files {
        fs::write(metadata_dir.join(format!("file-{index}")), [])?;
    }
    let listings = measure_sync(args.warmup, args.iterations, || {
        let mut count = 0;
        for entry in fs::read_dir(&metadata_dir)? {
            let entry = entry?;
            if !entry.file_name().to_string_lossy().starts_with("._") {
                count += 1;
            }
        }
        if count != args.metadata_files {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected {} entries, found {count}", args.metadata_files),
            ));
        }
        Ok(())
    });
    write_row(
        output,
        Row {
            experiment: "filesystem",
            operation: "list_directory",
            label: &args.label,
            metadata_entries: Some(args.metadata_files),
            ..Row::default()
        },
        &listings,
    )?;
    Ok(())
}

async fn measure_async<T, E, F, Fut>(
    warmup: usize,
    iterations: usize,
    mut operation: F,
) -> Measurements
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    for _ in 0..warmup {
        let _ = operation().await;
    }

    let mut measurements = Measurements::default();
    for _ in 0..iterations {
        let start = Instant::now();
        let result = operation().await;
        let elapsed = start.elapsed().as_secs_f64();
        measurements.wall_seconds += elapsed;
        if result.is_ok() {
            measurements.milliseconds.push(elapsed * 1000.0);
        } else {
            measurements.failures += 1;
        }
    }
    measurements
}

fn measure_sync<T, E, F>(warmup: usize, iterations: usize, mut operation: F) -> Measurements
where
    F: FnMut() -> Result<T, E>,
{
    for _ in 0..warmup {
        let _ = operation();
    }

    let mut measurements = Measurements::default();
    for _ in 0..iterations {
        let start = Instant::now();
        let result = operation();
        let elapsed = start.elapsed().as_secs_f64();
        measurements.wall_seconds += elapsed;
        if result.is_ok() {
            measurements.milliseconds.push(elapsed * 1000.0);
        } else {
            measurements.failures += 1;
        }
    }
    measurements
}

fn deterministic_data(size: usize) -> Vec<u8> {
    (0..size)
        .map(|index| ((index.wrapping_mul(31).wrapping_add(17)) % 251) as u8)
        .collect()
}

fn open_output(path: &str) -> io::Result<Box<dyn Write>> {
    if path == "-" {
        return Ok(Box::new(io::stdout()));
    }
    let path = Path::new(path);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    Ok(Box::new(File::create(path)?))
}

fn write_header(output: &mut dyn Write) -> io::Result<()> {
    writeln!(
        output,
        "experiment,operation,label,metadata_entries,file_size_bytes,chunk_size_bytes,clients,samples,successes,failures,p50_ms,p95_ms,p99_ms,mean_ms,throughput_ops_s,throughput_mib_s"
    )
}

fn write_row(output: &mut dyn Write, row: Row<'_>, measurements: &Measurements) -> io::Result<()> {
    let mut sorted = measurements.milliseconds.clone();
    sorted.sort_by(f64::total_cmp);
    let successes = measurements.successes();
    let samples = successes + measurements.failures;
    let mean = if successes == 0 {
        0.0
    } else {
        sorted.iter().sum::<f64>() / successes as f64
    };
    let throughput_ops = if measurements.wall_seconds > 0.0 {
        successes as f64 / measurements.wall_seconds
    } else {
        0.0
    };
    let throughput_mib = row
        .bytes_per_success
        .map(|bytes| throughput_ops * bytes as f64 / MIB)
        .unwrap_or(0.0);

    writeln!(
        output,
        "{},{},{},{},{},{},{},{},{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
        row.experiment,
        row.operation,
        row.label,
        optional(row.metadata_entries),
        optional(row.file_size_bytes),
        optional(row.chunk_size_bytes),
        optional(row.clients),
        samples,
        successes,
        measurements.failures,
        percentile(&sorted, 50.0),
        percentile(&sorted, 95.0),
        percentile(&sorted, 99.0),
        mean,
        throughput_ops,
        throughput_mib,
    )
}

fn optional(value: Option<usize>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((percentile / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}
