use anyhow::Result;
use peerhatch_config::Service;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn list(config: &Arc<peerhatch_config::Config>, id: &str) -> Result<Value> {
    let (mut client, server) = tokio::io::duplex(256);
    let (_, answer) = tokio::try_join!(
        peerhatch_core::serve(|| config.clone(), id, server),
        async {
            let request = br#"{"type":"LIST_SERVICES"}"#;
            client.write_u32(request.len() as u32).await?;
            client.write_all(request).await?;
            let mut body = vec![0; client.read_u32().await? as usize];
            client.read_exact(&mut body).await?;
            Ok(serde_json::from_slice(&body)?)
        },
    )?;
    Ok(answer)
}

#[tokio::test]
async fn names_are_scoped_to_the_authenticated_peer_and_can_exceed_the_handshake_size() -> Result<()>
{
    let first = "ab".repeat(32);
    let second = "cd".repeat(32);
    let config = format!(
        "version=1\n[peers.first]\nendpoint_id='{first}'\n[peers.second]\nendpoint_id='{second}'\n[port_access]\nconnect=['127.0.0.1:8000']"
    );
    let mut config = peerhatch_config::parse(&config).map_err(anyhow::Error::msg)?;
    let names: Vec<_> = (0..180)
        .map(|i| format!("published-service-number-{i:03}"))
        .collect();
    for name in &names {
        config.services.insert(
            name.clone(),
            Service {
                target: "127.0.0.1:8000".into(),
                allow_peers: vec!["first".into()],
            },
        );
    }
    let config = Arc::new(config);
    assert_eq!(
        list(&config, &first).await?,
        json!({"type":"SERVICES", "names":names})
    );
    assert_eq!(
        list(&config, &second).await?,
        json!({"type":"SERVICES", "names":[]})
    );
    assert_eq!(
        list(&config, &"ef".repeat(32)).await?,
        json!({"type":"ERROR", "code":"DENIED"})
    );
    Ok(())
}
