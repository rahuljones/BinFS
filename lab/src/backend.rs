use tonic::{async_trait, transport::Channel};
use tribbler::{
    err::TribResult,
    rpc::{
        trib_storage_client::TribStorageClient, Clock, Key, KeyValue as RpcKeyValue,
        Pattern as RpcPattern,
    },
    storage::{BinStorage, KeyList, KeyString, KeyValue, List, Pattern, Storage},
};

use crate::lab3::bin::Bin;

#[derive(Default)]
pub struct BinStorageClient {
    pub backs: Vec<String>,
    pub backends: Vec<BackEnd>,
}

impl BinStorageClient {
    pub async fn new(backs: Vec<String>) -> TribResult<Self> {
        let mut bins: Vec<BackEnd> = Vec::new();
        for back in &backs {
            // in case backend isn't online
            let channel = Channel::from_shared(back.clone())?.connect_lazy();
            let client = TribStorageClient::new(channel);
            bins.push(BackEnd { client });
        }

        Ok(BinStorageClient {
            backs,
            backends: bins,
        })
    }
}

#[async_trait]
impl BinStorage for BinStorageClient {
    async fn bin(&self, name: &str) -> TribResult<Box<dyn Storage>> {
        Ok(Box::new(Bin {
            backends: self.backends.clone(),
            bin_name: name.to_string(),
        }))
    }
}

#[derive(Clone)]
pub struct BackEnd {
    pub client: TribStorageClient<Channel>,
}

#[async_trait]
impl KeyString for BackEnd {
    async fn get(&self, key: &str) -> TribResult<Option<String>> {
        let mut client = self.client.clone();
        let r = client
            .get(Key {
                key: key.to_string(),
            })
            .await?;

        let value = r.into_inner().value;
        if value.is_empty() {
            Ok(None)
        } else {
            Ok(Some(value))
        }
    }

    async fn set(&self, kv: &KeyValue) -> TribResult<bool> {
        let mut client = self.client.clone();
        let r = client
            .set(RpcKeyValue {
                key: kv.key.clone(),
                value: kv.value.clone(),
            })
            .await?;

        Ok(r.into_inner().value)
    }

    async fn keys(&self, p: &Pattern) -> TribResult<List> {
        let mut client = self.client.clone();
        let r = client
            .keys(RpcPattern {
                prefix: p.prefix.clone(),
                suffix: p.suffix.clone(),
            })
            .await?;

        Ok(List(r.into_inner().list))
    }
}

#[async_trait]
impl KeyList for BackEnd {
    async fn list_get(&self, key: &str) -> TribResult<List> {
        let mut client = self.client.clone();
        let r = client
            .list_get(Key {
                key: key.to_string(),
            })
            .await?;

        Ok(List(r.into_inner().list))
    }

    async fn list_append(&self, kv: &KeyValue) -> TribResult<bool> {
        let mut client = self.client.clone();
        let r = client
            .list_append(RpcKeyValue {
                key: kv.key.clone(),
                value: kv.value.clone(),
            })
            .await?;

        Ok(r.into_inner().value)
    }

    async fn list_remove(&self, kv: &KeyValue) -> TribResult<u32> {
        let mut client = self.client.clone();
        let r = client
            .list_remove(RpcKeyValue {
                key: kv.key.clone(),
                value: kv.value.clone(),
            })
            .await?;

        Ok(r.into_inner().removed)
    }

    async fn list_keys(&self, p: &Pattern) -> TribResult<List> {
        let mut client = self.client.clone();
        let r = client
            .list_keys(RpcPattern {
                prefix: p.prefix.clone(),
                suffix: p.suffix.clone(),
            })
            .await?;

        Ok(List(r.into_inner().list))
    }
}

#[async_trait]
impl Storage for BackEnd {
    async fn clock(&self, at_least: u64) -> TribResult<u64> {
        let mut client = self.client.clone();
        let r = client
            .clock(Clock {
                timestamp: at_least,
            })
            .await?;

        Ok(r.into_inner().timestamp)
    }
}
