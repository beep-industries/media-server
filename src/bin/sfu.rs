use clap::Parser;
use dtls::extension::extension_use_srtp::SrtpProtectionProfile;
use log::info;
use opentelemetry::{KeyValue};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::{runtime, Resource};
use opentelemetry_stdout::MetricsExporterBuilder;
use sfu::{RTCCertificate, ServerConfig};
use std::collections::HashMap;
use std::io::Write;
use std::net::{IpAddr, UdpSocket};
use std::str::FromStr;
use std::sync::{mpsc, Arc};
use std::sync::mpsc::SyncSender;
use std::time::Duration;
use wg::WaitGroup;

use media_server::signal::{self, SignalingMessage, SignalingProtocolMessage};

use tonic::{transport::Server, Request, Response, Status};

pub mod signaling_grpc { include!(concat!(env!("OUT_DIR"), "/signaling.rs")); }
use signaling_grpc::signaling_server::{Signaling, SignalingServer};
use signaling_grpc::{OfferRequest, OfferResponse, LeaveRequest, LeaveResponse};

#[derive(Default, Debug, Copy, Clone, clap::ValueEnum)]
enum Level {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl From<Level> for log::LevelFilter {
    fn from(level: Level) -> Self {
        match level {
            Level::Error => log::LevelFilter::Error,
            Level::Warn => log::LevelFilter::Warn,
            Level::Info => log::LevelFilter::Info,
            Level::Debug => log::LevelFilter::Debug,
            Level::Trace => log::LevelFilter::Trace,
        }
    }
}

#[derive(Parser)]
#[command(name = "SFU Server (gRPC)")]
#[command(author = "Rusty Rain <y@ngr.tc>")]
#[command(version = "0.1.0")]
#[command(about = "An example of SFU Server with gRPC signaling", long_about = None)]
struct Cli {
    #[arg(long, default_value_t = format!("127.0.0.1"))]
    host: String,

    #[arg(long, default_value_t = String::from("127.0.0.1:50051"))]
    grpc_addr: String,

    #[arg(long, default_value_t = 3478)]
    media_port_min: u16,
    #[arg(long, default_value_t = 3478)]
    media_port_max: u16,

    #[arg(short, long)]
    force_local_loop: bool,
    #[arg(short, long)]
    debug: bool,
    #[arg(short, long, default_value_t = Level::Info)]
    #[clap(value_enum)]
    level: Level,
}

fn init_meter_provider(
    mut stop_rx: async_broadcast::Receiver<()>,
    wait_group: WaitGroup,
) -> SdkMeterProvider {
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap();

        rt.block_on(async move {
            let worker = wait_group.add(1);
            let exporter = MetricsExporterBuilder::default()
                .with_encoder(|writer, data| {
                    Ok(serde_json::to_writer_pretty(writer, &data).unwrap())
                })
                .build();
            let reader = PeriodicReader::builder(exporter, runtime::TokioCurrentThread)
                .with_interval(Duration::from_secs(30))
                .build();
            let meter_provider = SdkMeterProvider::builder()
                .with_reader(reader)
                .with_resource(Resource::new(vec![KeyValue::new("chat", "metrics")]))
                .build();
            let _ = tx.send(meter_provider.clone());

            let _ = stop_rx.recv().await;
            let _ = meter_provider.shutdown();
            worker.done();
            info!("meter provider is gracefully down");
        });
    });

    let meter_provider = rx.recv().unwrap();
    meter_provider
}

struct SignalingSvc {
    media_port_thread_map: Arc<HashMap<u16, SyncSender<SignalingMessage>>>,
}

#[tonic::async_trait]
impl Signaling for SignalingSvc {
    async fn offer(&self, request: Request<OfferRequest>) -> Result<Response<OfferResponse>, Status> {
        let req = request.into_inner();
        let session_id = req.session_id;
        let endpoint_id = req.endpoint_id;
        let offer_sdp = req.offer_sdp;

        // Pick port deterministically as in the original code.
        let mut sorted_ports: Vec<u16> = self.media_port_thread_map.keys().copied().collect();
        if sorted_ports.is_empty() {
            return Ok(Response::new(OfferResponse{ session_id, endpoint_id, answer_sdp: String::new(), error: "No media ports".into() }));
        }
        sorted_ports.sort();
        let port = sorted_ports[(session_id as usize) % sorted_ports.len()];
        let tx = self.media_port_thread_map.get(&port).ok_or_else(|| Status::unavailable("no worker for port"))?;

        let (response_tx, response_rx) = mpsc::sync_channel(1);
        tx.send(SignalingMessage { request: SignalingProtocolMessage::Offer{ session_id, endpoint_id, offer_sdp: bytes::Bytes::from(offer_sdp) }, response_tx })
            .map_err(|_| Status::internal("failed to send signaling message"))?;
        let response = response_rx.recv().map_err(|_| Status::internal("failed to receive answer"))?;
        match response {
            SignalingProtocolMessage::Answer{ session_id: sid, endpoint_id: eid, answer_sdp } => {
                let answer = String::from_utf8(answer_sdp.to_vec()).unwrap_or_default();
                Ok(Response::new(OfferResponse{ session_id: sid, endpoint_id: eid, answer_sdp: answer, error: String::new() }))
            }
            SignalingProtocolMessage::Err{ reason, .. } => {
                Ok(Response::new(OfferResponse{ session_id, endpoint_id, answer_sdp: String::new(), error: String::from_utf8(reason.to_vec()).unwrap_or_else(|e| e.to_string()) }))
            }
            _ => Ok(Response::new(OfferResponse{ session_id, endpoint_id, answer_sdp: String::new(), error: "invalid response".into() })),
        }
    }

