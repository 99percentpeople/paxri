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
fn retired_ssh_entries_are_inert_and_normal_tcp_services_still_work() {
    let config = parse(&format!("{HOME}\n[peers.laptop.ssh]\nuser='alice'\n[services.legacy]\ntarget={{sshd_config='/old/sshd_config'}}\nallow_peers=['laptop']\n[services.tcp22]\ntarget='127.0.0.1:22'\nallow_peers=['laptop']")).unwrap();
    assert!(!config.services.contains_key("legacy"));
    assert!(!config.peers["laptop"].allow_terminal);
    assert_eq!(config.services["tcp22"].target, "127.0.0.1:22");
    assert!(parse(&HOME.replace("\"127.0.0.1:3000\"", "{ typo = true }")).is_err());
}
