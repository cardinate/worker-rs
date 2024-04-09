use std::env;

use anyhow::{anyhow, Context, Result};
use camino::Utf8PathBuf as PathBuf;
use futures::{Stream, StreamExt};
use subsquid_messages::{
    envelope::Msg, query_executed, signatures::SignedMessage, DatasetRanges, InputAndOutput,
    LogsCollected, Ping, Pong, ProstMsg, Query, QueryExecuted, SizeAndHash,
};
use subsquid_network_transport::{
    cli::TransportArgs,
    transport::{P2PTransportBuilder, P2PTransportHandle},
    Keypair, PeerId,
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_stream::wrappers::{ReceiverStream, WatchStream};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
    logs_storage::LogsStorage,
    query::{error::QueryError, result::QueryResult},
    types::state::Ranges,
    util::{hash::sha3_256, UseOnce},
};

use super::QueryTask;

type MsgContent = Vec<u8>;
type Message = subsquid_network_transport::Message<MsgContent>;
const PING_TOPIC: &str = "worker_ping";
const LOGS_TOPIC: &str = "worker_query_logs";
const SERVICE_QUEUE_SIZE: usize = 16;
const CONCURRENT_MESSAGES: usize = 32;
const LOGS_SEND_INTERVAL_SEC: u64 = 600;

pub struct P2PTransport<MsgStream> {
    raw_msg_stream: UseOnce<MsgStream>,
    transport_handle: P2PTransportHandle<MsgContent>,
    logs_storage: LogsStorage,
    scheduler_id: PeerId,
    logs_collector_id: PeerId,
    worker_id: PeerId,
    keypair: Keypair,
    assignments_tx: watch::Sender<Ranges>,
    assignments_rx: UseOnce<watch::Receiver<Ranges>>,
    queries_tx: mpsc::Sender<QueryTask>,
    queries_rx: UseOnce<mpsc::Receiver<QueryTask>>,
}

pub async fn create_p2p_transport(
    args: TransportArgs,
    scheduler_id: String,
    logs_collector_id: String,
    logs_db_path: PathBuf,
) -> Result<P2PTransport<impl Stream<Item = Message>>> {
    let transport_builder = P2PTransportBuilder::from_cli(args).await?;
    let worker_id = transport_builder.local_peer_id();
    let keypair = transport_builder.keypair();
    info!("Local peer ID: {worker_id}");

    let (assignments_tx, assignments_rx) = watch::channel(Default::default());
    let (queries_tx, queries_rx) = mpsc::channel(SERVICE_QUEUE_SIZE);
    let (msg_receiver, transport_handle) = transport_builder.run().await?;
    transport_handle.subscribe(PING_TOPIC).await?;
    transport_handle.subscribe(LOGS_TOPIC).await?;

    Ok(P2PTransport {
        raw_msg_stream: UseOnce::new(msg_receiver),
        transport_handle,
        logs_storage: LogsStorage::new(logs_db_path.as_str()).await?,
        scheduler_id: scheduler_id
            .parse()
            .context("Couldn't parse scheduler id")?,
        logs_collector_id: logs_collector_id
            .parse()
            .context("Couldn't parse logs collector id")?,
        worker_id,
        keypair,
        assignments_tx,
        assignments_rx: UseOnce::new(assignments_rx),
        queries_tx,
        queries_rx: UseOnce::new(queries_rx),
    })
}

impl<MsgStream: Stream<Item = Message>> P2PTransport<MsgStream> {
    async fn run_receive_loop(&self, cancellation_token: CancellationToken) {
        let msg_stream = self.raw_msg_stream.take().unwrap();
        msg_stream
            .take_until(cancellation_token.cancelled_owned())
            .for_each_concurrent(CONCURRENT_MESSAGES, |msg| async move {
                let envelope = match subsquid_messages::Envelope::decode(msg.content.as_slice()) {
                    Ok(envelope) => envelope,
                    Err(e) => {
                        warn!("Couldn't parse p2p message: {e}");
                        return;
                    }
                };
                let peer_id = match msg.peer_id {
                    Some(peer_id) => peer_id,
                    None => {
                        warn!("Received p2p message without peer_id: '{:?}'", msg);
                        return;
                    }
                };
                match envelope.msg {
                    Some(Msg::Pong(pong)) => {
                        self.handle_pong(peer_id, pong);
                    }
                    Some(Msg::Query(query)) => {
                        self.handle_query(peer_id, query).await;
                    }
                    Some(Msg::LogsCollected(collected)) => {
                        self.handle_logs_collected(collected, peer_id).await;
                    }
                    _ => {
                        // ignore all other events
                    }
                }
            })
            .await;
    }

