use std::{
    net::TcpListener,
    sync::{mpsc, Arc},
    time::Duration,
};

use binfs::{BinFsConfig, BinFsService, FsResult};
use lab::{lab1, lab3};
use tokio::{sync::mpsc as tokio_mpsc, task::JoinHandle};
use tribbler::{config::BackConfig, storage::MemStorage};

struct RpcCluster {
    addresses: Vec<String>,
    shutdown: Vec<tokio_mpsc::Sender<()>>,
    tasks: Vec<JoinHandle<()>>,
}

impl RpcCluster {
    async fn start(count: usize) -> Self {
        let addresses = (0..count).map(|_| unused_address()).collect::<Vec<_>>();
        let (ready_tx, ready_rx) = mpsc::channel();
        let mut shutdown = Vec::new();
        let mut tasks = Vec::new();

        for address in &addresses {
            let (shutdown_tx, shutdown_rx) = tokio_mpsc::channel(1);
            shutdown.push(shutdown_tx);
            let config = BackConfig {
                addr: address.clone(),
                storage: Box::new(MemStorage::default()),
                ready: Some(ready_tx.clone()),
                shutdown: Some(shutdown_rx),
            };
            tasks.push(tokio::spawn(async move {
                lab1::serve_back(config).await.unwrap();
            }));
        }

        for _ in 0..count {
            assert!(ready_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        }
        Self {
            addresses,
            shutdown,
            tasks,
        }
    }

    async fn stop(self) {
        for sender in self.shutdown {
            let _ = sender.send(()).await;
        }
        for task in self.tasks {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap();
        }
    }
}

fn unused_address() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn service_runs_through_replicated_rpc_bins() -> FsResult<()> {
    let cluster = RpcCluster::start(3).await;
    let bins = lab3::new_bin_client(cluster.addresses.clone())
        .await
        .unwrap();
    let fs = BinFsService::new(
        Arc::from(bins),
        BinFsConfig {
            metadata_bin: "__rpc_test_meta__".to_string(),
            chunk_size: 8,
            data_bins: 8,
        },
    );

    fs.mkdir_path("/rpc", 0o755).await?;
    fs.write_file_path("/rpc/file", b"rpc round trip").await?;
    assert_eq!(fs.read_file_path("/rpc/file").await?, b"rpc round trip");
    assert_eq!(fs.list_dir_path("/rpc").await?, vec!["file".to_string()]);
    fs.unlink_path("/rpc/file").await?;
    fs.rmdir_path("/rpc").await?;

    cluster.stop().await;
    Ok(())
}
