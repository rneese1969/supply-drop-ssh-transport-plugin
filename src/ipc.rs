use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum PluginMsg {
    Ready {
        payload_limit: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    Open {
        id: String,
    },
    Recv {
        id: String,
        line: String,
    },
    Close {
        id: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum HostMsg {
    Send {
        id: String,
        text: String,
        #[serde(default)]
        hide_input: Option<bool>,
    },
    Kick {
        id: String,
    },
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_ready_as_line_protocol_shape() {
        let encoded = serde_json::to_string(&PluginMsg::Ready {
            payload_limit: Some(0),
            version: Some("1.2.3".to_owned()),
        })
        .unwrap();
        assert_eq!(
            encoded,
            r#"{"t":"ready","payload_limit":0,"version":"1.2.3"}"#
        );
    }

    #[test]
    fn omits_ready_version_when_none() {
        let encoded = serde_json::to_string(&PluginMsg::Ready {
            payload_limit: Some(0),
            version: None,
        })
        .unwrap();
        assert_eq!(encoded, r#"{"t":"ready","payload_limit":0}"#);
    }

    #[test]
    fn decodes_send_with_optional_hide_input() {
        let decoded: HostMsg =
            serde_json::from_str(r#"{"t":"send","id":"c1","text":"Password: ","hide_input":true}"#)
                .unwrap();

        match decoded {
            HostMsg::Send {
                id,
                text,
                hide_input,
            } => {
                assert_eq!(id, "c1");
                assert_eq!(text, "Password: ");
                assert_eq!(hide_input, Some(true));
            }
            _ => panic!("expected send"),
        }
    }
}