    async fn leave(&self, request: Request<LeaveRequest>) -> Result<Response<LeaveResponse>, Status> {
        let req = request.into_inner();
        let session_id = req.session_id;
        let endpoint_id = req.endpoint_id;
        let mut sorted_ports: Vec<u16> = self.media_port_thread_map.keys().copied().collect();
        if sorted_ports.is_empty() {
            return Ok(Response::new(LeaveResponse{ ok: false, error: "No media ports".into() }));
        }
        sorted_ports.sort();
        let port = sorted_ports[(session_id as usize) % sorted_ports.len()];
        if let Some(tx) = self.media_port_thread_map.get(&port) {
            let (response_tx, _rx) = mpsc::sync_channel(1);
            tx.send(SignalingMessage { request: SignalingProtocolMessage::Leave{ session_id, endpoint_id }, response_tx })
                .map_err(|_| Status::internal("failed to send leave message"))?;
        }
        Ok(Response::new(LeaveResponse{ ok: true, error: String::new() }))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if cli.debug {
        env_logger::Builder::new()
            .format(|buf, record| {
                writeln!(
                    buf,
                    "{}:{} [{}] {} - {}",
                    record.file().unwrap_or("unknown"),
                    record.line().unwrap_or(0),
                    record.level(),
                    chrono::Local::now().format("%H:%M:%S.%6f"),
                    record.args()
                )
            })
            .filter(None, cli.level.into())
            .init();
    }

    // Figure out host address for media sockets
    let host_addr = if cli.host == "127.0.0.1" && !cli.force_local_loop {
        media_server::util::select_host_address()
    } else {
        IpAddr::from_str(&cli.host)?
    };

    let media_ports: Vec<u16> = (cli.media_port_min..=cli.media_port_max).collect();
    let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(1);
    let mut media_port_thread_map: HashMap<u16, SyncSender<SignalingMessage>> = HashMap::new();

    // Certificates for DTLS
    let key_pair = rcgen::KeyPair::generate(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let certificates = vec![RTCCertificate::from_key_pair(key_pair)?];
    let dtls_handshake_config = Arc::new(
        dtls::config::ConfigBuilder::default()
            .with_certificates(
                certificates
                    .iter()
                    .map(|c| c.dtls_certificate.clone())
                    .collect(),
            )
            .with_srtp_protection_profiles(vec![SrtpProtectionProfile::Srtp_Aes128_Cm_Hmac_Sha1_80])
            .with_extended_master_secret(dtls::config::ExtendedMasterSecretType::Require)
            .build(false, None)?,
    );
    let sctp_endpoint_config = Arc::new(sctp::EndpointConfig::default());
    let sctp_server_config = Arc::new(sctp::ServerConfig::default());
    let server_config = Arc::new(
        ServerConfig::new(certificates)
            .with_dtls_handshake_config(dtls_handshake_config)
            .with_sctp_endpoint_config(sctp_endpoint_config)
            .with_sctp_server_config(sctp_server_config)
            .with_idle_timeout(Duration::from_secs(30)),
    );

    let (stop_meter_tx, stop_meter_rx) = async_broadcast::broadcast::<()>(1);
    let wait_group = WaitGroup::new();
    let meter_provider = init_meter_provider(stop_meter_rx, wait_group.clone());

    for port in media_ports {
        let worker = wait_group.add(1);
        let stop_rx = stop_rx.clone();
        let (signaling_tx, signaling_rx) = mpsc::sync_channel(1);
        let socket = UdpSocket::bind(format!("{host_addr}:{port}"))
            .expect(&format!("binding to {host_addr}:{port}"));
        media_port_thread_map.insert(port, signaling_tx);
        let server_config = server_config.clone();
        let meter_provider = meter_provider.clone();
        std::thread::spawn(move || {
            if let Err(err) = signal::sync_run(stop_rx, socket, signaling_rx, server_config, meter_provider) {
                eprintln!("run_sfu got error: {}", err);
            }
            worker.done();
        });
    }

    let svc = SignalingSvc { media_port_thread_map: Arc::new(media_port_thread_map) };

    let grpc_addr = cli.grpc_addr.parse()?;
    println!("Starting gRPC Signaling on {}", grpc_addr);

    // Unified graceful shutdown on Ctrl-C: stop gRPC, media threads, and metrics
    let shutdown = {
        let stop_tx = stop_tx.clone();
        let stop_meter_tx = stop_meter_tx.clone();
        async move {
            let _ = tokio::signal::ctrl_c().await;
            // Signal media workers
            let _ = stop_tx.send(());
            // Signal metrics provider
            let _ = stop_meter_tx.broadcast(()).await;
        }
    };

    Server::builder()
        .add_service(SignalingServer::new(svc))
        .serve_with_shutdown(grpc_addr, shutdown)
        .await?;

    // Ensure shutdown signals are sent even if server exited without Ctrl-C
    let _ = stop_tx.send(());
    let _ = stop_meter_tx.broadcast(()).await;
    // Drop the stop sender so all receivers observe disconnect and exit
    drop(stop_tx);

    // Wait for workers and metrics to finish gracefully
    let wg = wait_group.clone();
    let _ = tokio::task::spawn_blocking(move || wg.wait()).await;

    Ok(())
}