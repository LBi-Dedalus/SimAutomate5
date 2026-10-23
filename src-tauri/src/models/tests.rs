use serde_json::json;

use super::*;

fn spec(json: &str) -> Result<ConnectionSpec, String> {
    let req: ConnectRequest = serde_json::from_str(json).unwrap();
    ConnectionSpec::try_from(req)
}

#[test]
fn client_and_server_requests_are_parsed() {
    let spec_client = spec(r#"{"type":"ClientConnectRequest","ip":"  10.0.0.1 ","port":5000}"#);
    let ConnectionSpec::Client { host, port } = spec_client.unwrap() else {
        panic!("expected a client");
    };
    assert_eq!(host.as_str(), "10.0.0.1");
    assert_eq!(port.get(), 5000);

    let spec_server = spec(r#"{"type":"ServerStartRequest","port":65535}"#).unwrap();
    assert!(matches!(spec_server, ConnectionSpec::Server { port } if port.get() == 65535));
}

#[test]
fn invalid_client_and_server_requests_keep_their_messages() {
    assert_eq!(
        spec(r#"{"type":"ClientConnectRequest","ip":"  ","port":1}"#).unwrap_err(),
        "The host is required"
    );
    assert_eq!(
        spec(r#"{"type":"ClientConnectRequest","ip":"h","port":0}"#).unwrap_err(),
        "The port must be between 1 and 65535"
    );
    assert_eq!(
        spec(r#"{"type":"ServerStartRequest","port":0}"#).unwrap_err(),
        "The port must be between 1 and 65535"
    );
}

#[test]
fn ssh_client_request_is_parsed_with_its_tunnel() {
    let parsed = spec(
        r#"{"type":"SshClientConnectRequest","ip":"127.0.0.1","port":9000,
                "ssh_host":" vm ","ssh_port":22,"ssh_user":" bob "}"#,
    )
    .unwrap();
    let ConnectionSpec::SshClient { host, port, tunnel } = parsed else {
        panic!("expected an SSH client");
    };
    assert_eq!(host.as_str(), "127.0.0.1");
    assert_eq!(port.get(), 9000);
    assert_eq!(tunnel.host.as_str(), "vm");
    assert_eq!(tunnel.port.get(), 22);
    assert_eq!(tunnel.user.unwrap().as_str(), "bob");
}

#[test]
fn ssh_server_request_is_parsed_and_an_empty_user_is_none() {
    for user in ["", "   "] {
        let json = format!(
            r#"{{"type":"SshServerStartRequest","port":9000,"ssh_host":"vm","ssh_port":2222,"ssh_user":"{user}"}}"#
        );
        let ConnectionSpec::SshServer { port, tunnel } = spec(&json).unwrap() else {
            panic!("expected an SSH server");
        };
        assert_eq!(port.get(), 9000);
        assert_eq!(tunnel.port.get(), 2222);
        assert_eq!(tunnel.user, None);
    }
}

#[test]
fn invalid_ssh_requests_are_rejected() {
    let client = |ip: &str, port: u16, host: &str, ssh_port: u16| {
        spec(&format!(
            r#"{{"type":"SshClientConnectRequest","ip":"{ip}","port":{port},"ssh_host":"{host}","ssh_port":{ssh_port},"ssh_user":""}}"#
        ))
    };
    assert!(client("h", 1, "vm", 22).is_ok());
    assert_eq!(
        client(" ", 1, "vm", 22).unwrap_err(),
        "The host is required"
    );
    assert_eq!(
        client("h", 0, "vm", 22).unwrap_err(),
        "The port must be between 1 and 65535"
    );
    assert_eq!(
        client("h", 1, " ", 22).unwrap_err(),
        "The SSH host is required"
    );
    assert_eq!(
        client("h", 1, "vm", 0).unwrap_err(),
        "The SSH port must be between 1 and 65535"
    );

    assert_eq!(
        spec(
            r#"{"type":"SshServerStartRequest","port":0,"ssh_host":"vm","ssh_port":22,"ssh_user":""}"#
        )
        .unwrap_err(),
        "The port must be between 1 and 65535"
    );
    assert_eq!(
        spec(
            r#"{"type":"SshServerStartRequest","port":1,"ssh_host":"","ssh_port":22,"ssh_user":""}"#
        )
        .unwrap_err(),
        "The SSH host is required"
    );
    assert_eq!(
        spec(
            r#"{"type":"SshServerStartRequest","port":1,"ssh_host":"vm","ssh_port":0,"ssh_user":""}"#
        )
        .unwrap_err(),
        "The SSH port must be between 1 and 65535"
    );
}

#[test]
fn listening_status_serializes_lowercase() {
    let json = serde_json::to_string(&StatusPayload {
        session_id: "s1".to_string(),
        attempt: 2,
        status: ConnectionStatus::Listening,
    })
    .unwrap();
    assert_eq!(
        json,
        r#"{"session_id":"s1","attempt":2,"status":"listening"}"#
    );
}

#[test]
fn message_payload_omits_absent_raw_and_keeps_present_raw() {
    let mut payload = MessagePayload {
        session_id: "s1".to_string(),
        attempt: 1,
        msg_type: MessageType::SystemInfo,
        content: "hi".to_string(),
        timestamp: "t".to_string(),
        raw: None,
    };
    let json = serde_json::to_string(&payload).unwrap();
    assert!(!json.contains("raw"));
    assert!(json.contains(r#""session_id":"s1""#));
    assert!(json.contains(r#""attempt":1"#));

    payload.raw = Some(vec![0x0b, 0x41]);
    let json = serde_json::to_string(&payload).unwrap();
    assert!(json.contains(r#""raw":[11,65]"#));
}

#[test]
fn auto_build_request_keeps_the_no_etb_boolean_on_the_wire() {
    for (no_etb, mode) in [
        (false, AstmEndingMode::EtbBetweenRecords),
        (true, AstmEndingMode::EtxForEveryRecord),
    ] {
        let wire = json!({"input": "H|x", "no_etb": no_etb});
        let req: AutoBuildRequest = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(req.input, "H|x");
        assert_eq!(req.astm_ending, mode);
        assert_eq!(serde_json::to_value(&req).unwrap(), wire);
        assert_eq!(
            serde_json::to_string(&req).unwrap(),
            format!(r#"{{"input":"H|x","no_etb":{no_etb}}}"#)
        );
    }
    // The flag is required, like before.
    assert!(serde_json::from_value::<AutoBuildRequest>(json!({"input": "x"})).is_err());
}

#[test]
fn connect_and_log_wire_shapes_round_trip() {
    let wire = json!({"type":"SshClientConnectRequest","ip":"h","port":1,
        "ssh_host":"vm","ssh_port":22,"ssh_user":"bob"});
    let req: ConnectRequest = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&req).unwrap(), wire);

    let wire = json!({"level":"WRN","location":"a.js","message":"m"});
    let entry: FrontendLogEntry = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&entry).unwrap(), wire);
}