    async fn run_send_logs_loop(
        &self,
        cancellation_token: CancellationToken,
        interval: std::time::Duration,
    ) {
        let mut timer = tokio::time::interval(interval);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select!(
                _ = timer.tick() => {},
                _ = cancellation_token.cancelled() => {
                    break;
                },
            );

            let logs = {
                if !self.logs_storage.is_initialized() {
                    continue;
                }
                match self.logs_storage.get_logs().await {
                    Ok(logs) => logs,
                    Err(e) => {
                        warn!("Couldn't get logs from storage: {e:?}");
                        continue;
                    }
                }
            };

            info!("Sending {} logs to the logs collector", logs.len());
            let envelope = subsquid_messages::Envelope {
                msg: Some(Msg::QueryLogs(subsquid_messages::QueryLogs {
                    queries_executed: logs,
                })),
            };
            // TODO: limit message size
            let result = self
                .transport_handle
                .broadcast_msg(envelope.encode_to_vec(), LOGS_TOPIC);
            if let Err(e) = result {
                panic!("Couldn't send logs: {e:?}");
            }
        }
    }

    fn handle_pong(&self, peer_id: PeerId, pong: Pong) {
        use subsquid_messages::pong::Status;
        if peer_id != self.scheduler_id {
            warn!("Wrong pong message origin: '{}'", peer_id.to_string());
        }
        match pong.status {
            Some(Status::NotRegistered(())) => {
                error!("Worker not registered on chain");
            }
            Some(Status::UnsupportedVersion(())) => {
                error!("Worker version not supported by the scheduler");
            }
            Some(Status::JailedV1(())) => {
                warn!("Worker jailed until the end of epoch");
            }
            Some(Status::Jailed(reason)) => {
                warn!("Worker jailed until the end of epoch: {reason}");
            }
            Some(Status::Active(assignment)) => {
                self.assignments_tx
                    .send(assignment.datasets)
                    .expect("Assignment subscriber dropped");
            }
            None => {
                warn!("Invalid pong message: no status field");
            }
        }
    }

    async fn handle_logs_collected(&self, collected: LogsCollected, peer_id: PeerId) {
        if peer_id != self.logs_collector_id {
            warn!("Wrong LogsCollected message origin: {peer_id}");
            return;
        }
        let last_collected_seq_no = collected
            .sequence_numbers
            .get(&self.worker_id.to_base58())
            .map(|&x| x as usize);
        self.logs_storage
            .logs_collected(last_collected_seq_no)
            .await;
    }

    // Completes only when the query is processed and the result is sent
    async fn handle_query(&self, peer_id: PeerId, mut query: Query) {
        if !query.verify_signature(&peer_id) {
            warn!(
                "Query with invalid signature received from {}",
                peer_id.to_string()
            );
            return;
        }
        let query_id = if let Some(query_id) = query.query_id.clone() {
            query_id
        } else {
            warn!(
                "Query without query_id received from {}",
                peer_id.to_string()
            );
            return;
        };

        if !self.logs_storage.is_initialized() {
            warn!("Logs storage not initialized. Cannot execute queries yet.");
            return;
        }

        let result = self.process_query(peer_id, &query).await;
        if let Err(e) = &result {
            warn!("Query {query_id} execution failed: {e:?}");
        }

        let log = if let Err(QueryError::NoAllocation) = result {
            None
        } else {
            Some(self.generate_log(&result, query, peer_id))
        };
        self.send_query_result(query_id, peer_id, result).await;
        if let Some(log) = log {
            let result = self.logs_storage.save_log(log).await;
            if let Err(e) = result {
                warn!("Couldn't save query log: {e:?}");
            }
        }
    }

    async fn process_query(
        &self,
        peer_id: PeerId,
        query: &Query,
    ) -> std::result::Result<QueryResult, QueryError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        if let (Some(dataset), Some(query_str)) = (&query.dataset, &query.query) {
            let query = serde_json::from_str(query_str).map_err(anyhow::Error::from)?;
            match self.queries_tx.try_send(QueryTask {
                dataset: dataset.clone(),
                peer_id,
                query,
                response_sender: resp_tx,
            }) {
                Err(mpsc::error::TrySendError::Full(_)) => {
                    Err(anyhow!("Service overloaded"))?;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    panic!("Query subscriber dropped");
                }
                _ => {}
            }
        } else {
            Err(anyhow!("Some fields are missing in proto message"))?;
        }
        resp_rx
            .await
            .expect("Query processor didn't produce a result")
    }

    async fn send_query_result(
        &self,
        query_id: String,
        peer_id: PeerId,
        result: std::result::Result<QueryResult, QueryError>,
    ) {
        use subsquid_messages::query_result;
        let query_result = match result {
            Ok(result) => query_result::Result::Ok(subsquid_messages::OkResult {
                data: result.compressed_data,
                exec_plan: None,
            }),
            Err(e @ QueryError::NotFound) => query_result::Result::BadRequest(e.to_string()),
            Err(QueryError::NoAllocation) => query_result::Result::NoAllocation(()),
            Err(QueryError::BadRequest(e)) => query_result::Result::BadRequest(e),
            Err(QueryError::Other(e)) => query_result::Result::ServerError(e.to_string()),
        };
        let envelope = subsquid_messages::Envelope {
            msg: Some(Msg::QueryResult(subsquid_messages::QueryResult {
                query_id,
                result: Some(query_result),
            })),
        };
        if let Err(e) = self
            .transport_handle
            .send_direct_msg(envelope.encode_to_vec(), peer_id)
        {
            error!("Couldn't send query result: {e:?}");
            // TODO: add retries
        }
    }

    pub fn local_peer_id(&self) -> PeerId {
        self.worker_id
    }

    fn generate_log(
        &self,
        query_result: &std::result::Result<QueryResult, QueryError>,
        query: Query,
        client_id: PeerId,
    ) -> QueryExecuted {
        let result = match query_result {
            Ok(result) => query_executed::Result::Ok(InputAndOutput {
                num_read_chunks: Some(result.num_read_chunks as u32),
                output: Some(SizeAndHash {
                    size: Some(result.data_size as u32),
                    sha3_256: result.data_sha3_256.clone(),
                }),
            }),
            Err(e @ QueryError::NotFound) => query_executed::Result::BadRequest(e.to_string()),
            Err(QueryError::BadRequest(e)) => query_executed::Result::BadRequest(e.clone()),
            Err(QueryError::Other(e)) => query_executed::Result::ServerError(e.to_string()),
            Err(QueryError::NoAllocation) => panic!("Shouldn't send logs with NoAllocation error"),
        };
        let query_hash = sha3_256(
            query
                .query
                .as_ref()
                .expect("Hashing empty query")
                .as_bytes(),
        );
        let mut result = QueryExecuted {
            client_id: client_id.to_base58(),
            worker_id: self.worker_id.to_base58(),
            query_hash,
            query: Some(query),
            result: Some(result),
            exec_time_ms: None, // TODO: measure execution time
            ..Default::default()
        };
        result.sign(&self.keypair).expect("Couldn't sign query log");
        result
    }
}

