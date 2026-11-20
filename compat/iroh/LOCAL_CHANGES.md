# Native networking extensions

This directory contains the library sources and normalized Cargo manifest from `iroh 1.4.1` (upstream commit `d6c6963b9602d1b8dd2ba53b6519e7eb19cc7940`). The published crate has SHA-256 `e3689cab508af9bcef7816c1201b403595ec6aa20e7d15e1f9683bb4be4a5b17`. The manifest omits the upstream example and integration-test targets. The upstream licenses accompany the sources.

The gateway uses two extensions to the native endpoint.

- `NetReportConfig::qad_servers` supplies an independent set of QUIC address discovery servers. The network report probes them from its existing QUIC endpoint and compares observations from distinct destination IPs per address family. The data relay map still determines which servers are eligible to be the home relay.
- `Endpoint::watch_network_interfaces` shares the endpoint's OS network notifications. `Endpoint::set_ipv4_source` selects a physical source for new IPv4 paths on the wildcard UDP socket and refreshes address discovery. QUIC can retain its endpoint, existing authenticated connections and explicit path source addresses.

The application tests these APIs in `crates/tunnel-native/src/relay.rs` and `scripts/check-nat.sh --roam`. Compare these extensions against the published crate when updating iroh.
