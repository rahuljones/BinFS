use std::future::pending;

use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{
    transport::{Channel, Server},
    Request, Response, Status,
};
use tribbler::{
    config::BackConfig,
    err::TribResult,
    rpc::{
        trib_storage_client::TribStorageClient,
        trib_storage_server::{TribStorage, TribStorageServer},
        Bool, Clock, Key, KeyValue as RpcKeyValue, ListRemoveResponse, Pattern as RpcPattern,
        StringList, Value,
    },
    storage::{KeyValue, Pattern, Storage},
};

use crate::lab3::backend::BackEnd;

pub async fn new_client(addr: &str) -> TribResult<Box<dyn Storage>> {
    let channel = Channel::from_shared(addr.to_string())?.connect_lazy();
    Ok(Box::new(BackEnd {
        client: TribStorageClient::new(channel),
    }))
}

pub async fn serve_back(bc: BackConfig) -> TribResult<()> {
    let listener = TcpListener::bind(&bc.addr).await?;
    if let Some(ready) = bc.ready {
        ready.send(true)?;
    }

    let service = TribStorageServer::new(StorageService {
        storage: bc.storage,
    });

    match bc.shutdown {
        Some(mut shutdown) => {
            Server::builder()
                .add_service(service)
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async move {
                    let _ = shutdown.recv().await;
                })
                .await?;
        }
        None => {
            Server::builder()
                .add_service(service)
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), pending())
                .await?;
        }
    }

    Ok(())
}

struct StorageService {
    storage: Box<dyn Storage>,
}

fn storage_error(err: Box<dyn std::error::Error + Send + Sync>) -> Status {
    Status::internal(err.to_string())
}

#[tonic::async_trait]
impl TribStorage for StorageService {
    async fn get(&self, request: Request<Key>) -> Result<Response<Value>, Status> {
        let value = self
            .storage
            .get(&request.into_inner().key)
            .await
            .map_err(storage_error)?
            .unwrap_or_default();
        Ok(Response::new(Value { value }))
    }

    async fn set(&self, request: Request<RpcKeyValue>) -> Result<Response<Bool>, Status> {
        let kv = request.into_inner();
        let value = self
            .storage
            .set(&KeyValue {
                key: kv.key,
                value: kv.value,
            })
            .await
            .map_err(storage_error)?;
        Ok(Response::new(Bool { value }))
    }

    async fn keys(&self, request: Request<RpcPattern>) -> Result<Response<StringList>, Status> {
        let pattern = request.into_inner();
        let list = self
            .storage
            .keys(&Pattern {
                prefix: pattern.prefix,
                suffix: pattern.suffix,
            })
            .await
            .map_err(storage_error)?
            .0;
        Ok(Response::new(StringList { list }))
    }

    async fn list_get(&self, request: Request<Key>) -> Result<Response<StringList>, Status> {
        let list = self
            .storage
            .list_get(&request.into_inner().key)
            .await
            .map_err(storage_error)?
            .0;
        Ok(Response::new(StringList { list }))
    }

    async fn list_append(&self, request: Request<RpcKeyValue>) -> Result<Response<Bool>, Status> {
        let kv = request.into_inner();
        let value = self
            .storage
            .list_append(&KeyValue {
                key: kv.key,
                value: kv.value,
            })
            .await
            .map_err(storage_error)?;
        Ok(Response::new(Bool { value }))
    }

    async fn list_remove(
        &self,
        request: Request<RpcKeyValue>,
    ) -> Result<Response<ListRemoveResponse>, Status> {
        let kv = request.into_inner();
        let removed = self
            .storage
            .list_remove(&KeyValue {
                key: kv.key,
                value: kv.value,
            })
            .await
            .map_err(storage_error)?;
        Ok(Response::new(ListRemoveResponse { removed }))
    }

    async fn list_keys(&self, request: Request<RpcPattern>) -> Result<Response<StringList>, Status> {
        let pattern = request.into_inner();
        let list = self
            .storage
            .list_keys(&Pattern {
                prefix: pattern.prefix,
                suffix: pattern.suffix,
            })
            .await
            .map_err(storage_error)?
            .0;
        Ok(Response::new(StringList { list }))
    }

    async fn clock(&self, request: Request<Clock>) -> Result<Response<Clock>, Status> {
        let timestamp = self
            .storage
            .clock(request.into_inner().timestamp)
            .await
            .map_err(storage_error)?;
        Ok(Response::new(Clock { timestamp }))
    }
}