impl<MsgStream: Stream<Item = Message> + Send> super::Transport for P2PTransport<MsgStream> {
    async fn send_ping(&self, state: super::State) -> Result<()> {
        let mut ping = Ping {
            stored_ranges: state
                .datasets
                .into_iter()
                .map(|(dataset, ranges)| DatasetRanges {
                    url: dataset,
                    ranges: ranges.ranges,
                })
                .collect(),
            worker_id: Some(self.worker_id.to_string()),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            ..Default::default()
        };
        ping.sign(&self.keypair)?;
        let envelope = subsquid_messages::Envelope {
            msg: Some(Msg::Ping(ping)),
        };
        self.transport_handle
            .send_direct_msg(envelope.encode_to_vec(), self.scheduler_id)?;
        Ok(())
    }

    fn stream_assignments(&self) -> impl futures::Stream<Item = Ranges> + 'static {
        let rx = self.assignments_rx.take().unwrap();
        WatchStream::from_changes(rx)
    }

    fn stream_queries(&self) -> impl futures::Stream<Item = super::QueryTask> + 'static {
        let rx = self.queries_rx.take().unwrap();
        ReceiverStream::new(rx)
    }

    async fn run(&self, cancellation_token: CancellationToken) {
        tokio::join!(
            self.run_receive_loop(cancellation_token.clone()),
            self.run_send_logs_loop(
                cancellation_token,
                std::time::Duration::from_secs(LOGS_SEND_INTERVAL_SEC)
            ),
        );
    }
}
