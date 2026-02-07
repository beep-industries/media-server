#![warn(rust_2018_idioms)]
#![allow(dead_code)]

pub mod audio_callback;
pub(crate) mod description;
pub(crate) mod endpoint;
pub(crate) mod handler;
pub(crate) mod interceptor;
pub(crate) mod messages;
pub(crate) mod server;
pub(crate) mod session;
pub(crate) mod types;

pub use audio_callback::{AudioPacketInfo, AudioSender, is_audio_payload_type};
pub use description::RTCSessionDescription;
pub use handler::{
    datachannel::DataChannelHandler, demuxer::DemuxerHandler, dtls::DtlsHandler,
    exception::ExceptionHandler, gateway::GatewayHandler, interceptor::InterceptorHandler,
    sctp::SctpHandler, srtp::SrtpHandler, stun::StunHandler,
};
pub use server::{certificate::RTCCertificate, config::ServerConfig, states::ServerStates};
