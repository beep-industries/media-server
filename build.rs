fn main() {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile(&["proto/signaling.proto"], &["proto"]) // proto dir relative to project root
        .expect("failed to compile gRPC protos");
}