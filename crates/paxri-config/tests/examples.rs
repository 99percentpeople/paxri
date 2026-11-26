use paxri_config::parse;

const HOME: &str = include_str!("../../../examples/home.toml");
const LAPTOP: &str = include_str!("../../../examples/laptop.toml");

#[test]
fn documented_address_forward_requires_only_a_pairing() {
    assert!(parse(HOME).unwrap().peers.contains_key("laptop"));
    let laptop = parse(LAPTOP).unwrap();
    let forward = &laptop.forwards["home-web"];
    assert_eq!(forward.peer, "home");
    assert_eq!(forward.target, "127.0.0.1:3000");
}

#[test]
fn both_directions_validate_tcp_addresses() {
    for direction in ["local", "remote"] {
        let source = format!("{LAPTOP}\ndirection='{direction}'");
        for target in ["3000", "localhost:3000", "[::1]:3000"] {
            let cfg = parse(&source.replace("127.0.0.1:3000", target)).unwrap();
            assert_eq!(
                cfg.forwards["home-web"].target,
                paxri_config::tcp_target(target).unwrap()
            );
        }
        for invalid in ["dev-web", "localhost:0", "localhost:65536"] {
            assert!(parse(&source.replace("127.0.0.1:3000", invalid)).is_err());
        }
        assert!(parse(&format!("{source}\nservice='web'")).is_err());
    }
}

#[test]
fn existing_peers_with_a_saved_terminal_flag_still_load() {
    for flag in ["true", "false"] {
        let config = HOME.replace(
            "[peers.laptop]",
            &format!("[peers.laptop]\nallow_terminal = {flag}"),
        );
        assert!(parse(&config).unwrap().peers.contains_key("laptop"));
    }
}

#[test]
fn forwarding_discovery_respects_advertised_support_and_legacy_peers() {
    use paxri_config::{DeviceCapabilities, DevicePlatform::*};

    // The original snapshot contained only terminal capabilities.
    let legacy: DeviceCapabilities = serde_json::from_str(r#"{"shell":false,"exec":false}"#).unwrap();
    for capabilities in [None, Some(legacy)] {
        assert!(Windows.supports_forward_rules(capabilities));
        assert!(Unknown.supports_forward_rules(capabilities));
        assert!(!Android.supports_forward_rules(capabilities));
        assert!(!Web.supports_forward_rules(capabilities));
    }
    for advertised in [false, true] {
        let capabilities = Some(DeviceCapabilities { forward_rules: Some(advertised), ..legacy });
        assert_eq!(Linux.supports_forward_rules(capabilities), advertised);
        assert_eq!(Android.supports_forward_rules(capabilities), advertised);
    }
}

#[test]
fn listen_ports_are_unique_per_receiving_host_while_targets_can_be_shared() {
    let a = "aa".repeat(32);
    let b = "bb".repeat(32);
    // One local service (8080) can be reverse-forwarded to port 8080 on multiple different peers
    // using the same display name or no display name at all.
    let shared_target_reverse = format!(
        r#"version = 1
[peers.first]
endpoint_id = "{a}"
[peers.second]
endpoint_id = "{b}"
[forwards.7a9c1f3e]
name = "web"
peer = "first"
direction = "remote"
listen = "127.0.0.1:8080"
target = "8080"
[forwards.b42e8d10]
name = "web"
peer = "second"
direction = "remote"
listen = "127.0.0.1:8080"
target = "8080"
"#
    );
    let parsed = parse(&shared_target_reverse).unwrap();
    assert_eq!(parsed.forwards["7a9c1f3e"].name.as_deref(), Some("web"));
    assert_eq!(parsed.forwards["b42e8d10"].name.as_deref(), Some("web"));

    // The same receiving peer cannot bind the same listen port twice.
    let dup_remote = format!(
        r#"{shared_target_reverse}
[forwards.web-a-dup]
peer = "first"
direction = "remote"
listen = "0.0.0.0:8080"
target = "3000"
"#
    );
    assert!(parse(&dup_remote).is_err());

    // The same receiving peer CAN bind a different listen port for the same target.
    let diff_remote_port = format!(
        r#"{shared_target_reverse}
[forwards.web-a-alt]
peer = "first"
direction = "remote"
listen = "127.0.0.1:8081"
target = "8080"
"#
    );
    assert!(parse(&diff_remote_port).is_ok());

    // On this device (Local listening host), two forwards cannot reuse the same listen port
    // even when targeting different peers.
    let dup_local = format!(
        r#"version = 1
[peers.first]
endpoint_id = "{a}"
[peers.second]
endpoint_id = "{b}"
[forwards.web-a]
peer = "first"
listen = "127.0.0.1:8080"
target = "8080"
[forwards.web-b]
peer = "second"
listen = "127.0.0.1:8080"
target = "8080"
"#
    );
    assert!(parse(&dup_local).is_err());

    // Different local listen ports targeting the same service port are allowed.
    let diff_local = dup_local.replace("listen = \"127.0.0.1:8080\"\ntarget = \"8080\"\n", "listen = \"127.0.0.1:8080\"\ntarget = \"8080\"\n");
    let diff_local = diff_local.replacen("127.0.0.1:8080", "127.0.0.1:8081", 1);
    assert!(parse(&diff_local).is_ok());
}
