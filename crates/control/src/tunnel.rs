//! Bounded authenticated Kubernetes API port-forward bridge; no node/runtime access.
use anyhow::Result;
use k8s_openapi::api::core::v1::Pod;
use kube::{Api, Client};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{net::TcpListener, sync::Semaphore, task::JoinHandle};
use tokio_util::sync::CancellationToken;
pub struct Tunnel {
    pub address: SocketAddr,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}
impl Tunnel {
    pub async fn open(client: Client, namespace: &str, pod: &str, port: u16) -> Result<Self> {
        let pods: Api<Pod> = Api::namespaced(client, namespace);
        let initial = pods.portforward(pod, &[port]).await?;
        initial.abort();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let pod = pod.to_owned();
        let permits = Arc::new(Semaphore::new(8));
        let task = tokio::spawn(async move {
            let mut children = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _=stop.cancelled()=>break,
                    Some(_)=children.join_next(), if !children.is_empty()=>{},
                    accepted=listener.accept()=>{
                        let Ok((mut socket,_))=accepted else{break;};
                        let Ok(permit)=permits.clone().try_acquire_owned() else{continue;};
                        let pods=pods.clone();let pod=pod.clone();let stop=stop.clone();
                        children.spawn(async move {
                            let _permit=permit;
                            tokio::select!{
                                _=stop.cancelled()=>{},
                                _=tokio::time::timeout(Duration::from_secs(180),async {
                                    if let Ok(mut forwarding)=pods.portforward(&pod,&[port]).await {
                                        if let Some(mut stream)=forwarding.take_stream(port){let _=tokio::io::copy_bidirectional(&mut socket,&mut stream).await;}
                                        forwarding.abort();
                                    }
                                })=>{}
                            }
                        });
                    }
                }
            }
            children.abort_all();
            while children.join_next().await.is_some() {}
        });
        Ok(Self {
            address,
            cancel,
            task,
        })
    }
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}
