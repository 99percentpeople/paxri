use peerhatch_config::parse;

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
                peerhatch_config::tcp_target(target).unwrap()
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
