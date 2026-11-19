use tunnel_config::parse;

const HOME: &str = include_str!("../../../examples/home.toml");
const LAPTOP: &str = include_str!("../../../examples/laptop.toml");

#[test]
fn documented_forward_matches_the_published_service() {
    let home = parse(HOME).unwrap();
    let laptop = parse(LAPTOP).unwrap();
    let forward = &laptop.forwards["home-web"];
    assert_eq!(forward.peer, "home");
    let service = &home.services[forward.destination.address()];
    assert_eq!(service.target.to_string(), "127.0.0.1:3000");
    assert_eq!(service.allow_peers, ["laptop"]);
}

#[test]
fn misspelled_peer_in_an_allowlist_is_reported() {
    let input = HOME.replace("allow_peers = [\"laptop\"]", "allow_peers = [\"unknown\"]");
    assert!(parse(&input).unwrap_err().contains("unknown peer unknown"));
}

#[test]
fn supports_ipv6_and_dns_targets() {
    for target in ["[::1]:3000", "localhost:3000"] {
        assert!(parse(&HOME.replace("127.0.0.1:3000", target)).is_ok());
    }
}

#[test]
fn port_rules_have_one_destination_and_remote_listeners_need_a_tcp_target() {
    let direct = LAPTOP.replace(
        "service = \"dev-web\"",
        "direction = \"remote\"\ntarget = \"3000\"",
    );
    let config = parse(&direct).unwrap();
    assert_eq!(
        config.forwards["home-web"].destination.address(),
        "127.0.0.1:3000"
    );
    assert!(parse(&format!("{direct}\nservice='extra'")).is_err());
    assert!(parse(&format!("{direct}\ntypo='extra'")).is_err());
    assert!(parse(&format!("{LAPTOP}\ndirection='remote'")).is_err());
}

#[test]
fn sshd_targets_require_linux_and_an_absolute_config_path() {
    let target = r#""127.0.0.1:3000""#;
    let relative = HOME.replace(target, r#"{ sshd_config = "sshd_config" }"#);
    assert!(parse(&relative).is_err());
    let absolute = HOME.replace(target, r#"{ sshd_config = "/etc/ssh/sshd_config" }"#);
    assert_eq!(parse(&absolute).is_ok(), cfg!(target_os = "linux"));
    let typo = HOME.replace(
        target,
        r#"{ sshd_config = "/etc/sshd_config", extra = true }"#,
    );
    assert!(parse(&typo).is_err());
}
